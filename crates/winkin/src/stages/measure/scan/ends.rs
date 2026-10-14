//! What a line starting or ending at a boundary pays there, and the floats
//! anchored at a boundary.

use super::{BoundaryRoom, EdgeAmounts, LinePay, Scan, ScanWalk};
use crate::data::Id;
use crate::stages::analysis::{ClusterAttrs, ClusterId, Clusters};
use crate::stages::measure::hang::{hangs_end, hangs_first};
use crate::stages::measure::{LineEdgeCost, LineEdgeFlags};
use crate::style::HangingPunctuation;
use crate::unit::{InlineLayoutUnit, LayoutUnit};

/// What a line ending at the scan's next boundary would end with: the last
/// cluster that doesn't hang, the hyphen owed, and the seam's room.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct LineEndState {
    /// The line padding of the last cluster's text.
    padding: LayoutUnit,
    /// The last cluster's first character, `hanging-punctuation` and advance,
    /// where its text hangs punctuation.
    last: Option<(char, HangingPunctuation, LayoutUnit)>,
    /// A box edge that takes room stands after the last cluster, so that
    /// cluster is not at a line's end.
    blocked: bool,
    /// The width of the hyphen a line ending at the boundary owes, snapped to
    /// the grid. Set after a breakable soft hyphen.
    pub(super) hyphen_owed: Option<LayoutUnit>,
    /// The room the last seam put before the boundary.
    pub(super) seam_given: InlineLayoutUnit,
}

/// A mark that hangs at a line's end: its advance and how it hangs.
#[derive(Copy, Clone, Debug)]
struct HangingMark {
    advance: LayoutUnit,
    flags: LineEdgeFlags,
}

/// The margin boxes of the floats anchored at a boundary: `sum`, which a
/// max-content line sets beside it, and `widest`, below which no line goes.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct AnchoredFloats {
    pub(super) sum: InlineLayoutUnit,
    pub(super) widest: InlineLayoutUnit,
}

impl LineEndState {
    /// Returns what a line starting or ending at boundary `at` pays there, for
    /// the intrinsic sizes and as a line-edge cost, and takes the hyphen owed.
    ///
    /// A line pays only where it may start or end at `at`. It pays the cloned
    /// edges open across a break, the line padding at each end, a ruby
    /// column's overhang, the hyphen after a soft hyphen, and gives back a mark
    /// that hangs. `room` is what the items at `at` take; the cluster after
    /// `at` adds `step`.
    ///
    /// The intrinsic sizes also leave out a hanging mark (CSS Text 4, "hanging
    /// glyphs") and the room of a seam a line ending here gives back. The cost
    /// never holds that room: the breaker's line-end piece gives it back.
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
        // A line beside a ruby column has no text for it to overhang, so pays it.
        if can_start {
            across.start += room.overhang.start;
        }
        if can_end {
            across.end += room.overhang.end;
        }
        // A line ending at a break after a soft hyphen pays for the hyphen, as
        // Chrome's breaker adds it to its last item. Never at the text's end.
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
            // `first`: the block's first cluster, with no room before it, gives its
            // advance back to the first line.
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
            // The last cluster that doesn't hang, with no room after it, is a flex.
            // The intrinsic sizes leave out one that always hangs; min-content also
            // leaves out one that hangs where it must.
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
        // A zero-width hyphen (empty `hyphenate-character`) still marks the line.
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

    /// Records a cluster that doesn't hang, of advance `step`, in the segment
    /// `walk` holds. `first` reads its first character.
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

    /// Takes the hyphen owed at boundary `at` in plain text: [`cost`](Self::cost)'s
    /// answer where nothing else is paid. Returns `None` where none is owed.
    #[inline]
    pub(super) fn take_plain_hyphen(&mut self, at: ClusterId) -> Option<(LinePay, LineEdgeCost)> {
        let width = self.hyphen_owed.take()?;
        let pay = LinePay {
            edges: EdgeAmounts {
                start: InlineLayoutUnit::ZERO,
                end: InlineLayoutUnit::from_layout(width),
            },
            min_end_hang: InlineLayoutUnit::ZERO,
        };
        let edge = LineEdgeCost {
            at,
            end: width,
            start: LayoutUnit::ZERO,
            flex: LayoutUnit::ZERO,
            flags: LineEdgeFlags::HYPHEN,
        };
        Some((pay, edge))
    }

    /// Records that items at a boundary take `room`, so the cluster before
    /// them is not at a line's end.
    pub(super) fn pass_room(&mut self, room: InlineLayoutUnit) {
        if room != InlineLayoutUnit::ZERO {
            self.blocked = true;
        }
    }

    /// Returns the last cluster that doesn't hang, as the mark hanging past a
    /// line ending at a boundary, where nothing taking room follows it.
    ///
    /// `close` is the boundary's leading closes. `at_text_end` says the
    /// boundary is the text's end.
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

/// Returns whether a line may start or end at boundary `at`: the cluster
/// before offers a break or an emergency break, or ends its paragraph.
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
