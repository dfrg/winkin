//! Box edges: the room the items at a boundary take along the line.
//!
//! Every reader charges an edge through [`edge_room`], so all agree with the
//! prefix. The scan's line-end costs are in `scan/ends.rs`.

use super::InitialLetter;
use super::scan::BoundaryRoom;
use crate::data::Id;
use crate::stages::analysis::{Analysis, ClusterId};
use crate::stages::content::{BoxFacts, Content, ContentFlags, Item, ItemFlags, ItemId, ItemKind};
use crate::style::FirstLineVariant;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// The position of a break among the items at one boundary, fed one item at
/// a time, as Blink's `RewindOverflow` places it.
///
/// - Closes of earlier boxes, and boxes that open and close at the boundary,
///   end the line before.
/// - A box that opens there and closes later starts the line after, with
///   everything that follows it.
/// - A ruby container's opening, and any opening after a forced break,
///   starts the line after (`HandleForcedLineBreak`).
/// - An out-of-flow anchor ends the line before only ahead of the
///   boundary's first opening (`HandleOutOfFlowPositioned`); elsewhere it
///   starts the line after.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct BoundarySplit {
    /// Whether the boundary follows a forced break.
    forced: bool,
    /// Whether the break is fixed: every later item starts the line after.
    held: bool,
    /// Whether a box has opened at the boundary, so an anchor now starts the
    /// line after.
    opened: bool,
    /// The number of boxes opened at the boundary that are still open.
    depth: u32,
    /// The edge room of the boxes opened since the depth was last zero.
    group: InlineLayoutUnit,
}

impl BoundarySplit {
    /// Returns the split at boundary `at` before any of its items is taken.
    pub(crate) fn new(analysis: &Analysis, at: ClusterId) -> Self {
        Self {
            forced: analysis.clusters.follows_forced_break(at),
            ..Self::default()
        }
    }

    /// Takes the next item, of `kind` with edge room `room`, adding the room
    /// to `before` (ends the line before a break) or `after`.
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

    /// Returns whether the last item taken starts the line after a break.
    pub(crate) fn splits(&self) -> bool {
        self.held || self.depth > 0
    }
}

/// Returns the first item, from `from` on, that starts the line after a
/// break at boundary `at`, as [`BoundarySplit`] places the break.
///
/// Skips annotation items. Where none starts the line after, returns the
/// first item past the boundary.
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

/// Returns the room of the items at boundary `at`: the edges that end the
/// line before a break there, which the prefix entry at `at` includes, and
/// the rest, which it doesn't.
///
/// At the end of text that ends on a line, all room is in the first value.
/// `from` is the first item at the boundary, or else the item containing
/// the cluster after it.
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
    // No line starts at the text's end, so the last line takes everything.
    let ends_on_a_line = analysis.paragraphs.ends_on_a_line();
    if at == analysis.clusters.end_id() && ends_on_a_line {
        return (room.leading_closes + room.rest, InlineLayoutUnit::ZERO);
    }
    (room.leading_closes, room.rest)
}

/// Returns the room `item`'s box takes along the line at its opening edge
/// (`opens`) or its closing one.
///
/// A box takes the margin, border and padding at that end, in its own
/// `direction`. The initial letter's box, `letter`, takes its stored room,
/// less the letter's advance outside its ink. `::first-line` doesn't change
/// box edges.
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
/// and inline end.
pub(super) fn logical_edges(facts: &BoxFacts) -> (LayoutUnit, LayoutUnit) {
    let (left, right) = facts.room;
    facts.direction().line_order(left, right)
}
