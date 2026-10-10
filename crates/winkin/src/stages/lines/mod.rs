//! Line breaking: chooses the lines for one width and records each line's
//! extent.
//!
//! In: [`BreakInput`], every prepared table and the area. Out: [`Lines`],
//! with the block's [`BlockResult`], and the [`LinePlacements`] that line
//! layout reads.
//! Start at: [`break_lines`], then `Breaker::lines` in `fit`.
//!
//! - `fit` is the breaker: it fits one line at a time, reshaping unsafe
//!   edges, and places floats as the lines reach them.
//! - `score` weighs a paragraph's lines under `text-wrap-style: balance`
//!   and `pretty`.
//! - `height` works out each line box's extent and the shifts only the line
//!   settles.
//! - `annotate` makes each line's room for ruby and emphasis marks.
//! - `ruby` holds the pieces of ruby columns split across lines.
//! - `exclusions` is what the host answers: the area and the floats.
//! - `block` and `trim` finish the block: its end, baselines and
//!   `text-box-trim`.
//! - `placement` holds the facts handed to line layout.
//!
//! Each line is written once, whole, when it is chosen; line layout decides
//! nothing about its extent. Breaking reads the prepared tables and never
//! writes them, so a relayout at another width repeats nothing. An area of
//! any width gives valid lines.

mod annotate;
mod block;
mod exclusions;
mod fit;
mod fit_floats;
mod fit_reshape;
mod fit_ruby;
mod fit_search;
mod fit_wrap;
mod height;
mod placement;
mod ruby;
use alloc::boxed::Box;
pub(crate) use ruby::RubyPiece;
use ruby::{ContinuationShift, PieceLevel, RubyLines, RubyMark};
mod score;
#[cfg(test)]
mod tests;
mod trim;

use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

pub(super) use annotate::LevelBands;
pub(super) use block::BlockResult;
pub use exclusions::{
    Area, BlockExtents, Exclusions, ExclusionsCheckpoint, FloatRequest, InlineExtents,
    NoExclusions, PlacedFloat,
};
pub(crate) use placement::{LinePlacements, LineView};
use placement::{PendingLine, PlacementFacts};
pub(crate) use trim::TextBoxTrims;

/// Whether `stages`' lines may carry ruby annotations or emphasis marks,
/// which take room over a line and under it.
pub(crate) fn annotated(stages: Stages<'_>) -> bool {
    AnnotationRoom::wanted(stages.content, stages.measured)
}

use annotate::{AnnotationRoom, Carry};

use height::{BoxStack, CarriedBoxes, LineFonts};
use score::{LineChoice, Scoring};

use crate::config::{EmphasisRoom, Pretty, RubyBreakWithin};
use crate::data::{
    Id, Keyed, SortedCursor, Table, TextOffset, define_flags, define_id, find_sorted, heap_bytes,
};
#[cfg(test)]
use crate::stages::analysis::ClusterAttrs;
use crate::stages::analysis::{BidiLevel, ClusterId, Clusters, ParagraphId, Paragraphs};
use crate::stages::content::{BreakClearanceId, FloatId, ItemId, NodeId};
use crate::stages::measure::{
    AutospaceRules, Extent, LineEdgeCost, Measured, RubyColumnId, WordSpacingRule,
};
use crate::stages::shape::{
    GlyphSink, GlyphWord, ShapeSession, ShapedRunId, ShapingEdges, SidecarGlyph, SidecarGlyphId,
};
use crate::stages::{LineStages, Stages};
use crate::style::{FirstLineVariant, LineClamp, TextAlign, TextIndent};
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::work;
use fit::{Hold, ReshapedStart};
use fit_ruby::Continued;

/// What line breaking reads: the prepared stages, the area, and the config
/// choices read at every relayout.
pub(crate) struct BreakInput<'a> {
    pub(crate) stages: Stages<'a>,
    /// Where the lines go: their two ends, the block's start, the room above
    /// it, and where `line-clamp: auto` ends it.
    pub(crate) area: Area,
    /// How a line makes room for emphasis marks (`Config::emphasis_room`).
    pub(crate) emphasis_room: EmphasisRoom,
    /// The rule `text-wrap-style: pretty` follows (`Config::pretty`).
    pub(crate) pretty: Pretty,
    pub(crate) ruby_break_within: RubyBreakWithin,
}

/// Breaking's working memory, and its hand-off to line placement.
///
/// Each height and scoring phase has its own buffer. Placement reads only the
/// final facts table. The scratch is cleared, never dropped, so a warm
/// relayout allocates nothing.
pub(crate) struct BreakScratch {
    /// The final lines' facts, handed to line placement.
    pub(crate) placements: LinePlacements,
    /// The box stack a line with a shifted box is walked with.
    boxes: BoxStack,
    /// The boxes open across a line's start, whose struts an unshifted line
    /// holds.
    carried: CarriedBoxes,
    /// The annotation levels of the line being measured, which the breaker
    /// fills for each line.
    bands: LevelBands,
    /// What the scorer weighs a paragraph's lines by.
    scoring: Scoring,
    /// The levels of the ruby column being split, cut by its trials.
    level_cuts: Vec<ruby::LevelCut>,
}

impl BreakScratch {
    /// Empty scratch, allocating nothing.
    pub(crate) fn new() -> Self {
        Self {
            placements: LinePlacements::new(),
            boxes: BoxStack::new(),
            carried: CarriedBoxes::new(),
            bands: LevelBands::new(),
            scoring: Scoring::new(),
            level_cuts: Vec::new(),
        }
    }
}

heap_bytes! {
    BreakScratch { placements, boxes, carried, bands, scoring, level_cuts }
}

impl Default for BreakScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// Breaks the text of `input` into lines in its area, into `out`, which
/// starts empty.
///
/// Reshapes unsafe line edges with the shaping caches in `cx`, and works out
/// line heights in `scratch`. Only this function fills `out`, so a relayout
/// at another width repeats nothing prepared.
///
/// [`Breaker::line`] fits every line. It places floats and the initial
/// letter as the lines reach them. Content with neither asks the host for no
/// checkpoint.
///
/// `line-clamp: auto` works as Chrome's `CSSLineClamp`:
/// - The lines are laid out greedily until one would take the block past the
///   host's end, `text-box-trim` included.
/// - Everything is taken back and laid out again, clamped to the lines before
///   that one.
/// - Balanced or scored lines are chosen among the lines kept. Where they
///   reach past the end, the greedy lines are kept instead.
pub(crate) fn break_lines(
    input: &BreakInput<'_>,
    cx: &mut ShapeSession<'_, '_>,
    scratch: &mut BreakScratch,
    exclusions: &mut dyn Exclusions,
    out: &mut Lines,
) {
    scratch.placements.clear();
    scratch.carried.begin();
    let area = input.area;
    let room_above = LayoutUnit::from_px(area.room_above).max(LayoutUnit::ZERO);
    let clamp = Clamp::new(input.stages.content.block.line_clamp, area);
    let Clamp::Height(end) = clamp else {
        Breaker::new(input, cx, scratch, exclusions, clamp, room_above).lines(out, input);
        return;
    };
    let from = exclusions.checkpoint();
    // CSS Overflow 4 measures the clamp as if `text-wrap-style` were
    // `stable`. The kept lines are balanced or scored after.
    let measuring = Breaker::new(input, cx, scratch, exclusions, clamp, room_above);
    let chooses = measuring.choice() != LineChoice::Greedy;
    let measured = measuring.with_choice(LineChoice::Greedy).lines(out, input);
    if !measured.overran && !chooses {
        return;
    }
    let kept = out.lines.len();
    restart(from, scratch, exclusions, out);
    let clamp = if measured.overran {
        let Some(kept) = u32::try_from(kept).ok().filter(|&kept| kept > 0) else {
            // Clamped at the block's start: it keeps no line and no float.
            out.block.block_end = LayoutUnit::from_px(area.block_start);
            return;
        };
        Clamp::Lines(kept)
    } else {
        Clamp::None
    };
    let mut breaker = Breaker::new(input, cx, scratch, exclusions, clamp, room_above);
    breaker.lines(out, input);
    if !chooses || out.lines.is_empty() || out.block.block_end <= end {
        return;
    }
    restart(from, scratch, exclusions, out);
    Breaker::new(input, cx, scratch, exclusions, clamp, room_above)
        .with_choice(LineChoice::Greedy)
        .lines(out, input);
}

/// Takes back a layout of the block's lines to `from`.
///
/// Rewinds the host's floats, clears the lines, and resets the scratch a
/// break begins with.
fn restart(
    from: ExclusionsCheckpoint,
    scratch: &mut BreakScratch,
    exclusions: &mut dyn Exclusions,
    out: &mut Lines,
) {
    exclusions.rewind(from);
    out.clear();
    scratch.placements.clear();
    scratch.carried.begin();
}

/// The limit `line-clamp` sets on a layout of the block's lines.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Clamp {
    /// Nothing: every line is kept.
    None,
    /// The block keeps this many lines at most: `line-clamp: <n>`, or the
    /// count `line-clamp: auto` measured.
    ///
    /// The last line is cut for an ellipsis where text follows it.
    Lines(u32),
    /// `line-clamp: auto`, being measured: the lines are laid out until one
    /// would take the block's content past this end, in the host's
    /// positions. That line is not kept.
    Height(LayoutUnit),
}

impl Clamp {
    /// Returns the clamp `clamp` asks for in `area`.
    ///
    /// `auto` clamps by the area's block end. It clamps nothing where the
    /// area has no end, or one that is not a number.
    fn new(clamp: LineClamp, area: Area) -> Self {
        match clamp {
            LineClamp::Lines(lines) if lines > 0 => Self::Lines(lines),
            LineClamp::Auto => area
                .block_end
                .filter(|end| !end.is_nan())
                .map_or(Self::None, |end| Self::Height(LayoutUnit::from_px(end))),
            LineClamp::Lines(_) | LineClamp::None => Self::None,
        }
    }
}

define_id! {
    /// Names a line of the current layout, in block order.
    pub(crate) struct LineId(u32);
}

define_id! {
    /// Names a reshaped line edge in a layout's table of [`EdgeShape`]s.
    pub(crate) struct EdgeShapeId(u32);
}

define_id! {
    /// Names a cluster of a reshaped line edge: its glyph word and advance
    /// in the edge tables.
    pub(crate) struct EdgeClusterId(u32);
}

define_id! {
    /// Names a float a line placed, in a layout's table of [`LineFloat`]s.
    pub(crate) struct LineFloatId(u32);
}

define_id! {
    /// Names a baseline shift a line settled, in a layout's table of
    /// [`LineShift`]s.
    pub(crate) struct LineShiftId(u32);
}

/// The room a line may take, as its line-left and line-right ends on the
/// 1/64 grid.
///
/// It is the area narrowed by the band the host's floats leave.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) struct LineBand {
    /// Its line-left end.
    pub(crate) left: LayoutUnit,
    /// Its line-right end.
    pub(super) right: LayoutUnit,
}

impl From<InlineExtents> for LineBand {
    /// Rounds each end of `extents` to the nearest 1/64.
    ///
    /// An end that is not a number goes to zero, and an infinite one to the
    /// grid's end.
    fn from(extents: InlineExtents) -> Self {
        Self {
            left: LayoutUnit::from_px(extents.left),
            right: LayoutUnit::from_px(extents.right),
        }
    }
}

impl LineBand {
    /// Returns where the two bands overlap.
    fn meet(self, other: Self) -> Self {
        Self {
            left: self.left.max(other.left),
            right: self.right.min(other.right),
        }
    }

    /// Returns the band with its ends moved in by `left` and `right`.
    ///
    /// `text-group-align` sets a line in such a band.
    pub(super) fn inset(self, left: LayoutUnit, right: LayoutUnit) -> Self {
        Self {
            left: self.left + left,
            right: self.right - right,
        }
    }

    /// Returns its length, never negative.
    ///
    /// A band whose ends cross leaves no room.
    pub(super) fn width(self) -> LayoutUnit {
        (self.right - self.left).max(LayoutUnit::ZERO)
    }

    /// Returns how far a line in this band starts from `area`'s edge, on the
    /// start side of a paragraph at `level`.
    ///
    /// Tab stops count from here. They are multiples of `tab-size` from the
    /// content edge, so a line a float pushes in keeps the block's stops. The
    /// breaker and line layout both use this one lookup.
    pub(super) fn start_from(self, area: Self, level: BidiLevel) -> LayoutUnit {
        if level.is_rtl() {
            area.right - self.right
        } else {
            self.left - area.left
        }
    }

    /// Returns the band in pixels, as the host reads it.
    pub(crate) fn to_extents(self) -> InlineExtents {
        InlineExtents {
            left: self.left.to_px(),
            right: self.right.to_px(),
        }
    }
}

define_flags! {
    /// What else is true of a line.
    pub(crate) struct LineFlags(u8) {
        /// The block's first line, fitted, laid out and read in the first
        /// line's measure.
        const FIRST_LINE = 1 << 0;
        /// Ends at a soft hyphen that shows a hyphen, whose advance is its end
        /// cost.
        pub(crate) const HYPHENATED = 1 << 1;
        /// Its content is wider than its band.
        const OVERFLOWS = 1 << 2;
        /// Line layout cuts it for an ellipsis: clamped, or overflowing
        /// under `text-overflow: ellipsis`.
        pub(crate) const ELLIPSIS = 1 << 3;
        /// Its trailing white space ends a paragraph or the block, and hangs
        /// only where it overflows.
        ///
        /// These are preserved spaces before a forced break or at the text's
        /// end. Alignment reads the flag.
        pub(super) const CONDITIONAL_HANG = 1 << 4;
        /// Ends at an `overflow-wrap` emergency break, which Chrome's breaker
        /// takes only on retrying a line that overflowed.
        const EMERGENCY = 1 << 5;
        /// Ends where the scorer's plan put its end, as Chrome's `break_at_`
        /// ends a line under `balance` or `pretty`.
        ///
        /// Chrome's breaker stops at that offset before an absolutely
        /// positioned box's anchor there, so the anchor goes to the next
        /// line.
        pub(crate) const SCORED = 1 << 6;
    }
}

impl LineFlags {
    /// Whether the line's content overflowed its band: it still does, or it
    /// ends at an emergency break taken because it did.
    ///
    /// Balancing and `pretty` decline such a line, as Chrome's breaker
    /// disables its bisection and scoring on overflowing.
    fn overflowed(self) -> bool {
        self.contains(Self::OVERFLOWS) || self.contains(Self::EMERGENCY)
    }
}

/// What hangs past a line's ends: 16 bytes.
///
/// The line's [`CONDITIONAL_HANG`](LineFlags::CONDITIONAL_HANG) says whether
/// its white space ends with some that hangs only where it overflows.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) struct LineHang {
    /// The trailing white space: the line's whole reach less its
    /// [`width`](LineRecord::width).
    ///
    /// It sums the clusters' own advances, so a box's edge among them stays
    /// content.
    pub(crate) space: LayoutUnit,
    /// The part of `space` before the white space that hangs only where it
    /// overflows, on a line with some: an ideographic space where white
    /// space collapses, say.
    ///
    /// It hangs only where none of that white space fits, as nothing after
    /// it then takes room (CSS Text 3, section 4.1.3).
    pub(super) unconditional: LayoutUnit,
    /// Hanging punctuation at the start: the advance of the block's first
    /// cluster, where its start cost gives it back.
    pub(super) start: LayoutUnit,
    /// Hanging punctuation at the end: the end cost's flex, where the line
    /// takes it.
    pub(super) end: LayoutUnit,
}

/// A line edge reshaped at an unsafe break: 12 bytes.
///
/// Its clusters' glyph words and advances sit in [`ReshapedEdges`] from
/// `first` on, one each. They use the paragraph's glyph encoding, so one
/// reader decodes both. The edge's glyphs replace the paragraph's for these
/// clusters on this line only.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct EdgeShape {
    /// The first cluster it replaces, and one past its last.
    start: ClusterId,
    end: ClusterId,
    first: EdgeClusterId,
}

impl EdgeShape {
    /// Returns the edge replacing `clusters`, whose entries in the edge
    /// tables start at `first`.
    fn new(clusters: Range<ClusterId>, first: EdgeClusterId) -> Self {
        Self {
            start: clusters.start,
            end: clusters.end,
            first,
        }
    }

    /// Returns the clusters it replaces.
    #[inline]
    pub(super) fn clusters(&self) -> Range<ClusterId> {
        self.start..self.end
    }

    /// Returns `cluster`'s entry in the edge tables, if the edge replaces it.
    #[inline]
    pub(super) fn entry(&self, cluster: ClusterId) -> Option<EdgeClusterId> {
        (self.start <= cluster && cluster < self.end)
            .then(|| EdgeClusterId::new(self.first.get() + (cluster.get() - self.start.get())))
    }

    /// Returns the edge-table entries of the part of `clusters` it replaces,
    /// empty where it replaces none.
    pub(super) fn entries(&self, clusters: Range<ClusterId>) -> Range<EdgeClusterId> {
        let (from, to) = (clusters.start.max(self.start), clusters.end.min(self.end));
        if from >= to {
            return self.first..self.first;
        }
        let at = self.first.get() + (from.get() - self.start.get());
        EdgeClusterId::new(at)..EdgeClusterId::new(at + (to.get() - from.get()))
    }
}

/// The line edges reshaped at unsafe breaks, in shaping's glyph encoding.
///
/// The tables hold each edge's record, a glyph word and advance per cluster,
/// and the glyphs of expanded clusters. The breaker writes them while it fits
/// a line, and takes back what a rejected candidate wrote ([`EdgeMark`]).
/// Line layout and every reader read a line's edges in place of the
/// paragraph's glyphs, on that line only.
pub(crate) struct ReshapedEdges {
    /// Every edge the lines kept, each line's together, in line order.
    pub(super) shapes: Table<EdgeShapeId, EdgeShape>,
    /// A glyph word per reshaped cluster.
    pub(super) words: Table<EdgeClusterId, GlyphWord>,
    /// The glyphs of reshaped clusters that are expanded, which their words
    /// point into.
    pub(super) glyphs: Table<SidecarGlyphId, SidecarGlyph>,
    /// An advance per reshaped cluster, 16.16, indexed as `words` is.
    pub(super) advances: Table<EdgeClusterId, TextUnit>,
}

impl ReshapedEdges {
    /// Empty tables, allocating nothing.
    const fn new() -> Self {
        Self {
            shapes: Table::new(),
            words: Table::new(),
            glyphs: Table::new(),
            advances: Table::new(),
        }
    }

    /// Empties the tables, keeping every allocation.
    fn clear(&mut self) {
        self.shapes.clear();
        self.words.clear();
        self.glyphs.clear();
        self.advances.clear();
    }

    /// Returns the edges `line` reshaped, in order: none, one or two.
    #[inline]
    pub(super) fn line_edges(&self, line: &LineRecord) -> &[EdgeShape] {
        self.shapes
            .get_slice(line.shapes.clone())
            .unwrap_or_default()
    }

    /// Returns [`line_edges`](Self::line_edges)'s edges where one holds some of
    /// `clusters`, and none otherwise.
    #[inline]
    pub(super) fn line_edges_touching(
        &self,
        line: &LineRecord,
        clusters: &Range<ClusterId>,
    ) -> &[EdgeShape] {
        let shapes = self.line_edges(line);
        let holds = shapes.iter().any(|shape| {
            let held = shape.clusters();
            held.start < clusters.end && clusters.start < held.end
        });
        if holds { shapes } else { &[] }
    }

    /// Returns the exact sum of the advances of `entries`.
    pub(super) fn advance_sum(&self, entries: Range<EdgeClusterId>) -> InlineLayoutUnit {
        self.advances
            .get_slice(entries)
            .unwrap_or_default()
            .iter()
            .fold(InlineLayoutUnit::ZERO, |sum, &advance| {
                sum + InlineLayoutUnit::from_text(advance)
            })
    }

    /// Returns where the tables stand, to rewind to later.
    fn mark(&self) -> EdgeMark {
        EdgeMark {
            clusters: self.words.next_id(),
            glyphs: self.glyphs.next_id(),
        }
    }

    /// Takes back every cluster written since `mark`, its word, its advance
    /// and its glyphs.
    fn rewind(&mut self, mark: EdgeMark) {
        self.words.truncate(mark.clusters);
        self.advances.truncate(mark.clusters);
        self.glyphs.truncate(mark.glyphs);
    }

    /// Returns the shaping sink that writes into these tables.
    fn sink(&mut self) -> GlyphSink<'_, EdgeClusterId, TextUnit, Table<EdgeClusterId, TextUnit>> {
        GlyphSink::new(&mut self.words, &mut self.glyphs, &mut self.advances)
    }
}

heap_bytes! {
    ReshapedEdges { shapes, words, glyphs, advances }
}

/// A position in the edge tables, which a rejected candidate's edges rewind
/// to.
///
/// A cluster has a word and an advance, so those two tables share one id.
#[derive(Copy, Clone, Debug, Default)]
struct EdgeMark {
    clusters: EdgeClusterId,
    glyphs: SidecarGlyphId,
}

/// A position in the lines' tables, which a rejected layout of a paragraph
/// rewinds to ([`Lines::rewind`]).
#[derive(Copy, Clone, Debug)]
struct LinesMark {
    lines: LineId,
    shapes: EdgeShapeId,
    edges: EdgeMark,
    floats: LineFloatId,
    shifts: LineShiftId,
    /// The ruby pieces and their levels, where any are written.
    ruby: RubyMark,
}

/// A float a line placed, and where the host put its margin box.
///
/// Its size is the content's.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct LineFloat {
    /// Which float it is.
    pub(crate) float: FloatId,
    /// Where the host put its margin box's line-left edge.
    pub(crate) left: LayoutUnit,
    /// Where the host put its margin box's block-start edge.
    pub(crate) top: LayoutUnit,
}

/// A baseline shift only its line can settle: 8 bytes.
///
/// It raises `node`'s baseline, or lowers it where negative. Its whole
/// subtree moves with it.
/// - For `middle`, `text-top` and `text-bottom`, it counts from the parent's
///   baseline, on top of measurement's fixed shift.
/// - For `top` and `bottom`, it counts from the baseline of the nearest
///   enclosing `top` or `bottom` box, or the line's.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct LineShift {
    /// The box or atomic inline it moves.
    node: NodeId,
    /// How far it raises the node's baseline.
    shift: LayoutUnit,
}

impl Keyed for LineShift {
    type Key = NodeId;

    /// Returns the node it moves; a line's shifts are sorted by node.
    #[inline]
    fn key(&self) -> NodeId {
        self.node
    }
}

/// One line: every decision about its ends, written once. 80 bytes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct LineRecord {
    /// Its first cluster.
    start: ClusterId,
    /// One past its last: what hangs, its separator where it has one.
    end: ClusterId,
    /// Its paragraph.
    pub(crate) paragraph: ParagraphId,
    /// Its first item, as the breaker's item cursor stood when it was fitted.
    ///
    /// It is the first item at the line's start, empty items first, or else
    /// the one holding its first cluster across the break. Blink's break
    /// token names an item the same way. A walk over the line starts here
    /// with no search.
    pub(crate) first_item: ItemId,
    /// The room it is fitted in.
    pub(crate) band: LineBand,
    /// The `text-indent` it takes.
    pub(super) indent: LayoutUnit,
    /// Its content's width as fitted, on Chrome's 1/64 grid.
    ///
    /// The width runs from its start to its content's end. It measures the
    /// reshaped edges as shaped, adds the edge costs, and leaves out what
    /// hangs.
    pub(crate) width: LayoutUnit,
    /// What hangs past its ends.
    pub(crate) hang: LineHang,
    /// Its line box's extent around its baseline: the strut united with
    /// every item on it.
    pub(crate) extent: Extent,
    /// Its reshaped edges in [`ReshapedEdges`]: none, one or two.
    pub(super) shapes: Range<EdgeShapeId>,
    /// The floats whose anchors it holds, in placement order: those it
    /// starts beside, those beside it, and those below it.
    pub(crate) floats: Range<LineFloatId>,
    /// The baseline shifts it settled, sorted by node: none where nothing
    /// on it is `middle`, `text-top`, `text-bottom`, `top` or `bottom`.
    shifts: Range<LineShiftId>,
    /// What else is true of it.
    pub(crate) flags: LineFlags,
}

impl LineRecord {
    /// Returns its clusters, what hangs included.
    pub(crate) fn clusters(&self) -> Range<ClusterId> {
        self.start..self.end
    }

    /// Returns its content's end, before what hangs, for test invariants.
    ///
    /// Line placement gets the breaker's exact end through the scratch
    /// instead.
    #[cfg(test)]
    pub(super) fn content_end(&self, clusters: &Clusters) -> ClusterId {
        let mut end = self.end;
        while end > self.start {
            let last = ClusterId::new(end.get() - 1);
            if !clusters
                .attrs(last)
                .is_some_and(|attrs| attrs.has(ClusterAttrs::HANGS))
            {
                break;
            }
            end = last;
        }
        end
    }

    /// Returns its paragraph's base level.
    #[inline]
    pub(crate) fn level(&self, paragraphs: &Paragraphs) -> BidiLevel {
        paragraphs
            .get(self.paragraph)
            .map_or(BidiLevel::LTR, |paragraph| paragraph.level)
    }

    /// Returns how far its line box reaches over its baseline, zero for an
    /// empty line box.
    ///
    /// This places the baseline from the line box's top. Every unshifted item
    /// stands on it.
    #[inline]
    pub(crate) fn ascent(&self) -> LayoutUnit {
        self.extent.zero_if_none().ascent()
    }

    /// Returns whether it is its paragraph's last line, which
    /// `text-align-last` aligns.
    #[inline]
    pub(super) fn ends_paragraph(&self, paragraphs: &Paragraphs) -> bool {
        paragraphs.get(self.paragraph).is_some()
            && paragraphs.clusters(self.paragraph).end == self.end
    }

    /// Returns the variant whose styles it is set in: the first line's, or
    /// the standard one.
    ///
    /// A reader reads every prepared table for the line in this variant.
    #[inline]
    pub(crate) fn variant(&self) -> FirstLineVariant {
        if self.flags.contains(LineFlags::FIRST_LINE) {
            FirstLineVariant::FirstLine
        } else {
            FirstLineVariant::Standard
        }
    }
}

/// The lines, and the tables their records name.
///
/// Only [`break_lines`] clears and refills the tables, so a relayout keeps
/// every allocation.
pub(crate) struct Lines {
    /// The lines, in block order.
    pub(crate) lines: Table<LineId, LineRecord>,
    /// The line edges reshaped at unsafe breaks, which the lines' records
    /// name.
    pub(super) edges: ReshapedEdges,
    /// Every float the lines placed, once each, in placement order.
    ///
    /// Each line's floats are together. Floats no line reaches come last,
    /// placed where the text ends.
    pub(crate) floats: Table<LineFloatId, LineFloat>,
    /// The baseline shifts the lines settled, a line's together.
    pub(super) shifts: Table<LineShiftId, LineShift>,
    /// The pieces of the ruby columns the lines split, where any did.
    pub(crate) ruby: Option<Box<RubyLines>>,
    /// The block's trim, end and baselines.
    pub(crate) block: BlockResult,
    /// The last line's carry, where the content has annotations or marks.
    ///
    /// It is the room the last line leaves under its content, or, negative,
    /// how far its annotations and marks reach past its line box. It is zero
    /// for content with neither, or no line. Read it through
    /// [`room_below`](Self::room_below).
    annotated_carry: LayoutUnit,
    /// How far the first line's annotations move the initial letter down:
    /// what they move the line beyond a raised letter's raise.
    ///
    /// The kept first line writes it. It is zero where the line has no
    /// annotation over it, or no initial letter.
    pub(super) letter_shift: LayoutUnit,
    /// How many prefix positions the last break compared, for the tests
    /// that hold fitting to O(log L) a line.
    #[cfg(test)]
    probes: usize,
    /// Every range the last break reshaped, and how, in order, for the tests
    /// that hold it to reshaping each once a line.
    #[cfg(test)]
    reshaped: Vec<(Range<ClusterId>, ShapingEdges)>,
}

impl Lines {
    /// No lines, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            lines: Table::new(),
            edges: ReshapedEdges::new(),
            floats: Table::new(),
            shifts: Table::new(),
            ruby: None,
            block: BlockResult::EMPTY,
            annotated_carry: LayoutUnit::ZERO,
            letter_shift: LayoutUnit::ZERO,
            #[cfg(test)]
            probes: 0,
            #[cfg(test)]
            reshaped: Vec::new(),
        }
    }

    /// Empties the lines, keeping every allocation.
    pub(crate) fn clear(&mut self) {
        self.lines.clear();
        self.edges.clear();
        self.floats.clear();
        self.shifts.clear();
        if let Some(ruby) = &mut self.ruby {
            ruby.clear();
        }
        self.block = BlockResult::EMPTY;
        self.annotated_carry = LayoutUnit::ZERO;
        self.letter_shift = LayoutUnit::ZERO;
        #[cfg(test)]
        {
            self.probes = 0;
            self.reshaped.clear();
        }
    }

    /// Returns where the tables stand, to rewind to later.
    fn mark(&self) -> LinesMark {
        LinesMark {
            lines: self.lines.next_id(),
            shapes: self.edges.shapes.next_id(),
            edges: self.edges.mark(),
            floats: self.floats.next_id(),
            shifts: self.shifts.next_id(),
            ruby: self
                .ruby
                .as_deref()
                .map_or_else(RubyMark::default, RubyLines::mark),
        }
    }

    /// Takes back every line laid out since `mark`, with everything it
    /// placed, reshaped and settled.
    fn rewind(&mut self, mark: LinesMark) {
        self.lines.truncate(mark.lines);
        self.edges.shapes.truncate(mark.shapes);
        self.edges.rewind(mark.edges);
        self.floats.truncate(mark.floats);
        self.shifts.truncate(mark.shifts);
        if let Some(ruby) = &mut self.ruby {
            ruby.rewind(mark.ruby);
        }
    }

    /// Returns the first line ending at or past `cluster`, or one past the
    /// last where none does.
    pub(crate) fn first_reaching(&self, cluster: ClusterId) -> LineId {
        work::seek();
        LineId::new(
            self.lines
                .as_slice()
                .partition_point(|line| line.end < cluster),
        )
    }

    /// Returns the first line ending past `cluster`, or one past the last
    /// where none does.
    ///
    /// Where a line ends at `cluster`, this is the line after it: the line a
    /// downstream position there is on.
    /// [`first_reaching`](Self::first_reaching) gives the upstream one.
    pub(crate) fn at(&self, cluster: ClusterId) -> LineId {
        work::seek();
        LineId::new(
            self.lines
                .as_slice()
                .partition_point(|line| line.end <= cluster),
        )
    }

    /// Returns the room the last line leaves under its content for the block
    /// after it, zero where there is no line.
    ///
    /// Negative, it is how far annotations and emphasis marks reach past the
    /// line box, which `block_end` counts. `stages` are the ones the lines
    /// were broken from.
    ///
    /// Content with annotations or marks has each line's room worked out as
    /// it breaks, so this returns the stored carry. Otherwise it works out
    /// the last line's em boxes against its line box here, as Chrome does in
    /// `InlineLayoutAlgorithm::CreateLine` under `ContainsAnnotations()` and
    /// `IsLastLine()`. Working it out on demand saves a sixth of a one-line
    /// label's relayout.
    pub(crate) fn room_below(&self, stages: Stages<'_>) -> LayoutUnit {
        if AnnotationRoom::wanted(stages.content, stages.measured) {
            return self.annotated_carry;
        }
        let Some(last) = self.lines.as_slice().last() else {
            return LayoutUnit::ZERO;
        };
        let clusters = last.clusters();
        let stages = stages.variant(last.variant());
        stages.plain_carry(last.first_item, clusters.start, clusters.end, last.extent)
    }

    /// Returns how far `line` raises `node`'s baseline for its
    /// `vertical-align`, zero where it settled nothing for it.
    ///
    /// A line that settled nothing, as most do, is not searched.
    pub(super) fn shift(&self, line: &LineRecord, node: NodeId) -> LayoutUnit {
        if line.shifts.is_empty() {
            return LayoutUnit::ZERO;
        }
        find_sorted(self.shifts.slice(line.shifts.clone()), node)
            .map_or(LayoutUnit::ZERO, |shift| shift.shift)
    }
}

heap_bytes! {
    /// Its tables, not the tests' record of what the last break reshaped.
    Lines {
        lines, edges, floats, shifts, ruby;
        block, annotated_carry, letter_shift, #[cfg(test)] probes, #[cfg(test)] reshaped
    }
}

impl Default for Lines {
    fn default() -> Self {
        Self::new()
    }
}

// The breaker's state and the records its files share: `fit` builds and
// drives them, and `fit_search`, `fit_reshape`, `fit_floats`, `fit_wrap` and
// `fit_ruby` each add to `Breaker`'s `impl`.

/// Where the lines have got to: what the next line starts from.
///
/// A rejected layout of a paragraph rewinds to a saved cursor.
#[derive(Copy, Clone, Debug)]
struct Cursor {
    /// Where the next line box starts across the block, in the records'
    /// positions.
    top: LayoutUnit,
    /// How far the host's block positions stand above the records'.
    ///
    /// It is what `text-box-trim` moves the first line up by, once kept. Every
    /// later line moves with it. The block result trims the records by the
    /// same amount. The host is asked where the lines will stand, as Chrome
    /// lays each later line below the trimmed first.
    trimmed: LayoutUnit,
    /// The first item that may be on the next line.
    items: ItemId,
    /// The first float not yet placed. The text reaches floats in reading
    /// order, and each is placed once.
    floats: FloatId,
    /// The first forced break that clears floats not yet passed.
    clearances: BreakClearanceId,
    /// Where the last line's clearing break moved the block's end, in the
    /// host's positions, where it moved it.
    cleared: Option<LayoutUnit>,
    /// `line-clamp` ended the block: nothing after its last line is broken,
    /// and no later float is placed.
    clamped: bool,
    /// `line-clamp: auto` ended the block: the next line would take the
    /// block's content past its end, and is not kept.
    overran: bool,
}

impl Cursor {
    /// Returns the block's start, its first line box at `top`.
    fn new(top: LayoutUnit) -> Self {
        Self {
            top,
            trimmed: LayoutUnit::ZERO,
            items: ItemId::new(0),
            floats: FloatId::new(0),
            clearances: BreakClearanceId::new(0),
            cleared: None,
            clamped: false,
            overran: false,
        }
    }
}

/// A part of a line reshaped on its own, in the edge tables.
#[derive(Copy, Clone, Debug)]
struct ReshapedPiece {
    /// Its clusters and their edge-table entries: the line's edge record,
    /// if the line is kept.
    shape: EdgeShape,
    /// Its reshaped advance less the paragraph's over the same clusters:
    /// what it moves the line's end by.
    delta: InlineLayoutUnit,
    /// Whether breaking before its start changes the shaping, so it may not
    /// join the glyphs before it.
    start_unsafe: bool,
}

/// A line's reshaped pieces: none, one (its start, its end, or the whole
/// line), or two (its start and its end), in order.
#[derive(Copy, Clone, Debug, Default)]
struct ReshapedPieces {
    list: [Option<ReshapedPiece>; 2],
}

impl ReshapedPieces {
    fn push(&mut self, piece: Option<ReshapedPiece>) {
        let Some(piece) = piece else {
            return;
        };
        if let Some(slot) = self.list.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(piece);
        }
    }

    fn iter(&self) -> impl Iterator<Item = &ReshapedPiece> {
        self.list.iter().flatten()
    }

    /// Returns how far the pieces move the line's end.
    fn delta(&self) -> InlineLayoutUnit {
        self.iter()
            .fold(InlineLayoutUnit::ZERO, |sum, piece| sum + piece.delta)
    }

    /// Returns `cluster`'s reshaped advance, where a piece holds it.
    fn advance(&self, out: &Lines, cluster: ClusterId) -> Option<InlineLayoutUnit> {
        let at = self.iter().find_map(|piece| piece.shape.entry(cluster))?;
        out.edges
            .advances
            .get(at)
            .map(|&advance| InlineLayoutUnit::from_text(advance))
    }
}

/// A line start's reshape window up to `end`, and what shaping it on its
/// own moves the line's end by.
#[derive(Copy, Clone, Debug)]
struct StartWindow {
    /// The shaping run holding it, which its piece is shaped in.
    run: ShapedRunId,
    end: ClusterId,
    delta: InlineLayoutUnit,
    /// The line starts at an opening mark it trims, under `space-first` or
    /// `trim-start`.
    trims: bool,
}

impl StartWindow {
    /// Returns how a line start's window is shaped: as a line's start, its
    /// opening mark trimmed where `trims`.
    fn edges(trims: bool) -> ShapingEdges {
        ShapingEdges {
            line_start: true,
            trim_start: trims,
            trim_end: false,
        }
    }
}

/// A chosen line, before its height and records.
#[derive(Copy, Clone, Debug)]
struct Fitted {
    end: ClusterId,
    content_end: ClusterId,
    /// The `text-indent` it takes.
    indent: LayoutUnit,
    width: LayoutUnit,
    hang: LineHang,
    flags: LineFlags,
    /// Its start's reshape window, where it has one.
    window: Option<StartWindow>,
    pieces: ReshapedPieces,
}

/// A paragraph, as the breaker reads it.
#[derive(Clone, Debug)]
struct BreakerParagraph {
    /// It has a ruby column that may split across lines.
    breakable_ruby: bool,
    id: ParagraphId,
    start: ClusterId,
    end: ClusterId,
    /// The prefix sum at its start, which fitting measures positions from.
    origin: InlineLayoutUnit,
    /// Its text, the context a reshaped edge is shaped in.
    text: Range<TextOffset>,
    level: BidiLevel,
    /// Walked rather than searched: it has tabs, or its prefix is not
    /// monotone.
    walk: bool,
    /// Its prefix never goes back, so a cut's width grows with the cut.
    monotone: bool,
    /// It has tabs, which a line's walk and candidates size.
    tabs: bool,
    /// Some edge cost falls in it.
    costs: bool,
    /// What a line ending at its end pays there.
    ///
    /// Every line's fit tries the paragraph's end first. Kept here, that
    /// candidate leaves the line's cost cursor near the line.
    end_cost: LineEdgeCost,
    /// `text-autospace` puts room at a seam in it, which a line ending there
    /// gives back.
    seams: bool,
    /// It ends in a separator, so its last line ends at a forced break.
    forced: bool,
    /// Its lines need their ends exactly, so a break at a space is reshaped.
    exact_end: bool,
    /// An inline box in it has a strut past the block's, which a line it is
    /// open across may hold.
    struts: bool,
}

/// What one line's fitting holds fixed while its candidates are measured.
struct Fitting<'p> {
    /// How the rest of a column the line continues shifts prefix positions.
    shift: ContinuationShift,
    para: &'p BreakerParagraph,
    start: ClusterId,
    /// The first item at `start`, empty items first, or else the one
    /// holding its cluster: the line's [`first_item`](LineRecord::first_item). A
    /// walk over the line starts here with no search.
    first_item: ItemId,
    /// The paragraph's line-edge costs, where it has any: sought at `start`
    /// and moved with the candidates.
    costs: Option<SortedCursor<'p, LineEdgeCost>>,
    /// The edge tables before anything of this line's.
    mark: EdgeMark,
    /// The reshape window of an unsafe start.
    window: Option<StartWindow>,
    /// The window's piece in the tables, from `mark`.
    ///
    /// It is shaped once. Only a candidate reshaped as one piece takes it
    /// back.
    head: Option<ReshapedStart>,
    /// The most the line may take: its band plus 1/64, less its indent.
    room: LayoutUnit,
    start_cost: LayoutUnit,
    /// How far a mark hangs at its start under `hanging-punctuation: first`.
    hang_start: LayoutUnit,
    /// The line's start where fitting reads it.
    fit_start: InlineLayoutUnit,
    /// The `text-indent` it takes, which its tabs count from.
    indent: LayoutUnit,
    /// Where it starts from the block's content edge on its paragraph's
    /// start side, which tab stops count from.
    tab_origin: LayoutUnit,
}

/// The breaker: its view of the prepared tables, the shaping caches its
/// reshapes use, and the state of the line being fitted.
struct Breaker<'a, 'c, 'm, 'provider> {
    /// The prepared tables in the variant of the line being broken.
    stages: LineStages<'a>,
    /// The prepared tables in the first line's variant, where `::first-line`
    /// restyles something.
    ///
    /// They swap with `stages` while the first line is broken.
    first_line: Option<LineStages<'a>>,
    clusters: &'a Clusters,
    /// How a line makes room for annotations and marks, where the content
    /// has either.
    annotation: Option<AnnotationRoom<'a>>,
    /// Measurement's whole output, which holds each text's tab stops for
    /// both variants.
    measured: &'a Measured,
    shaping: &'c mut ShapeSession<'m, 'provider>,
    /// The scratch for line heights, carried boxes and annotation levels.
    scratch: &'c mut BreakScratch,
    /// The host, which places the floats the lines reach and gives each
    /// line its band.
    exclusions: &'c mut dyn Exclusions,
    /// Some node is an inline box, whose strut a line it is open across
    /// holds. Without one, no line looks for any.
    has_inline_boxes: bool,
    /// The block's `text-box-trim`, where it trims.
    ///
    /// Later lines stand where the trim moves the first. The host is asked
    /// for their bands and floats there.
    trims: Option<TextBoxTrims>,
    /// Some cluster is unsafe to break before. Without one, no edge is
    /// reshaped.
    reshapes: bool,
    /// Some style trims a wrapped line's opening mark (`space-first` or
    /// `trim-start`), so a line starting at one is reshaped.
    trims_starts: bool,
    /// Some text has punctuation a line's end may trim. Without it, no line
    /// looks.
    trims_ends: bool,
    /// Punctuation in a font with no `halt` is trimmed by halving its
    /// advance, and a reshaped edge does the same.
    halves_punctuation: bool,
    /// Some style spaces its letters or words, which a reshaped edge adds to
    /// its advances as the prefix does.
    spaced: bool,
    /// Which clusters take word-spacing, in the prefix and in a reshaped
    /// edge alike.
    words: WordSpacingRule,
    /// Where `text-autospace` puts room, in the prefix and in a reshaped
    /// edge alike.
    seams: AutospaceRules,
    text_align: TextAlign,
    /// The content has a ruby column, which opportunity searches step over
    /// whole.
    has_ruby_columns: bool,
    /// The ruby column the last column search ended near, which the next
    /// walks from: the breaker's searches stay near the line it fits.
    column: Cell<RubyColumnId>,
    /// Which opportunities a ruby column splits at.
    ruby_break_within: RubyBreakWithin,
    /// The ruby column the line being fitted continues, split by the line
    /// before.
    continued: Option<Continued>,
    /// `text-indent`, and the lines it applies to.
    text_indent: TextIndent,
    /// The content has a float, which the lines place as they reach it.
    /// Without one, no line asks the host to place anything.
    has_floats: bool,
    /// Some style sets an initial letter, which the first line may open
    /// with and the lines below make room for.
    has_initial_letter: bool,
    /// The block wraps its lines, so a line the floats narrow until it
    /// overflows moves down past them.
    ///
    /// A block that does not wrap keeps every line beside the floats,
    /// overflowing. Chrome's `ShouldWrapLine` decides this from the block's
    /// own style.
    wraps: bool,
    /// `text-overflow: ellipsis`: a line that overflows its band is cut for
    /// an ellipsis.
    ellipsis: bool,
    /// `line-clamp`: the most lines the block keeps, or the end `auto`
    /// measures them against.
    clamp: Clamp,
    /// How the paragraphs choose their lines: `text-wrap-style`, with the
    /// config's rule for `pretty`.
    choice: LineChoice,
    /// What the line being fitted is held to. It is `Free` except while a
    /// paragraph is balanced or scored.
    hold: Hold,
    /// The room the last kept line leaves the next at its end, or, for the
    /// first line, the room above the block.
    carry: Carry,
    /// The text item and shaping run the last line's extent read, which the
    /// next line's starts from.
    line_fonts: LineFonts,
}
