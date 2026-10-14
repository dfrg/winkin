//! The position a point hits, as Chrome's `PositionForPoint` finds it in an
//! inline formatting context.
//!
//! - The line is the one whose line box holds the point; glyph extents play
//!   no part. A box's bottom edge belongs to the next line, as in
//!   `PositionForPointInInlineFormattingContext`.
//! - Above the first line or in a gap, the next line is hit; below the last,
//!   the last. [`PastLines`] says whether such a point keeps its column
//!   (Windows) or goes to the line's start or end (macOS and Linux), as
//!   Blink's `ShouldMoveCaretToHorizontalBoundaryWhenPastTopOrBottom` does.
//! - In the line, the leaf under the point is hit. Where none is, the nearer
//!   leaf on either side is hit at its near edge, a tie going left, so a
//!   box's room splits at its middle and each half hits the position drawn
//!   on its side. Chrome's `PositionForPointInInlineBox` prefers the leaf
//!   after; this follows the two positions a box edge has.
//! - In text, the hit is the nearer edge of the cluster under the point. On
//!   an atomic inline it is before or after by halves, the middle before.
//! - Ruby annotation text is hit where the point is on it. An ellipsis or
//!   hyphen passes the hit to the hidden text under it.
//! - At the edge of a leaf in another direction, the hit is the position
//!   whose caret is drawn there, as Blink's `BidiAdjustment::AdjustForHitTest`
//!   finds it.
//!
//! The position is downstream, or upstream at its leaf's logical end, which
//! keeps a click past a wrapped line's end on that line.

use super::place::{self, ClusteredPosition, Leaf};
use super::{Affinity, Position};
use crate::config::PastLines;
use crate::data::Id;
use crate::layout::Layout;
use crate::layout::{CrossExtents, Line};
use crate::stages::lines::{InlineExtents, LineId};
use crate::work;

/// Returns the position the point `(inline, block)` hits, in the area's
/// coordinates.
///
/// Returns `None` where the layout has no lines.
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

/// Where a point is relative to the line it hits.
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
/// The line is the one whose box holds `block`, else the first after it,
/// else the last. Returns `None` where the layout has no lines. Binary-searches
/// the line boxes, which are ordered down the block.
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
/// A point past the lines hits here under [`PastLines::LineEnds`].
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

/// Returns the position `x` along `line`, from its box's left, hits at any
/// height.
///
/// Line motion lands here, as does a point past the lines.
pub(super) fn hit_x(layout: &Layout, line: &Line<'_>, x: f32) -> ClusteredPosition {
    hit_point(layout, line, x, None)
}

/// Returns the position `x` along `line` hits, at `y` across it where given.
///
/// Only combined text reads `y`, since its characters are stacked across
/// the line. Without `y`, combined text is hit as an atomic inline is.
fn hit_point(layout: &Layout, line: &Line<'_>, x: f32, y: Option<f32>) -> ClusteredPosition {
    // The leaf under the point, else the nearest leaves after and before it,
    // each with its neighbours.
    let mut under = None;
    let mut after: Option<(Beside<'_>, f32)> = None;
    let mut before: Option<(Beside<'_>, f32)> = None;
    let mut previous = None;
    let mut leaves = place::leaves(*line);
    while let Some(leaf) = leaves.next() {
        work::step();
        // This leaf is the right neighbour of a nearest leaf found just
        // before.
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
    // The leaf's cluster stays valid unless the bidi adjustment moved the
    // offset.
    ClusteredPosition::with_cluster(layout, position, cluster.filter(|_| kept))
}

/// A leaf a point hits, with the leaves drawn either side of it on its line.
#[derive(Copy, Clone)]
struct Beside<'a> {
    leaf: Leaf<'a>,
    /// The leaf before it in visual order.
    left: Option<Leaf<'a>>,
    /// The leaf after it in visual order.
    right: Option<Leaf<'a>>,
    /// Whether the leaf after it is still to come in the walk.
    open: bool,
}

/// Returns `offset` in the leaf `beside` holds, adjusted at a direction change.
///
/// Where `offset` is at the leaf's visual edge and the leaf touching that
/// edge has a lower level, returns that leaf's offset at the edge: the
/// position whose caret is drawn there, as Blink's
/// `BidiAdjustment::AdjustForHitTest` finds it. A leaf across a box's room
/// does not touch the edge, since each side of the room has its own caret.
///
/// Also returns whether the offset is unchanged.
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
fn affine(leaf: &Leaf<'_>, offset: usize) -> Position {
    if offset == leaf.end && leaf.start < leaf.end {
        Position::new(offset, Affinity::Upstream)
    } else {
        Position::new(offset, Affinity::Downstream)
    }
}
