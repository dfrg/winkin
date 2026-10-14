//! Boxes read back: atomic inlines and the parts of inline boxes.
//!
//! A box that keeps a fragment is an item on its lines. A culled box is found
//! by query from its descendants, as Blink answers a culled inline's
//! `getClientRects`. On each line it reaches, it yields one part per group of
//! its descendants' items that no other leaf splits: the union of their
//! rectangles. Every span is fully measured, so the query needs no relayout.
//! It walks only the lines the box's clusters reach, keeps a few words of
//! state and allocates nothing.

use core::fmt;
use core::ops::Range;
use core::slice;

use crate::data::{Id, Table};
use crate::layout::Layout;
use crate::stages::analysis::{BidiLevel, ClusterId};
use crate::stages::content::{Content, ItemFlags, KeyedBox, NodeId, NodeKey, Nodes};
use crate::stages::fonts::LineBaseline;
use crate::stages::fragments::{
    FragmentItem, FragmentItemFlags, FragmentItemId, FragmentItemKind, Fragments,
};
use crate::stages::lines::{InlineExtents, LineId};
use crate::stages::measure::{Extent, KeptBoxId, KeptBoxes};
use crate::unit::{InlineLayoutUnit, LayoutUnit};

use super::{CrossExtents, Line, TextRun};
use crate::data::IdRange;
use crate::style::FirstLineVariant;
use crate::work;

/// An atomic inline and its margin box on a line.
#[derive(Copy, Clone)]
pub struct Atomic<'a> {
    line: Line<'a>,
    item: &'a FragmentItem,
}

impl<'a> Atomic<'a> {
    /// Returns atomic inline item `item` of `line`.
    #[inline]
    pub(super) fn new(line: Line<'a>, item: &'a FragmentItem) -> Self {
        Self { line, item }
    }

    /// Returns the node key.
    pub fn key(&self) -> NodeKey {
        self.line.layout().content().nodes.key(self.item.node)
    }

    /// Returns the bidi level.
    pub fn level(&self) -> BidiLevel {
        self.item.level
    }

    /// Returns the byte range of the U+FFFC placeholder in
    /// [`Layout::text`](crate::Layout::text).
    ///
    /// The range endpoints are the caret positions before and after the inline.
    pub fn text_range(&self) -> Range<usize> {
        let range = self
            .item
            .text_range(&self.line.layout().analysis().clusters);
        range.start.get()..range.end.get()
    }

    /// Returns the margin-box width along the line, in pixels.
    pub fn advance(&self) -> f32 {
        self.item.advance().to_px()
    }

    /// Returns the margin-box extents along the line, relative to the line
    /// box's left edge.
    pub fn inline(&self) -> InlineExtents {
        self.item.inline_extents()
    }

    /// Returns the baseline's offset from the line box's top.
    ///
    /// Uses the margin-box bottom if the inline has no baseline.
    pub fn baseline(&self) -> f32 {
        self.item.block.to_px()
    }

    /// Returns the margin-box extents across the line, relative to the
    /// line-over edge.
    pub fn block(&self) -> CrossExtents {
        let (over, under) = self.block_extent();
        CrossExtents {
            over: over.to_px(),
            under: under.to_px(),
        }
    }

    /// Returns the margin-box extents across the line, on the 26.6 grid.
    fn block_extent(&self) -> (LayoutUnit, LayoutUnit) {
        let layout = self.line.layout();
        let content = layout.content();
        let atomic = content.nodes.items(self.item.node).start;
        // An annotation's items take no room on the base's line, so its
        // atomic inline's extent comes from its row.
        let annotated = content
            .items
            .get(atomic)
            .is_some_and(|item| item.flags.contains(ItemFlags::ANNOTATION));
        let extent = if annotated {
            content.item_atomic(atomic).map_or(Extent::NONE, |row| {
                Extent::from_atomic(
                    row,
                    content.block.writing_mode,
                    LineBaseline::from_content(content),
                )
            })
        } else {
            self.line.stages().measured().extents.get(atomic)
        };
        extent.around(self.item.block)
    }
}

impl fmt::Debug for Atomic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Atomic")
            .field("key", &self.key())
            .field("inline", &self.inline())
            .field("block", &self.block())
            .field("baseline", &self.baseline())
            .finish()
    }
}

/// A border-box fragment of an inline box on one line.
///
/// Retained fragments appear as [`Item::Box`](super::Item::Box) before their
/// descendants. [`Layout::box_fragments`](crate::Layout::box_fragments) also
/// returns fragments derived from descendants of culled boxes.
#[derive(Copy, Clone)]
pub struct BoxFragment<'a> {
    content: &'a Content,
    /// The kept boxes in the line's variant, which hold its extent.
    kept: &'a KeptBoxes,
    /// Its row there, or `None` for a culled box.
    kept_box: Option<KeptBoxId>,
    /// Its extent with the font's ascent and descent scaled, on a line
    /// `text-fit` scales.
    fitted: Option<Extent>,
    node: NodeId,
    line: LineId,
    level: BidiLevel,
    /// The border box's left along the line, exactly, from the line box's
    /// left.
    left: InlineLayoutUnit,
    /// The border box's right: its left plus its width on the 26.6 grid,
    /// rounded up as Chrome snaps it.
    right: InlineLayoutUnit,
    baseline: LayoutUnit,
    /// A culled box's top and bottom from the line box's top, found by the
    /// query; `None` for a kept box, whose extent is looked up when asked.
    across: Option<(LayoutUnit, LayoutUnit)>,
    flags: FragmentItemFlags,
    descendants: usize,
}

impl<'a> BoxFragment<'a> {
    /// Returns box item `item` of `line`, for a box that keeps its fragment.
    #[inline]
    pub(super) fn from_item(line: Line<'a>, item: &'a FragmentItem) -> Self {
        let left = item.inline;
        let layout = line.layout();
        let kept = line.stages().measured().kept_boxes();
        let scale = line.scale();
        let fitted = if scale == 1.0 {
            None
        } else {
            // The font's ascent and descent scale; padding and borders don't.
            let content = layout.content();
            let text = content.nodes.text_facts(item.node, line.variant());
            let font = layout
                .fonts()
                .primary_font(content.facts.text_request(text));
            let extent = item.kept_box().and_then(|at| kept.extent(at));
            extent.zip(font).map(|(extent, font)| {
                let (ascent, descent) = (font.metrics.ascent, font.metrics.descent);
                let more = |length: LayoutUnit| length.times_truncated(scale) - length;
                Extent::new(
                    extent.ascent() + more(ascent),
                    extent.descent() + more(descent),
                )
            })
        };
        Self {
            content: layout.content(),
            kept,
            kept_box: item.kept_box(),
            fitted,
            node: item.node,
            line: line.id(),
            level: item.level,
            left,
            right: left + InlineLayoutUnit::from_layout(item.size),
            baseline: item.block,
            across: None,
            flags: item.flags,
            descendants: item.descendants(),
        }
    }

    /// Returns the box node key.
    pub fn key(&self) -> NodeKey {
        self.content.nodes.key(self.node)
    }

    /// Returns the zero-based line index.
    pub fn line(&self) -> usize {
        self.line.get()
    }

    /// Returns the bidi level.
    pub fn level(&self) -> BidiLevel {
        self.level
    }

    /// Returns the border-box extents along the line, relative to the line
    /// box's left edge.
    pub fn inline(&self) -> InlineExtents {
        InlineExtents::from_exact(self.left, self.right)
    }

    /// Returns the border-box extents across the line, relative to the
    /// line-over edge.
    ///
    /// Retained fragments use font ascent and descent around the baseline,
    /// plus block-axis padding and borders. Culled fragments use the union
    /// of descendant extents.
    pub fn block(&self) -> CrossExtents {
        let (over, under) = self.block_extent();
        CrossExtents {
            over: over.to_px(),
            under: under.to_px(),
        }
    }

    /// Returns the border-box extents across the line, on the 26.6 grid.
    ///
    /// A kept box's extent is its row's.
    fn block_extent(&self) -> (LayoutUnit, LayoutUnit) {
        self.across.unwrap_or_else(|| {
            let extent =
                (self.fitted).or_else(|| self.kept_box.and_then(|at| self.kept.extent(at)));
            extent.unwrap_or(Extent::NONE).around(self.baseline)
        })
    }

    /// Returns the baseline's offset from the line box's top.
    pub fn baseline(&self) -> f32 {
        self.baseline.to_px()
    }

    /// Returns `true` if the box continues beyond the left side.
    ///
    /// The continuation may cross a line break or bidi reordering boundary.
    /// No border is drawn on this side.
    pub fn is_open_left(&self) -> bool {
        self.flags.contains(FragmentItemFlags::OPEN_LEFT)
    }

    /// Returns `true` if the box continues beyond the right side.
    pub fn is_open_right(&self) -> bool {
        self.flags.contains(FragmentItemFlags::OPEN_RIGHT)
    }

    /// Returns `true` if this fragment is derived from a culled box's
    /// descendants.
    pub fn is_culled(&self) -> bool {
        self.across.is_some()
    }

    /// Returns margin, border and padding widths on each side.
    ///
    /// Open sides occupy no space. Values use the layout grid and match
    /// the edge widths charged during line breaking.
    pub fn edges(&self) -> InlineEdges {
        let (nodes, facts) = (&self.content.nodes, &self.content.facts);
        let (left, right) = facts
            .box_facts(nodes.box_facts(self.node, FirstLineVariant::Standard))
            .room;
        let side = |open: bool, room: LayoutUnit| {
            if open { 0.0 } else { room.to_px() }
        };
        InlineEdges {
            left: side(self.is_open_left(), left),
            right: side(self.is_open_right(), right),
        }
    }

    /// Returns the number of following line items in this fragment.
    ///
    /// Includes the tail hidden by an ellipsis; see
    /// [`Line::all_items`](super::Line::all_items).
    /// Returns zero for culled boxes and annotation boxes. Annotation text
    /// is accessed through [`Annotation::runs`](super::Annotation::runs).
    pub fn descendants(&self) -> usize {
        self.descendants
    }
}

/// Margin, border and padding widths on the line-left and line-right sides.
///
/// Returned by [`BoxFragment::edges`]. Values use the layout grid.
/// Open sides have zero width, including continuations across line breaks
/// or bidi reordering. These are widths; [`InlineExtents`] describes
/// positions.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct InlineEdges {
    /// The line-left edge width.
    pub left: f32,
    /// The line-right edge width.
    pub right: f32,
}

impl fmt::Debug for BoxFragment<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BoxFragment")
            .field("key", &self.key())
            .field("line", &self.line())
            .field("inline", &self.inline())
            .field("block", &self.block())
            .field("open_left", &self.is_open_left())
            .field("open_right", &self.is_open_right())
            .field("culled", &self.is_culled())
            .finish()
    }
}

/// Every part of every inline box of one key, line by line: what
/// [`Layout::box_fragments`] walks.
#[derive(Clone)]
pub(super) struct BoxFragments<'a> {
    layout: &'a Layout,
    /// The boxes of the key not yet answered, in tree order.
    boxes: slice::Iter<'a, KeyedBox>,
    /// The box being answered, and the query's position in it.
    at: Option<Answering>,
}

impl<'a> BoxFragments<'a> {
    /// Creates the walk of every part of every inline box `key` names in
    /// `layout`.
    pub(super) fn new(layout: &'a Layout, key: NodeKey) -> Self {
        Self {
            layout,
            boxes: layout.content().key_boxes(key).iter(),
            at: None,
        }
    }
}

/// A query's position in one box.
#[derive(Copy, Clone, Debug)]
struct Answering {
    node: NodeId,
    /// Whether it keeps a fragment on lines in the ordinary styles.
    kept_in_text: bool,
    /// Whether it keeps a fragment on the block's first line, where that has
    /// styles of its own. A `::first-line` style may keep a box the other
    /// lines cull.
    kept_in_first_line: bool,
    /// Whether it is inside a ruby annotation, whose items follow the line's
    /// own.
    annotated: bool,
    /// The first cluster at or after its opening item.
    first: ClusterId,
    /// The first cluster at or after its closing item.
    last: ClusterId,
    /// The line being read.
    line: LineId,
    /// The next item to read on that line.
    item: FragmentItemId,
    /// Whether a part of it has been returned on this line.
    begun: bool,
    /// A culled box's baseline on the current line, found with its first part
    /// there. Every part on the line shares it.
    baseline: Option<LayoutUnit>,
}

impl Answering {
    /// Returns whether the box keeps a fragment on a line set in `variant`'s
    /// styles.
    fn keeps(&self, variant: FirstLineVariant) -> bool {
        match variant {
            FirstLineVariant::Standard => self.kept_in_text,
            FirstLineVariant::FirstLine => self.kept_in_first_line,
        }
    }

    /// Returns the items of `line` the box is among: the annotations' where
    /// it is inside one, the line's own otherwise.
    fn items(&self, fragments: &Fragments, line: LineId) -> Range<FragmentItemId> {
        if self.annotated {
            fragments.annotation_ids(line)
        } else {
            fragments.line_item_ids(line)
        }
    }
}

impl<'a> BoxFragments<'a> {
    /// Returns where answering box `node` begins: the first line whose
    /// clusters reach its first cluster, at that line's first item.
    fn begin(&self, node: NodeId) -> Answering {
        let layout = self.layout;
        let items = layout.content().nodes.items(node);
        let Range {
            start: first,
            end: last,
        } = layout.analysis().item_clusters.span(items.clone());
        let measured = layout.measured();
        let annotated = layout
            .content()
            .items
            .get(items.start)
            .is_some_and(|open| open.flags.contains(ItemFlags::ANNOTATION));
        // An annotation of a column split across lines is set on the lines
        // of the column's pieces, which hold the base, not the annotation's
        // clusters.
        let lines = layout.line_records();
        let columns = measured.text(FirstLineVariant::Standard).ruby_columns();
        let line = lines
            .ruby
            .as_ref()
            .filter(|_| annotated)
            .and_then(|ruby| ruby.annotation_line(lines, columns, first, false))
            .unwrap_or_else(|| lines.first_reaching(first));
        let kept = measured
            .text(FirstLineVariant::Standard)
            .kept_boxes()
            .get(node)
            .is_some();
        let mut at = Answering {
            node,
            kept_in_text: kept,
            kept_in_first_line: match measured.first_line() {
                Some(first) => first.kept_boxes().get(node).is_some(),
                None => kept,
            },
            annotated,
            first,
            last,
            line,
            item: FragmentItemId::new(0),
            begun: false,
            baseline: None,
        };
        at.item = at.items(layout.fragments(), line).start;
        at
    }

    /// Returns the next part of the box `at` answers, moving `at` past it, or
    /// `None` once its lines are done.
    fn answer(layout: &'a Layout, at: &mut Answering) -> Option<BoxFragment<'a>> {
        let fragments = layout.fragments();
        while let Some(line) = Line::new(layout, at.line) {
            work::step();
            // A line starting past its last cluster holds none of it: its
            // closing edges went with the line before.
            if line.record().clusters().start > at.last {
                return None;
            }
            let items = at.items(fragments, line.id());
            // Kept or culled by the line's own styles: the block's first line
            // uses its first-line styles, where it has them.
            let found = if at.keeps(line.variant()) {
                next_kept(line, items, at)
            } else {
                next_culled(line, items, at)
            };
            if found.is_some() {
                at.begun = true;
                return found;
            }
            at.line = LineId::new(at.line.get() + 1);
            at.item = at.items(fragments, at.line).start;
            at.begun = false;
            at.baseline = None;
        }
        None
    }
}

/// Returns the next of `line`'s items, `items`, that is the kept box `at`
/// answers.
fn next_kept<'a>(
    line: Line<'a>,
    items: Range<FragmentItemId>,
    at: &mut Answering,
) -> Option<BoxFragment<'a>> {
    let table = &line.layout().fragments().items;
    while at.item < items.end
        && let Some(item) = table.get(at.item)
    {
        work::step();
        at.item = FragmentItemId::new(at.item.get() + 1);
        let kind = item.kind();
        if matches!(
            kind,
            FragmentItemKind::Box | FragmentItemKind::AnnotationLine
        ) && item.node == at.node
        {
            return Some(BoxFragment::from_item(line, item));
        }
    }
    None
}

/// Returns the next part of the culled box `at` answers among `line`'s
/// items, `items`: the union of a group of its descendants' items that no
/// other leaf splits.
fn next_culled<'a>(
    line: Line<'a>,
    items: Range<FragmentItemId>,
    at: &mut Answering,
) -> Option<BoxFragment<'a>> {
    let layout = line.layout();
    let table = &layout.fragments().items;
    let part = culled_part(line, table, items.end, at)?;
    let rightmost = pass_to_next_part(line, table, items.end, at);
    // A box with a shift of its own keeps its fragment, so a culled one
    // stands on its parent's baseline: the innermost kept box's around it,
    // or the line's.
    let baseline = *at.baseline.get_or_insert_with(|| {
        let nodes = &layout.content().nodes;
        (layout.fragments())
            .culled_baseline(nodes, line.id(), at.node, at.annotated)
            .unwrap_or_else(|| line.record().ascent())
    });
    Some(BoxFragment {
        content: layout.content(),
        kept: line.stages().measured().kept_boxes(),
        kept_box: None,
        fitted: None,
        node: at.node,
        line: line.id(),
        level: part.level,
        left: part.left,
        right: part.right,
        baseline,
        across: Some((part.top, part.bottom)),
        flags: culled_sides(line, at, rightmost),
        descendants: 0,
    })
}

/// Unites the items of the culled box `at` answers from where `at` stands,
/// up to `end` or another box's leaf, moving `at` past them.
///
/// Returns `None` where the line holds no more of the box's items.
#[inline]
fn culled_part(
    line: Line<'_>,
    table: &Table<FragmentItemId, FragmentItem>,
    end: FragmentItemId,
    at: &mut Answering,
) -> Option<CulledPart> {
    let nodes = &line.layout().content().nodes;
    let mut part: Option<CulledPart> = None;
    while at.item < end
        && let Some(item) = table.get(at.item)
    {
        work::step();
        if is_counted(nodes, at.node, item, true) {
            let (left, right, top, bottom) = rectangle(line, item);
            part = Some(match part {
                None => CulledPart {
                    left,
                    right,
                    top,
                    bottom,
                    level: item.level,
                },
                Some(s) => CulledPart {
                    left: s.left.min(left),
                    right: s.right.max(right),
                    top: s.top.min(top),
                    bottom: s.bottom.max(bottom),
                    level: s.level.min(item.level),
                },
            });
        } else if item.is_leaf() && part.is_some() {
            // Another box's leaf splits the part.
            break;
        }
        at.item = FragmentItemId::new(at.item.get() + 1);
    }
    part
}

/// Moves `at` to where the box's next part on the line starts, and returns
/// whether the part just found is its rightmost there.
///
/// The part is the rightmost where no leaf of the box follows it on the
/// line. The scan runs from the part's end to the box's next leaf, and the
/// next part starts at the first of the box's items on the way. So each of
/// the line's items is read at most twice, however often reordering splits
/// the box. A box with one part reads nothing more.
#[inline]
fn pass_to_next_part(
    line: Line<'_>,
    table: &Table<FragmentItemId, FragmentItem>,
    end: FragmentItemId,
    at: &mut Answering,
) -> bool {
    let nodes = &line.layout().content().nodes;
    let mut rightmost = true;
    let mut resume = end;
    let rest = table.get_slice(at.item..end).unwrap_or_default();
    for (index, item) in (at.item..end).ids().zip(rest) {
        work::step();
        if resume == end && is_counted(nodes, at.node, item, true) {
            resume = index;
        }
        if is_counted(nodes, at.node, item, false) {
            rightmost = false;
            break;
        }
    }
    at.item = resume;
    rightmost
}

/// Returns the open sides of the culled part `at` just found on `line`,
/// which is the box's rightmost there where `rightmost`.
///
/// Its own edges bound it where its line holds them, as a kept box's do.
/// Its start edge is on the line holding its first cluster, and its end
/// edge on the line holding its last. The start is on the left when it
/// reads left to right, and on the right otherwise. The left edge goes on
/// its leftmost part of the line, the first found. The right edge goes on
/// its rightmost, which no other follows. A part split from another by
/// reordering is open where the split is.
#[inline]
fn culled_sides(line: Line<'_>, at: &Answering, rightmost: bool) -> FragmentItemFlags {
    let content = line.layout().content();
    let clusters = line.record().clusters();
    let starts = clusters.start <= at.first;
    let ends = at.last <= clusters.end;
    let own = content.nodes.box_facts(at.node, FirstLineVariant::Standard);
    let direction = content.facts.box_facts(own).direction();
    let (has_left, has_right) = direction.line_order(starts, ends);
    let leftmost = !at.begun;
    let mut flags = FragmentItemFlags::NONE;
    if !(leftmost && has_left) {
        flags.insert(FragmentItemFlags::OPEN_LEFT);
    }
    if !(rightmost && has_right) {
        flags.insert(FragmentItemFlags::OPEN_RIGHT);
    }
    flags
}

/// Returns whether `item` is one of `node`'s painted items that its culled
/// parts are made of: a leaf, or, where `boxes`, also a kept box inside it.
///
/// The part holding the box's last leaf on a line is its rightmost there.
fn is_counted(nodes: &Nodes, node: NodeId, item: &FragmentItem, boxes: bool) -> bool {
    let counted = item.is_painted_leaf()
        || (boxes
            && item.kind() == FragmentItemKind::Box
            && !item.flags.contains(FragmentItemFlags::HIDDEN));
    counted && nodes.contains(node, item.node)
}

/// A culled box's part, as far as found.
#[derive(Copy, Clone, Debug)]
struct CulledPart {
    left: InlineLayoutUnit,
    right: InlineLayoutUnit,
    top: LayoutUnit,
    bottom: LayoutUnit,
    level: BidiLevel,
}

/// Returns `item`'s rectangle on `line`: its left and right along the line,
/// the right on the 26.6 grid as a box's is, and its top and bottom across
/// it.
///
/// Across the line, a text run spans its font's ascent and descent, an
/// atomic inline its margin box, and a kept box its border box.
fn rectangle(
    line: Line<'_>,
    item: &FragmentItem,
) -> (InlineLayoutUnit, InlineLayoutUnit, LayoutUnit, LayoutUnit) {
    let left = item.inline;
    let right = left + InlineLayoutUnit::from_layout(item.size);
    let (top, bottom) = match item.kind() {
        FragmentItemKind::Atomic => Atomic::new(line, item).block_extent(),
        FragmentItemKind::Box => BoxFragment::from_item(line, item).block_extent(),
        _ => TextRun::new(line, item).block_extent(),
    };
    (left, right, top, bottom)
}

impl<'a> Iterator for BoxFragments<'a> {
    type Item = BoxFragment<'a>;

    fn next(&mut self) -> Option<BoxFragment<'a>> {
        let nodes = &self.layout.content().nodes;
        loop {
            if self.at.is_none() {
                // The next box of the key, in tree order. An anonymous ruby
                // container takes its annotation's key, and answers nothing.
                let node = loop {
                    work::step();
                    let node = self.boxes.next()?.node;
                    if !nodes.is_anonymous_ruby(node) {
                        break node;
                    }
                };
                self.at = Some(self.begin(node));
            }
            let layout = self.layout;
            let found = self.at.as_mut().and_then(|at| Self::answer(layout, at));
            if found.is_some() {
                return found;
            }
            self.at = None;
        }
    }
}
