//! The prefix advances and the line-edge cost table.

use alloc::vec::Vec;
use core::ops::Range;

use crate::data::{Id, SortedTable};
use crate::stages::analysis::ClusterId;
use crate::unit::InlineLayoutUnit;
#[cfg(test)]
use crate::unit::LayoutUnit;

use super::LineEdgeCost;

/// The running sum of the width up to each cluster boundary, in 48.16
/// `InlineLayoutUnit`s.
///
/// Each sum includes the closing box edges that lead its boundary, and the
/// text's end has a sum too, so a line's width is one subtraction. Fitting
/// reads positions rounded up to the 26.6 grid from the paragraph's start;
/// painting reads exact differences.
pub(crate) struct PrefixAdvances {
    /// One more entry than clusters: 8 bytes per cluster.
    pub(super) sums: Vec<InlineLayoutUnit>,
}

impl PrefixAdvances {
    pub(super) const fn new() -> Self {
        Self { sums: Vec::new() }
    }

    pub(super) fn clear(&mut self) {
        self.sums.clear();
    }

    /// Returns whether it holds no sum, as for a variant the build doesn't use.
    pub(super) fn is_empty(&self) -> bool {
        self.sums.is_empty()
    }

    /// Returns the exact sums at the start of each of `clusters`, for readers
    /// such as painting that walk many in a row.
    ///
    /// Empty where the range is out of bounds.
    #[inline]
    pub(crate) fn positions(&self, clusters: Range<ClusterId>) -> &[InlineLayoutUnit] {
        self.sums
            .get(clusters.start.get()..clusters.end.get())
            .unwrap_or_default()
    }

    /// Returns the exact sum at boundary `at`, or the end's sum past the end.
    #[inline]
    pub(crate) fn get(&self, at: ClusterId) -> InlineLayoutUnit {
        match self.sums.get(at.get()) {
            Some(&position) => position,
            None => self.sums.last().copied().unwrap_or_default(),
        }
    }

    /// Returns the fitting position of boundary `at` in the paragraph that
    /// starts at `origin`.
    ///
    /// The distance from the paragraph's start rounds up to the 26.6 grid, as
    /// Blink's cached character positions do (`ToCeil<LayoutUnit>`), but
    /// stays 48.16 so a long paragraph fits exactly. Blink's positions
    /// restart at each item; restarting at each paragraph makes a
    /// paragraph's first item round as there.
    #[inline]
    pub(crate) fn fit_position(&self, origin: ClusterId, at: ClusterId) -> InlineLayoutUnit {
        (self.get(at) - self.get(origin)).ceil_to_grid()
    }

    /// Returns the fitting width from `from` to `to` in the paragraph that
    /// starts at `origin`: `⌈P[to] − P[origin]⌉ − ⌈P[from] − P[origin]⌉`.
    ///
    /// Includes the boxes opening at `from` and closing at `to`. Saturates.
    #[inline]
    #[cfg(test)]
    pub(super) fn fit_width(
        &self,
        origin: ClusterId,
        from: ClusterId,
        to: ClusterId,
    ) -> LayoutUnit {
        (self.fit_position(origin, to) - self.fit_position(origin, from)).to_layout()
    }

    /// Returns the exact advance `P[to] − P[from]`.
    #[inline]
    pub(crate) fn advance(&self, from: ClusterId, to: ClusterId) -> InlineLayoutUnit {
        self.get(to) - self.get(from)
    }
}

/// The line-edge costs: what a line pays or gains for starting or ending at
/// a boundary.
///
/// One [`LineEdgeCost`] per boundary that has one, sorted by boundary. Empty
/// for plain text. The breaker reads it only in paragraphs flagged
/// [`HAS_EDGE_COSTS`](super::MeasureFlags::HAS_EDGE_COSTS).
pub(super) type LineEdgeCosts = SortedTable<LineEdgeCost>;
