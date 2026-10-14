//! The measurement scan: one walk over a variant's clusters.
//!
//! It writes each prefix entry once, and the scan's tables in
//! [`MeasuredText`]. Start at [`Scan::measure`], then [`Walking::boundary`].
//!
//! - `walk`: [`ScanWalk`], crossing the items at each boundary.
//! - `ends`: what a line starting or ending at a boundary pays.
//! - `ruby`: ruby columns, sized by looking ahead; `overhang`: how far a
//!   column overhangs the text beside it.
//! - `intrinsic`: the intrinsic widths.
//! - `em_boxes`: each text item's em box.
//! - `initial_letter`: the initial letter.

mod em_boxes;
mod ends;
mod initial_letter;
mod intrinsic;
mod overhang;
mod ruby;
mod walk;

use alloc::vec::Vec;
use core::cell::Cell;
use core::mem;
use core::ops::Range;

use super::autospace::{AutospaceRules, SeamContext, Seams};
use super::boxes::prepare_boxes;
use super::edges::{BoundarySplit, boundary_room, edge_room};
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
use crate::unit::{InlineLayoutUnit, TextUnit};
use crate::work;
use em_boxes::ItemEmScan;
use ends::{AnchoredFloats, LineEndState};
use initial_letter::initial_letter;
use intrinsic::{Indent, IntrinsicScan, Tab};
use ruby::RubyBoundaries;
use walk::OverhangNeighbour;

/// What one variant's scan reads, fixed for that variant.
///
/// The `has_*` gates say which phases the content needs. Only the walk
/// ([`ScanWalk`]) moves, so a look-ahead copies the walk alone.
pub(super) struct Scan<'a> {
    /// The stages before measurement, and the config.
    input: &'a MeasureInput<'a>,
    /// Whose styles the text is measured in.
    variant: FirstLineVariant,
    /// The variant's shaping: glyphs, which mark ligature continuations, and
    /// the shaping runs.
    shaped: &'a ShapedText,
    /// Each cluster's shaped advance, up to `end`.
    advances: &'a [TextUnit],
    /// The text its clusters are read in.
    source: VariantText<'a>,
    /// Where the scan stops: the text's end, or the first paragraph's.
    end: ClusterId,
    writing_mode: WritingMode,
    /// The baseline the lines are set on. An atomic inline without a
    /// baseline of its own sits on it.
    baseline: LineBaseline,
    /// Which clusters take word-spacing.
    words: WordSpacingRule,
    /// The shaped hyphens, which a line ending at a soft hyphen pays.
    generated: &'a GeneratedTexts,
    /// Each text's primary-font metrics.
    metrics: &'a Table<TextFactsId, TextMetrics>,
    /// The block's initial letter: its box's edges take what its measure says,
    /// and its items take no room across the line.
    ///
    /// The look-ahead that measures it reads a scan with none.
    letter: Option<InitialLetter>,
    /// Where `text-autospace` puts room. Seams are sought only in paragraphs
    /// with an ideograph.
    autospace: AutospaceRules,
    /// The text has a tab, so tab stops are sized.
    has_tabs: bool,
    /// Some box has an edge that takes room.
    has_edges: bool,
    /// Some box with an edge clones its decoration where it breaks.
    has_clones: bool,
    /// Some style spaces its letters or words.
    has_spacing: bool,
    /// Some style sets line padding.
    has_padding: bool,
    /// Some style hangs punctuation.
    has_hanging: bool,
    /// The content has ruby.
    has_ruby: bool,
    /// Some text is combined (`text-combine-upright`). Combined text takes
    /// no spacing and ends every autospace seam.
    has_combined: bool,
    /// Some paragraph has a soft hyphen. Without one, no line ending owes a
    /// hyphen, and the plain fast path skips its two hyphen tests a cluster.
    has_soft_hyphens: bool,
    /// The plain fast path is open ([`Walking::plain_fast_path`]): no style
    /// makes a text cluster pay beyond its advance.
    plain_fast_path_open: bool,
}

/// The scan's walk over the text, and the state that moves with it.
///
/// It is `Copy`: a look-ahead over a ruby column or the initial letter moves
/// a copy, reading the same [`Scan`].
#[derive(Copy, Clone)]
struct ScanWalk<'a> {
    /// The walk over the segments and the items between them.
    segments: Segments<'a>,
    /// What the walk yields next, taken at its boundary.
    ahead: Option<Step>,
    /// The segment holding the clusters from the scan's boundary to `until`.
    segment: Option<Segment>,
    /// The segment's item row.
    item: Option<&'a Item>,
    /// The segment's paragraph and its row.
    paragraph: Option<(ParagraphId, &'a Paragraph)>,
    /// The scan looks for autospace seams in the segment's paragraph
    /// ([`Scan::has_seams`]).
    has_seams: bool,
    /// The segment's clusters are combined text. Read only where some text is
    /// combined.
    combined: bool,
    /// The autospace facts and direction of the segment.
    seam: Option<SeamContext>,
    /// Where the segment ends. No item sits at a boundary before it.
    until: ClusterId,
    /// The text facts of the segment's item, in the variant.
    text: TextFactsId,
    /// The row `text` names.
    facts: &'a TextFacts,
    /// What the item's text adds after each cluster, on whichever line it is
    /// set. Zero unless some style spaces text.
    spacing: LetterWordSpacing,
    /// An atomic inline's margin box, the width of its U+FFFC.
    atomic: Option<InlineLayoutUnit>,
    /// The edges of the cloned boxes open.
    cloned: EdgeAmounts,
    /// The next atomic inline not passed.
    atomics: AtomicId,
    /// The next float not passed.
    floats: FloatId,
    /// The current text item's extent under `line-height: normal`, and the
    /// last font that widened it. Pushed once the scan leaves the item.
    pending: Option<(Extent, UsedFontId)>,
    /// The last thing passed that holds a place, for ruby overhang.
    leaf: OverhangNeighbour,
    /// The item holding the cluster after the boundary, as a leaf, and its
    /// first cluster.
    ///
    /// It becomes `leaf` only once the scan passes that cluster, so a ruby
    /// column starting at the boundary reads the leaf before it.
    entered: Option<(ClusterId, OverhangNeighbour)>,
    /// How many open inline boxes have struts past the block's. Each paragraph
    /// where it is above zero gets `BOXES_PAST_STRUT`.
    past_strut: u32,
}

/// Amounts paid at a line's two ends: `start` by a line starting at a
/// boundary, `end` by one ending there.
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
/// The edges leave out marks that always hang and the room of a seam a line
/// ending there gives back.
#[derive(Copy, Clone, Debug, Default)]
struct LinePay {
    edges: EdgeAmounts,
    /// What a min-content line ending here also gives back: a mark that hangs
    /// only where the line would not fit (`allow-end`).
    min_end_hang: InlineLayoutUnit,
}

/// The room the items at one boundary take along the line.
///
/// `leading_closes` comes before the prefix entry there, `rest` after it. A
/// ruby column ending there adds its width past its base to the leading
/// closes.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct BoundaryRoom {
    /// The boundary is inside a whole ruby column, so no line edge falls here.
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
    across: EdgeAmounts,
    /// How far the ruby column ending here overhangs the text before it, and
    /// the one starting here the text after it. A line ending or starting here
    /// pays it.
    overhang: EdgeAmounts,
    /// The floats anchored here, beside the line.
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
    /// Annotation items take nothing on the base's line. Floats and empty text
    /// take nothing and leave the leading closes leading, so a float between
    /// two closing edges doesn't part them.
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
            // An anchor takes no room, but may hold what follows it on the next line.
            self.split.take(kind, InlineLayoutUnit::ZERO, before, after);
        }
    }
}

impl Scan<'_> {
    /// Returns cluster `at`'s shaped advance as a 48.16 `InlineLayoutUnit`,
    /// or zero past `end` or where shaping wrote none.
    #[inline]
    fn advance(&self, at: ClusterId) -> InlineLayoutUnit {
        InlineLayoutUnit::from_text(self.advances.get(at.get()).copied().unwrap_or_default())
    }

    /// Measures one variant into `out`, and returns its intrinsic widths.
    ///
    /// The initial letter and the fixed box shifts come first, for a ruby
    /// look-ahead to read.
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
    /// `sums` gets one entry before each cluster and one at the end.
    fn walk(
        &self,
        sums: &mut Vec<InlineLayoutUnit>,
        ruby_starts: &mut Vec<ClusterId>,
        out: &mut MeasuredText,
    ) -> IntrinsicScan {
        let MeasureInput {
            content, analysis, ..
        } = *self.input;
        // The first line's scan writes only up to the first paragraph's end.
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
            // A missing advance is a bug in shaping; its cluster measures zero.
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
            has_soft_hyphens: analysis.flags.contains(ParagraphFlags::HAS_SOFT_HYPHEN),
            plain_fast_path_open: work::fast_paths() && !has_ruby && !has_padding && !has_hanging,
        }
    }

    /// Returns the script run of `segment`, through its shaping run's link.
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

    /// Returns `text`'s strut.
    fn strut(&self, text: TextFactsId) -> Extent {
        self.metrics
            .get(text)
            .map_or(Extent::NONE, |metrics| metrics.strut)
    }

    /// Returns `text`'s tab stops, or `None` where the text has no tab.
    fn tab_stops(&self, text: TextFactsId) -> Option<TabStops> {
        if !self.has_tabs {
            return None;
        }
        self.metrics.get(text).map(|metrics| metrics.tab)
    }

    /// Returns the paragraph holding the cluster at `at`, and the paragraph
    /// the boundary at `at` belongs to.
    ///
    /// `paragraph` is the previous boundary's. At the scan's end the boundary
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

/// One variant's walk over its boundaries, and the tables it writes.
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
    /// The paragraph the last boundary belongs to, and its flags so far.
    paragraph: ParagraphId,
    flags: MeasureFlags,
    writer: PrefixWriter,
    /// What a line starting or ending at the last boundary pays, for the
    /// intrinsic sizes.
    cost: LinePay,
    ruby: RubyBoundaries,
    /// The text's last line ends at its end; false where the text ends in a
    /// separator, leaving an empty last paragraph.
    ends_on_a_line: bool,
}

impl<'w, 'a> Walking<'w, 'a> {
    /// Returns the walk of `scan` before its first boundary.
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
    /// boundary.
    ///
    /// The phases, in order:
    /// 1. [`ScanWalk::cross`] crosses the items there.
    /// 2. [`RubyBoundaries::cross`] closes and opens ruby columns.
    /// 3. [`ScanWalk::step`] measures the cluster after it.
    /// 4. [`LineEndState::cost`] charges a line starting or ending there.
    /// 5. [`end_paragraphs`](Self::end_paragraphs) closes paragraphs, and the
    ///    prefix entry is written.
    /// 6. [`record_line_end`](Self::record_line_end) records what a line
    ///    ending after the cluster ends with.
    /// 7. [`plain_fast_path`](Self::plain_fast_path) runs on where it is open.
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
                (sum, self.widths.paragraph_offset()),
                &mut room,
                self.out,
            );
        }
        if at == clusters.end_id() && self.ends_on_a_line {
            // No line starts at the text's end: an empty or open box there is the
            // last line's.
            room.leading_closes += room.rest;
            room.rest = InlineLayoutUnit::ZERO;
        }
        // The cluster after the boundary is measured before the boundary's costs:
        // a mark hanging at a line's start is that cluster. Its advance includes
        // `text-spacing-trim`.
        let attrs = clusters.attrs(at);
        let class = attrs.map_or(ClusterClass::Text, ClusterAttrs::class);
        let seams = held.is_some() && self.walk.has_seams;
        let gap = if seams {
            InlineLayoutUnit::from_text(self.walk.seam_after(scan, &mut self.seams, at))
        } else {
            InlineLayoutUnit::ZERO
        };
        let step = if at < end {
            let advance = scan.advance(at);
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
        // The leading closes end the line before a break here.
        self.writer.sum += room.leading_closes;
        // A dip in the prefix between boundaries doesn't count: the breaker reads
        // only boundaries.
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
            // The cost and the open boxes carry into the next paragraph.
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
        // The floats anchored here sit beside the paragraph the boundary starts.
        // A float after a forced break is the next line's, as in Chrome's breaker.
        self.widths.floats(room.floats);
        self.writer.write(self.sums);
        // The rest start the line after.
        self.writer.sum += room.rest;
        self.ends.pass_room(room.rest);
        if at == end {
            return next_at;
        }
        // A tab takes nothing in the prefix. The intrinsic widths size it from the
        // pen to its stop.
        let tab = match class {
            ClusterClass::Tab if !self.walk.annotation() => {
                let reach = self.out.ruby_columns().tab_reach(at);
                let tab = (scan.tab_stops(self.walk.text)).map_or(Tab::NONE, |stops| {
                    self.widths.tab(self.writer.sum, stops, reach)
                });
                if scan.has_ruby {
                    self.ruby.pass_tab(scan, at, tab.paragraph_reach());
                }
                tab
            }
            _ => Tab::NONE,
        };
        self.writer.sum += step;
        if gap != InlineLayoutUnit::ZERO {
            self.flags.insert(MeasureFlags::HAS_AUTOSPACE);
        }
        // No break inside a ruby column: `IntrinsicScan::ruby` adds its pieces.
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
        // The gate: the content allows the fast path, the item is plain, and the
        // paragraph has no seams.
        if scan.plain_fast_path_open
            && gap == InlineLayoutUnit::ZERO
            && self.walk.is_plain()
            && held.is_some()
            && !self.walk.has_seams
        {
            return if scan.has_soft_hyphens {
                self.plain_fast_path::<true>(next_at)
            } else {
                self.plain_fast_path::<false>(next_at)
            };
        }
        next_at
    }

    /// Widens the current item's em box by the segment just entered.
    fn enter_em_box(&mut self) {
        if let Some(em) = &mut self.em_boxes
            && let Some(segment) = &self.walk.segment
        {
            em.enter(self.scan, segment, self.out);
        }
    }

    /// Ends each paragraph up to `next`, the one the boundary belongs to.
    ///
    /// Each writes its flags and intrinsic widths. The next starts with fresh
    /// line-end state and the flags `carried` across the boundary.
    fn end_paragraphs(&mut self, next: ParagraphId, carried: MeasureFlags) {
        let sum = self.writer.sum;
        while self.paragraph < next {
            if self.flags.contains(MeasureFlags::NONMONOTONE) {
                self.redo_min_lines(sum);
            }
            self.widths.end_paragraph(sum, self.cost);
            (self.out.paragraphs)
                .push_bounded(self.flags, "no more paragraphs than a ParagraphId names");
            self.flags = carried;
            self.paragraph = ParagraphId::new(self.paragraph.get() + 1);
            self.ends = LineEndState::default();
        }
    }

    /// Measures again the min-content lines of the current paragraph, whose
    /// prefix goes back, as Chrome's breaker sets them in no room. It ends at
    /// sum `end`.
    ///
    /// A line overflows at its first opportunity unless its content ends no
    /// further on than it started. Where nothing fits there, later
    /// opportunities are tried, as `Breaker::later_fit` does: from the first
    /// that fits, the line runs to the last that fits (WPT
    /// `inline-negative-margin-001`). A paragraph with tabs, ruby, hanging marks
    /// or line-edge costs keeps the scan's lines.
    #[cold]
    #[inline(never)]
    fn redo_min_lines(&mut self, end: InlineLayoutUnit) {
        let scan = self.scan;
        let analysis = scan.input.analysis;
        let paragraph = analysis.paragraphs.get(self.paragraph);
        let tabs = paragraph.is_some_and(|row| row.flags.contains(ParagraphFlags::HAS_TABS));
        if tabs
            || scan.has_ruby
            || scan.has_hanging
            || self.flags.contains(MeasureFlags::HAS_EDGE_COSTS)
            || scan
                .input
                .content
                .flags
                .contains(ContentFlags::INITIAL_LETTER)
        {
            return;
        }
        let range = analysis.paragraphs.clusters(self.paragraph);
        let clusters = &analysis.clusters;
        let sums: &[InlineLayoutUnit] = self.sums;
        let sum = |at: ClusterId| {
            if at >= range.end {
                end
            } else {
                sums.get(at.get()).copied().unwrap_or(end)
            }
        };
        let has = |at: ClusterId, bit| clusters.attrs(at).is_some_and(|attrs| attrs.has(bit));
        // The first boundary past `from` a line may end at.
        let next_end = |from: ClusterId| {
            let mut at = from;
            while at < range.end {
                work::step();
                let next = ClusterId::new(at.get() + 1);
                if has(at, ClusterAttrs::BREAK_AFTER) {
                    return next;
                }
                at = next;
            }
            range.end
        };
        // Where a line ending at `at` ends its content: before what hangs.
        let content_end = |at: ClusterId| {
            let mut first = at;
            while first > range.start && has(ClusterId::new(first.get() - 1), ClusterAttrs::HANGS) {
                work::step();
                first = ClusterId::new(first.get() - 1);
            }
            sum(first)
        };
        // The room of the boxes opening at `at`.
        let content = scan.input.content;
        let near = Cell::new(ItemId::new(0));
        let opening = |at: ClusterId| {
            if at >= range.end {
                return InlineLayoutUnit::ZERO;
            }
            let item = analysis.item_clusters.walk_to(near.get(), at);
            near.set(item);
            boundary_room(content, analysis, None, at, item).1
        };
        // Where a line ending at `at` ends its content, and where the next starts.
        // Opening boxes with negative room end the line, as Chrome's breaker takes
        // each item that fits.
        let ends = |at: ClusterId| {
            let rest = opening(at).min(InlineLayoutUnit::ZERO);
            (content_end(at) + rest, sum(at) + rest)
        };
        let widths = &mut self.widths;
        widths.restart_min_lines();
        let (mut start, mut pen, mut first) = (range.start, sum(range.start), true);
        while start < range.end {
            let scan_widths: &IntrinsicScan = widths;
            let width = |end: InlineLayoutUnit| scan_widths.min_line_width(pen, end, first);
            let fits = |at: ClusterId| {
                let (end, next) = ends(at);
                (width(end) <= InlineLayoutUnit::ZERO).then_some((at, width(end), next))
            };
            // Where nothing fits up to the first opportunity, try later ones; from the
            // first that fits, go on while they fit.
            let opportunity = next_end(start);
            let mut found = fits(opportunity);
            let mut at = opportunity;
            while found.is_none() && at < range.end {
                at = next_end(at);
                found = fits(at);
            }
            let (stop, line, next) = match found {
                Some(mut fitted) => {
                    while fitted.0 < range.end {
                        match fits(next_end(fitted.0)) {
                            Some(further) => fitted = further,
                            None => break,
                        }
                    }
                    fitted
                }
                None => (
                    opportunity,
                    width(content_end(opportunity)),
                    sum(opportunity),
                ),
            };
            widths.take_min_line(line, first);
            (start, pen, first) = (stop, next, false);
        }
    }

    /// Records what a line ending after the cluster at `at` ends with.
    ///
    /// - `gap`, the seam's room after the cluster, which the line gives back.
    /// - After a breakable soft hyphen, the hyphen, snapped to the grid as
    ///   Chrome's `HyphenResult::InlineSize` is.
    /// - Where the cluster doesn't hang, its padding and the mark that may
    ///   hang. `step` is its advance with `gap`.
    ///
    /// `outer_attrs` are `attrs` as the intrinsic sizes read them.
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
            // A mark hangs its own advance; the seam's room is given back already.
            let clusters = &scan.input.analysis.clusters;
            ends.content(walk, step - gap, || clusters.first_char(scan.source, at));
        }
    }

    /// Runs the plain fast path from boundary `from`: a tight loop where each
    /// cluster pays only its advance.
    ///
    /// It runs to the end of the current item, its paragraph, or the scan,
    /// entering the item's next segment where a shaping run ends inside it. A
    /// breakable soft hyphen's hyphen is charged at the next boundary
    /// ([`LineEndState::take_plain_hyphen`]).
    ///
    /// With `HYPHENS` false the text has no soft hyphen and the loop skips
    /// the hyphen tests.
    ///
    /// Returns where it stopped: the end, or before a sized tab or an atomic
    /// inline's U+FFFC, which the general body handles.
    ///
    /// It is exact because of its gate: no ruby, line padding or hanging
    /// punctuation, no seam in the paragraph, and no spacing, cloned edge or
    /// annotation in the item. Every other cost is zero.
    #[inline(always)]
    fn plain_fast_path<const HYPHENS: bool>(&mut self, from: ClusterId) -> ClusterId {
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
                let hyphen = if HYPHENS {
                    ends.take_plain_hyphen(at)
                } else {
                    None
                };
                let pay = match hyphen {
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
                let advance = scan.advance(at);
                writer.write(sums);
                writer.sum += advance;
                widths.cluster(attrs, advance, Tab::NONE, text);
                if HYPHENS
                    && class == ClusterClass::SoftHyphen
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

    /// Ends the walk past its last boundary, and returns the intrinsic widths.
    fn finish(mut self) -> IntrinsicScan {
        let scan = self.scan;
        let analysis = scan.input.analysis;
        // The last text item's extent.
        self.walk.flush(&mut self.out.extents);
        if let Some(em) = self.em_boxes.take() {
            em.finish(scan, self.out);
        }
        // The last paragraph ends at the text's end, after its closing edges. A
        // first-line scan has ended it already.
        if scan.end == analysis.clusters.end_id() {
            // An empty last paragraph that makes a line holds the room of the boxes
            // opening there, which no prefix sum holds.
            let sum = if analysis.paragraphs.has_end_line() {
                self.writer.sum
            } else {
                self.sums.last().copied().unwrap_or_default()
            };
            if self.flags.contains(MeasureFlags::NONMONOTONE) {
                self.redo_min_lines(sum);
            }
            self.widths.end_paragraph(sum, self.cost);
            if analysis.paragraphs.get(self.paragraph).is_some() {
                (self.out.paragraphs)
                    .push_bounded(self.flags, "no more paragraphs than a ParagraphId names");
            }
        }
        self.widths
    }
}

/// The one writer of the prefix sums, for the general body and the fast path.
///
/// It keeps the running sum and the entry at the last boundary, since the
/// breaker reads the prefix at boundaries only.
#[derive(Copy, Clone, Debug, Default)]
struct PrefixWriter {
    sum: InlineLayoutUnit,
    last: InlineLayoutUnit,
}

impl PrefixWriter {
    /// Returns whether the sum is under the last boundary's entry: the prefix
    /// goes back.
    fn went_back(self) -> bool {
        self.sum < self.last
    }

    /// Appends the sum to `sums` as the current boundary's entry.
    #[inline]
    fn write(&mut self, sums: &mut Vec<InlineLayoutUnit>) {
        sums.push(self.sum);
        self.last = self.sum;
    }
}

/// The prefix as a ruby look-ahead reads it: sums before the scan's
/// boundary, shaped advances from it on.
#[derive(Copy, Clone)]
pub(super) struct ScanPrefix<'s> {
    /// The sums written so far.
    sums: &'s [InlineLayoutUnit],
    /// Each cluster's shaped advance, up to `end`.
    advances: &'s [TextUnit],
    /// Where the scan stops.
    end: ClusterId,
}

impl ScanPrefix<'_> {
    /// Returns the entry at `at`: a sum before the scan's boundary, an advance
    /// from it on, zero at the scan's end, and `None` past it.
    pub(super) fn get(self, at: ClusterId) -> Option<InlineLayoutUnit> {
        match self.sums.get(at.get()) {
            Some(&sum) => Some(sum),
            None if at <= self.end => {
                let advance = self.advances.get(at.get()).copied().unwrap_or_default();
                Some(InlineLayoutUnit::from_text(advance))
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
