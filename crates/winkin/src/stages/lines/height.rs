//! How tall a line is: its line box's extent, and the baseline shifts only
//! the line can settle.
//!
//! The extent is the union, from the strut, of what is on the line:
//! - every item's extent, where its box's baseline puts it;
//! - the strut of every box open across the line's start, as Blink's
//!   `OnBeginPlaceItems` keeps an open box's `text_metrics` for the next
//!   line.
//!
//! Where nothing is shifted, every baseline is the line's and the union is
//! flat. The boxes open across the start come from `carried_struts`, which
//! walks up few boxes and keeps a record where a line starts inside many.
//! A line then costs its own items however deep the boxes nest.
//!
//! Where something is shifted, the line is walked as Blink's
//! `InlineLayoutStateStack` walks it. Each open box unites what it holds
//! around its own baseline and applies its shift when it closes, as
//! `ApplyBaselineShift` does. Measurement holds the shifts that do not
//! depend on the line. This module settles the rest:
//! - `middle` centres the box's extent on its parent's baseline raised by
//!   half the parent's x-height.
//! - `text-top` and `text-bottom` wait for the parent to close. They set the
//!   box's top on the top of the parent's unleaded text, or its bottom on
//!   the bottom.
//! - `top` and `bottom` wait for the nearest `top` or `bottom` box around
//!   them, or the line. They are settled last, against its aligned subtree:
//!   everything it holds but the `top` and `bottom` boxes and their content.
//!   A taller pinned box grows the subtree's other edge, as Blink's
//!   `MetricsForTopAndBottomAlign` does. The whole subtree moves with its
//!   box.
//!
//! The settled shifts go in `shifts`, a line's together and sorted by node.
//! Line layout adds them to measurement's and places everything once.
//!
//! Two of Blink's ways break CSS 2.1 for `top` and `bottom`, so this module
//! follows CSS: a `top` box inside a raised span lands on the line's top,
//! and a box inside a `top` box does not grow the line it is pinned to.
//!
//! Nothing here allocates once warm. Any nesting depth is walked without
//! recursion. Each box links the boxes waiting for it, so settling never
//! looks at another box's.

use alloc::vec::Vec;
use core::mem;
use core::ops::Range;

use super::annotate::{AnnotationContinuation, Carry, EmWalk};
use super::{Breaker, BreakerParagraph, LineShift, LineShiftId, Lines};
use crate::data::{Id, RunCursor, Table, define_id, heap_bytes, sort_by_key};
use crate::stages::LineStages;
use crate::stages::analysis::ClusterId;
use crate::stages::content::{
    ContentFlags, Item, ItemFlags, ItemId, ItemKind, NodeId, NodeKind, TextFactsId, TextLineHeight,
};
use crate::stages::fonts::FontLineMetrics;
use crate::stages::measure::{self, Extent, normal_extent};
use crate::stages::shape::ShapedRunId;
use crate::style::VerticalAlign;
use crate::unit::LayoutUnit;
use crate::work;

define_id! {
    /// Names a box open on the line being measured, in a [`BoxStack`].
    ///
    /// The line's own box is the first.
    struct OpenBoxId(u32);
}

define_id! {
    /// Names a closed box waiting for a box around it to close, in a
    /// [`BoxStack`].
    struct WaitingId(u32);
}

/// A line walked box by box, where something on it is shifted.
///
/// It holds the boxes open on the line and those closed and waiting for a
/// box around them.
pub(super) struct BoxStack {
    /// The boxes open on the line being measured, the line's own first.
    stack: Table<OpenBoxId, OpenBox>,
    /// Boxes closed on it and waiting for a box around them to close.
    waiting: Table<WaitingId, Waiting>,
    /// The boxes open across the line's start, innermost first, which the
    /// walk opens before the line's items.
    carried: Vec<NodeId>,
    /// The scale `text-fit` sets the line's text in, which its boxes'
    /// shifts take: 1 where it keeps its size.
    scale: f32,
}

impl BoxStack {
    /// Returns an empty stack, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            stack: Table::new(),
            waiting: Table::new(),
            carried: Vec::new(),
            scale: 1.0,
        }
    }
}

heap_bytes! {
    BoxStack { stack, waiting, carried; scale }
}

heap_bytes! {
    CarriedBoxes { record, walked; at, deep }
}

/// The boxes open across a line's start, whose struts an unshifted line
/// holds.
///
/// [`carried_struts`] walks up the parent links where a line starts inside
/// few boxes. Once a line starts inside more than [`SHALLOW`], it reads a
/// record kept from line to line instead.
pub(super) struct CarriedBoxes {
    /// The boxes open before item `at`, outermost first, each with its strut
    /// united with those around it.
    ///
    /// It moves on with the lines, so no line walks the boxes. It stays
    /// empty, with `deep` false, until a line starts inside more than
    /// [`SHALLOW`] boxes.
    record: Vec<Ancestor>,
    at: ItemId,
    deep: bool,
    /// The boxes among an item's ancestors, innermost first, that start the
    /// record.
    walked: Vec<NodeId>,
}

/// The most boxes a line's start may be inside for the breaker to walk up
/// them.
///
/// Up to this depth, walking costs as little as reading a record. Past it,
/// the breaker keeps the record in [`CarriedBoxes`], at a step an item and
/// one a line.
const SHALLOW: usize = 16;

impl CarriedBoxes {
    /// Returns an empty record, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            record: Vec::new(),
            at: ItemId::new(0),
            deep: false,
            walked: Vec::new(),
        }
    }

    /// Forgets the record at the start of a break.
    pub(super) fn begin(&mut self) {
        self.record.clear();
        self.at = ItemId::new(0);
        self.deep = false;
    }

    /// Starts the record at item `holding` from the parent links.
    ///
    /// It takes the item's node where that is a box, as for a `<wbr>` the
    /// builder made, and every box among its ancestors.
    fn start(&mut self, stages: &LineStages<'_>, holding: ItemId, item: &Item) {
        let nodes = &stages.content.nodes;
        self.walked.clear();
        let node = item.node;
        if stages.is_box(node) {
            self.walked.push(node);
        }
        self.walked.extend(stages.boxes_around(node));
        self.record.clear();
        let walked = mem::take(&mut self.walked);
        for &node in walked.iter().rev() {
            self.push_box(stages, node, nodes.items(node).start);
        }
        self.walked = walked;
        self.at = holding;
        self.deep = true;
    }

    /// Pushes box `node`, opened by item `open`, onto the record, its strut
    /// united with those of the boxes around it.
    fn push_box(&mut self, stages: &LineStages<'_>, node: NodeId, open: ItemId) {
        let strut = stages.measured.extents.get(open);
        let struts = self
            .record
            .last()
            .map_or(strut, |outer| outer.struts.unite(strut));
        self.record.push(Ancestor {
            node,
            opened: stages.analysis.item_clusters.start(open),
            struts,
        });
    }

    /// Moves the record on to item `to`, opening and closing boxes as the
    /// items between do.
    ///
    /// Lines break in order, so a break passes each item once. A line
    /// before the record restarts it: [`carried_struts`] uses the parent
    /// links, and this falls back to the first item.
    fn move_to(&mut self, stages: &LineStages<'_>, to: ItemId) {
        if to < self.at {
            self.record.clear();
            self.at = ItemId::new(0);
        }
        while self.at < to {
            work::step();
            let at = self.at;
            self.at = ItemId::new(at.get() + 1);
            let Some(item) = stages.content.items.get(at) else {
                break;
            };
            let node = item.node;
            match item.kind {
                ItemKind::Open if stages.is_box(node) => self.push_box(stages, node, at),
                ItemKind::Close if self.record.last().is_some_and(|open| open.node == node) => {
                    self.record.pop();
                }
                _ => {}
            }
        }
    }
}

/// A box open on the line, with how far its content so far reaches around
/// its baseline, as Blink's `InlineBoxState::metrics`.
#[derive(Copy, Clone, Debug)]
struct OpenBox {
    node: NodeId,
    /// Its text facts, whose primary font places `text-top`, `text-bottom`
    /// and `middle` boxes inside it.
    text: TextFactsId,
    align: VerticalAlign,
    metrics: Extent,
    /// The nearest `top` or `bottom` box, itself or one around it, or else
    /// the line's own box.
    ///
    /// A `top` or `bottom` box inside it waits for this one.
    pinned: OpenBoxId,
    /// The first and last boxes waiting for it, linked in the order they
    /// closed; `None` where nothing waits.
    waiting: Option<(WaitingId, WaitingId)>,
}

/// A box open where a line starts, as the breaker's record of them keeps it.
#[derive(Copy, Clone, Debug)]
struct Ancestor {
    node: NodeId,
    /// Its opening item's first cluster.
    opened: ClusterId,
    /// Its strut united with those of every box around it.
    struts: Extent,
}

/// A closed box whose place waits for a box around it, as Blink's
/// `PendingPositions`.
///
/// `next` links the next box waiting for the same one.
#[derive(Copy, Clone, Debug)]
struct Waiting {
    node: NodeId,
    align: VerticalAlign,
    metrics: Extent,
    next: Option<WaitingId>,
}

/// The item lookups a line's height makes, in the line's variant.
impl LineStages<'_> {
    /// Returns whether something on a line may be shifted off its parent's
    /// baseline.
    ///
    /// Shifts come only from `vertical-align` or `dominant-baseline`, so
    /// plain content answers from its flags without reading the rare tables.
    #[inline]
    pub(super) fn has_shifts(&self) -> bool {
        let flags = self.content.flags;
        flags.contains(ContentFlags::VERTICAL_ALIGN)
            || (flags.contains(ContentFlags::DOMINANT_BASELINE)
                && !self.measured.shifts().is_empty())
    }

    /// Returns whether `node` is an inline box.
    fn is_box(&self, node: NodeId) -> bool {
        self.content
            .nodes
            .kind(node)
            .is_some_and(NodeKind::is_inline_box)
    }

    /// Fills `carried` with the boxes open across `start`, innermost first.
    ///
    /// `item` holds the cluster at `start`. Only ancestors that opened
    /// before `start` count, since those opening there are the line's own
    /// items.
    fn carried(&self, item: &Item, start: ClusterId, carried: &mut Vec<NodeId>) {
        carried.clear();
        let nodes = &self.content.nodes;
        let item_clusters = &self.analysis.item_clusters;
        carried.extend(
            self.boxes_around(item.node)
                .filter(|&node| item_clusters.start(nodes.items(node).start) < start),
        );
    }

    /// Returns the inline boxes around `node`, innermost first, up the
    /// parent links.
    ///
    /// It takes one work step a node and at most as many steps as there are
    /// nodes, whatever the links say.
    fn boxes_around(&self, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        let nodes = &self.content.nodes;
        let mut node = node;
        (0..nodes.len())
            .map_while(move |_| {
                work::step();
                let parent = nodes.parent(node);
                (parent != node).then(|| {
                    node = parent;
                    parent
                })
            })
            .filter(|&node| self.is_box(node))
    }

    /// Returns the item holding the cluster at `start`, with its id.
    ///
    /// It walks from the line's first item `from`, as kept in
    /// [`LineRecord::first_item`](super::LineRecord::first_item), past items at the
    /// start that hold no clusters. No search is needed.
    fn start_item(&self, from: ItemId, start: ClusterId) -> Option<(ItemId, &Item)> {
        let item_clusters = &self.analysis.item_clusters;
        let mut cursor = item_clusters.cursor(from);
        let count = self.content.items.next_id();
        while cursor.end() <= start && cursor.id() < count {
            work::step();
            item_clusters.step(&mut cursor);
        }
        let id = cursor.id();
        Some((id, self.content.items.get(id)?))
    }

    /// Returns the `vertical-align` of box or atomic inline `node`.
    fn align(&self, node: NodeId) -> VerticalAlign {
        let content = self.content;
        let facts = content.nodes.box_facts(node, self.variant);
        content.facts.box_facts(facts).align
    }

    /// Calls `each` for every item on the line from `start` to `end`, with
    /// its clusters, and returns the first item that may be on the next line.
    ///
    /// An item holding clusters is on every line holding one of them. An
    /// item holding none is on the line its boundary starts, except at a
    /// break, where the items up to the last close end this line
    /// ([`items_after_break`](measure::items_after_break)). At the text's end
    /// every item is the last line's.
    #[inline]
    pub(super) fn items_on_line(
        &self,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
        mut each: impl FnMut(ItemId, Range<ClusterId>),
    ) -> ItemId {
        let item_clusters = &self.analysis.item_clusters;
        let count = self.content.items.next_id();
        let text_end = end == self.analysis.clusters.end_id();
        let mut resume = None;
        let mut item = from;
        // The first items on this line and on the next at the breaks before
        // and after it. A line starting past the text's start starts at a
        // break.
        let mut line_first = None;
        let mut next_line = None;
        while item < count {
            work::step();
            let Range {
                start: first,
                end: next,
            } = item_clusters.range(item);
            if first > end {
                break;
            }
            let empty = first == next;
            if first == end && !text_end {
                // The items up to the last close at the break are this line's.
                let after = *next_line.get_or_insert_with(|| {
                    measure::items_after_break(self.content, self.analysis, end, item)
                });
                if !empty || item >= after {
                    break;
                }
                resume.get_or_insert(item);
            }
            // An item across the break is the next line's too.
            if next > end && resume.is_none() {
                resume = Some(item);
            }
            let on_line = if !empty {
                next > start
            } else if first == start && start.get() > 0 {
                // The items up to the last close at the break before are the
                // line before's.
                let mine = *line_first.get_or_insert_with(|| {
                    measure::items_after_break(self.content, self.analysis, start, item)
                });
                item >= mine
            } else {
                first >= start
            };
            if on_line {
                each(item, first..next);
            }
            item = ItemId::new(item.get() + 1);
        }
        resume.unwrap_or(item)
    }

    /// Calls `each` for every item on the line from `start` to `end`, with
    /// its extent on the line, and returns the first item that may be on the
    /// next line.
    ///
    /// An item's extent is measurement's, except for text across the line's
    /// start or end that its fonts widened. That text is widened only by the
    /// fonts of its clusters on the line, as Chrome's
    /// `InlineBoxState::AccumulateUsedFonts` reads the fallback fonts of the
    /// line's part of the shape result. `fonts` carries the text item and
    /// the cursor over the shaping runs from the line before.
    ///
    /// Where the line makes room for annotations and marks, `ems` gathers
    /// its em boxes in the same walk.
    #[inline]
    pub(super) fn extents_on_line(
        &self,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
        fonts: &mut LineFonts,
        mut ems: Option<&mut EmWalk<'_, '_>>,
        mut each: impl FnMut(ItemId, Extent),
    ) -> ItemId {
        let extents = &self.measured.extents;
        self.items_on_line(from, start, end, |item, clusters| {
            if let Some(ems) = ems.as_deref_mut() {
                ems.visit(self, item, clusters.clone());
            }
            let extent = extents.get(item);
            if clusters.start >= start && clusters.end <= end {
                each(item, extent);
            } else {
                let on_line = clusters.start.max(start)..clusters.end.min(end);
                each(item, self.text_on_line(item, extent, on_line, fonts));
            }
        })
    }

    /// Returns the extent of text item `item`'s clusters `on_line`, where
    /// measurement gives the whole item `extent`.
    ///
    /// Only text under `line-height: normal` is widened by its fonts. Its
    /// extent on the line is its primary font's, united with that of each
    /// font its clusters there are shaped in. Measurement united the fonts of
    /// all the item's clusters, so once a font widens the text to the whole
    /// item's extent, no other can widen it further.
    #[inline(never)]
    fn text_on_line(
        &self,
        item: ItemId,
        extent: Extent,
        on_line: Range<ClusterId>,
        fonts: &mut LineFonts,
    ) -> Extent {
        if extent.is_none() || on_line.is_empty() {
            return extent;
        }
        let own = match fonts.item {
            Some((known, own)) if known == item => own,
            _ => {
                let own = self.widened_text(item, extent);
                fonts.item = Some((item, own));
                own
            }
        };
        let Some(own) = own else {
            return extent;
        };
        let shaped = &self.shaped.runs;
        // The cursor's run starts at or before `reusable`.
        let cursor = match &mut fonts.runs {
            Some((cursor, reusable)) if *reusable <= on_line.start => {
                *reusable = on_line.end;
                cursor
            }
            runs => match shaped.cursor_containing(on_line.start) {
                Some(cursor) => &mut runs.insert((cursor, on_line.end)).0,
                None => return extent,
            },
        };
        let mut united = own;
        let mut last = None;
        loop {
            work::step();
            let before = cursor.id();
            if cursor.end() > on_line.start {
                let font = shaped.get(before).map(|run| run.font);
                // A font the line's text used already widens nothing.
                if font != last
                    && let Some(used) = font.and_then(|font| self.fonts.used.get(font))
                {
                    united = united.unite(normal_extent(&used.metrics));
                    last = font;
                }
                if cursor.end() >= on_line.end || united == extent {
                    return united;
                }
            }
            shaped.step(cursor);
            if cursor.id() == before {
                return united;
            }
        }
    }

    /// Returns the primary font's extent of text item `item`, where its
    /// fonts widened it past that to its measured `extent`.
    ///
    /// `None` where they did not, or where `item` is no text under
    /// `line-height: normal`, whose fonts widen nothing.
    fn widened_text(&self, item: ItemId, extent: Extent) -> Option<Extent> {
        let content = self.content;
        let found = content.items.get(item)?;
        if found.kind != ItemKind::Text {
            return None;
        }
        let text = self.text_facts(found.node);
        if !matches!(content.facts.text(text).line_height, TextLineHeight::Normal) {
            return None;
        }
        let primary = self.fonts.primary_font(content.facts.text_request(text))?;
        let own = normal_extent(&primary.metrics);
        (own != extent).then_some(own)
    }
}

/// What a line's text extents carry to the next line: the text item last
/// asked about, and the cursor over the shaping runs.
///
/// Lines are broken forward, so the text across one line's end is across
/// the next line's start, and that line's runs follow. Each line then reads
/// the item's facts and seeks its first run only where they change.
#[derive(Default)]
pub(super) struct LineFonts {
    /// The text item, with its primary font's extent where its fonts
    /// widened it.
    item: Option<(ItemId, Option<Extent>)>,
    /// The cursor, and the cluster at or after its run's start from which
    /// a line may reuse it.
    runs: Option<(RunCursor<ShapedRunId, ClusterId>, ClusterId)>,
}

/// Returns an unshifted line's `extent` united with the strut of every box
/// open across its `start`.
///
/// Blink likewise keeps an open box's text metrics for the next line. The
/// node of the item holding the start does not count, even where it is a
/// box, as for a `<wbr>` the builder made. It stays out of the fit loop,
/// which calls it only where the content has a box.
///
/// The cost is constant a line, however deep the boxes nest. A line inside
/// few boxes walks up them. Past [`SHALLOW`], `carried` keeps a record with
/// each box's strut united with those around it. A line then reads one
/// union, and the items between two lines' starts are passed once.
#[inline(never)]
pub(super) fn carried_struts(
    stages: &LineStages<'_>,
    carried: &mut CarriedBoxes,
    from: ItemId,
    start: ClusterId,
    mut extent: Extent,
) -> Extent {
    let Some((holding, item)) = stages.start_item(from, start) else {
        return extent;
    };
    if !carried.deep {
        let nodes = &stages.content.nodes;
        let mut boxes = stages.boxes_around(item.node);
        for node in boxes.by_ref().take(SHALLOW) {
            let open = nodes.items(node).start;
            if stages.analysis.item_clusters.start(open) < start {
                extent = extent.unite(stages.measured.extents.get(open));
            }
        }
        if boxes.next().is_none() {
            return extent;
        }
        // Deeper than that, the record takes over and holds every box.
        carried.start(stages, holding, item);
    } else if holding < carried.at {
        // A line before the record's: balancing and scoring lay a paragraph
        // out again from its start. The record restarts there from the
        // parent links, at the depth's cost.
        carried.start(stages, holding, item);
    }
    carried.move_to(stages, holding);
    let outer = carried
        .record
        .iter()
        .rev()
        .inspect(|_| work::step())
        .filter(|open| open.node != item.node)
        .find(|open| open.opened < start);
    outer.map_or(extent, |open| extent.unite(open.struts))
}

/// Returns the extent of a line with shifted boxes, and the first item that
/// may be on the next line.
///
/// The boxes shift as Blink's `InlineLayoutStateStack` shifts them. The
/// shifts the line settles are pushed to `shifts`, sorted by node. The line
/// holds `clusters`, from item `from`, and `ems` gathers its em boxes where
/// given.
///
/// A line whose text `text-fit` scales by `scale` takes its items' extents
/// and its boxes' shifts at that scale.
#[allow(clippy::too_many_arguments)]
pub(super) fn shifted(
    stages: &LineStages<'_>,
    boxes: &mut BoxStack,
    shifts: &mut Table<LineShiftId, LineShift>,
    fonts: &mut LineFonts,
    ems: Option<&mut EmWalk<'_, '_>>,
    from: ItemId,
    clusters: Range<ClusterId>,
    scale: f32,
) -> (Extent, ItemId) {
    let Range { start, end } = clusters;
    let nodes = &stages.content.nodes;
    let items = &stages.content.items;
    let measured = stages.measured;
    boxes.stack.clear();
    boxes.waiting.clear();
    boxes.scale = scale;
    let scaled = |item: ItemId, extent: Extent| {
        if scale == 1.0 {
            extent
        } else {
            stages.scaled_extent(item, extent, scale)
        }
    };
    let settled = shifts.next_id();
    let root = NodeId::BLOCK;
    let line = OpenBox {
        node: root,
        text: stages.text_facts(root),
        align: VerticalAlign::Baseline,
        metrics: if scale == 1.0 {
            measured.extents.strut
        } else {
            stages.scaled_strut(stages.text_facts(root), scale)
        },
        pinned: OpenBoxId::new(0),
        waiting: None,
    };
    boxes
        .stack
        .push_bounded(line, "an empty stack takes the line's box");
    // The boxes open across the start, outermost first, each with its own
    // strut, as Blink keeps an open box's text metrics for the next line.
    if let Some((_, item)) = stages.start_item(from, start) {
        let mut carried = mem::take(&mut boxes.carried);
        stages.carried(item, start, &mut carried);
        for &node in carried.iter().rev() {
            work::step();
            let open = nodes.items(node).start;
            boxes.open(stages, node, scaled(open, measured.extents.get(open)));
        }
        boxes.carried = carried;
    }
    let resume = stages.extents_on_line(from, start, end, fonts, ems, |id, extent| {
        let Some(item) = items.get(id) else {
            return;
        };
        let extent = scaled(id, extent);
        if item.flags.contains(ItemFlags::ANNOTATION) {
            return;
        }
        let node = item.node;
        match item.kind {
            ItemKind::Open if stages.is_box(node) => boxes.open(stages, node, extent),
            ItemKind::Close => {
                // A close at the line's start ends a box of the line before,
                // which this line never opened.
                if boxes.stack.len() > 1 && boxes.stack.last().is_some_and(|b| b.node == node) {
                    boxes.close(stages, shifts);
                }
            }
            ItemKind::Atomic => {
                boxes.open(stages, node, extent);
                boxes.close(stages, shifts);
            }
            _ => boxes.unite(extent),
        }
    });
    // The boxes still open end with the line, innermost first, and then the
    // line's own box settles what waits for it.
    while boxes.stack.len() > 1 {
        work::step();
        boxes.close(stages, shifts);
    }
    boxes.settle(stages, shifts, OpenBoxId::new(0));
    if let Some(found) = shifts.get_slice_mut(settled..shifts.next_id()) {
        sort_by_key(found, |shift| shift.node);
    }
    let extent = boxes
        .stack
        .get(OpenBoxId::new(0))
        .map_or(Extent::NONE, |line| line.metrics);
    (extent, resume)
}

impl BoxStack {
    /// Opens `node`'s box on the line with `extent` around its baseline.
    fn open(&mut self, stages: &LineStages<'_>, node: NodeId, extent: Extent) {
        let align = stages.align(node);
        let pinned = if matches!(align, VerticalAlign::Top | VerticalAlign::Bottom) {
            self.stack.next_id()
        } else {
            self.stack
                .last()
                .map_or(OpenBoxId::new(0), |parent| parent.pinned)
        };
        let open = OpenBox {
            node,
            text: stages.text_facts(node),
            align,
            metrics: extent,
            pinned,
            waiting: None,
        };
        self.stack
            .push_bounded(open, "no more boxes open than an OpenBoxId names");
    }

    /// Leaves the closed box `node` waiting for the open box `owner`.
    ///
    /// It goes last among the boxes waiting for `owner`.
    fn wait(&mut self, owner: OpenBoxId, node: NodeId, align: VerticalAlign, metrics: Extent) {
        let waiting = Waiting {
            node,
            align,
            metrics,
            next: None,
        };
        let Some(at) = self.waiting.push(waiting) else {
            debug_assert!(false, "no more boxes wait on a line than a WaitingId names");
            return;
        };
        let Some(open) = self.stack.get_mut(owner) else {
            return;
        };
        match open.waiting {
            None => open.waiting = Some((at, at)),
            Some((first, last)) => {
                open.waiting = Some((first, at));
                if let Some(before) = self.waiting.get_mut(last) {
                    before.next = Some(at);
                }
            }
        }
    }

    /// Unites `extent` into the innermost open box.
    fn unite(&mut self, extent: Extent) {
        if let Some(open) = self.stack.last_mut() {
            open.metrics = open.metrics.unite(extent);
        }
    }

    /// Closes the innermost open box, as Blink's `EndBoxState` and
    /// `ApplyBaselineShift` do.
    ///
    /// It settles what waits for the box, then shifts the box. The box is
    /// united into its parent or left waiting. Settled shifts go in
    /// `shifts`.
    fn close(&mut self, stages: &LineStages<'_>, shifts: &mut Table<LineShiftId, LineShift>) {
        let Some(index) = self.stack.last_id() else {
            return;
        };
        self.settle(stages, shifts, index);
        let Some(open) = self.stack.pop() else {
            return;
        };
        // The box around it is now the stack's last.
        let Some(around) = self.stack.last_id() else {
            return;
        };
        let Some((parent, pinned)) = self
            .stack
            .get(around)
            .map(|parent| (parent.text, parent.pinned))
        else {
            return;
        };
        let mut own = stages.measured.shift(open.node);
        if self.scale != 1.0 {
            // The scaled shift goes in the line's settled shifts, as what it
            // adds to the measured one.
            let more = stages.scaled_shift(open.node, open.align, self.scale);
            record(shifts, open.node, more);
            own = own + more;
        }
        match open.align {
            // Waits for the nearest pinned box around it, or the line.
            VerticalAlign::Top | VerticalAlign::Bottom => {
                self.wait(pinned, open.node, open.align, open.metrics);
            }
            VerticalAlign::TextTop | VerticalAlign::TextBottom => {
                self.wait(around, open.node, open.align, open.metrics);
            }
            VerticalAlign::Middle => {
                // The extent's middle goes on the parent's baseline raised
                // by half the parent's x-height. The half rounds onto the
                // grid as Chrome's `FromFloatRound` rounds it.
                let half_x = primary_metrics(stages, parent).map_or(LayoutUnit::ZERO, |m| {
                    LayoutUnit::from_px(m.x_height.to_px() / 2.0)
                });
                let metrics = open.metrics.zero_if_none();
                let up = half_x - (metrics.ascent() - metrics.descent()).half();
                record(shifts, open.node, up);
                self.unite(open.metrics.raised(up + own));
            }
            _ => self.unite(open.metrics.raised(own)),
        }
    }

    /// Places the boxes waiting for the open box `index`, as Blink's
    /// `ApplyBaselineShift` on `pending_descendants`.
    ///
    /// `text-top` and `text-bottom` boxes go first. Then `top` and `bottom`
    /// boxes are pinned against what the box now holds. Settled shifts go in
    /// `shifts`.
    ///
    /// A box nothing waits for costs one test. Otherwise only its own
    /// waiting list is walked, so a line looks at each waiting box at most
    /// three times.
    fn settle(
        &mut self,
        stages: &LineStages<'_>,
        shifts: &mut Table<LineShiftId, LineShift>,
        index: OpenBoxId,
    ) {
        let Some((text, (first, _))) = self
            .stack
            .get(index)
            .and_then(|open| Some((open.text, open.waiting?)))
        else {
            return;
        };
        // The waiting list is consumed here, whatever this finds.
        if let Some(open) = self.stack.get_mut(index) {
            open.waiting = None;
        }
        // Its text's primary font's ascent and descent, unleaded.
        let font = primary_metrics(stages, text);
        let (ascent, descent) = font.map_or((LayoutUnit::ZERO, LayoutUnit::ZERO), |m| {
            (m.ascent, m.descent)
        });
        let mut pinned = false;
        let mut at = Some(first);
        while let Some(waiting) = at.and_then(|at| self.waiting.get(at).copied()) {
            work::step();
            at = waiting.next;
            let metrics = waiting.metrics.zero_if_none();
            let up = match waiting.align {
                VerticalAlign::TextTop => ascent - metrics.ascent(),
                VerticalAlign::TextBottom => metrics.descent() - descent,
                _ => {
                    pinned = true;
                    continue;
                }
            };
            record(shifts, waiting.node, up);
            self.unite_box(index, metrics.raised(up));
        }
        if pinned {
            // The aligned subtree, and how far the pinned boxes grow it.
            let subtree = self
                .stack
                .get(index)
                .map_or(Extent::NONE, |open| open.metrics.zero_if_none());
            let mut reach = subtree;
            let mut at = Some(first);
            while let Some(waiting) = at.and_then(|at| self.waiting.get(at)) {
                work::step();
                at = waiting.next;
                let height = waiting.metrics.zero_if_none().height();
                if height > reach.height() {
                    reach = match waiting.align {
                        VerticalAlign::Top => {
                            Extent::new(subtree.ascent(), height - subtree.ascent())
                        }
                        VerticalAlign::Bottom => {
                            Extent::new(height - subtree.descent(), subtree.descent())
                        }
                        _ => reach,
                    };
                }
            }
            let mut at = Some(first);
            while let Some(waiting) = at.and_then(|at| self.waiting.get(at).copied()) {
                work::step();
                at = waiting.next;
                let metrics = waiting.metrics.zero_if_none();
                let up = match waiting.align {
                    VerticalAlign::Top => reach.ascent() - metrics.ascent(),
                    VerticalAlign::Bottom => metrics.descent() - reach.descent(),
                    _ => continue,
                };
                record(shifts, waiting.node, up);
                self.unite_box(index, metrics.raised(up));
            }
        }
    }

    /// Unites `extent` into the open box at `index`.
    fn unite_box(&mut self, index: OpenBoxId, extent: Extent) {
        if let Some(open) = self.stack.get_mut(index) {
            open.metrics = open.metrics.unite(extent);
        }
    }
}

/// Records a nonzero shift settled for `node` in `shifts`.
fn record(shifts: &mut Table<LineShiftId, LineShift>, node: NodeId, shift: LayoutUnit) {
    if shift != LayoutUnit::ZERO && shifts.push(LineShift { node, shift }).is_none() {
        debug_assert!(false, "no more shifts than a LineShiftId names");
    }
}

/// Returns the unleaded line metrics of the primary font for `text`.
///
/// `middle`, `text-top` and `text-bottom` measure by these, not by the
/// leaded text metrics.
pub(super) fn primary_metrics<'a>(
    stages: &LineStages<'a>,
    text: TextFactsId,
) -> Option<&'a FontLineMetrics> {
    let request = stages.content.facts.text_request(text);
    stages.fonts.primary_font(request).map(|used| &used.metrics)
}

/// What a line's height measure finds.
///
/// It holds the line box's extent, the first item that may be on the next
/// line, the room the line leaves the next at its end, and how far its
/// annotations move an initial letter it opens down.
#[derive(Copy, Clone, Debug)]
pub(super) struct Height {
    pub(super) extent: Extent,
    pub(super) next_item: ItemId,
    pub(super) carry: Carry,
    pub(super) letter_shift: LayoutUnit,
}

/// A line's height, as more of the breaker's `impl`.
impl<'a: 'c, 'c, 'm, 'provider> Breaker<'a, 'c, 'm, 'provider> {
    /// Returns how tall a line from `start` is taken to be before its own
    /// height is known.
    ///
    /// That is the strut's height. For the line an initial letter opens, it
    /// reaches the letter's margin box's bottom if further. Chrome's
    /// `ComputeTotalBlockSize` does the same.
    pub(super) fn reserve(&self, start: ClusterId) -> LayoutUnit {
        let strut = self.stages.measured.extents.strut.box_height();
        if start == ClusterId::new(0)
            && let Some(letter) = self.stages.measured.initial_letter()
        {
            let (top, height) = (letter.exclusion_top, letter.exclusion_height);
            return strut.max(top + height);
        }
        strut
    }

    /// Returns the line box's height for a line from `start` to `end`, from
    /// item `from` on.
    ///
    /// The extent unites the strut, every item on the line at its baseline,
    /// and the strut of every box open across its start. The result also
    /// holds the next line's first item and the room left at the line's end.
    ///
    /// Where nothing is shifted, this is one flat union. Otherwise the line
    /// is walked box by box, and settles the shifts only it can. Both find
    /// the line's items with
    /// [`LineStages::extents_on_line`](crate::stages::LineStages::extents_on_line).
    pub(super) fn extent(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
    ) -> Height {
        let (extent, next_item) = self.natural_extent(out, para, from, start, end);
        // A raised initial letter moves its line down by how far it stands
        // above the paragraph, as Chrome's
        // `SetInitialLetterBlockStartAdjustment` does. The line box grows
        // over its baseline by as much.
        let extent = match self.stages.measured.initial_letter() {
            Some(letter) if start == ClusterId::new(0) && letter.raise > LayoutUnit::ZERO => {
                let extent = extent.zero_if_none();
                Extent::new(extent.ascent() + letter.raise, extent.descent())
            }
            _ => extent,
        };
        let Some(annotation) = &self.annotation else {
            return Height {
                extent,
                next_item,
                carry: self.carry,
                letter_shift: LayoutUnit::ZERO,
            };
        };
        let columns = self.line_columns(start..end);
        if self.line_scale != 1.0 {
            self.scratch.ems.scale(self.line_scale);
        }
        let (annotated, carry) = annotation.room(
            &self.stages,
            &mut self.scratch.bands,
            &self.scratch.ems,
            start..end,
            extent,
            AnnotationContinuation {
                scale: self.line_scale,
                carry: self.carry,
                columns,
                parts: out
                    .ruby
                    .as_deref()
                    .map(|ruby| (ruby, ruby.recent_line_parts(out.lines.next_id()))),
            },
        );
        // Annotations that move the line down past a raised letter's raise
        // move the letter down with it, as Chrome's
        // `ComputeInitialLetterBoxBlockStartAdjustment` does.
        let letter_shift = if start == ClusterId::new(0) && !extent.is_none() {
            (annotated.ascent() - extent.ascent()).max(LayoutUnit::ZERO)
        } else {
            LayoutUnit::ZERO
        };
        Height {
            extent: annotated,
            next_item,
            carry,
            letter_shift,
        }
    }

    /// Returns the line's own extent, before annotations and marks make
    /// room (see [`extent`](Self::extent)).
    fn natural_extent(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
    ) -> (Extent, ItemId) {
        // A line making room for annotations and marks gathers its em boxes
        // in the walk that measures its extent.
        let room = self.annotation.as_ref();
        let ems = &mut self.scratch.ems;
        let mut ems = room.map(|room| EmWalk::new(&self.stages, Some(room), ems, start..end));
        let scale = self.line_scale;
        if self.stages.has_shifts() || scale != 1.0 {
            let kept = out.kept_shifts_end();
            let shifts = &mut out.shifts;
            shifts.truncate(kept);
            let boxes = &mut self.scratch.boxes;
            let fonts = &mut self.line_fonts;
            let ems = ems.as_mut();
            let clusters = start..end;
            return shifted(
                &self.stages,
                boxes,
                shifts,
                fonts,
                ems,
                from,
                clusters,
                scale,
            );
        }
        let mut extent = self.stages.measured.extents.strut;
        let fonts = &mut self.line_fonts;
        let next = self
            .stages
            .extents_on_line(from, start, end, fonts, ems.as_mut(), |_, own| {
                extent = extent.unite(own);
            });
        // The boxes open across the start, whose struts the line holds.
        // Content with no box skips this. So does a paragraph whose every
        // box strut the block's strut already holds.
        let boxes = self
            .stages
            .content
            .flags
            .contains(ContentFlags::INLINE_BOXES);
        if boxes && para.struts {
            extent = carried_struts(&self.stages, &mut self.scratch.carried, from, start, extent);
        }
        (extent, next)
    }
}
