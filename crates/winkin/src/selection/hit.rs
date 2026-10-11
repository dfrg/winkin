//! The position a point hits, as Chrome's `PositionForPoint` finds it in an inline formatting context.
//!
//! - The line is the one whose box holds the point. A box's bottom edge
//!   belongs to the next line, as in `PositionForPointInInlineFormattingContext`.
//!   The line box decides, not its glyphs.
//! - Above the first line or in a gap, the next line is hit. Below the last,
//!   the last is hit. [`PastLines`] says whether such a point keeps its column
//!   (Windows) or goes to the line's start or end (macOS and Linux), as Blink's
//!   `ShouldMoveCaretToHorizontalBoundaryWhenPastTopOrBottom` does.
//! - In the line, the hit is in the leaf under the point. Where no leaf is
//!   under it, the nearer leaf either side is hit at its near edge, a tie
//!   going left. A box's room is so split at its middle, each half hitting
//!   the position drawn on its side. Chrome's `PositionForPointInInlineBox`
//!   prefers the leaf after; this follows the two positions a box edge has.
//! - In text, the hit is the nearer edge of the cluster under the point. On
//!   an atomic inline it is before or after by its halves, the middle before.
//! - A ruby annotation's text is hit where the point is on it. An ellipsis or
//!   hyphen passes the hit to the hidden text under it.
//! - At the edge of a leaf in another direction, the hit is the position
//!   whose caret is drawn there, as Blink's `BidiAdjustment::AdjustForHitTest`
//!   finds it.
//!
//! The position is downstream, or upstream at its leaf's logical end. That
//! keeps a click past a wrapped line's end on that line.

use super::place::{self, ClusteredPosition, Leaf};
use super::{Affinity, Position};
use crate::config::PastLines;
use crate::data::Id;
use crate::layout::Layout;
use crate::layout::{CrossExtents, Line};
use crate::stages::lines::{InlineExtents, LineId};
use crate::work;

/// Returns the position the point `inline`, `block` hits, in the area's coordinates.
///
/// `None` before the layout has lines.
pub(crate) fn hit_test(
    layout: &Layout,
    inline: f32,
    block: f32,
    past: PastLines,
) -> Option<Position> {
    let (line, beyond) = line_by_block(layout, block)?;
    let metrics = line.metrics();
    let x = inline - metrics.left;
    let y = block - metrics.top;
    let hit = match (beyond, past) {
        (Beyond::Above, PastLines::LineEnds) => line_end(layout, &line, false),
        (Beyond::Below, PastLines::LineEnds) => line_end(layout, &line, true),
        _ => line_hit(layout, &line, x, y),
    };
    Some(place::snap(layout, hit).position)
}

/// Where a point is across the lines.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Beyond {
    /// In the line's box.
    Inside,
    /// Before the line: above the first, or between it and the one before.
    Above,
    /// After the last line.
    Below,
}

/// Returns the line `block` falls on and where the point is relative to it.
///
/// It is the line whose box holds `block`, else the first after it, else the
/// last. `None` where the layout has no lines. It binary-searches the line
/// boxes, which follow one another down the block.
fn line_by_block(layout: &Layout, block: f32) -> Option<(Line<'_>, Beyond)> {
    let count = layout.lines().len();
    // The first line whose box ends past the point.
    let (mut low, mut high) = (0, count);
    while low < high {
        work::step();
        let middle = low + (high - low) / 2;
        let metrics = Line::new(layout, LineId::new(middle))?.metrics();
        if metrics.top + metrics.height() <= block {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    match Line::new(layout, LineId::new(low)) {
        Some(line) if line.metrics().top <= block => Some((line, Beyond::Inside)),
        Some(line) => Some((line, Beyond::Above)),
        None => Some((
            Line::new(layout, LineId::new(count.checked_sub(1)?))?,
            Beyond::Below,
        )),
    }
}

/// Returns the position at `line`'s logical start, or its end where `end`.
///
/// A point past the lines hits it on macOS and Linux.
fn line_end(layout: &Layout, line: &Line<'_>, end: bool) -> ClusteredPosition {
    let clusters = line.record().clusters();
    let start = line.text_range().start;
    if end {
        let at = place::leaves(*line)
            .map(|leaf| leaf.end)
            .max()
            .unwrap_or(start);
        ClusteredPosition::from_walk(layout, clusters.end, Position::new(at, Affinity::Upstream))
    } else {
        let start = Position::new(start, Affinity::Downstream);
        ClusteredPosition::from_walk(layout, clusters.start, start)
    }
}

/// Returns the position the point `x`, `y` hits in `line`, from its box's left and top.
fn line_hit(layout: &Layout, line: &Line<'_>, x: f32, y: f32) -> ClusteredPosition {
    // An annotation's text, where the point is on it.
    for leaf in place::annotation_leaves(*line) {
        work::step();
        let InlineExtents { left, right } = leaf.inline;
        let Some(run) = leaf.run() else {
            continue;
        };
        let CrossExtents { over, under } = run.block();
        if left <= x && x < right && over <= y && y < under {
            let (offset, cluster) = leaf.hit(layout, x);
            return ClusteredPosition::with_cluster(layout, affine(&leaf, offset), cluster);
        }
    }
    hit_point(layout, line, x, Some(y))
}

/// Returns the position `x` along `line`, from its box's left, hits at any height.
///
/// Line motion lands there, and so does a point past the lines.
pub(super) fn hit_x(layout: &Layout, line: &Line<'_>, x: f32) -> ClusteredPosition {
    hit_point(layout, line, x, None)
}

/// Returns the position `x` along `line` hits, at `y` across it where given.
///
/// Only combined text reads `y`, since its characters stand across the line.
/// Without `y`, combined text is hit as an atomic inline is.
fn hit_point(layout: &Layout, line: &Line<'_>, x: f32, y: Option<f32>) -> ClusteredPosition {
    // The leaf under the point, else the nearest after it and before it,
    // each with the leaves beside it.
    let mut under = None;
    let mut after: Option<(Beside<'_>, f32)> = None;
    let mut before: Option<(Beside<'_>, f32)> = None;
    let mut previous = None;
    let mut leaves = place::leaves(*line);
    while let Some(leaf) = leaves.next() {
        work::step();
        // A nearest leaf found at the leaf before has this one on its right.
        for (nearest, _) in [&mut after, &mut before].into_iter().flatten() {
            if nearest.open {
                nearest.right = Some(leaf);
                nearest.open = false;
            }
        }
        let InlineExtents { left, right } = leaf.inline;
        if left <= x && x < right {
            under = Some(Beside {
                leaf,
                left: previous,
                right: leaves.next(),
                open: false,
            });
            break;
        }
        let beside = Beside {
            leaf,
            left: previous,
            right: None,
            open: true,
        };
        if left > x && after.is_none_or(|(_, nearest)| left < nearest) {
            after = Some((beside, left));
        }
        if right <= x && before.is_none_or(|(_, nearest)| right > nearest) {
            before = Some((beside, right));
        }
        previous = Some(leaf);
    }
    let (beside, (offset, cluster)) = match (under, after, before) {
        (Some(beside), _, _) => {
            let leaf = &beside.leaf;
            let across = y.and_then(|y| leaf.hit_across(layout, y));
            let hit = across.map_or_else(
                || leaf.hit(layout, x),
                |(offset, cluster)| (offset, Some(cluster)),
            );
            (beside, hit)
        }
        // In a gap the nearer leaf is hit at its edge facing the point, a tie
        // going left, so a box's room is split at its middle.
        (None, Some((_, left)), Some((before, right))) if x - right <= left - x => {
            let offset = before.leaf.edge_offset(true);
            (before, (offset, None))
        }
        (None, Some((beside, _)), _) => {
            let offset = beside.leaf.edge_offset(false);
            (beside, (offset, None))
        }
        (None, None, Some((beside, _))) => {
            let offset = beside.leaf.edge_offset(true);
            (beside, (offset, None))
        }
        (None, None, None) => {
            let clusters = &layout.analysis().clusters;
            let start = line.record().clusters().start;
            return ClusteredPosition::from_cluster(clusters, start, Affinity::Downstream);
        }
    };
    let (position, kept) = adjusted(&beside, offset);
    // The cluster the leaf found is the position's unless the hit moved to
    // another offset beside it.
    ClusteredPosition::with_cluster(layout, position, cluster.filter(|_| kept))
}

/// A leaf a point hits, with the leaves drawn either side of it on its line.
#[derive(Copy, Clone)]
struct Beside<'a> {
    leaf: Leaf<'a>,
    /// The leaf before it in visual order, and the one after.
    left: Option<Leaf<'a>>,
    right: Option<Leaf<'a>>,
    /// Whether the leaf after it is still to come in the walk.
    open: bool,
}

/// Returns `offset` in the leaf `beside` holds, adjusted at a direction change.
///
/// Where `offset` is at the leaf's visual edge and the leaf meeting that
/// edge is nearer the paragraph's level, it returns that leaf's offset at
/// the edge. That is the position whose caret is drawn there, as Blink's
/// `BidiAdjustment::AdjustForHitTest` finds it. A leaf across a box's room
/// does not meet the edge, since each side of the room has its own caret.
/// With the position comes whether the offset is still `offset`.
fn adjusted(beside: &Beside<'_>, offset: usize) -> (Position, bool) {
    let leaf = &beside.leaf;
    let right = leaf.edge_offset(true) == offset;
    let left = leaf.edge_offset(false) == offset;
    let neighbour = if right {
        beside
            .right
            .filter(|next| next.inline.left == leaf.inline.right)
    } else if left {
        beside
            .left
            .filter(|next| next.inline.right == leaf.inline.left)
    } else {
        None
    };
    match neighbour {
        Some(neighbour) if neighbour.level() < leaf.level() => {
            let edge = neighbour.edge_offset(left);
            (affine(&neighbour, edge), edge == offset)
        }
        _ => (affine(leaf, offset), true),
    }
}

/// Returns `offset` in `leaf` as a position, upstream at the leaf's logical end.
///
/// Upstream keeps it on the leaf's line where the line wraps there.
/// Elsewhere it is downstream.
fn affine(leaf: &Leaf<'_>, offset: usize) -> Position {
    if offset == leaf.end && leaf.start < leaf.end {
        Position::new(offset, Affinity::Upstream)
    } else {
        Position::new(offset, Affinity::Downstream)
    }
}
