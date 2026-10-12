//! The measurement scan: one walk over a variant's clusters.
//!
//! In: the stages before measurement, shaping's advances among them. Out:
//! the prefix, written once an entry, and the scan's tables in
//! [`MeasuredText`]. Start at [`Scan::measure`], then [`Walking::boundary`].
//!
//! - `walk` holds the impl of [`ScanWalk`], which crosses the items at
//!   each boundary and enters the segment after them.
//! - `ends` works out what a line starting or ending at a boundary pays.
//! - `ruby` sizes ruby columns by looking ahead, and `overhang` works out
//!   how far a column overhangs the text beside it.
//! - `intrinsic` adds up the intrinsic widths.
//! - `em_boxes` records each text item's em box.
//! - `initial_letter` measures an initial letter.
//!
//! The scan's state types live here, so each of those files reads them.

mod em_boxes;
mod ends;
mod initial_letter;
mod intrinsic;
mod overhang;
mod ruby;
mod walk;

use alloc::vec::Vec;
use core::mem;
use core::ops::Range;

use super::autospace::{AutospaceRules, SeamContext, Seams};
use super::boxes::prepare_boxes;
use super::edges::{BoundarySplit, edge_room};
use super::spacing::{LetterWordSpacing, WordSpacingRule};
use super::tabs::TabStops;
use super::{
    Extent, GeneratedTexts, InitialLetter, MeasureFlags, MeasureInput, MeasureScratch,
    MeasuredText, TextMetrics,
};
use crate::data::{Id, IdRange, Table, make_room};
use crate::stages::analysis::{
    Analysis, ClusterAttrs, ClusterClass, ClusterId, Paragraph, ParagraphFlags, ParagraphId,
    ScriptRun,
};
use crate::stages::content::{
    AtomicId, Content, ContentFlags, FloatId, Item, ItemFlags, ItemId, ItemKind, NodeId, TextFacts,
    TextFactsId, VariantText,
};
use crate::stages::fonts::{LineBaseline, UsedFontId};
use crate::stages::shape::{ShapeSession, ShapedText};
use crate::stages::{Segment, Segments, Step};
use crate::style::{FirstLineVariant, WritingMode};
use crate::unit::InlineLayoutUnit;
use crate::work;
use em_boxes::ItemEmScan;
use ends::{AnchoredFloats, LineEndState};
use initial_letter::initial_letter;
use intrinsic::{Indent, IntrinsicScan, Tab};
use ruby::RubyBoundaries;
use walk::OverhangNeighbour;

/// What one variant's scan reads, fixed for that variant.
///
/// It holds the stages before measurement, the variant, where the scan
/// stops, the text metrics and hyphens made earlier in the stage, and the
/// block's initial letter. Its `has_*` gates say which phases the content
/// needs at all. The walk ([`ScanWalk`]) moves and this doesn't, so a
/// look-ahead copies the walk alone.
pub(super) struct Scan<'a> {
    /// The stages before measurement, and the config.
    input: &'a MeasureInput<'a>,
    /// Whose styles the text is measured in.
    variant: FirstLineVariant,
    /// The variant's shaping: the glyphs, which say which clusters continue
    /// a ligature, and the shaping runs the walk holds.
    shaped: &'a ShapedText,
    /// Each cluster's shaped advance, up to `end`.
    advances: &'a [InlineLayoutUnit],
    /// The text its clusters are read in.
    source: VariantText<'a>,
    /// Where the scan stops: the text's end, or the first paragraph's.
    end: ClusterId,
    writing_mode: WritingMode,
    /// The baseline the lines are set on. An atomic inline without a
    /// baseline of its own sits on it.
    baseline: LineBaseline,
    /// Which clusters take word-spacing: the build's rule.
    words: WordSpacingRule,
    /// The hyphens, shaped, which a line ending at a soft hyphen pays.
    generated: &'a GeneratedTexts,
    /// What each text's facts measure in its primary font.
    metrics: &'a Table<TextFactsId, TextMetrics>,
    /// The block's initial letter.
    ///
    /// Its box's edges take what its measure says, and its items take no
    /// room across the line. The look-ahead that measures it reads a scan
    /// with none.
    letter: Option<InitialLetter>,
    /// Where `text-autospace` puts room.
    ///
    /// The scan looks for seams only where some style sets it, and then only
    /// in paragraphs with an ideograph.
    autospace: AutospaceRules,
    /// The text has a tab. Tab stops are sized only then.
    has_tabs: bool,
    /// Some box has an edge that takes room. Without one, no boundary takes
    /// any.
    has_edges: bool,
    /// Some box with an edge clones its decoration where it breaks.
    has_clones: bool,
    /// Some style spaces its letters or words. Without one, the scan works
    /// out no spacing.
    has_spacing: bool,
    /// Some style sets line padding. The scan tracks it only then.
    has_padding: bool,
    /// Some style hangs punctuation. The scan tracks it only then.
    has_hanging: bool,
    /// The content has ruby. The scan looks for columns and their overhang
    /// only then.
    has_ruby: bool,
    /// Some text is combined (`text-combine-upright`). Combined text takes
    /// no spacing and ends every autospace seam.
    has_combined: bool,
    /// The plain fast path is open ([`Walking::plain_fast_path`]).
    ///
    /// No style sets anything that a cluster inside a text item could pay
    /// for beyond its advance.
    plain_fast_path_open: bool,
}

/// The scan's walk over the text, and the state that moves with it.
///
/// It holds:
/// - the segment holding the cluster after the scan's boundary, with its
///   item's text facts and its paragraph;
/// - cursors on the atomic inlines and floats;
/// - the edges of the cloned boxes open;
/// - the last leaf passed, for a ruby column's overhang.
///
/// It is `Copy`. A look-ahead over a ruby column or the initial letter
/// moves a copy, leaving the scan's own in place. Both read the same
/// [`Scan`].
#[derive(Copy, Clone)]
struct ScanWalk<'a> {
    /// The walk over the segments and the items between them.
    segments: Segments<'a>,
    /// What the walk yields next. The scan takes it when it reaches its
    /// boundary.
    ahead: Option<Step>,
    /// The segment holding the clusters from the scan's boundary to
    /// `until`.
    segment: Option<Segment>,
    /// The segment's item row, read where the scan enters the item.
    item: Option<&'a Item>,
    /// The segment's paragraph and its row, read where the scan enters the
    /// segment.
    paragraph: Option<(ParagraphId, &'a Paragraph)>,
    /// The scan looks for autospace seams in the segment's paragraph
    /// ([`Scan::has_seams`]).
    has_seams: bool,
    /// The segment's clusters are combined text.
    ///
    /// The scan reads the script run's orientation on entering the segment,
    /// and only where some text is combined.
    combined: bool,
    /// The autospace facts and direction, resolved on entering a segment.
    seam: Option<SeamContext>,
    /// Where the segment ends. No item sits at a boundary before it.
    until: ClusterId,
    /// The text facts of the segment's item, in the variant.
    text: TextFactsId,
    /// The row `text` names.
    facts: &'a TextFacts,
    /// What the item's text adds after each of its clusters.
    ///
    /// This holds on whichever line the clusters are set: an annotation's
    /// own or the base's. It is zero unless some style spaces text.
    spacing: LetterWordSpacing,
    /// The width of the U+FFFC where the item is an atomic inline: its
    /// margin box.
    atomic: Option<InlineLayoutUnit>,
    /// The edges of the cloned boxes open so far.
    cloned: EdgeAmounts,
    /// The next atomic inline not passed.
    atomics: AtomicId,
    /// The next float not passed.
    floats: FloatId,
    /// The extent of the text item the scan is in under `line-height:
    /// normal`, and the last font that widened it.
    ///
    /// The font of each of the item's segments widens it. The scan pushes it
    /// once it leaves the item.
    pending: Option<(Extent, UsedFontId)>,
    /// The last thing the scan passed that holds a place, for a ruby
    /// column's overhang.
    leaf: OverhangNeighbour,
    /// The item holding the cluster after the scan's boundary, as a leaf,
    /// and its first cluster.
    ///
    /// Once the scan is past that cluster, the item becomes `leaf`. So a
    /// ruby column starting at the boundary reads the leaf before it, not
    /// its own base's first item.
    entered: Option<(ClusterId, OverhangNeighbour)>,
    /// How many open inline boxes have struts that reach past the block's.
    ///
    /// Each paragraph where it is above zero gets `BOXES_PAST_STRUT`.
    past_strut: u32,
}

/// Amounts paid at a line's two ends: `start` by a line starting at a
/// boundary, `end` by one ending there.
///
/// They hold the edges of the cloned boxes open across a break, the line
/// padding at each end, or a ruby column's overhang.
#[derive(Copy, Clone, Debug, Default)]
struct EdgeAmounts {
    start: InlineLayoutUnit,
    end: InlineLayoutUnit,
}

impl EdgeAmounts {
    /// Returns whether both amounts are zero.
    fn is_empty(self) -> bool {
        self.start == InlineLayoutUnit::ZERO && self.end == InlineLayoutUnit::ZERO
    }
}

/// What the intrinsic sizes pay where a line starts or ends at a boundary.
///
/// The edges hold the cloned boxes' edges and the line padding, less the
/// marks that always hang there and the room of a seam a line ending there
/// gives back.
#[derive(Copy, Clone, Debug, Default)]
struct LinePay {
    edges: EdgeAmounts,
    /// What a min-content line ending here gives back beyond `edges.end`.
    ///
    /// It is a mark that hangs only where the line would not fit with it
    /// (`allow-end`), which max-content counts.
    min_end_hang: InlineLayoutUnit,
}

/// The room the items at one boundary take along the line.
///
/// The edges that end the line before a break there ([`BoundarySplit`])
/// come before the prefix entry there. The rest come after it. A ruby
/// column ending there adds its width past its base to the leading closes.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct BoundaryRoom {
    /// The boundary is inside a whole ruby column, so no line edge falls
    /// here.
    inside_column: bool,
    /// The edges that end the line before a break here.
    pub(super) leading_closes: InlineLayoutUnit,
    /// The edges that start the line after a break here.
    pub(super) rest: InlineLayoutUnit,
    /// Where a break falls among the items.
    split: BoundarySplit,
    /// The opening item of a ruby container that opens a column here.
    ruby_open: Option<ItemId>,
    /// The edges of the cloned boxes open across a break here.
    ///
    /// A break falls where [`BoundarySplit`] places it.
    across: EdgeAmounts,
    /// How far the ruby column ending here overhangs the text before it,
    /// and the one starting here the text after it.
    ///
    /// A line that ends or starts here has no such text, so it pays the
    /// overhang.
    overhang: EdgeAmounts,
    /// The floats anchored here. They take no room along the line and sit
    /// beside it.
    floats: AnchoredFloats,
}

impl BoundaryRoom {
    /// Returns no room at boundary `at`, before any of its items.
    pub(super) fn from_boundary(analysis: &Analysis, at: ClusterId) -> Self {
        Self {
            split: BoundarySplit::new(analysis, at),
            ..Self::default()
        }
    }

    /// Adds the room `item` takes.
    ///
    /// Annotation items, the annotation's own marks included, take nothing
    /// on the base's line. Floats and empty text take nothing and leave the
    /// leading closes leading. So a float anchored between two closing
    /// edges doesn't part them.
    pub(super) fn add(&mut self, content: &Content, letter: Option<&InitialLetter>, item: &Item) {
        if item.flags.contains(ItemFlags::ANNOTATION) {
            return;
        }
        let kind = item.kind;
        let (before, after) = (&mut self.leading_closes, &mut self.rest);
        if kind.is_open() || kind.is_close() {
            let room = edge_room(content, letter, item, kind.is_open());
            self.split.take(kind, room, before, after);
        } else if kind == ItemKind::Absolute {
            // An anchor takes no room, but may hold what follows it on the
            // line after.
            self.split.take(kind, InlineLayoutUnit::ZERO, before, after);
        }
    }
}

impl Scan<'_> {
    /// Measures one variant into `out`, and returns its intrinsic widths.
    ///
    /// It measures the initial letter and the fixed box shifts first, so a
    /// ruby look-ahead can read them. Then the walk turns `advances` into the
    /// prefix and records the item extents and em boxes.
    pub(super) fn measure(
        mut self,
        cx: &mut ShapeSession<'_, '_>,
        scratch: &mut MeasureScratch,
        out: &mut MeasuredText,
    ) -> IntrinsicScan {
        let (content, variant) = (self.input.content, self.variant);
        out.extents.strut = self.strut(content.nodes.text_facts(NodeId::BLOCK, variant));
        self.letter = initial_letter(&self, cx.provider(), self.advances, out.extents.strut);
        if self.letter.is_some() {
            out.rare_mut().initial_letter = self.letter;
        }
        prepare_boxes(self.input, variant, self.end, self.metrics, out);
        let mut sums = mem::take(&mut out.prefix.sums);
        let widths = self.walk(&mut sums, &mut scratch.ruby_starts, out);
        out.prefix.sums = sums;
        widths
    }

    /// Writes the prefix sums into `sums`, and `out`'s paragraph flags,
    /// extents and line-edge costs. Returns the intrinsic widths.
    ///
    /// `sums` takes one entry before each cluster and one at the end, summed
    /// from the shaped advances. The text's variant covers the whole text. The
    /// first line's variant covers the first paragraph, in its
    /// `::first-line` styles, with the same tables over the same clusters.
    /// [`Walking::boundary`] measures each boundary in turn.
    fn walk(
        &self,
        sums: &mut Vec<InlineLayoutUnit>,
        ruby_starts: &mut Vec<ClusterId>,
        out: &mut MeasuredText,
    ) -> IntrinsicScan {
        let MeasureInput {
            content, analysis, ..
        } = *self.input;
        // The text's scan writes an extent for every item and flags for every
        // paragraph. The first line's writes them up to the first
        // paragraph's end.
        if self.variant == FirstLineVariant::Standard {
            out.extents.reserve(content.items.len());
            out.paragraphs.reserve(analysis.paragraphs.len());
        }
        sums.clear();
        make_room(sums, self.end.get() + 1);
        let mut walking = Walking::new(self, sums, ruby_starts, out);
        let mut at = ClusterId::new(0);
        while at <= self.end {
            at = walking.boundary(at);
        }
        walking.finish()
    }
}

impl<'a> Scan<'a> {
    /// Returns the scan of `input` in `variant`'s styles, with no initial
    /// letter yet.
    ///
    /// Word-spacing goes where `words` says. The scan reads the hyphens in
    /// `generated` and the text metrics in `metrics`.
    pub(super) fn new(
        input: &'a MeasureInput<'a>,
        variant: FirstLineVariant,
        words: WordSpacingRule,
        generated: &'a GeneratedTexts,
        metrics: &'a Table<TextFactsId, TextMetrics>,
    ) -> Self {
        let MeasureInput {
            content,
            analysis,
            shaped,
            ..
        } = *input;
        let flags = content.flags;
        let has_edges = flags.contains(ContentFlags::BOXES_WITH_EDGES);
        let has_padding = flags.contains(ContentFlags::LINE_PADDING);
        let has_hanging = flags.contains(ContentFlags::HANGING_PUNCTUATION);
        let has_ruby = flags.contains(ContentFlags::RUBY);
        let shaped = shaped.text(variant);
        let end = input.reach(variant).end;
        Self {
            input,
            variant,
            shaped,
            // A missing advance is a bug in shaping; its cluster then
            // measures zero.
            advances: shaped.advances.get(..end.get()).unwrap_or(&shaped.advances),
            source: content.text(variant),
            end,
            writing_mode: content.block.writing_mode,
            baseline: LineBaseline::from_content(content),
            words,
            generated,
            metrics,
            letter: None,
            autospace: AutospaceRules::new(content),
            has_tabs: analysis.flags.contains(ParagraphFlags::HAS_TABS),
            has_edges,
            has_clones: has_edges && flags.contains(ContentFlags::CLONE_BOXES),
            has_spacing: flags.contains(ContentFlags::NONZERO_SPACING),
            has_padding,
            has_hanging,
            has_ruby,
            has_combined: analysis.flags.contains(ParagraphFlags::HAS_COMBINED),
            plain_fast_path_open: work::fast_paths() && !has_ruby && !has_padding && !has_hanging,
        }
    }

    /// Returns the script run of `segment`, through its shaping run's link.
    ///
    /// Each step is one indexed load.
    fn script(&self, segment: &Segment) -> Option<&'a ScriptRun> {
        let run = self.shaped.runs.get(segment.run)?;
        self.input.analysis.runs.get(run.script_run)
    }

    /// Returns the used font `segment` is shaped in.
    fn font(&self, segment: &Segment) -> Option<UsedFontId> {
        self.shaped.runs.get(segment.run).map(|run| run.font)
    }

    /// Returns whether the scan looks for autospace seams in `para`.
    fn has_seams(&self, para: &Paragraph) -> bool {
        self.autospace.any() && para.flags.contains(ParagraphFlags::HAS_EAST_ASIAN)
    }

    /// Returns the strut of text with the facts `text`.
    fn strut(&self, text: TextFactsId) -> Extent {
        self.metrics
            .get(text)
            .map_or(Extent::NONE, |metrics| metrics.strut)
    }

    /// Returns where the tabs of text with the facts `text` stop.
    ///
    /// Returns `None` where the text has no tab.
    fn tab_stops(&self, text: TextFactsId) -> Option<TabStops> {
        if !self.has_tabs {
            return None;
        }
        self.metrics.get(text).map(|metrics| metrics.tab)
    }

    /// Returns the paragraph holding the cluster at `at`, and the paragraph
    /// the boundary at `at` belongs to.
    ///
    /// The walk `walk` stands at `at`, and `paragraph` is the previous
    /// boundary's. At the scan's end no cluster follows, and the boundary
    /// belongs to the text's last paragraph, or to the one starting there.
    #[inline]
    fn boundary_paragraphs(
        &self,
        walk: &ScanWalk<'a>,
        at: ClusterId,
        paragraph: ParagraphId,
    ) -> (Option<(ParagraphId, &'a Paragraph)>, ParagraphId) {
        let analysis = self.input.analysis;
        let last = ParagraphId::new(analysis.paragraphs.len().saturating_sub(1));
        let held = walk.paragraph.filter(|_| at < self.end);
        let next = match held {
            Some((id, _)) => id,
            None if at == analysis.clusters.end_id() => last,
            None if at == self.end => ParagraphId::new(paragraph.get() + 1).min(last),
            None => paragraph,
        };
        (held, next)
    }
}

/// One variant's walk over its boundaries: the scan's walk, the tables it
/// writes, and what it carries from one boundary to the next.
struct Walking<'w, 'a> {
    scan: &'w Scan<'a>,
    /// The prefix sums before the walk's boundary.
    sums: &'w mut Vec<InlineLayoutUnit>,
    /// The ruby look-ahead's cursors, one per level.
    ruby_starts: &'w mut Vec<ClusterId>,
    out: &'w mut MeasuredText,
    walk: ScanWalk<'a>,
    seams: Seams,
    em_boxes: Option<ItemEmScan>,
    ends: LineEndState,
    widths: IntrinsicScan,
    /// The paragraph the last boundary belongs to, and what was found of it
    /// so far.
    paragraph: ParagraphId,
    flags: MeasureFlags,
    writer: PrefixWriter,
    /// What a line starting or ending at the last boundary pays, as the
    /// intrinsic sizes count it.
    cost: LinePay,
    ruby: RubyBoundaries,
    /// The text's last line ends at its end. Where the text ends in a
    /// separator, its last paragraph is empty and holds no line.
    ends_on_a_line: bool,
}

impl<'w, 'a> Walking<'w, 'a> {
    /// Returns the walk of `scan` before its first boundary, writing `prefix`
    /// and `out`.
    fn new(
        scan: &'w Scan<'a>,
        sums: &'w mut Vec<InlineLayoutUnit>,
        ruby_starts: &'w mut Vec<ClusterId>,
        out: &'w mut MeasuredText,
    ) -> Self {
        let content = scan.input.content;
        let em_boxes = ItemEmScan::new(scan, out);
        Self {
            scan,
            sums,
            ruby_starts,
            out,
            walk: ScanWalk::new(scan),
            seams: Seams::new(scan.autospace),
            em_boxes,
            ends: LineEndState::default(),
            widths: IntrinsicScan::new(Indent::new(content, scan.letter.is_some())),
            paragraph: ParagraphId::new(0),
            flags: MeasureFlags::NONE,
            writer: PrefixWriter::default(),
            cost: LinePay::default(),
            ruby: RubyBoundaries::new(),
            ends_on_a_line: scan.input.analysis.paragraphs.ends_on_a_line(),
        }
    }

    /// Measures boundary `at` and the cluster after it, and returns the next
    /// boundary to measure.
    ///
    /// Each phase is a call:
    /// 1. The walk crosses the items there into the next segment
    ///    ([`ScanWalk::cross`]).
    /// 2. A ruby column ending there closes, and one starting there is sized
    ///    ([`RubyBoundaries::cross`]).
    /// 3. The cluster after it is measured ([`ScanWalk::step`]).
    /// 4. A line starting or ending there is charged ([`LineEndState::cost`]).
    /// 5. The paragraphs ending there close
    ///    ([`end_paragraphs`](Self::end_paragraphs)), and the prefix entry is
    ///    written here.
    /// 6. What a line ending after the cluster ends with is recorded
    ///    ([`record_line_end`](Self::record_line_end)).
    /// 7. The plain fast path runs on from there where it is open
    ///    ([`plain_fast_path`](Self::plain_fast_path)).
    fn boundary(&mut self, at: ClusterId) -> ClusterId {
        let scan = self.scan;
        let clusters = &scan.input.analysis.clusters;
        let (end, text_start) = (scan.end, ClusterId::new(0));
        let next_at = ClusterId::new(at.get() + 1);
        let (mut room, entered) = self.walk.cross(scan, at, Some(&mut self.out.extents));
        if entered && at < end {
            self.enter_em_box();
        }
        if self.walk.past_strut > 0 {
            self.flags.insert(MeasureFlags::BOXES_PAST_STRUT);
        }
        let (held, next) = scan.boundary_paragraphs(&self.walk, at, self.paragraph);
        let starts_paragraph = held.is_some_and(|(_, para)| para.start == at);
        if scan.has_ruby {
            let sum = self.writer.sum;
            let prefix = ScanPrefix {
                sums: self.sums,
                advances: scan.advances,
                end: scan.end,
            };
            self.ruby.cross(
                scan,
                &self.walk,
                self.seams,
                prefix,
                self.ruby_starts,
                at,
                sum,
                &mut room,
                self.out,
            );
        }
        if at == clusters.end_id() && self.ends_on_a_line {
            // No line starts at the text's end, so nothing there is the
            // next line's: an empty box ending the text, or one left
            // open, is the last line's.
            room.leading_closes += room.rest;
            room.rest = InlineLayoutUnit::ZERO;
        }
        // Measure the cluster after the boundary before the boundary's
        // costs, since a mark hanging at a line's start is that cluster.
        // It adds its advance, the spacing after it, and the room of a
        // seam after it. The shaper puts `text-spacing-trim` in the
        // advance.
        let attrs = clusters.attrs(at);
        let class = attrs.map_or(ClusterClass::Text, ClusterAttrs::class);
        let seams = held.is_some() && self.walk.has_seams;
        let gap = if seams {
            InlineLayoutUnit::from_text(self.walk.seam_after(scan, &mut self.seams, at))
        } else {
            InlineLayoutUnit::ZERO
        };
        let step = if at < end {
            let advance = scan.advances.get(at.get()).copied().unwrap_or_default();
            self.walk.step(scan, class, advance, at) + gap
        } else {
            InlineLayoutUnit::ZERO
        };
        let column = self.ruby.column;
        room.inside_column = column.is_some_and(|open| open.start < at && at < open.end);
        let (paid, edge) = (self.ends).cost(scan, at, starts_paragraph, &room, &self.walk, step);
        let has_cost = edge.is_some();
        if let Some(edge) = edge {
            self.out.rare_mut().edge_costs.push(edge);
            self.flags.insert(MeasureFlags::HAS_EDGE_COSTS);
        }
        self.cost = paid;
        // The leading closing edges end the line before a break here,
        // and the paragraph before, where one ends here.
        self.writer.sum += room.leading_closes;
        // The prefix going back from the last boundary to this one marks
        // the paragraph, which may end here. A dip between the two
        // boundaries that recovers by this one doesn't count: nobody
        // reads it.
        if at > text_start && self.writer.went_back() {
            self.flags.insert(MeasureFlags::NONMONOTONE);
        }
        if at == text_start {
            self.widths.start_text(self.cost);
        }
        if let Some((whole, first, last, widest)) = self.ruby.ended.take() {
            self.widths
                .ruby(self.writer.sum, whole, first, last, widest);
        }
        self.widths.boundary(self.writer.sum, self.cost);
        self.ends.pass_room(room.leading_closes);
        if self.paragraph < next {
            // The cost is the next paragraph's start's too, and so are
            // the boxes open across it.
            let mut carried = MeasureFlags::NONE;
            if has_cost {
                carried.insert(MeasureFlags::HAS_EDGE_COSTS);
            }
            if self.walk.past_strut > 0 {
                carried.insert(MeasureFlags::BOXES_PAST_STRUT);
            }
            self.end_paragraphs(next, carried);
        }
        if column.is_some_and(|open| open.may_break) {
            self.flags.insert(MeasureFlags::HAS_BREAKABLE_RUBY);
        }
        // The floats anchored here sit beside the paragraph the boundary
        // starts. A float after a forced break is the next line's, as
        // Chrome's breaker leaves it.
        self.widths.floats(room.floats);
        self.writer.write(self.sums);
        // The rest start the line after it.
        self.writer.sum += room.rest;
        self.ends.pass_room(room.rest);
        // The scan's end has no cluster after it.
        if at == end {
            return next_at;
        }
        // A tab takes nothing in the prefix. In the intrinsic widths it
        // takes the distance from the pen to its stop.
        let tab = match class {
            ClusterClass::Tab if !self.walk.annotation() => {
                let reach = self.out.ruby_columns().tab_reach(at);
                let tab = (scan.tab_stops(self.walk.text)).map_or(Tab::NONE, |stops| {
                    self.widths.tab(self.writer.sum, stops, reach)
                });
                if scan.has_ruby {
                    self.ruby.pass_tab(at, tab.paragraph_reach());
                }
                tab
            }
            _ => Tab::NONE,
        };
        self.writer.sum += step;
        if gap != InlineLayoutUnit::ZERO {
            self.flags.insert(MeasureFlags::HAS_AUTOSPACE);
        }
        // Intrinsic sizing still treats each column as one object.
        let outer_attrs = attrs.map(|attrs| {
            let inside = column.is_some_and(|open| at.get() + 1 < open.end.get());
            if inside {
                attrs.without(ClusterAttrs::BREAK_AFTER | ClusterAttrs::EMERGENCY_AFTER)
            } else {
                attrs
            }
        });
        (self.widths).cluster(outer_attrs, step, tab, self.walk.facts.flags);
        self.record_line_end(at, class, (attrs, outer_attrs), gap, step);
        // Run the plain fast path from the next boundary where its gate holds:
        // the fast path is open for the content, the item is plain, and the
        // paragraph has no autospace seams.
        if scan.plain_fast_path_open
            && gap == InlineLayoutUnit::ZERO
            && self.walk.is_plain()
            && held.is_some()
            && !self.walk.has_seams
        {
            return self.plain_fast_path(next_at);
        }
        next_at
    }

    /// Widens the em box of the item the walk is in by the segment it just
    /// entered.
    fn enter_em_box(&mut self) {
        if let Some(em) = &mut self.em_boxes
            && let Some(segment) = &self.walk.segment
        {
            em.enter(self.scan, segment, self.out);
        }
    }

    /// Ends each paragraph from the walk's up to `next`, the one the
    /// boundary belongs to.
    ///
    /// Each paragraph ended writes its flags and its intrinsic widths at the
    /// running sum with the boundary's cost. The next starts with fresh
    /// line-end state and the flags `carried` across the boundary.
    fn end_paragraphs(&mut self, next: ParagraphId, carried: MeasureFlags) {
        let sum = self.writer.sum;
        while self.paragraph < next {
            self.widths.end_paragraph(sum, self.cost);
            (self.out.paragraphs)
                .push_bounded(self.flags, "no more paragraphs than a ParagraphId names");
            self.flags = carried;
            self.paragraph = ParagraphId::new(self.paragraph.get() + 1);
            self.ends = LineEndState::default();
        }
    }

    /// Records what a line ending after the cluster at `at` ends with.
    ///
    /// - The room of the seam after the cluster, `gap`, which the line gives
    ///   back.
    /// - After a soft hyphen a line may break after, the hyphen, its width
    ///   snapped to the grid as Chrome's `HyphenResult::InlineSize` is.
    /// - Where the cluster doesn't hang, its padding and the mark that may
    ///   hang. `step` is its advance with `gap`.
    ///
    /// The cluster has `class` and its own `attrs`, and `outer_attrs` as the
    /// intrinsic sizes read it.
    #[inline]
    fn record_line_end(
        &mut self,
        at: ClusterId,
        class: ClusterClass,
        (attrs, outer_attrs): (Option<ClusterAttrs>, Option<ClusterAttrs>),
        gap: InlineLayoutUnit,
        step: InlineLayoutUnit,
    ) {
        let (scan, walk, ends) = (self.scan, &self.walk, &mut self.ends);
        ends.seam_given = gap;
        if class == ClusterClass::SoftHyphen
            && !walk.annotation()
            && outer_attrs.is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER))
        {
            let generated = scan.generated;
            ends.hyphen_owed =
                (generated.hyphen(walk.text)).map(|hyphen| generated.snapped(hyphen));
        }
        let hangs = attrs.is_some_and(|attrs| attrs.has(ClusterAttrs::HANGS));
        if (scan.has_padding || scan.has_hanging) && !hangs {
            // A mark hanging past a line's end hangs its own advance. A line
            // ending after it gives the seam's room back already.
            let clusters = &scan.input.analysis.clusters;
            ends.content(walk, step - gap, || clusters.first_char(scan.source, at));
        }
    }

    /// Runs the plain fast path from boundary `from`: a tight loop over
    /// boundaries where each cluster pays only its advance.
    ///
    /// The fast path runs to the end of the current item, its paragraph, or
    /// the scan. It writes each boundary's sum into the prefix, and moves the
    /// walk into the item's next segment where a shaping run ends inside the
    /// item. It marks the paragraph where the prefix goes back, and keeps the
    /// intrinsic widths up to date.
    ///
    /// A soft hyphen a line may break after owes its hyphen to the boundary
    /// after it, which charges it ([`LineEndState::take_plain_hyphen`]).
    ///
    /// Returns the boundary it stopped at: the fast path's end, or the
    /// boundary before a sized tab or an atomic inline's U+FFFC. The general
    /// body handles those, and a hyphen still owed there.
    ///
    /// The fast path is exact because of its gate. The content has no ruby,
    /// line padding or hanging punctuation. The paragraph has no autospace
    /// seam. The item has no spacing, cloned edge or annotation. So every
    /// cost the general body would work out here but the hyphen's is zero.
    #[inline(always)]
    fn plain_fast_path(&mut self, from: ClusterId) -> ClusterId {
        let scan = self.scan;
        let clusters = &scan.input.analysis.clusters;
        let mut at = from;
        loop {
            let fast_end = self.walk.until.min(scan.end);
            let text = self.walk.facts.flags;
            let Self {
                sums,
                writer,
                flags,
                widths,
                ends,
                out,
                walk,
                ..
            } = self;
            while at < fast_end {
                let attrs = clusters.attrs(at);
                let class = attrs.map_or(ClusterClass::Text, ClusterAttrs::class);
                let tab = class == ClusterClass::Tab && scan.has_tabs;
                if tab || class == ClusterClass::Object {
                    return at;
                }
                let pay = match ends.take_plain_hyphen(at) {
                    Some((pay, edge)) => {
                        out.rare_mut().edge_costs.push(edge);
                        flags.insert(MeasureFlags::HAS_EDGE_COSTS);
                        pay
                    }
                    None => LinePay::default(),
                };
                if writer.went_back() {
                    flags.insert(MeasureFlags::NONMONOTONE);
                }
                widths.boundary(writer.sum, pay);
                let advance = scan.advances.get(at.get()).copied().unwrap_or_default();
                writer.write(sums);
                writer.sum += advance;
                widths.cluster(attrs, advance, Tab::NONE, text);
                if class == ClusterClass::SoftHyphen
                    && attrs.is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER))
                {
                    let generated = scan.generated;
                    ends.hyphen_owed =
                        (generated.hyphen(walk.text)).map(|hyphen| generated.snapped(hyphen));
                }
                at = ClusterId::new(at.get() + 1);
            }
            if at == fast_end && at < scan.end && self.walk.continues(scan, &mut self.out.extents) {
                self.enter_em_box();
                continue;
            }
            return at;
        }
    }

    /// Ends the walk past its last boundary, and returns the intrinsic
    /// widths.
    fn finish(mut self) -> IntrinsicScan {
        let scan = self.scan;
        let analysis = scan.input.analysis;
        // The last text item's extent, which its fonts widened to the end.
        self.walk.flush(&mut self.out.extents);
        if let Some(em) = self.em_boxes.take() {
            em.finish(scan, self.out);
        }
        // The last paragraph ends at the text's end, after its closing
        // edges. A line ending there pays what the last boundary charged. A
        // scan that stops at the first paragraph's end has ended it already.
        if scan.end == analysis.clusters.end_id() {
            let sum = self.sums.last().copied().unwrap_or_default();
            self.widths.end_paragraph(sum, self.cost);
            if analysis.paragraphs.get(self.paragraph).is_some() {
                (self.out.paragraphs)
                    .push_bounded(self.flags, "no more paragraphs than a ParagraphId names");
            }
        }
        self.widths
    }
}

/// The one writer of the prefix sums, in the general body and the plain fast path
/// alike.
///
/// It holds the running sum and the prefix entry at the last boundary. The
/// next boundary is compared with that entry, since the breaker reads the
/// prefix at boundaries only.
#[derive(Copy, Clone, Debug, Default)]
struct PrefixWriter {
    sum: InlineLayoutUnit,
    last: InlineLayoutUnit,
}

impl PrefixWriter {
    /// Returns whether the sum is under the prefix at the last boundary,
    /// which means the prefix goes back.
    fn went_back(self) -> bool {
        self.sum < self.last
    }

    /// Appends the sum to `sums`, as the prefix entry at the current
    /// boundary.
    #[inline]
    fn write(&mut self, sums: &mut Vec<InlineLayoutUnit>) {
        sums.push(self.sum);
        self.last = self.sum;
    }
}

/// The prefix as a ruby look-ahead reads it at the scan's boundary: the
/// running sums before the boundary, and the shaped advances from it on.
#[derive(Copy, Clone)]
pub(super) struct ScanPrefix<'s> {
    /// The sums written, one before each boundary the scan passed.
    sums: &'s [InlineLayoutUnit],
    /// Each cluster's shaped advance, up to `end`.
    advances: &'s [InlineLayoutUnit],
    /// Where the scan stops.
    end: ClusterId,
}

impl ScanPrefix<'_> {
    /// Returns the entry at `at`: a sum before the scan's boundary, an
    /// advance from it on, and zero at the scan's end. Returns `None` past
    /// the end.
    pub(super) fn get(self, at: ClusterId) -> Option<InlineLayoutUnit> {
        match self.sums.get(at.get()) {
            Some(&sum) => Some(sum),
            None if at <= self.end => {
                Some(self.advances.get(at.get()).copied().unwrap_or_default())
            }
            None => None,
        }
    }

    /// Returns the sum of the entries at `clusters`, or zero where they
    /// reach past the scan's end.
    pub(super) fn total(self, clusters: Range<ClusterId>) -> InlineLayoutUnit {
        if clusters.end > ClusterId::new(self.end.get() + 1) {
            return InlineLayoutUnit::ZERO;
        }
        clusters.ids().fold(InlineLayoutUnit::ZERO, |sum, at| {
            sum + self.get(at).unwrap_or_default()
        })
    }
}
