//! Line height: a line box's extent, and the baseline shifts only the line
//! can settle.
//!
//! The extent is the strut united with every item's extent at its box's
//! baseline, and with the strut of every box open across the line's start
//! (Blink's `OnBeginPlaceItems` keeps an open box's `text_metrics`).
//!
//! An unshifted line is one flat union; [`carried_struts`] finds the boxes
//! open across its start at a cost independent of nesting depth.
//!
//! A line with a shifted box or `text-fit` scaling is walked box by box, as
//! Blink's `InlineLayoutStateStack` does. Each box unites its content around
//! its own baseline and applies its shift when it closes. Measurement holds
//! the shifts that do not depend on the line. This module settles the rest:
//! - `middle` centres the box on its parent's baseline raised by half the
//!   parent's x-height.
//! - `text-top` and `text-bottom` align with the parent's unleaded text once
//!   the parent closes.
//! - `top` and `bottom` settle last, against the aligned subtree of the
//!   nearest `top` or `bottom` box around them, or of the line. A taller
//!   pinned box grows the subtree's other edge, as Blink's
//!   `MetricsForTopAndBottomAlign` grows it.
//!
//! Settled shifts go in the lines' rare `shifts` table, sorted by node within
//! a line. Placement adds them to measurement's.
//!
//! For `top` and `bottom` this follows CSS 2.1 where Blink does not: a `top`
//! box inside a raised span lands on the line's top, and a box inside a
//! `top` box does not grow the line.
//!
//! Nothing here allocates once warm or recurses.

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
    /// Names a box open on the line, in a [`BoxStack`]; the line's box is
    /// first.
    struct OpenBoxId(u32);
}

define_id! {
    /// Names a closed box waiting for a box around it, in a [`BoxStack`].
    struct WaitingId(u32);
}

/// The box-by-box walk of a line with a shifted box or scaled text.
pub(super) struct BoxStack {
    /// The boxes open on the line, the line's own box first.
    stack: Table<OpenBoxId, OpenBox>,
    /// The closed boxes waiting for a box around them.
    waiting: Table<WaitingId, Waiting>,
    /// The boxes open across the line's start, innermost first.
    carried: Vec<NodeId>,
    /// The line's `text-fit` scale, which also scales its boxes' shifts.
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

/// The boxes open across a line's start, recorded from line to line where a
/// line starts inside more than [`SHALLOW`] boxes.
pub(super) struct CarriedBoxes {
    /// The boxes open before item `at`, outermost first, each with its strut
    /// united with those around it.
    record: Vec<Ancestor>,
    /// The item the record stands before.
    at: ItemId,
    /// Whether the record is in use.
    deep: bool,
    /// Scratch for the boxes around an item, innermost first.
    walked: Vec<NodeId>,
}

/// The most boxes around a line's start that the breaker walks up rather
/// than keeping a record in [`CarriedBoxes`].
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
    /// The item's own node counts where it is a box, as for a `<wbr>` the
    /// builder made.
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

    /// Pushes box `node`, opened by item `open`, with its strut united with
    /// those around it.
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

    /// Moves the record forward to item `to`, opening and closing boxes as
    /// the items between do.
    ///
    /// An item before the record restarts it from the first item.
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

/// A box open on the line.
#[derive(Copy, Clone, Debug)]
struct OpenBox {
    node: NodeId,
    /// Its text facts, whose primary font places `text-top`, `text-bottom`
    /// and `middle` boxes inside it.
    text: TextFactsId,
    align: VerticalAlign,
    /// Its content's extent so far, as Blink's `InlineBoxState::metrics`.
    metrics: Extent,
    /// The nearest `top` or `bottom` box, itself or one around it, or else
    /// the line's box. `top` and `bottom` boxes inside it wait for that box.
    pinned: OpenBoxId,
    /// The first and last boxes waiting for it, in closing order.
    waiting: Option<(WaitingId, WaitingId)>,
}

/// A box open where a line starts, as [`CarriedBoxes`] records it.
#[derive(Copy, Clone, Debug)]
struct Ancestor {
    node: NodeId,
    /// Its opening item's first cluster.
    opened: ClusterId,
    /// Its strut united with those of every box around it.
    struts: Extent,
}

/// A closed box whose shift waits for a box around it, after Blink's
/// `PendingPositions`.
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
    /// Content without `vertical-align` or `dominant-baseline` answers from
    /// its flags alone.
    #[inline]
    pub(super) fn has_shifts(&self) -> bool {
        let flags = self.content.flags;
        flags.contains(ContentFlags::VERTICAL_ALIGN)
            || (flags.contains(ContentFlags::DOMINANT_BASELINE)
                && (!self.measured.shifts().is_empty() || self.measured.has_dominant_aligns()))
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
    /// `item` holds the cluster at `start`. A box opening at `start` is one
    /// of the line's items, so it does not count.
    fn carried(&self, item: &Item, start: ClusterId, carried: &mut Vec<NodeId>) {
        carried.clear();
        let nodes = &self.content.nodes;
        let item_clusters = &self.analysis.item_clusters;
        carried.extend(
            self.boxes_around(item.node)
                .filter(|&node| item_clusters.start(nodes.items(node).start) < start),
        );
    }

    /// Returns the inline boxes around `node`, innermost first.
    ///
    /// It takes one work step a node and stops after as many steps as there
    /// are nodes, even where the links form a cycle.
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
    /// It walks forward from the line's first item `from`, past empty items.
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

    /// Returns the `vertical-align` of box or atomic inline `node`, or the
    /// alignment it takes from its parent's `dominant-baseline`.
    fn align(&self, node: NodeId) -> VerticalAlign {
        if let Some(align) = self.measured.dominant_align(node) {
            return align;
        }
        let content = self.content;
        let facts = content.nodes.box_facts(node, self.variant);
        content.facts.box_facts(facts).align
    }

    /// Calls `each` for every item on the line from `start` to `end`, with
    /// its clusters, and returns the first item that may be on the next line.
    ///
    /// An empty item is on the line its boundary starts, except that at a
    /// break the empty items up to the last close end the line before
    /// ([`items_after_break`](measure::items_after_break)). At the text's
    /// end every item is the last line's.
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
        // The first items of this line and the next, found once each.
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
                // The items up to the last close belong to the line before.
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
    /// Text across the line's start or end is widened only by the fonts of
    /// its clusters on the line, as Blink's
    /// `InlineBoxState::AccumulateUsedFonts` does. Where given, `ems` gathers
    /// the line's em boxes in the same walk.
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
    /// Only text under `line-height: normal` is widened by its fonts. The
    /// walk stops once the union reaches `extent`, which unites them all.
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
        // A kept cursor whose run ends before the line's text is sought
        // again: one seek beats stepping over every run between.
        let cursor = match &mut fonts.runs {
            Some((cursor, reusable))
                if *reusable <= on_line.start && cursor.end() > on_line.start =>
            {
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
                // A run in the font just united adds nothing.
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
    /// fonts widened it to `extent`.
    ///
    /// `None` where they did not, or where `item` is not text under
    /// `line-height: normal`.
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

/// The text item and shaping-run cursor one line's extents pass to the
/// next, which starts in the same text.
#[derive(Default)]
pub(super) struct LineFonts {
    /// The text item, with its primary font's extent where its fonts
    /// widened it.
    item: Option<(ItemId, Option<Extent>)>,
    /// The cursor, and the cluster from which a line may reuse it.
    runs: Option<(RunCursor<ShapedRunId, ClusterId>, ClusterId)>,
}

/// Returns an unshifted line's `extent` united with the strut of every box
/// open across its `start`.
///
/// The node of the item holding the start does not count, even where it is
/// a box. A line inside at most [`SHALLOW`] boxes walks up them; deeper, it
/// reads `carried`, which unites each box's strut with those around it.
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
        // Deeper, the record takes over.
        carried.start(stages, holding, item);
    } else if holding < carried.at {
        // A line before the record's, as balancing and scoring lay a
        // paragraph out again.
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

/// Returns the extent of a line with shifted boxes or scaled text, and the
/// first item that may be on the next line.
///
/// The line holds `clusters` from item `from`, with its text at `scale`.
/// Its settled shifts are pushed to `shifts`, sorted by node. Where given,
/// `ems` gathers its em boxes.
///
/// Out of line, so that an unshifted line keeps its code small.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
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
    // The boxes open across the start, outermost first, each with its strut.
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
                // A close at the line's start ends a box of the line before.
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
    // The boxes still open close with the line. Then the line's box settles
    // what waits for it.
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

    /// Leaves the closed box `node` last in line for the open box `owner`.
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
    /// Settles what waits for the box, then shifts it into its parent or
    /// leaves it waiting.
    fn close(&mut self, stages: &LineStages<'_>, shifts: &mut Table<LineShiftId, LineShift>) {
        let Some(index) = self.stack.last_id() else {
            return;
        };
        self.settle(stages, shifts, index);
        let Some(open) = self.stack.pop() else {
            return;
        };
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
            // The settled shift holds what scaling adds to the measured one.
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
                // Half the x-height rounds as Blink's `FromFloatRound` does.
                let half_x = primary_metrics(stages, parent)
                    .map_or(LayoutUnit::ZERO, FontLineMetrics::half_x_height);
                let metrics = open.metrics.zero_if_none();
                let up = half_x - (metrics.ascent() - metrics.descent()).half();
                record(shifts, open.node, up);
                self.unite(open.metrics.raised(up + own));
            }
            _ => self.unite(open.metrics.raised(own)),
        }
    }

    /// Places the boxes waiting for the open box `index`, as Blink's
    /// `ApplyBaselineShift` does for `pending_descendants`.
    ///
    /// `text-top` and `text-bottom` boxes go first, then `top` and `bottom`
    /// boxes are pinned against what the box holds.
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
        if let Some(open) = self.stack.get_mut(index) {
            open.waiting = None;
        }
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
            // The aligned subtree, then grown by the tallest pinned box.
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

/// Returns the unleaded line metrics of the primary font for `text`, by
/// which `middle`, `text-top` and `text-bottom` align.
pub(super) fn primary_metrics<'a>(
    stages: &LineStages<'a>,
    text: TextFactsId,
) -> Option<&'a FontLineMetrics> {
    let request = stages.content.facts.text_request(text);
    stages.fonts.primary_font(request).map(|used| &used.metrics)
}

/// A measured line's height and what it passes to the next line.
#[derive(Copy, Clone, Debug)]
pub(super) struct Height {
    /// The line box's extent around its baseline.
    pub(super) extent: Extent,
    /// The first item that may be on the next line.
    pub(super) next_item: ItemId,
    /// The room the line leaves the next at its end.
    pub(super) carry: Carry,
    /// How far the first line's annotations move the initial letter down.
    pub(super) letter_shift: LayoutUnit,
}

/// A line's height, as more of the breaker's `impl`.
impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Returns the height a line from `start` is fitted at before its own
    /// height is known.
    ///
    /// That is the strut's, or on an initial letter's line the letter's
    /// margin box bottom where lower, as Blink's `ComputeTotalBlockSize` takes it.
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

    /// Returns the height of the line from `start` to `end`, whose first
    /// item is `from`.
    ///
    /// The extent grows for a raised initial letter and makes room for
    /// annotations and marks.
    pub(super) fn extent(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
    ) -> Height {
        let (extent, next_item) = self.natural_extent(out, para, from, start, end);
        // A raised initial letter moves its line down by its raise, as
        // Blink's `SetInitialLetterBlockStartAdjustment` does, growing the
        // line box on its block-start side.
        let extent = match self.stages.measured.initial_letter() {
            Some(letter) if start == ClusterId::new(0) && letter.raise > LayoutUnit::ZERO => {
                let extent = extent.zero_if_none();
                if self.stages.content.block.writing_mode.flips_lines() {
                    Extent::new(extent.ascent(), extent.descent() + letter.raise)
                } else {
                    Extent::new(extent.ascent() + letter.raise, extent.descent())
                }
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
        // Annotations that move the line down past the letter's raise move
        // the letter by the excess, as Blink's
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

    /// Returns the line's extent before an initial letter, annotations and
    /// marks change it, and the first item that may be on the next line.
    fn natural_extent(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        from: ItemId,
        start: ClusterId,
        end: ClusterId,
    ) -> (Extent, ItemId) {
        let room = self.annotation.as_ref();
        let ems = &mut self.scratch.ems;
        let mut ems = room.map(|room| EmWalk::new(&self.stages, Some(room), ems, start..end));
        let scale = self.line_scale;
        if self.stages.has_shifts() || scale != 1.0 {
            let kept = out.kept_shifts_end();
            let shifts = &mut out.rare_mut().shifts;
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
        // Only content with an inline box whose strut passes the block's has
        // boxes to carry.
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
