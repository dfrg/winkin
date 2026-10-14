//! Pen positions at cluster boundaries, and each cluster's width on a line.

use core::ops::Range;

use super::{AutospaceRules, edges};
use crate::data::Id;
use crate::stages::LineStages;
use crate::stages::analysis::{ClusterId, Paragraph};
use crate::stages::content::{ContentFlags, ItemId};
use crate::unit::InlineLayoutUnit;

/// Pen positions and cluster advances in one line's variant.
impl LineStages<'_> {
    /// Returns the exact pen position for drawing `cluster`'s glyphs.
    ///
    /// This is the prefix entry, which includes the leading closing edges,
    /// plus the room of the items after them: opening edges and empty boxes,
    /// recomputed through [`edge_room`](super::edge_room). `from` is the
    /// first item at the boundary, or else the item containing `cluster`.
    /// Past the last cluster, returns the prefix's end.
    ///
    /// Where no box has an edge, reads one prefix entry and makes no call.
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

    /// Returns the exact pen position after the last glyph before boundary
    /// `at`.
    ///
    /// This is the prefix entry less the leading closing edges, so the
    /// clusters `a..b` of one item take `pen_end(b) − pen(a)`. It also leaves
    /// out an autospace seam's room drawn by the text after `at`
    /// ([`AutospaceRules::room_before`]). `paragraph` contains the cluster
    /// before `at`; with `None`, no seam is looked for. `from` is as for
    /// [`pen`](Self::pen).
    ///
    /// Where no box has an edge and no seam can have room, reads one prefix
    /// entry and makes no call.
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

    /// Returns the exact width `cluster` adds along the line.
    ///
    /// That is `P[c + 1] − P[c]` less the box edges and empty ruby columns
    /// on either side: its advance and the spacing after it. Past the last
    /// cluster, returns zero. A hanging cluster hangs this much; box edges
    /// among hanging clusters stay content.
    ///
    /// With no box edges and no ruby, reads two prefix entries; otherwise
    /// searches for the items at the cluster. A walk calls
    /// [`cluster_advance_near`](Self::cluster_advance_near) instead.
    pub(crate) fn cluster_advance(&self, cluster: ClusterId) -> InlineLayoutUnit {
        self.cluster_advance_near(cluster, &mut None)
    }

    /// Returns [`cluster_advance`](Self::cluster_advance) for a walk over
    /// clusters in either direction.
    ///
    /// `near` is the walk's item cursor, `None` until first needed. Where
    /// some box has an edge it moves to the first item at `cluster`, so a
    /// walk costs one search and a step per item passed.
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

    /// Returns [`cluster_advance_near`](Self::cluster_advance_near) where some
    /// box has an edge or some text is ruby.
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

    /// Returns the advance of `clusters`, less the boundary room `rest`
    /// before them and `closes` after them.
    ///
    /// Also subtracts the room of empty ruby columns at either end.
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

    /// Subtracts from `advance` the room of empty ruby columns at either end
    /// of `clusters`. The closing end counts only at the end of text that
    /// ends on a line.
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
