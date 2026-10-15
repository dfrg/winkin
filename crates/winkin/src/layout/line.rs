//! A layout's lines read back, with the items on each in visual order.
//!
//! A [`Line`] views one line record from breaking and the `Line` item line
//! layout placed for it. Each [`Item`] views one fragment item. The other
//! views (`run`, `boxes`, `ruby`, `floats`, `paint`) read the layout through
//! a `Line`.

use core::fmt;
use core::ops::Range;
use core::slice::Iter;

use super::floats::Floats;
use super::paint::{Decorates, Paint, PaintWalk};
use super::ruby::Annotations;
use super::{Annotation, Atomic, BoxFragment, FloatPlacement, Layout, TextRun};
use crate::data::Id;
use crate::stages::LineStages;
use crate::stages::analysis::BidiLevel;
use crate::stages::content::NodeKey;
use crate::stages::fragments::{
    ClusterWalk, FragmentItem, FragmentItemFlags, FragmentItemKind, GlyphWalk,
};
use crate::stages::lines::{self, InlineExtents, LineFlags, LineId};
use crate::style::FirstLineVariant;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// Every line of a layout, in block order: what [`Layout::lines`] walks.
#[derive(Clone)]
pub(super) struct Lines<'a> {
    layout: &'a Layout,
    /// The lines not yet returned, from either end.
    ids: Range<LineId>,
}

impl<'a> Lines<'a> {
    /// Creates the walk of every line from `layout`'s last break.
    #[inline]
    pub(super) fn new(layout: &'a Layout) -> Self {
        Self {
            layout,
            ids: LineId::new(0)..layout.line_records().lines.next_id(),
        }
    }
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    #[inline]
    fn next(&mut self) -> Option<Line<'a>> {
        let id = self.ids.start;
        if id >= self.ids.end {
            return None;
        }
        self.ids.start = LineId::new(id.get() + 1);
        Line::new(self.layout, id)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.ids.end.get().saturating_sub(self.ids.start.get());
        (left, Some(left))
    }
}

impl ExactSizeIterator for Lines<'_> {}

impl<'a> DoubleEndedIterator for Lines<'a> {
    fn next_back(&mut self) -> Option<Line<'a>> {
        if self.ids.start >= self.ids.end {
            return None;
        }
        let id = LineId::new(self.ids.end.get() - 1);
        self.ids.end = id;
        Line::new(self.layout, id)
    }
}

/// A line and its positioned items.
#[derive(Copy, Clone)]
pub struct Line<'a> {
    layout: &'a Layout,
    id: LineId,
    record: &'a lines::LineRecord,
}

impl<'a> Line<'a> {
    /// Returns the zero-based line index.
    pub fn index(&self) -> usize {
        self.id.get()
    }

    /// Returns the byte range in [`Layout::text`](crate::Layout::text).
    ///
    /// Includes hanging whitespace and the final `\n` for a forced break.
    pub fn text_range(&self) -> Range<usize> {
        let clusters = self.record.clusters();
        let ends = &self.layout.analysis().clusters;
        ends.start(clusters.start).get()..ends.start(clusters.end).get()
    }

    /// Returns the line-box position and extents.
    pub fn metrics(&self) -> LineMetrics {
        let record = self.record;
        let extent = record.extent.zero_if_none();
        let (ascent, descent) = (extent.ascent(), extent.descent());
        let head = self.head();
        // The line box's top after `text-box-trim`, which its item holds.
        let top = head.map_or(LayoutUnit::ZERO, |head| head.block);
        let left = head.map_or_else(
            || InlineLayoutUnit::from_layout(record.band.left),
            |head| head.inline,
        );
        LineMetrics {
            left: left.to_px(),
            top: top.to_px(),
            baseline: (top + ascent).to_px(),
            ascent: ascent.to_px(),
            descent: descent.to_px(),
            width: record.width.to_px(),
            hang: record.hang.space.to_px(),
            band: record.band.to_extents(),
        }
    }

    /// Returns visible items in visual order, from left to right.
    ///
    /// Box fragments precede their descendants. Omits the tail hidden by an
    /// ellipsis.
    #[inline]
    pub fn items(&self) -> impl Iterator<Item = Item<'a>> + Clone + use<'a> {
        LineItems::new(self, false)
    }

    /// Returns paint operations in Chrome paint order.
    ///
    /// Starts with the background, then yields the items in visual order,
    /// each box before what it holds. Each text run comes with its underlines
    /// and overlines before it, and its emphasis marks and lines through
    /// after it. Ruby annotations come last. See [`paint`](crate::paint) for
    /// the full order.
    ///
    /// `decorates` specifies decoration lines for the block and each box
    /// that sets [`ComputedStyle::decorates`](crate::ComputedStyle::decorates).
    /// Does not allocate.
    pub fn paints<F: Fn(NodeKey) -> Decorates>(
        &self,
        decorates: F,
    ) -> impl Iterator<Item = Paint<'a>> + use<'a, F> {
        PaintWalk::new(self, decorates)
    }

    /// Returns ruby annotations in logical column order.
    ///
    /// Within each column, annotations are ordered from the base outward,
    /// over before under. Annotation text is excluded from [`items`](Self::items).
    /// Does not allocate.
    pub fn annotations(&self) -> impl Iterator<Item = Annotation<'a>> + Clone + use<'a> {
        Annotations::new(*self)
    }

    /// Returns floats anchored on this line in placement order.
    ///
    /// Includes floats placed at the line top before fitting, then beside
    /// the line, then below it if they did not fit. Anchors occupy no line
    /// space and are excluded from [`items`](Self::items).
    pub fn floats(&self) -> impl Iterator<Item = FloatPlacement> + Clone + use<'a> {
        Floats::new(self.layout, self.layout.line_records().line_floats(self.id))
    }

    /// Returns all items, including the tail hidden by an ellipsis.
    ///
    /// Uses the order of [`items`](Self::items). Hidden text remains laid
    /// out for caret positioning and hit testing.
    #[inline]
    pub fn all_items(&self) -> impl Iterator<Item = Item<'a>> + Clone + use<'a> {
        LineItems::new(self, true)
    }

    /// Returns the zero-based paragraph index.
    ///
    /// Each forced break starts a paragraph.
    pub fn paragraph(&self) -> usize {
        self.record.paragraph.get()
    }

    /// Returns the paragraph base bidi level: even for LTR, odd for RTL.
    pub fn level(&self) -> BidiLevel {
        self.record.level(&self.layout.stages().analysis.paragraphs)
    }

    /// Returns `true` if the line ends with a generated hyphen, at a soft
    /// hyphen or a hyphenation opportunity.
    ///
    /// The generated hyphen follows the text in the item sequence.
    pub fn is_hyphenated(&self) -> bool {
        self.record.flags.contains(LineFlags::HYPHENATED)
    }

    /// Returns `true` if the line is truncated with an ellipsis.
    ///
    /// Applies to overflowing `text-overflow: ellipsis` lines and the last
    /// retained `line-clamp` line when more text follows. The ellipsis is
    /// the first generated item for RTL lines and the last for LTR lines.
    /// Hidden text is excluded from [`items`](Self::items).
    pub fn has_ellipsis(&self) -> bool {
        self.record.flags.contains(LineFlags::ELLIPSIS)
    }
}

// What the other views read of a line.
impl<'a> Line<'a> {
    /// Returns line `id` of `layout`, or `None` past the last.
    #[inline]
    pub(crate) fn new(layout: &'a Layout, id: LineId) -> Option<Self> {
        let record = layout.line_records().lines.get(id)?;
        Some(Self { layout, id, record })
    }

    /// Returns the line's layout, which the other views read.
    #[inline]
    pub(super) fn layout(&self) -> &'a Layout {
        self.layout
    }

    /// Returns the line's id.
    #[inline]
    pub(super) fn id(&self) -> LineId {
        self.id
    }

    /// Returns the line's record from line breaking.
    #[inline]
    pub(crate) fn record(&self) -> &'a lines::LineRecord {
        self.record
    }

    /// Returns the scale `text-fit` sets the line's text in, or 1.
    #[inline]
    pub(crate) fn scale(&self) -> f32 {
        self.layout.line_records().scale(self.id)
    }

    /// Returns the line's `Line` item, which places the line box.
    ///
    /// Returns `None` after [`Layout::size_lines`], which places no items.
    pub(super) fn head(&self) -> Option<&'a FragmentItem> {
        let fragments = self.layout.fragments();
        let head = *fragments.line_heads.get(self.id)?;
        fragments.items.get(head)
    }

    /// Returns which styles the line is set in: the first line's or the
    /// ordinary ones.
    ///
    /// Every prepared table read for this line is read in this variant.
    #[inline]
    pub(super) fn variant(&self) -> FirstLineVariant {
        self.record.variant()
    }

    /// Returns the prepared stages in the line's variant.
    #[inline]
    pub(crate) fn stages(&self) -> LineStages<'a> {
        self.layout.stages().variant(self.variant())
    }

    /// Returns the walk of text item `item`'s glyphs, placed exactly.
    #[inline]
    pub(super) fn glyph_walk(&self, item: &FragmentItem) -> GlyphWalk<'a> {
        GlyphWalk::new(
            &self.layout.read_input(),
            &self.stages(),
            self.id,
            self.record,
            item,
        )
    }

    /// Returns the walk of text item `item`'s clusters, placed exactly.
    #[inline]
    pub(super) fn cluster_walk(&self, item: &FragmentItem) -> ClusterWalk<'a> {
        ClusterWalk::new(
            &self.layout.read_input(),
            &self.stages(),
            self.id,
            self.record,
            item,
        )
    }
}

impl fmt::Debug for Line<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Line")
            .field("text_range", &self.text_range())
            .field("paragraph", &self.paragraph())
            .field("level", &self.level().get())
            .field("metrics", &self.metrics())
            .finish()
    }
}

/// Line-box position and extents, in pixels.
///
/// `left` and `top` use the coordinates of the [`Area`](crate::Area)
/// passed to line breaking. Item positions are relative to this origin.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
#[non_exhaustive]
pub struct LineMetrics {
    /// The aligned line-box left position along the line.
    ///
    /// Item positions are relative to this value. Hanging content in an
    /// RTL line may extend to its left.
    pub left: f32,
    /// The line-box top position across the block, after `text-box-trim`.
    pub top: f32,
    /// The baseline position across the block: `top + ascent`.
    pub baseline: f32,
    /// The line-box extent above the baseline.
    pub ascent: f32,
    /// The line-box extent below the baseline.
    pub descent: f32,
    /// The fitted content width, on the 1/64-pixel layout grid.
    ///
    /// Excludes hanging content. Includes reshaped widths at unsafe line
    /// edges.
    pub width: f32,
    /// The width of hanging whitespace.
    pub hang: f32,
    /// The available inline extents, narrowed by floats.
    ///
    /// Content wider than this band overflows because no fitting break was
    /// available.
    pub band: InlineExtents,
}

impl LineMetrics {
    /// Returns the line-box height.
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }
}

/// Extents across a line, relative to the line-over edge.
///
/// Both positions increase toward line-under; `over` is the smaller value.
/// The writing mode maps these directions to the page. Line-over is right
/// for vertical lines and left for `sideways-lr`.
///
/// [`BlockExtents`](crate::BlockExtents) instead measures from block-start.
/// The two coordinate systems differ in `vertical-lr`, where lines stack
/// from the left but line-over is on the right.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct CrossExtents {
    /// The line-over end, from the line box's over edge.
    pub over: f32,
    /// The line-under end, from the line box's over edge.
    pub under: f32,
}

impl CrossExtents {
    /// Returns the extent size, or zero if the endpoints are reversed.
    pub fn size(&self) -> f32 {
        let size = self.under - self.over;
        if size > 0.0 { size } else { 0.0 }
    }
}

/// A positioned item on a line.
#[derive(Copy, Clone, Debug)]
pub enum Item<'a> {
    /// Shaped text of one node, one used font and one bidi level.
    ///
    /// A tab, and white space hanging past the line's end, are runs of their
    /// own.
    Text(TextRun<'a>),
    /// Text the layout generates: a hyphen or an ellipsis.
    Generated(TextRun<'a>),
    /// An atomic inline: its margin box.
    Atomic(Atomic<'a>),
    /// The border box of a retained inline-box fragment.
    ///
    /// Precedes the items of the fragment. Culled boxes are available through
    /// [`Layout::box_fragments`](crate::Layout::box_fragments).
    Box(BoxFragment<'a>),
}

/// The items of one line, in visual order and pre-order: what
/// [`Line::items`] and [`Line::all_items`] walk.
#[derive(Clone)]
pub(crate) struct LineItems<'a> {
    line: Line<'a>,
    items: Iter<'a, FragmentItem>,
    /// Whether the tail an ellipsis hides is included.
    hidden: bool,
}

impl<'a> LineItems<'a> {
    /// Creates the walk of `line`'s items, including the tail an ellipsis
    /// hides where `hidden`.
    #[inline]
    pub(crate) fn new(line: &Line<'a>, hidden: bool) -> Self {
        Self {
            line: *line,
            items: line.layout.fragments().line_items(line.id).iter(),
            hidden,
        }
    }
}

impl<'a> Iterator for LineItems<'a> {
    type Item = Item<'a>;

    #[inline]
    fn next(&mut self) -> Option<Item<'a>> {
        for item in self.items.by_ref() {
            work::step();
            if !self.hidden && item.flags.contains(FragmentItemFlags::HIDDEN) {
                continue;
            }
            return Some(match item.kind() {
                FragmentItemKind::Text => Item::Text(TextRun::new(self.line, item)),
                FragmentItemKind::Generated => Item::Generated(TextRun::new(self.line, item)),
                FragmentItemKind::Atomic => Item::Atomic(Atomic::new(self.line, item)),
                FragmentItemKind::Box => Item::Box(BoxFragment::from_item(self.line, item)),
                // A line's head is not among its items, and its ruby
                // annotations are after them (`Line::annotations`).
                FragmentItemKind::Line | FragmentItemKind::AnnotationLine => continue,
            });
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.items.len()))
    }
}
