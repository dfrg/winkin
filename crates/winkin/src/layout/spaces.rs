//! The spaces decoration bars leave out, as `text-decoration-skip-spaces`
//! sets them.
//!
//! The values:
//! - `start` and `end` leave out the spaces at the line's logical ends,
//!   across runs and boxes, up to the first and from the last character
//!   that is neither a space nor invisible. An atomic inline counts as such
//!   a character.
//! - `all` also leaves out each run of spaces between two characters of one
//!   text run. Spaces at a text run's ends inside the line keep their bar.
//! - `all` also leaves the positive letter-spacing off the right end of the
//!   text run that ends the line's content on the right.
//!
//! The run's own style decides, whichever box draws the bar.
//!
//! Each left-out space comes off the side of the bar it lies on. In a
//! right-to-left text run, Chrome 155 instead takes the width of the line's
//! leading spaces off the run's left and that of its trailing ones off its
//! right, cutting into the letters where the two differ.

use core::ops::Range;

use super::Line;
use crate::data::{Id, Table};
use crate::stages::analysis::{ClusterId, Clusters};
use crate::stages::content::{ContentFlags, FixedSpacing, TextFlags};
use crate::stages::fragments::{
    ClusterWalk, FragmentItem, FragmentItemFlags, FragmentItemId, FragmentItemKind,
};
use crate::stages::lines::InlineExtents;
use crate::unit::InlineLayoutUnit;
use crate::work;

/// Where a line's leading and trailing spaces end, and what `all` reads.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineSpaces {
    /// The first cluster past the line's leading spaces: its end on a line
    /// of spaces only.
    lead: ClusterId,
    /// The first of the line's trailing spaces: its start on a line of
    /// spaces only.
    trail: ClusterId,
    /// Whether some text sets `all`.
    all: bool,
    /// The text item that ends the line's content on the right, where some
    /// text sets `all`.
    right: Option<FragmentItemId>,
}

impl LineSpaces {
    /// Finds the leading and trailing spaces of `line`, whose own items are
    /// `own` of `items`; none where nothing on it is `decorated`.
    ///
    /// A line that starts and ends with a character reads one cluster at
    /// each end.
    #[inline]
    pub(super) fn new(
        line: &Line<'_>,
        items: &Table<FragmentItemId, FragmentItem>,
        own: Range<FragmentItemId>,
        decorated: bool,
    ) -> Self {
        // Bars are asked for only on a decorated line, so these are never
        // read.
        if !decorated {
            return Self {
                lead: ClusterId::new(0),
                trail: ClusterId::new(0),
                all: false,
                right: None,
            };
        }
        let layout = line.layout();
        let range = line.record().clusters();
        let clusters = &layout.analysis().clusters;
        let mut lead = range.start;
        while lead < range.end && is_edge(clusters, lead) {
            work::step();
            lead = ClusterId::new(lead.get() + 1);
        }
        let mut trail = range.end;
        if lead == range.end {
            trail = range.start;
        } else {
            while trail > lead && is_edge(clusters, ClusterId::new(trail.get() - 1)) {
                work::step();
                trail = ClusterId::new(trail.get() - 1);
            }
        }
        let all = layout
            .content()
            .flags
            .contains(ContentFlags::SKIPS_ALL_SPACES);
        let right = if all {
            let first = own.start;
            let own = items.get_slice(own).unwrap_or_default();
            right_content(own, first, lead, trail)
        } else {
            None
        };
        Self {
            lead,
            trail,
            all,
            right,
        }
    }

    /// Returns what the bars over text item `item`, `id`, on `line` leave
    /// out, or `None` where they span it whole.
    #[inline]
    pub(super) fn skipped(
        &self,
        line: &Line<'_>,
        id: FragmentItemId,
        item: &FragmentItem,
    ) -> Option<SkippedSpaces> {
        let clusters = item.clusters();
        if clusters.start >= self.lead && clusters.end <= self.trail && !self.all {
            return None;
        }
        self.item_skips(line, id, item)
    }

    /// Returns what the bars over `item` leave out, by its style.
    #[inline(never)]
    fn item_skips(
        &self,
        line: &Line<'_>,
        id: FragmentItemId,
        item: &FragmentItem,
    ) -> Option<SkippedSpaces> {
        let content = line.layout().content();
        let facts = &content.facts;
        let text = facts.text(content.nodes.text_facts(item.node, line.variant()));
        let clusters = item.clusters();
        let lead = if text.has(TextFlags::SKIPS_START_SPACES) {
            self.lead.clamp(clusters.start, clusters.end)
        } else {
            clusters.start
        };
        let trail = if text.has(TextFlags::SKIPS_END_SPACES) {
            self.trail.clamp(clusters.start, clusters.end)
        } else {
            clusters.end
        };
        let interior = text.has(TextFlags::SKIPS_ALL_SPACES);
        if lead == clusters.start && trail == clusters.end && !interior {
            return None;
        }
        // The letter-spacing after the line's last letter on the right, left
        // off under `all`; a negative one stays.
        let mut trim = InlineLayoutUnit::ZERO;
        if interior && self.right == Some(id) {
            let shaping = facts.shaping(text.shaping);
            let letter = InlineLayoutUnit::from_text(shaping.letter);
            let scale = line.scale();
            trim = if scale == 1.0 || shaping.fixed.contains(FixedSpacing::LETTER) {
                letter
            } else {
                letter.times(scale)
            }
            .max(InlineLayoutUnit::ZERO);
        }
        Some(SkippedSpaces {
            lead,
            trail,
            interior,
            trim,
        })
    }
}

/// Returns whether `cluster` may stand among a line's leading or trailing
/// spaces: a space, or a cluster that draws nothing.
fn is_edge(clusters: &Clusters, cluster: ClusterId) -> bool {
    clusters
        .class(cluster)
        .is_some_and(|class| class.is_decoration_space() || class.is_invisible())
}

/// Returns the text item that ends the line's content on the right: the
/// rightmost leaf holding a cluster between the leading and trailing spaces,
/// where it is text.
fn right_content(
    own: &[FragmentItem],
    first: FragmentItemId,
    lead: ClusterId,
    trail: ClusterId,
) -> Option<FragmentItemId> {
    for (at, item) in own.iter().enumerate().rev() {
        work::step();
        if item.flags.contains(FragmentItemFlags::HIDDEN) {
            continue;
        }
        match item.kind() {
            FragmentItemKind::Atomic => return None,
            FragmentItemKind::Text => {
                let clusters = item.clusters();
                if clusters.start < trail && clusters.end > lead {
                    return Some(FragmentItemId::new(first.get() + at));
                }
            }
            _ => {}
        }
    }
    None
}

/// What the bars over one text item leave out.
#[derive(Copy, Clone, Debug)]
pub(super) struct SkippedSpaces {
    /// The clusters before it lead the line.
    lead: ClusterId,
    /// The clusters from it trail the line.
    trail: ClusterId,
    /// Whether a run of spaces between two of its characters is left out.
    interior: bool,
    /// The letter-spacing left off its rightmost bar's right end.
    trim: InlineLayoutUnit,
}

/// The pieces of one bar over a text item, between the spaces it leaves
/// out, in logical order.
///
/// It keeps where its walk of the item's clusters stands, not the walk, so
/// a line's paint walk stays small; each piece takes up the walk again.
#[derive(Copy, Clone)]
pub(super) struct BarPieces<'a> {
    line: Line<'a>,
    item: &'a FragmentItem,
    skipped: SkippedSpaces,
    /// Where the walk stands, or `None` before it starts.
    at: Option<(usize, InlineLayoutUnit)>,
    /// The piece being gathered: its left and right.
    piece: Option<(InlineLayoutUnit, InlineLayoutUnit)>,
    /// The spaces after the piece's last character, which a later
    /// character of the item leaves out.
    gap: Option<(InlineLayoutUnit, InlineLayoutUnit)>,
    /// Whether a piece has been handed out.
    started: bool,
}

impl<'a> BarPieces<'a> {
    /// Splits the bars over text item `item` of `line` as `skipped` says.
    pub(super) fn new(line: Line<'a>, item: &'a FragmentItem, skipped: SkippedSpaces) -> Self {
        Self {
            line,
            item,
            skipped,
            at: None,
            piece: None,
            gap: None,
            started: false,
        }
    }

    /// Returns `piece` as extents, trimmed where it is the item's rightmost;
    /// `last` says no piece follows it.
    fn hand_out(
        &mut self,
        (left, right): (InlineLayoutUnit, InlineLayoutUnit),
        last: bool,
    ) -> Option<InlineExtents> {
        let rightmost = if self.item.level.is_rtl() {
            !self.started
        } else {
            last
        };
        self.started = true;
        let right = if rightmost {
            (right - self.skipped.trim).max(left)
        } else {
            right
        };
        (right > left).then(|| InlineExtents::from_exact(left, right))
    }

    /// Returns the next piece from `walk`, which stands where this left off.
    fn next_piece(&mut self, walk: &mut ClusterWalk<'a>) -> Option<InlineExtents> {
        let clusters = &self.line.layout().analysis().clusters;
        loop {
            let Some((cluster, left, advance)) = walk.next() else {
                // Spaces ending the item inside the line keep their bar.
                let gap = self.gap.take();
                let piece = self.piece.take()?;
                let piece = gap.map_or(piece, |gap| join(Some(piece), gap));
                if let Some(extents) = self.hand_out(piece, true) {
                    return Some(extents);
                }
                continue;
            };
            work::step();
            let extent = (left, left + advance);
            let skipped = &self.skipped;
            if cluster < skipped.lead || cluster >= skipped.trail {
                // Past the leading spaces nothing is gathered, so this
                // ends the last piece at the trailing spaces.
                self.gap = None;
                if let Some(piece) = self.piece.take()
                    && let Some(extents) = self.hand_out(piece, true)
                {
                    return Some(extents);
                }
                continue;
            }
            let Some(class) = clusters.class(cluster) else {
                continue;
            };
            if skipped.interior && self.piece.is_some() {
                if class.is_decoration_space() || (self.gap.is_some() && class.is_invisible()) {
                    self.gap = Some(join(self.gap, extent));
                    continue;
                }
                // A character after spaces inside the item: the spaces are
                // left out, and a new piece starts here.
                if self.gap.take().is_some() {
                    if let Some(piece) = self.piece.replace(extent)
                        && let Some(extents) = self.hand_out(piece, false)
                    {
                        return Some(extents);
                    }
                    continue;
                }
            }
            self.piece = Some(join(self.piece, extent));
        }
    }
}

/// Returns `extent` joined to `to`, or `extent` alone.
fn join(
    to: Option<(InlineLayoutUnit, InlineLayoutUnit)>,
    extent: (InlineLayoutUnit, InlineLayoutUnit),
) -> (InlineLayoutUnit, InlineLayoutUnit) {
    to.map_or(extent, |(left, right)| {
        (left.min(extent.0), right.max(extent.1))
    })
}

impl Iterator for BarPieces<'_> {
    type Item = InlineExtents;

    fn next(&mut self) -> Option<InlineExtents> {
        let mut walk = self.line.cluster_walk(self.item);
        if let Some(at) = self.at {
            walk = walk.resumed(at);
        }
        let piece = self.next_piece(&mut walk);
        self.at = Some(walk.position());
        piece
    }
}
