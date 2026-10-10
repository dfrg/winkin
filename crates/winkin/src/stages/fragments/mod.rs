//! Line layout: places each line's items, one flat table for the block.
//!
//! In: [`PlaceInput`], every prepared table, the lines and the area. Out:
//! [`Fragments`].
//! Start at: [`place_fragments`], then `Placer::lines` in `place`.
//!
//! - `place` lays out one line at a time: its pieces, alignment, bidi
//!   reordering, box fragments and ruby, then writes its items once.
//! - `read` reads a text item back: its glyphs and its clusters' advances.
//! - `anchors` turns where each absolutely positioned box's anchor stands
//!   into its static position.
//!
//! Each line is a `Line` item heading the items on it, in visual order, as
//! Blink's `FragmentItems`. Inline positions are exact 48.16 offsets from the
//! line box's left. A box gets an item only where measurement kept it. The
//! lines already written stay whole when a table fills.

mod anchors;
mod boxes;
mod cut;
mod pieces;
mod place;
mod read;
mod ruby;
#[cfg(test)]
mod tests;

use alloc::boxed::Box;
use core::cmp::Ordering;
use core::fmt;
use core::ops::Range;

use crate::config::{EllipsisSpace, TabJustification};
use crate::data::{
    Id, Keyed, RunCursor, SortedCursor, SortedTable, Table, define_flags, define_id, heap_bytes,
    index_to_u32, u32_to_index,
};
use crate::stages::analysis::{
    Analysis, BidiLevel, ClusterId, Clusters, ControlLevel, ItemClusters, ParagraphId,
    RunOrientation,
};
use crate::stages::content::{Content, Item, ItemId, NodeId};
use crate::stages::lines::{Area, BlockResult, LineBand, LineId, LinePlacements, Lines, RubyPiece};
use crate::stages::measure::{
    AutospaceRules, GeneratedPieces, JustifyOpportunities, JustifySummary, KeptBoxId, KeptBoxes,
    RubyColumnId, RubyColumns, RubyLevelId, ShapedGeneratedId,
};
use crate::stages::{LineStages, Stages, lines};
use crate::style::{TextAlign, TextAlignLast, TextJustify};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

use crate::stages::fonts::UsedFontId;
use crate::stages::shape::ShapedRunId;
use pieces::TabLine;
use place::{Group, LineBoxId, RubyEdges};

pub(crate) use place::PlaceScratch;

// What a line's views read a text item through.
pub(crate) use read::{ClusterWalk, ExactGlyph, GlyphWalk, ReadInput};

/// What line layout reads: the prepared tables, the lines, the area and the
/// config choices it makes at every relayout.
pub(crate) struct PlaceInput<'a> {
    pub(crate) stages: Stages<'a>,
    pub(crate) lines: &'a Lines,
    /// Breaking's transient facts, in the same order as the retained rows.
    pub(crate) placements: &'a LinePlacements,
    /// The area the lines were broken in. Tab stops count from its content
    /// edge.
    pub(crate) area: Area,
    /// Whether a justified line widens its tabs (`Config::tab_justification`).
    pub(crate) tab_justification: TabJustification,
    /// Whether a cut keeps a space before its ellipsis
    /// (`Config::ellipsis_space`).
    pub(crate) ellipsis_space: EllipsisSpace,
}

/// Lays out every line of `input` into `out`, one line at a time.
///
/// Uses the block's trim, end and baselines that breaking finished, and
/// works in `scratch`. Fills `out`, which starts empty; nothing else fills
/// it.
pub(crate) fn place_fragments(
    input: &PlaceInput<'_>,
    scratch: &mut PlaceScratch,
    out: &mut Fragments,
) {
    debug_assert_eq!(input.lines.lines.len(), input.placements.len());
    // Each line writes its `Line` item, and each content item at least one
    // item: room for those at once, where a fresh layout's tables would
    // grow by doubling.
    let lines = input.lines.lines.len();
    out.line_heads.reserve(lines);
    out.items
        .reserve(lines.saturating_add(input.stages.content.items.len()));
    scratch.begin();
    Placer::new(input, input.lines.block).lines(scratch, out);
}

define_id! {
    /// Names a fragment item in a layout's table of [`FragmentItem`]s.
    pub(crate) struct FragmentItemId(u32);
}

/// What a fragment item is.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub(crate) enum FragmentItemKind {
    /// A line box, heading the items on it.
    Line,
    /// Text of one node, one used font and one bidi level, drawn with its
    /// glyphs.
    ///
    /// A tab is an item of its own ([`FragmentItemFlags::TAB`]). So is
    /// preserved white space hanging past the line's end
    /// ([`FragmentItemFlags::HANGS`]).
    Text,
    /// Text the layout made: a hyphen or an ellipsis.
    Generated,
    /// An atomic inline: its margin box.
    Atomic,
    /// A box part of an inline box that keeps its fragment: its border
    /// box, heading the items inside it.
    Box,
    /// A ruby annotation line, heading its boxes' items and its text items.
    /// It follows its line's own items.
    AnnotationLine,
}

impl FragmentItemKind {
    /// Returns the kind in the low three bits of `bits`.
    ///
    /// A value that is no kind's maps to the last kind; no item writes one.
    #[inline]
    const fn from_bits(bits: u16) -> Self {
        match bits & FragmentItem::KIND {
            0 => Self::Line,
            1 => Self::Text,
            2 => Self::Generated,
            3 => Self::Atomic,
            4 => Self::Box,
            _ => Self::AnnotationLine,
        }
    }
}

define_flags! {
    /// What else is true of a fragment item: 1 byte.
    ///
    /// No flag marks right-to-left: an item's odd level says it.
    pub(crate) struct FragmentItemFlags(u8) {
        /// Laid out and not painted: the tail an ellipsis hides, kept for
        /// carets and hit testing.
        pub(crate) const HIDDEN = 1 << 0;
        /// Preserved white space hanging past the line's end. Its box covers
        /// it, and alignment sees only the part that fits.
        pub(crate) const HANGS = 1 << 1;
        /// A box part whose left side is not the box's own edge. The box
        /// goes on past it, on another line or across a reordering.
        pub(crate) const OPEN_LEFT = 1 << 2;
        /// A box part whose right side is not the box's own edge.
        pub(crate) const OPEN_RIGHT = 1 << 3;
        /// Text whose style sets emphasis marks, which a reader yields from
        /// its clusters.
        const EMPHASIS = 1 << 4;
        /// A tab: a text item of one cluster that draws nothing.
        ///
        /// Its size is the distance to its stop where it lands on the line,
        /// on layout's grid.
        pub(crate) const TAB = 1 << 5;
        /// A ruby annotation's text, on an annotation line. Its clusters step
        /// by their glyphs' own advances, since the prefix sums give them
        /// none.
        const ANNOTATION = 1 << 6;
        /// A line's item, in content where a box decorates its text.
        ///
        /// An inline box holds some of the line or is open across it, so the
        /// line's paint looks among its culled boxes for one that decorates.
        /// Without the flag, none can.
        pub(crate) const BOXED = 1 << 7;
    }
}

/// One placed thing on a line: 40 bytes.
///
/// Every field is final when the item is written. The plain-data fields are
/// public and read directly. `a`, `b`, `c` and `d` mean what the kind says,
/// read through the accessors:
///
/// | Kind | `a` | `b` | `c` | `d` |
/// |---|---|---|---|---|
/// | `Line` | the items after it that are the line's | the first box item to paint, its [`FragmentItemId`]; 0 where the line has none | none | none |
/// | `Text` | its first cluster | one past its last | its shaping run, a [`ShapedRunId`] | the content item holding its first cluster, an [`ItemId`] |
/// | `Atomic` | its first cluster | one past its last | none | none |
/// | `Box` | the items after it inside it; none for a box inside an annotation, whose text is the annotation's | the next box item to paint, its [`FragmentItemId`]; 0 for the last, and for a box inside an annotation, which its annotation paints | its kept box, a [`KeptBoxId`] | none |
/// | `AnnotationLine` | the items after it that are its own: its boxes that keep a fragment, then its text | its [`RubyLevelId`] | none | none |
/// | `Generated` | the boundary it stands at | its generated text, its [`ShapedGeneratedId`] | none | none |
///
/// A ruby annotation's text and boxes name no shaping run and no kept box,
/// and their readers look them up. The links cost 8 bytes an item, 4 of
/// them alignment, and save a search per text item and box part read.
///
/// **Positions.**
/// - A `Line` item's `inline` is its line box's left edge after alignment,
///   from the area's line-left. Its `block` is the line box's top after
///   `text-box-trim`, from the area's block start. Its `size` is zero.
/// - Every other item's `inline` is its exact left edge from its line box's
///   left. Its `block` is its baseline from the line box's top, with its
///   box's shift and its own.
/// - `size` is the item's reach along the line on layout's grid, rounded up
///   as Chrome snaps a text item's width. For a box it is the border box,
///   for an atomic the margin box, for an annotation line its width.
///
/// **A leaf's advance** (text, generated text or an atomic) is stored
/// exactly ([`advance`](Self::advance)). The item keeps `size` and how far
/// rounding shifted it, under 1/64 px, in ten bits packed beside the kind.
/// A reader gets a leaf's exact right edge on any line, justified or
/// reshaped. A box's or an atomic's reach over and under its baseline lives
/// in measurement's extents, not here.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub(crate) struct FragmentItem {
    /// Its exact start along the line: a line's from the area's line-left,
    /// anything else's from its line box's left.
    pub(crate) inline: InlineLayoutUnit,
    /// The node it is of: the block for a line.
    pub(crate) node: NodeId,
    a: u32,
    b: u32,
    c: u32,
    d: u32,
    /// Its reach along the line, on layout's grid: zero for a line.
    pub(crate) size: LayoutUnit,
    /// A line's top across the block; anything else's baseline from its
    /// line box's top.
    pub(crate) block: LayoutUnit,
    /// Its kind in the low three bits ([`kind`](Self::kind)). Above them, for
    /// a leaf, how far its exact advance falls short of `size`, in 1/65536 px
    /// ([`advance`](Self::advance)), in ten bits. The top bit marks text
    /// that `ruby-align` spread room into ([`is_spread`](Self::is_spread)).
    kind_short: u16,
    /// What else is true of it.
    pub(crate) flags: FragmentItemFlags,
    /// Its bidi level: a line's, its paragraph's.
    pub(crate) level: BidiLevel,
}

impl FragmentItem {
    /// The bits of `kind_short` the kind takes. The advance's shortfall sits
    /// above them.
    const KIND: u16 = 0b111;

    /// The bits of `kind_short`, past the kind's, the advance's shortfall
    /// takes: under 1/64 px, so under 1024.
    const SHORT: u16 = 0x3FF;

    /// The bit of `kind_short` that marks spread text.
    const SPREAD: u16 = 1 << 15;

    /// What `c` or `d` hold where the kind names nothing there.
    const NONE: u32 = u32::MAX;

    /// Packs `kind` with a shortfall of `short` 1/65536 px below the size.
    #[inline]
    const fn pack(kind: FragmentItemKind, short: u16) -> u16 {
        (kind as u16) | (short << 3)
    }

    /// Makes a line's item, heading `descendants` items. `first` is the
    /// first box item to paint, where the line has one.
    fn from_line(
        level: BidiLevel,
        left: LayoutUnit,
        top: LayoutUnit,
        descendants: usize,
        first: Option<FragmentItemId>,
        flags: FragmentItemFlags,
    ) -> Self {
        Self {
            inline: InlineLayoutUnit::from_layout(left),
            node: NodeId::BLOCK,
            a: index_to_u32(descendants),
            // No box item is the table's first, which is a line's.
            b: first.map_or(0, |first| index_to_u32(first.get())),
            c: Self::NONE,
            d: Self::NONE,
            size: LayoutUnit::ZERO,
            block: top,
            kind_short: Self::pack(FragmentItemKind::Line, 0),
            flags,
            level,
        }
    }

    /// Makes a generated item: a hyphen or an ellipsis, `text`, drawn for
    /// `node` at the boundary `at`.
    ///
    /// It starts at `inline`, is exactly `advance` wide, and sits on
    /// `baseline`.
    #[allow(clippy::too_many_arguments)]
    fn from_generated(
        node: NodeId,
        level: BidiLevel,
        at: ClusterId,
        text: ShapedGeneratedId,
        inline: InlineLayoutUnit,
        advance: InlineLayoutUnit,
        baseline: LayoutUnit,
        flags: FragmentItemFlags,
    ) -> Self {
        let (size, short) = advance.to_layout_and_short();
        Self {
            inline,
            node,
            a: index_to_u32(at.get()),
            b: index_to_u32(text.get()),
            c: Self::NONE,
            d: Self::NONE,
            size,
            block: baseline,
            kind_short: Self::pack(FragmentItemKind::Generated, short),
            flags,
            level,
        }
    }

    /// Makes a box part of the box `node`, heading `descendants` items.
    ///
    /// It paints before the box item `next`, or last where `next` is `None`.
    #[allow(clippy::too_many_arguments)]
    fn from_box_part(
        node: NodeId,
        level: BidiLevel,
        inline: InlineLayoutUnit,
        size: LayoutUnit,
        baseline: LayoutUnit,
        flags: FragmentItemFlags,
        descendants: usize,
        next: Option<FragmentItemId>,
    ) -> Self {
        Self {
            inline,
            node,
            a: index_to_u32(descendants),
            // No box item is the table's first, which is a line's.
            b: next.map_or(0, |next| index_to_u32(next.get())),
            c: Self::NONE,
            d: Self::NONE,
            size,
            block: baseline,
            kind_short: Self::pack(FragmentItemKind::Box, 0),
            flags,
            level,
        }
    }

    /// Returns this box part naming its kept box, `kept`.
    fn with_kept_box(self, kept: KeptBoxId) -> Self {
        Self {
            c: index_to_u32(kept.get()),
            ..self
        }
    }

    /// Makes an annotation line for the annotation `node`, level `id` of its
    /// ruby column.
    ///
    /// It spans `width` from `left`, has its baseline at `baseline` from the
    /// line box's top, and heads `descendants` items: its boxes and its text.
    fn from_annotation(
        node: NodeId,
        level: BidiLevel,
        id: RubyLevelId,
        left: InlineLayoutUnit,
        width: LayoutUnit,
        baseline: LayoutUnit,
        descendants: usize,
    ) -> Self {
        Self {
            inline: left,
            node,
            a: index_to_u32(descendants),
            b: index_to_u32(id.get()),
            c: Self::NONE,
            d: Self::NONE,
            size: width,
            block: baseline,
            kind_short: Self::pack(FragmentItemKind::AnnotationLine, 0),
            flags: FragmentItemFlags::NONE,
            level,
        }
    }

    /// Returns its kind.
    #[inline]
    pub(crate) fn kind(&self) -> FragmentItemKind {
        FragmentItemKind::from_bits(self.kind_short)
    }

    /// Returns whether it is a leaf: text, an atomic inline or a generated
    /// text.
    #[inline]
    pub(crate) fn is_leaf(&self) -> bool {
        matches!(
            self.kind(),
            FragmentItemKind::Text | FragmentItemKind::Atomic | FragmentItemKind::Generated
        )
    }

    /// Returns its exact reach along the line; `size` is this rounded up, as
    /// Chrome snaps a text item's width.
    ///
    /// - Text: its clusters' advances, with reshaped line edges and any
    ///   justification room.
    /// - A tab: its width where it landed, on layout's grid.
    /// - Generated text: its shaped text's advance.
    /// - An atomic: its margin box.
    /// - A box or an annotation line: its size. A line: zero.
    #[inline]
    pub(crate) fn advance(&self) -> InlineLayoutUnit {
        InlineLayoutUnit::from_layout_less(self.size, (self.kind_short >> 3) & Self::SHORT)
    }

    /// Returns the ruby level an annotation line sets, or `None` for any
    /// other item.
    pub(crate) fn ruby_level(&self) -> Option<RubyLevelId> {
        (self.kind() == FragmentItemKind::AnnotationLine)
            .then(|| RubyLevelId::new(u32_to_index(self.b)))
    }

    /// Whether it is text that carries emphasis marks. A tab draws nothing,
    /// so it carries none.
    #[inline]
    pub(crate) fn is_marked(&self) -> bool {
        self.kind() == FragmentItemKind::Text
            && self.flags.contains(FragmentItemFlags::EMPHASIS)
            && !self.flags.contains(FragmentItemFlags::TAB)
    }

    /// Returns the clusters a leaf holds, in logical order; none for a line
    /// or a box.
    pub(crate) fn clusters(&self) -> Range<ClusterId> {
        match self.kind() {
            FragmentItemKind::Text | FragmentItemKind::Atomic => {
                ClusterId::new(u32_to_index(self.a))..ClusterId::new(u32_to_index(self.b))
            }
            FragmentItemKind::Generated => {
                let at = ClusterId::new(u32_to_index(self.a));
                at..at
            }
            FragmentItemKind::Line | FragmentItemKind::Box | FragmentItemKind::AnnotationLine => {
                Range::default()
            }
        }
    }

    /// Returns the clusters it draws: a text item's, none for any other.
    #[inline]
    fn text_clusters(&self) -> Range<ClusterId> {
        if self.kind() == FragmentItemKind::Text {
            self.clusters()
        } else {
            Range::default()
        }
    }

    /// Returns how many items after it are its own; zero for a leaf.
    pub(crate) fn descendants(&self) -> usize {
        match self.kind() {
            FragmentItemKind::Line | FragmentItemKind::Box | FragmentItemKind::AnnotationLine => {
                u32_to_index(self.a)
            }
            FragmentItemKind::Text | FragmentItemKind::Atomic | FragmentItemKind::Generated => 0,
        }
    }

    /// Returns the next box item to paint after this one, if any.
    ///
    /// From a line's item it is the line's first box item. From a box item
    /// it is the next on the line. Either is among the line's own items
    /// ([`Fragments::line_item_ids`]).
    ///
    /// Box items paint by box in tree order, and a box's parts left to
    /// right. Line layout links them as it writes them, so a painter follows
    /// the links instead of searching the line.
    pub(crate) fn painted_next(&self) -> Option<FragmentItemId> {
        (matches!(self.kind(), FragmentItemKind::Line | FragmentItemKind::Box) && self.b > 0)
            .then(|| FragmentItemId::new(u32_to_index(self.b)))
    }

    /// Returns the shaping run of a text item's first shaped cluster, or of
    /// its first cluster where none is shaped.
    ///
    /// Its used font and orientation are that run's. `None` for any other
    /// item, and for an annotation's text, which names none.
    #[inline]
    pub(crate) fn run(&self) -> Option<ShapedRunId> {
        (self.kind() == FragmentItemKind::Text && self.c != Self::NONE)
            .then(|| ShapedRunId::new(u32_to_index(self.c)))
    }

    /// Returns the content item holding a text item's first cluster, or
    /// `None` for any other item.
    #[inline]
    pub(crate) fn text_item(&self) -> Option<ItemId> {
        (self.kind() == FragmentItemKind::Text && self.d != Self::NONE)
            .then(|| ItemId::new(u32_to_index(self.d)))
    }

    /// Whether `ruby-align` spread room into this text item, which the
    /// fragments' spread rows say how much of.
    #[inline]
    pub(crate) fn is_spread(&self) -> bool {
        self.kind_short & Self::SPREAD != 0
    }

    /// Returns the kept box a box item is a part of, or `None` for any other
    /// item, and for a box inside an annotation, which names none.
    #[inline]
    pub(crate) fn kept_box(&self) -> Option<KeptBoxId> {
        (self.kind() == FragmentItemKind::Box && self.c != Self::NONE)
            .then(|| KeptBoxId::new(u32_to_index(self.c)))
    }

    /// Returns the generated text a generated item draws, or `None` for any
    /// other item.
    pub(crate) fn generated(&self) -> Option<ShapedGeneratedId> {
        (self.kind() == FragmentItemKind::Generated)
            .then(|| ShapedGeneratedId::new(u32_to_index(self.b)))
    }
}

impl fmt::Debug for FragmentItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FragmentItem")
            .field("kind", &self.kind())
            .field("inline", &self.inline)
            .field("advance", &self.advance())
            .field("size", &self.size)
            .field("block", &self.block)
            .field("node", &self.node)
            .field("a", &self.a)
            .field("b", &self.b)
            .field("c", &self.c)
            .field("d", &self.d)
            .field("flags", &self.flags)
            .field("level", &self.level)
            .finish()
    }
}

/// The line's spare room per justification opportunity, in 48.16: 32 bytes.
///
/// The room divides truncated, as in Chrome's `ShapeResultSpacing`
/// (`expansion / count`). The remainder goes to the opportunity Chrome fills
/// last, as its `NextExpansion` does, so the shares add up exactly. That is
/// the last one of the logically last item, or its first if that item is
/// right-to-left.
///
/// Each item's `inline` and advance already include the room. A reader adds
/// a cluster's room ([`room`](Self::room)) to its advance and moves its
/// glyphs by the room before it. Nothing is stored per cluster.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
struct LineJustification {
    summary: JustifySummary,
    /// The room each opportunity gets, the last one aside.
    per: InlineLayoutUnit,
    /// The cluster whose opportunity is filled last: its after one where it
    /// has one, else its before one.
    last: ClusterId,
    /// What the last takes beyond `per`, in 1/65536 px. It is less than the
    /// line's count of opportunities, so it fits a `u32`.
    leftover: u32,
}

impl LineJustification {
    /// Spreads `extra` over `count` opportunities, the last of them
    /// `last`'s. Returns `None` when `count` is zero.
    fn new(
        extra: InlineLayoutUnit,
        count: u32,
        last: ClusterId,
        summary: JustifySummary,
    ) -> Option<Self> {
        if count == 0 {
            return None;
        }
        let count = i64::from(count);
        let per = extra.raw() / count;
        // Less than `count`, and not negative for room that is not.
        let leftover = extra.raw() - per.saturating_mul(count);
        Some(Self {
            per: InlineLayoutUnit::from_raw(per),
            summary,
            last,
            leftover: u32::try_from(leftover).unwrap_or(0),
        })
    }

    /// Returns what the last opportunity takes beyond `per`.
    fn leftover(&self) -> InlineLayoutUnit {
        InlineLayoutUnit::from_raw(i64::from(self.leftover))
    }

    /// Returns what `count` opportunities take together, with the leftover
    /// when `last` says the last one is among them.
    ///
    /// This is how much a piece of the line widens.
    fn share(&self, count: u32, last: bool) -> InlineLayoutUnit {
        let each = InlineLayoutUnit::from_raw(self.per.raw().saturating_mul(i64::from(count)));
        if last { each + self.leftover() } else { each }
    }

    /// Returns the room `cluster` takes before and after it, given which
    /// opportunities it has.
    #[inline]
    fn room(
        &self,
        cluster: ClusterId,
        before: bool,
        after: bool,
    ) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let last = cluster == self.last;
        let share = |takes: bool, rest: bool| match (takes, rest) {
            (false, _) => InlineLayoutUnit::ZERO,
            (true, false) => self.per,
            (true, true) => self.per + self.leftover(),
        };
        // Chrome takes a cluster's before opportunity first.
        (share(before, last && !after), share(after, last))
    }
}

/// A justified range's amounts and which of its clusters have opportunities.
///
/// The range is a line, or a ruby base or annotation spread as a line of its
/// own. Line layout builds one for the line it places, and a reader for the
/// item it reads. Both count opportunities by the same rule.
#[derive(Clone, Debug)]
struct Justified<'a> {
    justification: LineJustification,
    opportunities: JustifyOpportunities<'a>,
}

impl<'a> Justified<'a> {
    /// Spreads `room` over the opportunities of `clusters`, justified as a
    /// line of their own under `justify`.
    ///
    /// This serves a ruby base or an annotation. Returns `None` where there
    /// is no room or no opportunity.
    fn new(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
        room: InlineLayoutUnit,
    ) -> Option<Self> {
        if room <= InlineLayoutUnit::ZERO {
            return None;
        }
        let opportunities =
            JustifyOpportunities::new(content, analysis, clusters.clone(), justify)?;
        let count = opportunities.count_range(clusters.clone());
        let last = opportunities.last(clusters)?;
        Some(Self {
            justification: LineJustification::new(room, count, last, opportunities.summary())?,
            opportunities,
        })
    }

    /// Spreads `room` over the opportunities of ruby base `clusters` in
    /// `column`, as [`new`](Self::new) does for other text.
    fn from_base(
        content: &'a Content,
        analysis: &'a Analysis,
        clusters: Range<ClusterId>,
        justify: TextJustify,
        room: InlineLayoutUnit,
        rubies: &'a RubyColumns,
        column: (RubyColumnId, RubyColumnId),
    ) -> Option<Self> {
        if room <= InlineLayoutUnit::ZERO {
            return None;
        }
        let opportunities = JustifyOpportunities::from_base(
            content,
            analysis,
            clusters.clone(),
            justify,
            rubies,
            column,
        )?;
        let count = opportunities.count_range(clusters.clone());
        let last = opportunities.last(clusters)?;
        Some(Self {
            justification: LineJustification::new(room, count, last, opportunities.summary())?,
            opportunities,
        })
    }

    /// Returns the room `cluster` takes before and after it; none outside
    /// the justified range.
    ///
    /// On an ASCII line justified at spaces, the cluster's class decides
    /// (`JustifyOpportunities::is_simple`).
    #[inline]
    fn room(&self, cluster: ClusterId) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let (before, after) = if self.opportunities.is_simple() {
            (false, self.opportunities.simple_after(cluster))
        } else {
            self.opportunities.at(cluster)
        };
        self.justification.room(cluster, before, after)
    }
}

/// Facts kept only for a justified line: 40 bytes.
///
/// `content_end` fits in the padding between the line key and the
/// justification.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
struct JustifiedLine {
    line: LineId,
    content_end: ClusterId,
    justification: LineJustification,
}
impl Keyed for JustifiedLine {
    type Key = LineId;
    fn key(&self) -> LineId {
        self.line
    }
}

define_id! {
    /// Names an absolutely positioned box's static position in a layout's
    /// table of [`Anchor`]s.
    pub(crate) struct AnchorId(u32);
}

/// An absolutely positioned box's static position, where line layout put
/// its anchor: 32 bytes.
///
/// It follows Chrome's `PlaceOutOfFlowObjects`. An inline-level box stands
/// where its anchor is on its line, at the line box's top. A block-level box
/// stands at the block's start edge, below the line where in-flow content
/// comes before it there. An anchor on no line box stands where an empty
/// line would start, at the lines' end.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Anchor {
    /// The box's node, whose one item is the anchor.
    pub(crate) node: NodeId,
    /// The line it falls on, or `None` where it falls on no line box.
    pub(crate) line: Option<LineId>,
    /// Where the box's inline-start margin edge goes, from the area's
    /// line-left.
    pub(crate) inline: InlineLayoutUnit,
    /// Where its block-start margin edge goes, from the area's block start.
    pub(crate) block: LayoutUnit,
    /// Its inline start is on the right: its bidi level is odd, or, for a
    /// block-level box, the block reads right to left.
    pub(crate) rtl: bool,
}

/// Line layout's output: the fragment items of every line.
///
/// Only [`place_fragments`] clears and refills it, so a relayout keeps every
/// allocation.
pub(crate) struct Fragments {
    /// Every line's items, line by line: the `Line` item, then its items in
    /// visual order, pre-order.
    pub(crate) items: Table<FragmentItemId, FragmentItem>,
    /// Each line's `Line` item.
    pub(crate) line_heads: Table<LineId, FragmentItemId>,
    /// The tables only some content fills: the justified lines and the
    /// static positions.
    ///
    /// The box is made the first time line layout writes either, and kept
    /// after. Most layouts hold only the empty pointer.
    rare: Option<Box<RareFragments>>,
}

/// The room `ruby-align` spread into one text item, which its extent and
/// its outermost clusters take in: 24 bytes.
#[derive(Copy, Clone, PartialEq, Debug)]
struct SpreadText {
    line: LineId,
    /// The item's first cluster, which names it among its line's items.
    cluster: ClusterId,
    /// The room on its left and on its right.
    left: InlineLayoutUnit,
    right: InlineLayoutUnit,
}

impl Keyed for SpreadText {
    type Key = (LineId, ClusterId);
    fn key(&self) -> (LineId, ClusterId) {
        (self.line, self.cluster)
    }
}

/// The tables of [`Fragments`] that only some content fills.
struct RareFragments {
    /// The justified lines' amounts, sorted by line; empty where no line is
    /// justified.
    justified: SortedTable<JustifiedLine>,
    /// The text items `ruby-align` spread room into, sorted by line and
    /// first cluster.
    spread: SortedTable<SpreadText>,
    /// The absolutely positioned boxes' static positions, in node order:
    /// one for each anchor in the content.
    anchors: Table<AnchorId, Anchor>,
}

impl Default for RareFragments {
    fn default() -> Self {
        Self {
            justified: SortedTable::new(),
            spread: SortedTable::new(),
            anchors: Table::new(),
        }
    }
}

heap_bytes! {
    RareFragments { justified, spread, anchors }
}

impl Fragments {
    /// Makes an empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            items: Table::new(),
            line_heads: Table::new(),
            rare: None,
        }
    }

    /// Empties the items, keeping every allocation.
    pub(crate) fn clear(&mut self) {
        self.items.clear();
        self.line_heads.clear();
        if let Some(rare) = &mut self.rare {
            rare.justified.clear();
            rare.spread.clear();
            rare.anchors.clear();
        }
    }

    /// Returns the room `ruby-align` spread on the left and right of text
    /// item `item` on line `line`: zero where it spread none.
    pub(crate) fn spread_room(
        &self,
        line: LineId,
        item: &FragmentItem,
    ) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let none = (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        if !item.is_spread() {
            return none;
        }
        self.rare
            .as_deref()
            .and_then(|rare| rare.spread.get((line, item.clusters().start)))
            .map_or(none, |row| (row.left, row.right))
    }

    /// Returns where the spread rows stand, which the line being written
    /// sorts its own rows from.
    fn spread_mark(&self) -> usize {
        self.rare.as_ref().map_or(0, |rare| rare.spread.len())
    }

    /// Writes the spread rows of `pieces`' text, items of line `line`, in
    /// any order: [`settle_spread`](Self::settle_spread) sorts them.
    fn push_spread(&mut self, line: LineId, pieces: &[Piece]) {
        for piece in pieces {
            work::step();
            if piece.kind == PieceKind::Text
                && piece.room != (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO)
            {
                self.rare_mut().spread.push_unsorted(SpreadText {
                    line,
                    cluster: piece.start,
                    left: piece.room.0,
                    right: piece.room.1,
                });
            }
        }
    }

    /// Sorts the spread rows the line being written pushed since `mark`.
    fn settle_spread(&mut self, mark: usize) {
        if let Some(rare) = &mut self.rare {
            rare.spread.sort_from(mark);
        }
    }

    /// Returns the rare tables for line layout to write, made empty the first
    /// time.
    fn rare_mut(&mut self) -> &mut RareFragments {
        self.rare.get_or_insert_with(Box::default)
    }

    /// Returns the static positions, in node order: none where the content
    /// has no absolutely positioned box.
    pub(crate) fn anchors(&self) -> &[Anchor] {
        self.rare
            .as_deref()
            .map_or(&[], |rare| rare.anchors.as_slice())
    }

    /// Returns the ids of every item of `line`: its `Line` item, its items
    /// and its annotation lines, up to the next line's. The range is empty
    /// for a line past the last.
    #[inline]
    fn line_span_ids(&self, line: LineId) -> Range<FragmentItemId> {
        let Some(&head) = self.line_heads.get(line) else {
            return Range::default();
        };
        let end = self
            .line_heads
            .get(LineId::new(line.get() + 1))
            .map_or(self.items.next_id(), |&next| next);
        head..end.max(head)
    }

    /// Returns the ids of the items on `line` after its `Line` item, in
    /// visual order and pre-order; none for a line past the last.
    #[inline]
    pub(crate) fn line_item_ids(&self, line: LineId) -> Range<FragmentItemId> {
        let span = self.line_span_ids(line);
        if span.is_empty() {
            return span;
        }
        let descendants = self
            .items
            .get(span.start)
            .map_or(0, FragmentItem::descendants);
        let start = span.start.get().saturating_add(1);
        FragmentItemId::new(start)..FragmentItemId::new(start.saturating_add(descendants))
    }

    /// Returns the items on `line` after its `Line` item, in visual order and
    /// pre-order; none for a line past the last.
    #[inline]
    pub(crate) fn line_items(&self, line: LineId) -> &[FragmentItem] {
        self.items
            .get_slice(self.line_item_ids(line))
            .unwrap_or_default()
    }

    /// Returns the ids of `line`'s annotation lines, each heading its text.
    ///
    /// They sit after the line's own items and before the next line. The
    /// range is empty for a line past the last or with no ruby.
    pub(crate) fn annotation_ids(&self, line: LineId) -> Range<FragmentItemId> {
        let span = self.line_span_ids(line);
        let items = self.line_item_ids(line);
        items.end..span.end.max(items.end)
    }

    /// Returns `line`'s annotation lines, each heading its text (see
    /// [`annotation_ids`](Self::annotation_ids)).
    pub(crate) fn line_annotations(&self, line: LineId) -> &[FragmentItem] {
        self.items
            .get_slice(self.annotation_ids(line))
            .unwrap_or_default()
    }

    /// Returns `line`'s annotation line for ruby level `level`, or `None`.
    ///
    /// A line's annotation lines are in level order, so this binary searches.
    /// Each probe steps back over an annotation's text to its head.
    pub(crate) fn annotation_line(
        &self,
        line: LineId,
        level: RubyLevelId,
    ) -> Option<&FragmentItem> {
        let items = self.line_annotations(line);
        let (mut low, mut high) = (0, items.len());
        while low < high {
            work::step();
            let mut head = low + (high - low) / 2;
            while head > low && items.get(head)?.kind() != FragmentItemKind::AnnotationLine {
                work::step();
                head -= 1;
            }
            let found = items.get(head)?;
            let at = found.ruby_level()?;
            match at.cmp(&level) {
                Ordering::Equal => return Some(found),
                Ordering::Less => low = head + 1 + found.descendants(),
                Ordering::Greater => high = head,
            }
        }
        None
    }

    /// Returns `line`'s justification record, or `None` where it is not
    /// justified.
    ///
    /// The lookup is a binary search, skipped where no line is justified.
    #[inline]
    fn justification(&self, line: LineId) -> Option<JustifiedLine> {
        let justified = &self.rare.as_deref()?.justified;
        if justified.is_empty() {
            return None;
        }
        justified.get(line).copied()
    }

    /// Returns the justified lines' amounts, sorted by line, for the tests.
    #[cfg(test)]
    fn justified(&self) -> &[JustifiedLine] {
        self.rare
            .as_deref()
            .map_or(&[], |rare| rare.justified.as_slice())
    }
}

heap_bytes! {
    Fragments { items, line_heads, rare }
}

/// Returns `node`'s baseline on a line of `items`, from the line box's top.
///
/// A kept box's baseline is its own item's. Anything else takes the
/// innermost kept box around it, or `line`'s baseline outside every box. A
/// culled box has no shift of its own, so this is right for it too. The walk
/// visits the items once.
pub(crate) fn box_baseline(
    content: &Content,
    line: &lines::LineRecord,
    items: &[FragmentItem],
    node: NodeId,
) -> LayoutUnit {
    let nodes = &content.nodes;
    items
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Box && nodes.contains(item.node, node))
        .max_by_key(|item| item.node)
        .map_or_else(|| line.ascent(), |item| item.block)
}

// The placer and the pieces its files share: `place` builds and drives
// them, and `pieces`, `boxes`, `cut` and `ruby` each
// add to `Placer`'s `impl`.

/// What a piece is.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum PieceKind {
    /// Clusters of one node, one used font and one level.
    ///
    /// A tab is a piece of its own, as wide as the distance to its stop. So
    /// is preserved white space hanging past the line's end. Each is flagged.
    Text,
    /// An atomic inline's margin box.
    Atomic,
    /// Where a box that keeps a fragment starts on the line, taking no room
    /// (Blink's box placeholder).
    ///
    /// It belongs to the box, so a box holding nothing else on the line
    /// still has a place.
    Place,
    /// A ruby container's edge, which takes room and bounds no box part.
    Room,
    /// `line-padding` at one of the line's ends, inside the innermost box
    /// there.
    ///
    /// It is at the paragraph's level, so reordering leaves it at the line's
    /// end.
    Padding,
    /// Room a ruby column puts around or inside its base, or inside an
    /// annotation.
    ///
    /// It is at the level of the text beside it, so reordering keeps it with
    /// that text.
    Gap,
    /// Room `ruby-align` spreads at an opportunity of a base's or an
    /// annotation's text, at the level and of the node of the text beside
    /// it. The text takes it in before its items are written.
    Spread,
    /// The hyphen drawn after a line broken at a soft hyphen, as Blink's
    /// `PlaceHyphen` places it.
    ///
    /// It is generated text at the soft hyphen's level, in its box.
    Hyphen,
    /// A box's bidi controls, which take no room and draw nothing.
    ///
    /// Its level splits the runs on either side, as Blink's level-only line
    /// item for a bidi control does.
    Control,
    /// An absolutely positioned box's anchor, which takes no room and draws
    /// nothing.
    ///
    /// It has a level of its own, as Blink's line item for an out-of-flow
    /// box does. Where reordering puts it is the box's static position.
    Absolute,
}

impl PieceKind {
    /// Whether the piece is an item of its own.
    #[inline]
    fn is_leaf(self) -> bool {
        self.fragment().is_some()
    }

    /// Whether the piece has no level of its own. Reordering gives it the
    /// next piece's level, as Blink's opaque items take theirs.
    fn is_opaque(self) -> bool {
        matches!(self, Self::Place | Self::Room)
    }

    /// Returns the item kind a piece of this kind writes, or `None` where it
    /// writes no item.
    #[inline]
    fn fragment(self) -> Option<FragmentItemKind> {
        match self {
            Self::Text => Some(FragmentItemKind::Text),
            Self::Atomic => Some(FragmentItemKind::Atomic),
            Self::Hyphen => Some(FragmentItemKind::Generated),
            Self::Place
            | Self::Room
            | Self::Padding
            | Self::Gap
            | Self::Spread
            | Self::Control
            | Self::Absolute => None,
        }
    }
}

/// One thing a line places, in scratch until its items are written.
#[derive(Copy, Clone, Debug)]
struct Piece {
    kind: PieceKind,
    node: NodeId,
    level: BidiLevel,
    /// Its clusters. A placeholder or an edge sits at `start`, holding none.
    start: ClusterId,
    end: ClusterId,
    /// For text, the used font of its shaped clusters, where it has any.
    font: Option<UsedFontId>,
    /// For text, the shaping run of its first shaped cluster, or of its
    /// first cluster where none is shaped. Its item names it.
    run: Option<ShapedRunId>,
    /// For text, the content item holding its first cluster.
    item: Option<ItemId>,
    /// For text, how its glyphs stand.
    ///
    /// One text item keeps one orientation, since its reader draws it one
    /// way. It is horizontal for every piece unless some text stands upright.
    orientation: RunOrientation,
    /// Its exact reach along the line.
    advance: InlineLayoutUnit,
    /// Its exact start from the line box's left, written once the line is
    /// reordered.
    inline: InlineLayoutUnit,
    /// The innermost kept box it belongs to on the line; `None` outside
    /// every kept box. A placeholder's is its own box.
    owner: Option<LineBoxId>,
    /// How many of the line's justification opportunities it holds, where
    /// the line is justified.
    opportunities: u32,
    /// How far its baseline is raised from the line's: its box's shift, plus
    /// an atomic inline's own.
    shift: LayoutUnit,
    /// Its item's flags.
    ///
    /// `TAB` and `HANGS` are set as the piece is made. `HIDDEN` marks a piece
    /// an ellipsis cut hides whole: it keeps its place and is not painted.
    flags: FragmentItemFlags,
    /// For text, the room `ruby-align` spread on its left and its right,
    /// which its advance took in from the gaps beside it.
    room: (InlineLayoutUnit, InlineLayoutUnit),
}

impl Piece {
    /// Makes a piece of `node` at `level`, holding `clusters`, `advance`
    /// wide, in the box `owner` and raised `shift`.
    ///
    /// It starts with no font, no position, no opportunities and no flags.
    #[inline]
    fn new(
        kind: PieceKind,
        node: NodeId,
        level: BidiLevel,
        clusters: Range<ClusterId>,
        advance: InlineLayoutUnit,
        owner: Option<LineBoxId>,
        shift: LayoutUnit,
    ) -> Self {
        Self {
            kind,
            node,
            level,
            start: clusters.start,
            end: clusters.end,
            font: None,
            run: None,
            item: None,
            orientation: RunOrientation::Horizontal,
            advance,
            inline: InlineLayoutUnit::ZERO,
            owner,
            opportunities: 0,
            shift,
            flags: FragmentItemFlags::NONE,
            room: (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO),
        }
    }

    /// Whether it is content text set on the line: not a tab, and not
    /// hanging.
    #[inline]
    fn is_text(&self) -> bool {
        self.kind == PieceKind::Text
            && !self.flags.contains(FragmentItemFlags::TAB)
            && !self.flags.contains(FragmentItemFlags::HANGS)
    }

    /// Returns its item of `kind`, a leaf holding its clusters, flagged
    /// `flags`, on `baseline` from the line box's top less its shift.
    ///
    /// Text names its shaping run and its content item.
    #[inline]
    fn item(
        &self,
        kind: FragmentItemKind,
        flags: FragmentItemFlags,
        baseline: LayoutUnit,
    ) -> FragmentItem {
        let (size, short) = self.advance.to_layout_and_short();
        FragmentItem {
            inline: self.inline,
            node: self.node,
            a: index_to_u32(self.start.get()),
            b: index_to_u32(self.end.get()),
            c: self
                .run
                .map_or(FragmentItem::NONE, |run| index_to_u32(run.get())),
            d: self
                .item
                .map_or(FragmentItem::NONE, |item| index_to_u32(item.get())),
            size,
            block: baseline - self.shift,
            kind_short: FragmentItem::pack(kind, short)
                | if self.room == (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO) {
                    0
                } else {
                    FragmentItem::SPREAD
                },
            flags,
            level: self.level,
        }
    }

    /// Whether a cut keeps its logically first clusters.
    ///
    /// A cut keeps the clusters on the line's start side. Those are the first
    /// where the piece reads the same way as the paragraph at `level`.
    #[inline]
    fn keeps_first(&self, level: BidiLevel) -> bool {
        self.level.is_rtl() == level.is_rtl()
    }

    /// Makes a spread gap of `room` at boundary `at`, with this piece's node,
    /// level and box, so reordering keeps the two together.
    fn gap(&self, at: ClusterId, room: InlineLayoutUnit) -> Self {
        Self::new(
            PieceKind::Spread,
            self.node,
            self.level,
            at..at,
            room,
            self.owner,
            self.shift,
        )
    }
}

/// Line layout's view of the prepared tables, and its cursors.
///
/// The block's first line uses the first line's shaping, measure and
/// `::first-line` styles where it has them. [`enter`](Self::enter) switches
/// `stages` to that variant and back, restarting the cursors.
struct Placer<'a> {
    /// Every prepared table, both variants.
    input: &'a PlaceInput<'a>,
    /// The tables in the variant the current line uses.
    stages: LineStages<'a>,
    clusters: &'a Clusters,
    items: &'a Table<ItemId, Item>,
    item_clusters: &'a ItemClusters,
    count: ClusterId,
    /// The boxes that keep a fragment, in node order.
    boxes: &'a KeptBoxes,
    lines: &'a Lines,
    /// The block result from breaking, which places every line.
    block: BlockResult,
    /// The current line's paragraph, and its level, read once on entering
    /// it.
    paragraph: ParagraphId,
    level: BidiLevel,
    text_align: TextAlign,
    text_align_last: TextAlignLast,
    /// Some box has an edge with room. Without it, no edge takes room and no
    /// boundary is checked.
    has_box_edges: bool,
    /// Some paragraph has clusters at another level (`MIXED_LEVELS`).
    /// Without it, every piece is at its paragraph's level and L1 splits
    /// nothing.
    has_mixed_levels: bool,
    /// Where `text-autospace` puts room, which a seam after right-to-left
    /// text draws with the text after it.
    autospace: AutospaceRules,
    /// The last paragraph holds clusters, so every item at the text's end is
    /// on the last line.
    ends_on_a_line: bool,
    /// The first item not yet placed. The lines take items in order.
    item: ItemId,
    /// The levels of the box controls not yet met, in item order.
    controls: &'a [ControlLevel],
    /// The shaping run the walk is in, and where it ends. It only moves
    /// forward, a run at a time.
    run: RunCursor<ShapedRunId, ClusterId>,
    /// The first kept box not yet opened.
    kept: KeptBoxId,
    /// Where the walk over the measured baseline shifts stands, once a box
    /// has asked for one.
    shift_rows: Option<SortedCursor<'a, (NodeId, LayoutUnit)>>,
    /// Some paragraph has a tab.
    has_tabs: bool,
    /// The area the lines were broken in. Tab stops count from its content
    /// edge.
    area: LineBand,
    /// The line's tabs.
    tab_line: TabLine,
    /// Whether a justified line widens its tabs.
    justification: TabJustification,
    /// Whether a space before an ellipsis stays before it.
    ellipsis_space: EllipsisSpace,
    /// Some style sets `line-padding`.
    has_line_padding: bool,
    /// How `text-group-align` narrows every line's band.
    group: Group,
    /// The ruby columns, in text order, and their levels.
    rubies: &'a RubyColumns,
    /// The first ruby column not yet met. The lines meet columns in order.
    column: RubyColumnId,
    /// Where `column` stood as the line started, before it went back to
    /// the first column continued onto the line.
    line_column: RubyColumnId,
    /// The ruby column pieces continued onto this line.
    ruby_pieces: &'a [RubyPiece],
    /// The room a spread ruby base shares among its justification
    /// opportunities, given out as its pieces are made.
    ///
    /// This is what Chrome's `JustifyResults` spreads over a ruby line.
    spread: Option<Justified<'a>>,
    /// The generated text of the hyphen the current line ends with, found
    /// once as its piece is made.
    ///
    /// A line has one hyphen at most, so the pieces don't carry it.
    hyphen: Option<GeneratedPieces>,
    /// The innermost ruby column open where the walk is, as an index into
    /// the line's columns.
    open_column: Option<usize>,
    /// The block has ruby and some right-to-left text.
    ///
    /// Then no ruby column sits flush with a line's edge, as in Chrome when
    /// bidi is on.
    ruby_in_bidi: bool,
    /// Which ends of the current line a ruby column sits flush with.
    ruby_edges: RubyEdges,
    /// Some style sets emphasis marks, so text items are flagged.
    has_emphasis: bool,
    /// Some box decorates its text (`ContentFlags::DECORATED_BOXES`). Then
    /// each line says whether an inline box holds some of it.
    has_decorated_boxes: bool,
    /// How many inline boxes the items so far open and don't close: those
    /// open across the next line's start.
    open_boxes: u32,
    /// An inline box holds some of the line being laid out, or is open
    /// across it.
    boxed: bool,
    /// Some text stands upright in a vertical line. Only then can two runs
    /// differ in orientation, so only then do pieces split by it.
    has_upright: bool,
    /// The content holds an absolutely positioned box, whose anchor line
    /// layout places.
    has_anchors: bool,
}
