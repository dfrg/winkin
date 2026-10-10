//! Box edges: the room the items at a boundary take along the line.
//!
//! [`edge_room`] is the one function every reader charges an edge by. The
//! scan's line-end costs live in `scan_ends.rs`.

use super::InitialLetter;
use super::scan::BoundaryRoom;
use crate::data::Id;
use crate::stages::analysis::{Analysis, ClusterId};
use crate::stages::content::{BoxFacts, Content, ContentFlags, Item, ItemFlags, ItemId, ItemKind};
use crate::style::FirstLineVariant;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// Where a break at one boundary falls among the items there, read one item
/// at a time, as Chrome's `RewindOverflow` rewinds to a break.
///
/// A box that opens and closes at the boundary ends the line before: an
/// empty box after a line's last space stays on it. A box that opens there
/// and closes later starts the line after, and so does what follows it.
/// Closes of earlier boxes end the line before. A ruby container's opening
/// is no part of a line's end, nor is any opening after a forced break, as
/// `HandleForcedLineBreak` takes only the close tags right after it.
///
/// An absolutely positioned box's anchor ends the line before only ahead of
/// the boundary's first opening. Chrome's breaker may break after it there,
/// since it takes the break opportunity of the item before it
/// (`HandleOutOfFlowPositioned`). Elsewhere it starts the line after with
/// everything after it, or with the box it is in, as `RewindOverflow` holds
/// it untrailable. Where the line's trailing white space overflows, line
/// layout moves the anchors that end the line to the next
/// (`Placer::pieces`).
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct BoundarySplit {
    /// The boundary follows a forced break.
    forced: bool,
    /// The break is fixed: every item from here on starts the line after.
    held: bool,
    /// A box opened at the boundary: Chrome's breaker can no longer break
    /// after an anchor here.
    opened: bool,
    /// How many boxes opened at the boundary are open still.
    depth: u32,
    /// The room of the boxes opened since the depth was last zero.
    group: InlineLayoutUnit,
}

impl BoundarySplit {
    /// Returns the split at boundary `at` before any of its items.
    pub(crate) fn new(analysis: &Analysis, at: ClusterId) -> Self {
        Self {
            forced: analysis.clusters.follows_forced_break(at),
            ..Self::default()
        }
    }

    /// Takes an item of `kind` whose edge takes `room`, adding the room to
    /// `before`, what ends the line before a break here, or to `after`.
    pub(crate) fn take(
        &mut self,
        kind: ItemKind,
        room: InlineLayoutUnit,
        before: &mut InlineLayoutUnit,
        after: &mut InlineLayoutUnit,
    ) {
        if self.held {
            *after += room;
        } else if kind == ItemKind::RubyOpen || (self.forced && kind.is_open()) {
            self.held = true;
            *after += room;
        } else if kind == ItemKind::Absolute {
            self.held = self.forced || self.opened;
        } else if kind.is_open() {
            if self.depth == 0 {
                self.group = InlineLayoutUnit::ZERO;
            }
            self.opened = true;
            self.depth += 1;
            self.group += room;
            *after += room;
        } else if kind.is_close() {
            if self.depth == 0 {
                *before += room;
            } else {
                self.depth -= 1;
                *after += room;
                self.group += room;
                if self.depth == 0 {
                    *after = *after - self.group;
                    *before += self.group;
                    self.group = InlineLayoutUnit::ZERO;
                }
            }
        }
    }

    /// Returns whether an item taken so far starts the line after a break.
    pub(crate) fn splits(&self) -> bool {
        self.held || self.depth > 0
    }
}

/// Returns the first item, from `from` on, that starts the line after a
/// break at boundary `at`, as [`BoundarySplit`] places the break.
pub(crate) fn items_after_break(
    content: &Content,
    analysis: &Analysis,
    at: ClusterId,
    from: ItemId,
) -> ItemId {
    let mut split = BoundarySplit::new(analysis, at);
    let mut first = None;
    let (mut before, mut after) = (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
    let mut id = from;
    while let Some(item) = content.items.get(id) {
        work::step();
        let clusters = analysis.item_clusters.range(id);
        if clusters.start != at || !clusters.is_empty() {
            break;
        }
        if !item.flags.contains(ItemFlags::ANNOTATION) {
            split.take(item.kind, InlineLayoutUnit::ZERO, &mut before, &mut after);
            if !split.splits() {
                first = None;
            } else if first.is_none() {
                first = Some(id);
            }
            if split.held {
                break;
            }
        }
        id = ItemId::new(id.get() + 1);
    }
    first.unwrap_or(id)
}

/// Returns the room of the items at boundary `at`, as the prefix was built
/// with it.
///
/// The first value is the edges that end the line before a break there
/// ([`BoundarySplit`]), which come before the prefix there. The second is
/// the rest, which comes after it. See
/// [`LineStages::pen`](crate::stages::LineStages::pen) and
/// [`LineStages::cluster_advance`](crate::stages::LineStages::cluster_advance).
///
/// `from` is the first item at the boundary, or else the one holding the
/// cluster after it. A reader asking about one boundary holds it. The scan
/// walks the items itself.
pub(super) fn boundary_room(
    content: &Content,
    analysis: &Analysis,
    letter: Option<&InitialLetter>,
    at: ClusterId,
    from: ItemId,
) -> (InlineLayoutUnit, InlineLayoutUnit) {
    if !content.flags.contains(ContentFlags::BOXES_WITH_EDGES) {
        return (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
    }
    let mut room = BoundaryRoom::from_boundary(analysis, at);
    let mut id = from;
    while let Some(item) = content.items.get(id) {
        work::step();
        let clusters = analysis.item_clusters.range(id);
        if clusters.start != at || !clusters.is_empty() {
            break;
        }
        room.add(content, letter, item);
        id = ItemId::new(id.get() + 1);
    }
    // No line starts at the text's end. Where the last paragraph has
    // clusters, everything there is the last line's, as in the scan.
    let ends_on_a_line = analysis.paragraphs.ends_on_a_line();
    if at == analysis.clusters.end_id() && ends_on_a_line {
        return (room.leading_closes + room.rest, InlineLayoutUnit::ZERO);
    }
    (room.leading_closes, room.rest)
}

/// Returns the room `item`'s box takes along the line at its opening edge
/// (`opens`) or its closing one.
///
/// - A box takes the margin, border and padding of its inline start or
///   end, in its own `direction`. Chrome charges an open tag and a close
///   tag the same way.
/// - The block's initial letter's box, `letter`, takes what its measure
///   gave it: those edges less the letter's advance outside its ink.
///
/// `item` is a box's or a ruby container's opening or closing item. A box's
/// edges come from its facts, which `::first-line` doesn't change.
///
/// The prefix, the breaker's costs and line layout's pieces all charge an
/// edge through this function. So what line layout places adds up to what
/// the prefix measured.
pub(crate) fn edge_room(
    content: &Content,
    letter: Option<&InitialLetter>,
    item: &Item,
    opens: bool,
) -> InlineLayoutUnit {
    if let Some(letter) = letter
        && letter.node == item.node
    {
        return letter.edge_room(opens);
    }
    let facts = content.facts.box_facts(
        content
            .nodes
            .box_facts(item.node, FirstLineVariant::Standard),
    );
    let (start, end) = logical_edges(facts);
    InlineLayoutUnit::from_layout(if opens { start } else { end })
}

/// Returns the room a box with the facts `facts` takes at its inline start
/// and its inline end.
///
/// These are its line-left and line-right sides, swapped for a box that
/// reads right to left.
pub(super) fn logical_edges(facts: &BoxFacts) -> (LayoutUnit, LayoutUnit) {
    let (left, right) = facts.room;
    facts.direction().line_order(left, right)
}
