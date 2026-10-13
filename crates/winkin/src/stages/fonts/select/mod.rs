//! Chooses a font for each cluster and writes the font runs.
//!
//! The walk goes in text order, one **span** at a time. A span is the
//! clusters of one text item within one script run, so it has one style and
//! one pair of candidate lists. Each cluster is handled by the first rule
//! that applies:
//!
//! 1. A cluster that draws nothing takes the font of the cluster before it.
//!    Tabs, separators, an atomic inline's U+FFFC and a lone
//!    default-ignorable draw nothing.
//! 2. A grapheme that a style boundary divides follows Chrome. If the parts
//!    shape alike (one [`ShapingFactsId`]), the first part chooses with the
//!    whole grapheme's text and the later parts take its font. Otherwise each
//!    part chooses by its own style, and a mark may leave its base.
//! 3. The cluster takes the first candidate, in CSS order, that the memo does
//!    not rule out, that accepts the presentation and that covers the grapheme.
//!    A fallback font's run ends where an earlier font covers again, so a CJK
//!    fallback takes no digits or spaces from the family the text asked for.
//! 4. If no matched font covers it, the walk matches the next family. Once
//!    every family is matched, it walks fontwich's fallback for each missing
//!    character, once per list and character. A character that no installed
//!    family maps is noted once per context and never walked again. Then it
//!    tries a forced presentation's base form, then the candidate that draws
//!    most of it, and last the primary font, which draws `.notdef` as Chrome
//!    does.
//! 5. A grapheme holding a variation sequence first looks over the whole list for
//!    a font that also has the sequence. Only if none has it does it take
//!    the first font that covers the base. Blink matches clusters in these
//!    same two passes.
//! 6. The chosen font gives a used font: the instance for the style's
//!    variations and features at its used size. If the font lacks small
//!    capitals the style asks for, the text splits where Blink's
//!    `SmallCapsIterator` splits it, and the uppercased side gets a smaller
//!    used font. So a font run also ends where the case changes.
//! 7. Under `font-variant-position: super` or `sub` that may be synthesized,
//!    the cluster's font waits for the end of its **position run**. The
//!    feature is used only if the fonts' `sups` or `subs` covers every
//!    character of the run (CSS Fonts 4, section 6.5). Otherwise the whole
//!    run is synthesized (see `positions`). The run spans script runs and
//!    inline box edges while the text shapes alike.
//!
//! The **uncovered memo** keeps, per candidate, the next cluster it covers.
//! A candidate that misses scans ahead once and is skipped until then. So
//! text that only the k-th candidate covers costs one bit test per
//! character, not k. The memo records coverage of the base characters, so a
//! candidate that lacks a variation sequence stays in for the second pass.
//! A face still pending is skipped the same way, until the next cluster
//! that wants it.
//!
//! The walk reads the context's candidate lists without changing them. When
//! it needs a change (a family matched, a fallback walked, a font found
//! unusable, a position feature's coverage read), it stops and returns the
//! change. The caller applies it and restarts the walk at that cluster with
//! the memo kept. Each change happens once per context, so a warm build
//! never stops.

mod choice;
mod generated;
mod kept;
mod pick;

pub(super) use generated::select_generated;
#[cfg(test)]
pub(super) use kept::PER_CLUSTER;

use alloc::vec::Vec;
use core::mem;
use core::ops::Range;

use fontwich::{Charset, CharsetPage, Presentation};

use super::context::{CacheChange, FontContext};
use super::coverage;
use super::features;
use super::lists::{CandidateId, CandidateListId, ListKey, MatchRequest};
use super::used::UsedFontInterner;
use super::{DefaultLanguage, FontResolution, FontRun, FontRuns, UsedFontId};
use super::{FontInput, WantedFaces};
use crate::config::PositionSynthesis;
use crate::data::{HeapBytes, Id, IdRange, Table, TextOffset, heap_bytes};
use crate::stages::analysis::{ClusterAttrs, ClusterEnd, ClusterId, ScriptRunId};
use crate::stages::content::{
    FontRequest, FontRequestId, ItemId, ItemKind, NodeId, ShapingFactsId, TextCursor, TextFactsId,
};
use crate::style::{FirstLineVariant, FontVariantEmoji, FontVariantPosition};
use crate::unicode::{self, GraphemeClusterBreak};
use crate::work;
use choice::{Choice, Choices, PositionPlan, ResolvedRequest, choose, feature_coverage};
use kept::{KeptCandidate, shortcuts};
use pick::{GraphemeNeeds, Pages, SpanText, feature_covers, resolve};

/// How many times one span's walk may stop for a change before the rest
/// of it is set in its primary font.
///
/// Each change happens once per context, a few per family, so this is never
/// reached. It only guarantees that no input makes the loop endless.
const MAX_STOPS: u32 = 1 << 16;

/// The run loop's inputs besides the context.
///
/// It holds the stage's input, the variant being chosen for, and what the
/// stage resolved before any run.
pub(super) struct Input<'a> {
    pub(super) stage: &'a FontInput<'a>,
    /// The text's or the first line's. The variant picks the nodes' facts
    /// and the characters drawn, which a first-line transform may change.
    pub(super) variant: FirstLineVariant,
    /// The language text whose style names none chooses its fonts in.
    pub(super) default_language: DefaultLanguage,
    /// What each font request resolves to.
    pub(super) resolutions: &'a Table<FontRequestId, FontResolution>,
    /// The text the clusters are chosen for in the variant, read in order
    /// across the spans.
    pub(super) source: &'a TextCursor<'a>,
}

impl<'a> Input<'a> {
    /// The text facts `node` has in the variant.
    fn text(&self, node: NodeId) -> TextFactsId {
        self.stage.content.nodes.text_facts(node, self.variant)
    }

    /// The font request text with the text facts `text` is set in.
    fn request(&self, text: TextFactsId) -> FontRequestId {
        self.stage.content.facts.text_request(text)
    }

    /// The primary font of `request`, or the first used font for a request
    /// the content does not have, which none is.
    fn primary(&self, request: FontRequestId) -> UsedFontId {
        self.resolutions
            .get(request)
            .map_or(UsedFontId::new(0), |resolution| resolution.primary)
    }
}

/// What selecting fonts changes as it goes: the context's caches, the
/// stage's scratch, the layout's used fonts and the faces wanted.
///
/// Made once per build and passed whole to each step that chooses fonts.
pub(super) struct Selecting<'a> {
    pub(super) cx: &'a mut FontContext,
    pub(super) scratch: &'a mut RunScratch,
    pub(super) used: UsedFontInterner<'a>,
    /// The pending faces a cluster wants, each once, in the order first
    /// wanted.
    pub(super) wanted: &'a mut WantedFaces,
}

/// The run loop's working memory, in the stage's scratch.
///
/// It holds nothing between calls and keeps its capacity.
pub(super) struct RunScratch {
    /// What a span's walk keeps while it tries candidates.
    walk: SpanScratch,
    /// The run of text in a `font-variant-position` whose fonts wait on
    /// whether the feature covers all of it.
    position: PositionRun,
}

impl RunScratch {
    /// Empty scratch, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            walk: SpanScratch::new(),
            position: PositionRun::new(),
        }
    }

    /// Forgets everything, keeping the capacity.
    pub(super) fn clear(&mut self) {
        self.walk.clear();
        self.position.clear();
    }
}

/// What a span's walk keeps while it tries candidates.
struct SpanScratch {
    /// The uncovered memo: per candidate, the next cluster it covers, for
    /// the span's text and emoji lists. Zero means unknown.
    memo: [Table<CandidateId, ClusterId>; 2],
    /// Whether the span's last asking cluster changes when uppercased.
    ///
    /// A cluster that starts with a mark takes this value, as Blink's
    /// `SmallCapsIterator` passes over marks. It survives the walk's stops,
    /// like the memo, so a restart at a mark gives the same answer.
    upper: bool,
    /// The used fonts each candidate gives each request under each script,
    /// found once per build. Many boxes in a few styles ask for the same
    /// choice again and again.
    choices: Choices,
}

impl SpanScratch {
    /// Empty, allocating nothing.
    const fn new() -> Self {
        Self {
            memo: [Table::new(), Table::new()],
            upper: false,
            choices: Choices::new(),
        }
    }

    /// Forgets everything, keeping the capacity.
    fn clear(&mut self) {
        self.start_span();
        self.choices.clear();
    }

    /// Forgets what the walk of the span before kept: the memo and the
    /// side of case.
    fn start_span(&mut self) {
        for memo in &mut self.memo {
            memo.clear();
        }
        self.upper = false;
    }
}

/// The used font a cluster is set in, or the two it waits between.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Placed {
    /// This one, whatever else the run holds.
    Font(UsedFontId),
    /// `feature`, shaping with the position's feature, where the position
    /// run's fonts have the feature's glyph for every character of it, and
    /// `synthesized` otherwise.
    Either {
        feature: UsedFontId,
        synthesized: UsedFontId,
    },
}

impl Placed {
    /// The used font, where the position run holding it is `covered` or
    /// not.
    fn font(self, covered: bool) -> UsedFontId {
        match self {
            Self::Font(font) => font,
            Self::Either { feature, .. } if covered => feature,
            Self::Either { synthesized, .. } => synthesized,
        }
    }
}

/// The position run being gathered (see the module documentation).
///
/// Its clusters' fonts wait on whether every character has the feature's
/// glyph, which only the run's end tells. It lives in the stage's scratch,
/// so its capacity is kept.
struct PositionRun {
    /// The shaping facts of the text it was opened in, while one is open:
    /// it goes on while the text shapes alike.
    shaping: Option<ShapingFactsId>,
    /// Every character that draws has the feature's glyph, so far.
    covered: bool,
    /// Each group of its clusters set alike, from its first cluster.
    pieces: Vec<(ClusterId, Placed)>,
}

impl PositionRun {
    /// None open, allocating nothing.
    const fn new() -> Self {
        Self {
            shaping: None,
            covered: true,
            pieces: Vec::new(),
        }
    }

    /// Forgets the run, keeping the capacity.
    fn clear(&mut self) {
        self.shaping = None;
        self.covered = true;
        self.pieces.clear();
    }

    /// Whether a character is still to be asked about: a run is open, and
    /// nothing in it has been found wanting.
    fn asks(&self) -> bool {
        self.shaping.is_some() && self.covered
    }

    /// A character of the run has no glyph of the feature, or no glyph at
    /// all: the run is synthesized.
    fn uncover(&mut self) {
        self.covered = false;
    }
}

heap_bytes! {
    RunScratch { walk, position }
}

impl HeapBytes for SpanScratch {
    fn heap_bytes(&self) -> usize {
        let Self {
            memo,
            upper: _,
            choices,
        } = self;
        let memo: usize = memo.iter().map(HeapBytes::heap_bytes).sum();
        memo + choices.heap_bytes()
    }
}

heap_bytes! {
    PositionRun { pieces; shaping, covered }
}

/// What the loop writes: the runs, with the position run whose clusters
/// wait for its end.
struct Output<'a> {
    runs: &'a mut FontRuns,
    /// The font the last cluster was given.
    last: Option<Placed>,
    position: &'a mut PositionRun,
}

impl<'a> Output<'a> {
    /// Writing into `runs`, gathering a position run in `position`.
    fn new(runs: &'a mut FontRuns, position: &'a mut PositionRun) -> Self {
        position.clear();
        Self {
            runs,
            last: None,
            position,
        }
    }

    /// `cluster` is set as `placed` says: now, or, in a position run, when
    /// the run ends.
    fn push(&mut self, cluster: ClusterId, placed: Placed) {
        if self.position.shaping.is_some() {
            if self.position.pieces.last().map(|&(_, held)| held) != Some(placed) {
                self.position.pieces.push((cluster, placed));
            }
        } else {
            debug_assert!(
                matches!(placed, Placed::Font(_)),
                "only a position run waits"
            );
            set_from(self.runs, cluster, placed.font(false));
        }
        self.last = Some(placed);
    }

    /// `cluster` draws nothing, and takes the last cluster's font, or
    /// `primary` where there is none.
    fn push_nothing(&mut self, cluster: ClusterId, primary: UsedFontId) {
        self.push(cluster, self.last.unwrap_or(Placed::Font(primary)));
    }

    /// `cluster` is set in `primary` for want of any font that draws it,
    /// which draws `.notdef`: no variant glyph is to be had for it either.
    fn push_missing(&mut self, cluster: ClusterId, primary: UsedFontId) {
        self.position.uncover();
        self.push(cluster, Placed::Font(primary));
    }

    /// Each cluster of `range` draws nothing, as
    /// [`push_nothing`](Self::push_nothing) sets one.
    ///
    /// Each would take what the first took, so one push sets them all.
    fn push_nothing_range(&mut self, range: Range<ClusterId>, primary: UsedFontId) {
        if !range.is_empty() {
            self.push_nothing(range.start, primary);
        }
    }

    /// Each cluster of `range` is set in `primary`, as
    /// [`push_missing`](Self::push_missing) sets one.
    ///
    /// Each would push what the first pushed, so one push sets them all.
    fn push_missing_range(&mut self, range: Range<ClusterId>, primary: UsedFontId) {
        if !range.is_empty() {
            self.push_missing(range.start, primary);
        }
    }

    /// Text that shapes as `shaping` says, whose `font-variant-position`
    /// may be synthesized, comes next: it goes on the run open for text
    /// that shapes alike, or ends that run and opens one.
    fn open_position(&mut self, shaping: ShapingFactsId) {
        if self.position.shaping != Some(shaping) {
            self.close_position();
            self.position.shaping = Some(shaping);
            self.position.covered = true;
        }
    }

    /// Ends the position run, if one is open: each of its clusters is set
    /// in the font shaping with the feature where the run is covered, and
    /// in the synthesized one otherwise.
    fn close_position(&mut self) {
        if self.position.shaping.take().is_none() {
            return;
        }
        let covered = self.position.covered;
        for &(cluster, placed) in &self.position.pieces {
            set_from(self.runs, cluster, placed.font(covered));
        }
        self.position.pieces.clear();
        self.position.covered = true;
        if let Some(last) = &mut self.last {
            *last = Placed::Font(last.font(covered));
        }
    }
}

/// `cluster` and those after it are set in `font`, in `runs`, from now until
/// another run starts: a run, unless the last is in `font` already.
fn set_from(runs: &mut FontRuns, cluster: ClusterId, font: UsedFontId) {
    if runs.last().map(|run| run.font) != Some(font) {
        let pushed = runs.push(FontRun {
            start: cluster,
            font,
        });
        debug_assert!(pushed.is_some(), "no more runs than clusters");
    }
}

/// Whether `request`'s `font-variant-position` is synthesized where the
/// fonts do not cover a run of it: `super` or `sub`, under
/// [`PositionSynthesis::SynthesizeMissing`], where `font-synthesis-position` allows.
fn decides_position(request: &FontRequest, synthesis: PositionSynthesis) -> bool {
    request.font.variant_position != FontVariantPosition::Normal
        && synthesis == PositionSynthesis::SynthesizeMissing
        && request.font.synthesis.position
}

/// A span: the clusters of one text item in one script run.
///
/// It carries the candidate lists they try and what they ask of a font.
struct SelectionSpan<'a> {
    end: ClusterId,
    /// Where its item's graphemes cross the item's edges (see
    /// [`ItemGraphemes`]).
    graphemes: ItemGraphemes,
    /// What its candidate list for text is keyed by, and that list.
    key: ListKey,
    text: CandidateListId,
    /// Its candidate list for emoji, found or made where a cluster first
    /// asks for emoji presentation.
    emoji: Option<CandidateListId>,
    resolved: ResolvedRequest<'a>,
}

/// Selects fonts for `range`'s clusters into `runs`.
///
/// Each item uses its node's text facts in the input's variant: the node's
/// own, or its `::first-line` facts for the first line. Pending faces a
/// cluster wants go in the wanted faces.
pub(super) fn select_runs(
    input: &Input<'_>,
    range: Range<ClusterId>,
    sel: &mut Selecting<'_>,
    runs: &mut FontRuns,
) {
    // The position run is the scratch's, taken while the walk borrows the
    // rest and given back after, so its capacity is kept.
    let mut position = mem::replace(&mut sel.scratch.position, PositionRun::new());
    let out = &mut Output::new(runs, &mut position);
    let FontInput {
        content, analysis, ..
    } = *input.stage;
    let default_language = input.default_language;
    let item_clusters = &analysis.item_clusters;
    let script_runs = &analysis.runs;
    let text_end = analysis.clusters.end_id();
    let mut script = script_runs.cursor(ScriptRunId::new(0), text_end);
    // The node of the item holding the cluster before the item in hand.
    let mut before: Option<NodeId> = None;
    for (item, entry) in content.items.iter() {
        if !entry.kind.has_text() {
            // An inline box's edge and a float's or an absolutely positioned
            // box's anchor leave the text on either side one position run; a
            // ruby's bounds end it, an annotation being a line of its own.
            if !matches!(
                entry.kind,
                ItemKind::Open | ItemKind::Close | ItemKind::Float | ItemKind::Absolute
            ) {
                out.close_position();
            }
            continue;
        }
        let clusters = item_clusters.range(item);
        let held_before = before;
        if !clusters.is_empty() {
            before = Some(entry.node);
        }
        let start = clusters.start.max(range.start);
        let end = clusters.end.min(range.end);
        if start >= end {
            continue;
        }
        let text = input.text(entry.node);
        let request_id = input.request(text);
        let primary = input.primary(request_id);
        // An atomic's U+FFFC and a `<br>`'s `\n` draw nothing, and end a
        // position run: the text either side of one is not contiguous.
        if entry.kind != ItemKind::Text {
            out.close_position();
            out.push_nothing_range(start..end, primary);
            continue;
        }
        let facts = &content.facts;
        let request = facts.request(request_id);
        if decides_position(request, input.stage.position_synthesis) {
            out.open_position(facts.text(text).shaping);
        } else {
            out.close_position();
        }
        let list = request.font.families;
        let matching = MatchRequest::from(request);
        let Some(&resolution) = input.resolutions.get(request_id) else {
            out.push_missing_range(start..end, primary);
            continue;
        };
        let graphemes = ItemGraphemes::new(input, item, text, clusters, held_before);
        let mut at = start;
        while at < end {
            script_runs.step_to(&mut script, at, text_end);
            let run_end = script.end().min(end).max(ClusterId::new(at.get() + 1));
            let span = script_runs.get(script.id()).and_then(|run| {
                let language = default_language.fonts_language(content.languages.get(run.language));
                let key = ListKey {
                    list,
                    script: run.script,
                    language,
                    request: matching,
                    emoji: false,
                };
                Some(SelectionSpan {
                    end: run_end,
                    graphemes,
                    key,
                    text: sel.cx.span_list(key)?,
                    emoji: None,
                    resolved: ResolvedRequest {
                        request,
                        request_id,
                        resolution,
                        script: features::opentype_script(run.script),
                    },
                })
            });
            match span {
                Some(mut span) => walk_span(input, &mut span, at, sel, out),
                None => {
                    out.push_missing_range(at..run_end, primary);
                }
            }
            at = run_end;
        }
    }
    out.close_position();
    sel.scratch.position = position;
}

/// Walks `span` from `from`, making each change the walk stops for.
///
/// The span's list for emoji is found or made here. Where the table cannot
/// take it, the span's clusters from there on are missing.
fn walk_span(
    input: &Input<'_>,
    span: &mut SelectionSpan<'_>,
    from: ClusterId,
    sel: &mut Selecting<'_>,
    out: &mut Output<'_>,
) {
    sel.scratch.walk.start_span();
    let mut at = from;
    for _ in 0..MAX_STOPS {
        let Some((change, cluster)) = walk(input, span, at, sel, out) else {
            return;
        };
        at = cluster;
        let CacheChange::EmojiList = change else {
            sel.cx.apply(change);
            continue;
        };
        let key = ListKey {
            emoji: true,
            ..span.key
        };
        span.emoji = sel.cx.span_list(key);
        if span.emoji.is_none() {
            out.push_missing_range(at..span.end, span.resolved.resolution.primary);
            return;
        }
    }
    debug_assert!(false, "a span's walk stopped {MAX_STOPS} times");
    debug_assert!(false, "a span's walk stopped {MAX_STOPS} times");
    out.push_missing_range(at..span.end, span.resolved.resolution.primary);
}

/// Where a text item's graphemes cross its edges.
///
/// A grapheme that a style boundary divides has its later parts as
/// continuation clusters. It is one grapheme where the parts' items shape alike
/// (one [`ShapingFactsId`]), and the later parts take the first part's font.
/// Read once per item from its shaping facts and its neighbours', so the
/// walk never looks up a cluster's item.
#[derive(Copy, Clone)]
struct ItemGraphemes {
    /// The item's first cluster, where it joins the part of its grapheme
    /// before it: a continuation, after an item that shapes alike.
    joined: Option<ClusterId>,
    /// The item's last cluster.
    last: ClusterId,
    /// The last cluster of the grapheme `last` starts: `last`, or past the
    /// item the later parts of its grapheme that join it, each starting an
    /// item that shapes alike.
    tail: ClusterId,
}

impl ItemGraphemes {
    /// Returns the graphemes at the edges of `item`.
    ///
    /// `text` is the item's text facts in the input's variant, `clusters`
    /// its non-empty cluster range, and `before` the node of the item holding
    /// the cluster before it. Later items are read only when the cluster
    /// after the item is a divided grapheme's later part.
    fn new(
        input: &Input<'_>,
        item: ItemId,
        text: TextFactsId,
        clusters: Range<ClusterId>,
        before: Option<NodeId>,
    ) -> Self {
        let FontInput {
            content, analysis, ..
        } = *input.stage;
        let divided = |cluster: ClusterId| analysis.clusters.is_continuation(cluster);
        let facts = &content.facts;
        let shaping = |node: NodeId| facts.text(input.text(node)).shaping;
        let own = facts.text(text).shaping;
        let joined = (divided(clusters.start) && before.is_some_and(|node| shaping(node) == own))
            .then_some(clusters.start);
        let last = ClusterId::new(clusters.end.get().saturating_sub(1));
        let mut tail = last;
        if divided(clusters.end) {
            let items = &content.items;
            for next in (ItemId::new(item.get() + 1)..ItemId::new(items.len())).ids() {
                work::step();
                let held = analysis.item_clusters.range(next);
                if held.is_empty() {
                    continue;
                }
                let joins = held.start.get() == tail.get() + 1
                    && divided(held.start)
                    && items
                        .get(next)
                        .is_some_and(|entry| entry.kind.has_text() && shaping(entry.node) == own);
                if !joins {
                    break;
                }
                tail = held.start;
                // A later cluster of the item is no grapheme's later part.
                if held.end.get() > tail.get() + 1 {
                    break;
                }
            }
        }
        Self { joined, last, tail }
    }

    /// Whether `cluster` is one grapheme with the part of its grapheme before
    /// it, whose font it takes.
    fn joins(&self, cluster: ClusterId) -> bool {
        self.joined == Some(cluster)
    }

    /// The last cluster of the grapheme `cluster` starts.
    fn grapheme_last(&self, cluster: ClusterId) -> ClusterId {
        if cluster == self.last {
            self.tail
        } else {
            cluster
        }
    }
}

/// What a span's walk carries from one cluster to the next.
struct SpanWalk<'l> {
    /// The charset page each of the text list's and the emoji list's first
    /// candidates last found.
    text_pages: Pages<'l>,
    emoji_pages: Pages<'l>,
    /// The last candidate chosen here and the used fonts it sets text in, so
    /// that a run of clusters in one font costs no lookup a cluster: the
    /// span is one style, so a candidate is the same one or two used
    /// fonts throughout.
    chosen: Option<((CandidateListId, CandidateId), Choice)>,
    /// What the chosen candidate's position feature covers, where it has one
    /// a run decides on, and the page of it last found: one bit test a
    /// character while the run stays in the candidate.
    feature_chars: Option<&'l Charset>,
    feature_page: Option<CharsetPage<'l>>,
    /// The next cluster to set, and where its text starts.
    next: ClusterId,
    start: TextOffset,
}

impl<'l> SpanWalk<'l> {
    /// Starts a walk at `from`, whose text starts at `start`, having found
    /// no page and chosen nothing.
    fn new(from: ClusterId, start: TextOffset) -> Self {
        Self {
            text_pages: Pages::new(),
            emoji_pages: Pages::new(),
            chosen: None,
            feature_chars: None,
            feature_page: None,
            next: from,
            start,
        }
    }

    /// Returns the choice `candidate` makes: the one held where it is the
    /// last candidate chosen, else what `make` makes.
    ///
    /// `make` returns the choice with what its position feature covers,
    /// `None` where the candidate has no font, or the change to make first.
    fn choice(
        &mut self,
        candidate: (CandidateListId, CandidateId),
        make: impl FnOnce() -> Result<Option<(Choice, Option<&'l Charset>)>, CacheChange>,
    ) -> Result<Option<Choice>, CacheChange> {
        if let Some((held, choice)) = self.chosen
            && held == candidate
        {
            return Ok(Some(choice));
        }
        let Some((choice, feature_chars)) = make()? else {
            return Ok(None);
        };
        self.feature_chars = feature_chars;
        self.feature_page = None;
        self.chosen = Some((candidate, choice));
        Ok(Some(choice))
    }
}

/// Walks `span` from `from` to its end, or to the first cluster the walk
/// must stop at, which it returns with the change it stopped for.
fn walk(
    input: &Input<'_>,
    span: &SelectionSpan<'_>,
    from: ClusterId,
    sel: &mut Selecting<'_>,
    out: &mut Output<'_>,
) -> Option<(CacheChange, ClusterId)> {
    let Selecting {
        cx,
        scratch,
        used,
        wanted,
    } = sel;
    let scratch = &mut scratch.walk;
    let clusters = &input.stage.analysis.clusters;
    let resolved = &span.resolved;
    let primary = resolved.resolution.primary;
    let (lists, coverages, mut fonts) = cx.walk_caches();
    let Some(text_list) = lists.get(span.text) else {
        out.push_missing_range(from..span.end, primary);
        return None;
    };
    let emoji_list = span.emoji.and_then(|id| Some((lists.get(id)?, id)));
    let span_text = SpanText {
        source: input.source,
        clusters,
        end: span.end,
        graphemes: span.graphemes,
    };
    let [text_memo, emoji_memo] = &mut scratch.memo;
    let mut state = SpanWalk::new(from, clusters.start(from));
    while state.next < span.end {
        let cluster = state.next;
        state.next = ClusterId::new(cluster.get() + 1);
        // Each cluster's attributes and end are read once, and its end is
        // where the next one starts.
        let start = state.start;
        let attrs = clusters.attrs(cluster);
        let end = clusters.end(cluster);
        state.start = end.map_or(start, ClusterEnd::end);
        // The later part of a grapheme that shapes like the part before it
        // was chosen for with that part, and takes its font.
        if span_text.graphemes.joins(cluster)
            && let Some(last) = out.last
        {
            out.push(cluster, last);
            continue;
        }
        let (class, end) = match (attrs.map(ClusterAttrs::class), end) {
            (Some(class), Some(end)) if !coverage::draws_nothing(class) => (class, end),
            _ => {
                out.push_nothing(cluster, primary);
                continue;
            }
        };
        // What the cluster is chosen for with: it, and the later parts of its
        // grapheme that join it.
        let (chars, selector) = span_text.grapheme(cluster, start, end);
        let variant = resolved.request.font.variant_emoji;
        let needs = GraphemeNeeds::new(class, chars, selector, variant);
        let (list, list_id, memo, pages) = if needs.presentation.base() == Presentation::Emoji {
            let Some((emoji_list, emoji)) = emoji_list else {
                return Some((CacheChange::EmojiList, cluster));
            };
            (emoji_list, emoji, &mut *emoji_memo, &mut state.emoji_pages)
        } else {
            (text_list, span.text, &mut *text_memo, &mut state.text_pages)
        };
        let pick =
            |needs: &GraphemeNeeds<'_>| span_text.pick(list, memo, pages, wanted, cluster, needs);
        let found = match resolve(lists, list, list_id, &needs, pick) {
            Ok(found) => found,
            Err(change) => return Some((change, cluster)),
        };
        // Nothing draws any of it: the primary font's `.notdef`.
        let Some(at) = found else {
            out.push_missing(cluster, primary);
            continue;
        };
        let made = state.choice((list_id, at), || {
            let Some(held) = list.get(at) else {
                return Ok(None);
            };
            let Some(font) = held.font() else {
                return Ok(None);
            };
            let key = resolved.key(list_id, at);
            let choice = scratch.choices.find_or_choose(key, || {
                choose(input, resolved, font, held.family(), &mut fonts, used)
            })?;
            let feature_chars = feature_coverage(coverages, choice, font, key)?;
            Ok(Some((choice, feature_chars)))
        });
        let choice = match made {
            Ok(Some(choice)) => choice,
            Ok(None) => {
                out.push_missing(cluster, primary);
                continue;
            }
            Err(change) => return Some((change, cluster)),
        };
        let upper_side = on_upper_side(choice, chars, &mut scratch.upper);
        let placed = place(
            choice,
            upper_side,
            chars,
            state.feature_chars,
            &mut state.feature_page,
            out.position,
        );
        out.push(cluster, placed);
        // The clusters after it that the text list would set in the same
        // font are set so without asking it (see `KeptCandidate`), where the
        // font sets every cluster alike and nothing waits on a position run.
        if list_id == span.text
            && shortcuts()
            && choice.same == choice.upper
            && matches!(choice.position, PositionPlan::Settled)
            && let Some(mut kept) = KeptCandidate::new(
                text_list,
                text_memo,
                &state.text_pages,
                at,
                cluster,
                span.end,
                variant != FontVariantEmoji::Normal,
            )
        {
            state.next = span_text.kept_end(&mut kept, state.next);
            state.start = clusters.start(state.next);
        }
    }
    None
}

/// Returns whether `chars` is set on the uppercased side of `choice`.
///
/// Only where the font sets the two sides of a change of case apart is a
/// cluster's case asked: most text is set alike either way. `upper` keeps
/// the answer, which a cluster that starts with a mark takes.
#[inline]
fn on_upper_side(choice: Choice, chars: &str, upper: &mut bool) -> bool {
    choice.same != choice.upper && {
        *upper = match chars.chars().next() {
            Some(ch) if is_mark(ch) => *upper,
            Some(ch) => features::changes_when_uppercased(ch),
            None => false,
        };
        *upper
    }
}

/// Returns how a cluster saying `chars` is set under `choice`, on its
/// uppercased side where `upper_side` says so.
///
/// Under a position plan, a character that lacks the feature's glyph
/// uncovers the open `position` run. `feature_chars` is what the feature
/// covers and `feature_page` the page of it last found.
#[inline]
fn place<'c>(
    choice: Choice,
    upper_side: bool,
    chars: &str,
    feature_chars: Option<&'c Charset>,
    feature_page: &mut Option<CharsetPage<'c>>,
    position: &mut PositionRun,
) -> Placed {
    let font = if upper_side {
        choice.upper
    } else {
        choice.same
    };
    match choice.position {
        PositionPlan::Settled => Placed::Font(font),
        PositionPlan::Lacking => {
            position.uncover();
            Placed::Font(font)
        }
        PositionPlan::Covering {
            same: made_same,
            upper: made_upper,
            ..
        } => {
            if position.asks() && !feature_covers(feature_chars, chars, feature_page) {
                position.uncover();
            }
            Placed::Either {
                feature: font,
                synthesized: if upper_side { made_upper } else { made_same },
            }
        }
    }
}

/// Whether `ch` extends the grapheme before it: a mark, which takes the case
/// of the letter it is on.
fn is_mark(ch: char) -> bool {
    unicode::rare_props(ch).grapheme_cluster_break() == GraphemeClusterBreak::Extend
}
