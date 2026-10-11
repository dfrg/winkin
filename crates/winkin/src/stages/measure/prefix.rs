//! Stored prefix measurements.

use alloc::vec::Vec;
use core::ops::Range;

use crate::data::{Id, SortedTable};
use crate::stages::analysis::ClusterId;
use crate::unit::InlineLayoutUnit;
#[cfg(test)]
use crate::unit::LayoutUnit;

use super::LineEdgeCost;

/// The prefix advances: the running sum of the room taken up to each cluster
/// boundary.
///
/// Each sum includes the closing box edges that lead the boundary, and the
/// text's end has one too. A line's width is then one subtraction.
///
/// Readers use only these methods, so the representation can change. Fitting
/// reads each position rounded up to layout's grid from its paragraph's
/// start. Painting reads the differences exactly.
pub(crate) struct PrefixAdvances {
    /// One more entry than clusters: 8 bytes a cluster.
    pub(super) sums: Vec<InlineLayoutUnit>,
}

impl PrefixAdvances {
    pub(super) const fn new() -> Self {
        Self { sums: Vec::new() }
    }

    pub(super) fn clear(&mut self) {
        self.sums.clear();
    }

    /// Whether it holds no sum: a variant the build had no use for.
    pub(super) fn is_empty(&self) -> bool {
        self.sums.is_empty()
    }

    /// Returns the exact running sums before each of `clusters`, as
    /// [`get`](Self::get) gives them.
    ///
    /// A reader that walks many in a row, such as a painter drawing a run's
    /// glyphs, takes them from one slice. Empty where the range is not the
    /// text's.
    #[inline]
    pub(crate) fn positions(&self, clusters: Range<ClusterId>) -> &[InlineLayoutUnit] {
        self.sums
            .get(clusters.start.get()..clusters.end.get())
            .unwrap_or_default()
    }

    /// The running sum at boundary `at`, exactly: everything before it, and
    /// the closing edges that lead the items there. Past the end, the end's.
    #[inline]
    pub(crate) fn get(&self, at: ClusterId) -> InlineLayoutUnit {
        match self.sums.get(at.get()) {
            Some(&position) => position,
            None => self.sums.last().copied().unwrap_or_default(),
        }
    }

    /// Where fitting reads boundary `at` in the paragraph starting at
    /// `origin`: its position from the paragraph's start, rounded up to
    /// layout's grid, as Chrome's cached character positions are
    /// (`ToCeil<LayoutUnit>` of the running sum). Still 48.16, so a
    /// paragraph past the grid's range fits as exactly as any other.
    ///
    /// It measures from the paragraph's start because Chrome's positions
    /// start again at each item and this crate's at each paragraph. A
    /// paragraph's first item then rounds exactly as Chrome's does, wherever
    /// the paragraph before ended.
    #[inline]
    pub(crate) fn fit_position(&self, origin: ClusterId, at: ClusterId) -> InlineLayoutUnit {
        (self.get(at) - self.get(origin)).ceil_to_grid()
    }

    /// The fitting width of the clusters from `from` to `to` in the
    /// paragraph starting at `origin`, on layout's grid:
    /// `⌈P[to] − P[origin]⌉ − ⌈P[from] − P[origin]⌉`, the boxes opening at
    /// `from` and closing at `to` included. Saturates past the grid's range.
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

    /// The exact advance from boundary `from` to boundary `to`, which is what
    /// painting reads: `P[to] − P[from]`.
    #[inline]
    pub(crate) fn advance(&self, from: ClusterId, to: ClusterId) -> InlineLayoutUnit {
        self.get(to) - self.get(from)
    }
}

/// The line-edge costs: what a line pays or gains because it starts or ends
/// at a boundary.
///
/// The table holds one [`LineEdgeCost`] per boundary that has any, sorted by
/// boundary, and only where a line can start or end. It is sparse and empty
/// for plain text. The breaker looks only in paragraphs flagged
/// [`HAS_EDGE_COSTS`](super::MeasureFlags::HAS_EDGE_COSTS). A boundary with
/// no entry costs nothing.
pub(super) type LineEdgeCosts = SortedTable<LineEdgeCost>;
