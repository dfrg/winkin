//! Line ends in the scan: what a line starting or ending at a boundary
//! pays there, and the room the items at a boundary take along the line.

use super::hang::{hangs_end, hangs_first};
use super::scan::{BoundaryRoom, EdgeAmounts, LinePay, Scan, ScanWalk};
use super::{LineEdgeCost, LineEdgeFlags};
use crate::data::Id;
use crate::stages::analysis::{ClusterAttrs, ClusterId, Clusters};
use crate::style::HangingPunctuation;
use crate::unit::{InlineLayoutUnit, LayoutUnit};

/// What a line ending at the scan's next boundary would end with.
///
/// - The last cluster of the paragraph so far that doesn't hang. Its line
///   padding and a mark that may hang past the line's end come from it. The
///   scan tracks it only where the content sets either.
/// - The hyphen the line draws, after a soft hyphen.
/// - The room of a seam before the boundary, which the line gives back.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct LineEndState {
    /// The line padding of the text holding the last cluster.
    padding: LayoutUnit,
    /// The last cluster's first character, its text's
    /// `hanging-punctuation` and its advance, where its text hangs
    /// punctuation.
    last: Option<(char, HangingPunctuation, LayoutUnit)>,
    /// A box edge that takes room stands after the last cluster, so nothing
    /// before the edge is at a line's end.
    blocked: bool,
    /// The width of the hyphen a line ending at the boundary owes, snapped
    /// to the grid.
    ///
    /// It is set where the cluster before is a soft hyphen a line may break
    /// after.
    pub(super) hyphen_owed: Option<LayoutUnit>,
    /// The room the last seam put after the cluster before the boundary.
    pub(super) seam_given: InlineLayoutUnit,
}

/// A mark that hangs where a line ends after it: its advance, and the
/// line-edge flags that say how it hangs.
#[derive(Copy, Clone, Debug)]
struct HangingMark {
    advance: LayoutUnit,
    flags: LineEdgeFlags,
}

/// The margin boxes of the floats anchored at a boundary, as the intrinsic
/// sizes count them.
///
/// `sum` is all of them, which a max-content line sets beside it. `widest`
/// is the widest, which no narrower line can hold.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct AnchoredFloats {
    pub(super) sum: InlineLayoutUnit,
    pub(super) widest: InlineLayoutUnit,
}

impl LineEndState {
    /// Works out what a line starting or ending at boundary `at` pays or
    /// gains there, and takes the hyphen owed.
    ///
    /// A line pays only where it may start or end at `at`.
    /// `starts_paragraph` says a paragraph starts there. The amounts are:
    /// - the edges of the cloned boxes open across a break;
    /// - the line padding of the innermost text at each end;
    /// - how far a ruby column overhangs the text beside it;
    /// - the hyphen a line ending after a soft hyphen draws;
    /// - a mark that hangs.
    ///
    /// `room` is what the items at `at` take. The cluster after `at`, in the
    /// segment `walk` holds, adds `step`.
    ///
    /// Returns two things:
    /// - What the intrinsic sizes pay there: the cloned edges and the
    ///   padding, as the lines they stand for would, less a mark that hangs
    ///   there (CSS Text 4, "hanging glyphs"). Less a seam's room that a
    ///   line ending here gives back too. The breaker's line-end piece gives
    ///   that back, so no cost holds it.
    /// - The line-edge cost, where there is one.
    pub(super) fn cost(
        &mut self,
        scan: &Scan<'_>,
        at: ClusterId,
        starts_paragraph: bool,
        room: &BoundaryRoom,
        walk: &ScanWalk<'_>,
        step: InlineLayoutUnit,
    ) -> (LinePay, Option<LineEdgeCost>) {
        let (end, text_start) = (scan.end, ClusterId::new(0));
        let clusters = &scan.input.analysis.clusters;
        let breaks = !room.inside_column && is_break_boundary(clusters, at);
        let can_start = at < end && (breaks || starts_paragraph);
        let can_end = at > text_start && (breaks || at == end);
        // The edges of the cloned boxes open across a break.
        let mut across = if room.across.is_empty() || at == end || !breaks {
            EdgeAmounts::default()
        } else {
            room.across
        };
        // The line padding of the innermost text at each end.
        if scan.has_padding {
            if can_start {
                across.start += InlineLayoutUnit::from_layout(walk.facts.padding);
            }
            if can_end {
                across.end += InlineLayoutUnit::from_layout(self.padding);
            }
        }
        // A line starting or ending beside a ruby column has no text for it
        // to overhang, so it pays the overhang.
        if can_start {
            across.start += room.overhang.start;
        }
        if can_end {
            across.end += room.overhang.end;
        }
        // A line ending here after a soft hyphen draws a hyphen and pays for
        // it, as Chrome's breaker adds it to its last item. This happens at
        // a break only, never at the text's end.
        let hyphen = self.hyphen_owed.take().filter(|_| breaks && at < end);
        if let Some(width) = hyphen {
            across.end += InlineLayoutUnit::from_layout(width);
        }
        let mut edge = LineEdgeCost {
            at,
            end: across.end.to_layout(),
            start: across.start.to_layout(),
            flex: LayoutUnit::ZERO,
            flags: LineEdgeFlags::NONE,
        };
        if hyphen.is_some() {
            edge.flags.insert(LineEdgeFlags::HYPHEN);
        }
        let mut min_end_hang = InlineLayoutUnit::ZERO;
        if scan.has_hanging {
            // `first`: the block's first cluster, with no room before it,
            // gives its advance back to the block's first line where its
            // text's `hanging-punctuation` says so.
            if at == text_start
                && can_start
                && room.rest == InlineLayoutUnit::ZERO
                && step.to_layout() > LayoutUnit::ZERO
                && walk.item().is_some()
                && clusters
                    .first_char(scan.source, at)
                    .is_some_and(|ch| hangs_first(walk.facts.hanging, ch))
            {
                edge.start = edge.start - step.to_layout();
                edge.flags.insert(LineEdgeFlags::FIRST_LINE_ONLY);
                across.start = across.start - step;
            }
            // At the end, the last cluster that doesn't hang, with no room
            // after it, is a flex. The breaker takes it always or where the
            // line needs it. The intrinsic sizes leave out one that always
            // hangs, and min-content one that hangs where it must.
            if can_end && let Some(mark) = self.hanging_mark(room.leading_closes, at == end) {
                edge.flex = mark.advance;
                edge.flags.insert(mark.flags);
                let advance = InlineLayoutUnit::from_layout(mark.advance);
                if mark.flags.contains(LineEdgeFlags::HANG_FORCED) {
                    across.end = across.end - advance;
                } else {
                    min_end_hang = advance;
                }
            }
        }
        if can_end {
            across.end = across.end - self.seam_given;
        }
        // A hyphen of no width (an empty `hyphenate-character`) still marks
        // the line that ends here.
        let has_cost = edge.start != LayoutUnit::ZERO
            || edge.end != LayoutUnit::ZERO
            || edge.flex != LayoutUnit::ZERO
            || edge.flags.contains(LineEdgeFlags::HYPHEN);
        let pay = LinePay {
            edges: across,
            min_end_hang,
        };
        (pay, has_cost.then_some(edge))
    }

    /// Records a cluster that doesn't hang, in the segment `walk` holds,
    /// adding `step`.
    ///
    /// `first` reads its first character. It is called only where a mark
    /// may hang.
    pub(super) fn content(
        &mut self,
        walk: &ScanWalk<'_>,
        step: InlineLayoutUnit,
        first: impl FnOnce() -> Option<char>,
    ) {
        let facts = walk.facts;
        self.padding = facts.padding;
        self.blocked = false;
        self.last = match walk.item() {
            Some(_) if facts.hanging != HangingPunctuation::NONE => {
                first().map(|ch| (ch, facts.hanging, step.to_layout()))
            }
            _ => None,
        };
    }

    /// Records that items at a boundary take `room` along the line.
    ///
    /// Past room-taking items, the cluster before them is not at a line's
    /// end.
    pub(super) fn pass_room(&mut self, room: InlineLayoutUnit) {
        if room != InlineLayoutUnit::ZERO {
            self.blocked = true;
        }
    }

    /// Returns the mark hanging past the end of a line ending at a boundary.
    ///
    /// The boundary's leading closes take `close`. `at_text_end` says the
    /// boundary is the text's end. The mark is the last cluster that doesn't
    /// hang, where nothing that takes room stands after it.
    fn hanging_mark(&self, close: InlineLayoutUnit, at_text_end: bool) -> Option<HangingMark> {
        if self.blocked || close != InlineLayoutUnit::ZERO {
            return None;
        }
        let (ch, hanging, advance) = self.last?;
        // Spacing can take a mark's advance below zero; then nothing hangs.
        if advance <= LayoutUnit::ZERO {
            return None;
        }
        let flags = hangs_end(hanging, ch, at_text_end)?;
        Some(HangingMark { advance, flags })
    }
}

/// Returns whether a line may start or end at boundary `at`, past the
/// text's start.
///
/// It may where the cluster before offers a break or an emergency break, or
/// ends its paragraph.
fn is_break_boundary(clusters: &Clusters, at: ClusterId) -> bool {
    let Some(before) = at.get().checked_sub(1) else {
        return false;
    };
    clusters.attrs(ClusterId::new(before)).is_some_and(|attrs| {
        attrs.has(ClusterAttrs::BREAK_AFTER)
            || attrs.has(ClusterAttrs::EMERGENCY_AFTER)
            || attrs.class().is_forced_break()
    })
}
