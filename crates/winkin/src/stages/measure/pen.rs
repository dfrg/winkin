//! Where the pen stands at a cluster, and what a cluster adds along the line.

use core::ops::Range;

use super::{AutospaceRules, edges};
use crate::data::Id;
use crate::stages::LineStages;
use crate::stages::analysis::{ClusterId, Paragraph};
use crate::stages::content::{ContentFlags, ItemId};
use crate::unit::InlineLayoutUnit;

/// What a line reads of the measurements in its variant: where the pen
/// stands, and what a cluster adds.
impl LineStages<'_> {
    /// Returns where the pen stands to draw `cluster`'s glyphs, exactly.
    ///
    /// It is the boundary's position in the prefix, which comes after the
    /// leading closing edges there, plus the room of every item after them:
    /// the opening edges, and the whole of an empty box. So a box's first
    /// glyphs start inside its opening edge, and the glyphs after a box
    /// start past its closing edge.
    ///
    /// The room comes from the items at the boundary, through the function
    /// the prefix was built with ([`edge_room`](super::edge_room)), so it
    /// isn't stored twice. `from` is the first item at the boundary, or else
    /// the one holding `cluster`. It is a walk's item cursor, so no item is
    /// searched for. Past the last cluster, it returns the end's position.
    ///
    /// Where no box has an edge it reads one prefix entry and makes no call.
    #[inline]
    pub(crate) fn pen(&self, cluster: ClusterId, from: ItemId) -> InlineLayoutUnit {
        let (content, analysis, measured) = (self.content, self.analysis, self.measured);
        let edged = content.flags.contains(ContentFlags::BOXES_WITH_EDGES);
        let room = if edged && cluster.get() < analysis.clusters.len() {
            edges::boundary_room(content, analysis, measured.initial_letter(), cluster, from).1
        } else {
            InlineLayoutUnit::ZERO
        };
        measured.prefix.get(cluster) + room
    }

    /// Returns where the pen stands after the last glyph before boundary
    /// `at`, exactly.
    ///
    /// It is the boundary's position in the prefix, less the leading closing
    /// edges there. Those edges end the box or the line before, not the
    /// cluster. This is the other end of [`pen`](Self::pen): the clusters
    /// from `a` to `b` of one item take `pen_end(b) − pen(a)` along the line.
    /// At the text's end, every item is the last line's, so it returns the
    /// end of the last glyph. `from` is as `pen` takes it.
    ///
    /// An autospace seam's room that the text after `at` draws
    /// ([`AutospaceRules::room_before`]) is left out too. `paragraph` holds
    /// the cluster before `at`; where it is `None`, no seam is looked for.
    ///
    /// Where no box has an edge and no seam can draw room before the text
    /// after it, it reads one prefix entry and makes no call.
    #[inline]
    pub(crate) fn pen_end(
        &self,
        at: ClusterId,
        from: ItemId,
        paragraph: Option<&Paragraph>,
    ) -> InlineLayoutUnit {
        let (content, analysis, measured) = (self.content, self.analysis, self.measured);
        let edged = content.flags.contains(ContentFlags::BOXES_WITH_EDGES);
        let closes = if edged && at.get() <= analysis.clusters.len() {
            edges::boundary_room(content, analysis, measured.initial_letter(), at, from).0
        } else {
            InlineLayoutUnit::ZERO
        };
        let seam = paragraph.map_or(InlineLayoutUnit::ZERO, |paragraph| {
            InlineLayoutUnit::from_text(
                AutospaceRules::new(content).room_before(self, paragraph, at),
            )
        });
        measured.prefix.get(at) - closes - seam
    }

    /// Returns what `cluster` itself adds along the line, exactly.
    ///
    /// That is its advance and the spacing after it, without the box edges
    /// either side: `P[c + 1] − P[c]`, less the opening edges before it and
    /// the closing edges after it. Past the last cluster, it returns zero.
    ///
    /// The breaker takes this off a line for each cluster that hangs at its
    /// end, as the intrinsic sizes do. Where a box's edge stands among the
    /// hanging clusters, the edge stays content and only the clusters' own
    /// advances hang.
    ///
    /// Cost: where no box has an edge, it reads two prefix entries. Where
    /// one has, it seeks the items at the cluster's start by halving. A walk
    /// over clusters asks [`cluster_advance_near`](Self::cluster_advance_near)
    /// instead.
    pub(crate) fn cluster_advance(&self, cluster: ClusterId) -> InlineLayoutUnit {
        self.cluster_advance_near(cluster, &mut None)
    }

    /// Returns what `cluster` itself adds along the line, as
    /// [`cluster_advance`](Self::cluster_advance) does, for a walk over
    /// clusters, forward or back.
    ///
    /// `near` is the first item at a boundary the walk stood at, or `None`
    /// before it has needed one. Only where some box has an edge does it
    /// move, to the first item at `cluster`'s start: walked to from where it
    /// was, or sought the first time. So a walk costs one search, and a step
    /// per item it passes.
    ///
    /// Content with no box edge and no ruby reads the prefix sums inline;
    /// the rest is out of line.
    #[inline]
    pub(crate) fn cluster_advance_near(
        &self,
        cluster: ClusterId,
        near: &mut Option<ItemId>,
    ) -> InlineLayoutUnit {
        let flags = self.content.flags;
        if flags.contains(ContentFlags::BOXES_WITH_EDGES) || flags.contains(ContentFlags::RUBY) {
            return self.edged_cluster_advance(cluster, near);
        }
        if cluster.get() >= self.analysis.clusters.len() {
            return InlineLayoutUnit::ZERO;
        }
        let after = ClusterId::new(cluster.get() + 1);
        self.measured.prefix.advance(cluster, after)
    }

    /// Returns what `cluster` adds along the line where some box has an
    /// edge or some text is ruby, as
    /// [`cluster_advance_near`](Self::cluster_advance_near) does.
    #[inline(never)]
    fn edged_cluster_advance(
        &self,
        cluster: ClusterId,
        near: &mut Option<ItemId>,
    ) -> InlineLayoutUnit {
        let (content, analysis, measured) = (self.content, self.analysis, self.measured);
        let count = analysis.clusters.len();
        if cluster.get() >= count {
            return InlineLayoutUnit::ZERO;
        }
        let after = ClusterId::new(cluster.get() + 1);
        let (rest, closes) = if content.flags.contains(ContentFlags::BOXES_WITH_EDGES) {
            let (letter, items) = (measured.initial_letter(), &analysis.item_clusters);
            let from = match *near {
                Some(item) => items.walk_to(item, cluster),
                None => items.cursor_containing(cluster).id(),
            };
            let to = items.walk_to(from, after);
            *near = Some(from);
            (
                edges::boundary_room(content, analysis, letter, cluster, from).1,
                edges::boundary_room(content, analysis, letter, after, to).0,
            )
        } else {
            (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO)
        };
        self.span_advance(cluster..after, rest, closes)
    }

    /// Returns the advance of the clusters `clusters`, less the boundary
    /// room `rest` before them and `closes` after them.
    ///
    /// An empty ruby column's extra width is boundary room too, though no
    /// item in the caller's walk has an edge for it.
    #[inline]
    pub(crate) fn span_advance(
        &self,
        clusters: Range<ClusterId>,
        rest: InlineLayoutUnit,
        closes: InlineLayoutUnit,
    ) -> InlineLayoutUnit {
        let advance = self.measured.prefix.advance(clusters.start, clusters.end) - rest - closes;
        if !self.content.flags.contains(ContentFlags::RUBY) {
            return advance;
        }
        self.ruby_span(clusters, advance)
    }

    /// Takes the extra width of empty ruby columns at either end off
    /// `advance`.
    #[inline(never)]
    fn ruby_span(&self, clusters: Range<ClusterId>, advance: InlineLayoutUnit) -> InlineLayoutUnit {
        let rubies = self.measured.ruby_columns();
        let opening = InlineLayoutUnit::from_layout(rubies.empty_room(clusters.start));
        let closing = if clusters.end == self.analysis.clusters.end_id()
            && self.analysis.paragraphs.ends_on_a_line()
        {
            InlineLayoutUnit::from_layout(rubies.empty_room(clusters.end))
        } else {
            InlineLayoutUnit::ZERO
        };
        advance - opening - closing
    }
}
