//! Where a position is: snapped to a caret stop, on its line, beside its item, and its caret.
//!
//! - A caret stop is a grapheme cluster boundary. A grapheme split at an item
//!   boundary is one stop, unless a box's edges take room inside it; Chrome's
//!   caret then stops at the edge. A character a text transform changed the
//!   length of is one stop.
//! - The line holds the offset downstream, or ends at it upstream. A position
//!   upstream past everything a line placed, after a wrap's removed white
//!   space or a forced break, is on the next line. Blink finds no item to put
//!   it by there (`CanResolveInlineCaretPositionAfterFragment`).
//! - The item holds the offset. Where two meet at it, the one nearer the
//!   paragraph's level wins, the side Chrome draws the caret on. At equal
//!   levels the one before wins, as Blink's `ComputeInlineCaretPosition`
//!   resolves it, so a caret at a padded box's edge stands outside the
//!   padding. Ruby annotation text is searched where the base text has no
//!   match.
//! - A soft hyphen is not matched to Chrome. Blink's line ends with the
//!   generated hyphen, so the position after the soft hyphen stays on that
//!   line even downstream, and the next line's start is no caret position.
//!   Here downstream is the next line's start, as at any wrap.

use super::{Affinity, Caret, Carets, Position};
use crate::data::{Id, TextOffset};
use crate::layout::Layout;
use crate::layout::{Atomic, CrossExtents, Item, Line, TextRun};
use crate::stages::analysis::{BidiLevel, ClusterId, Clusters};
use crate::stages::content::{BoxFlags, ItemKind};
use crate::stages::lines::{InlineExtents, LineId};
use crate::style::FirstLineVariant;
use crate::work;

/// A leaf of a line as a caret sees it: a text run or an atomic inline.
///
/// Boxes are not leaves. Neither is generated text, an ellipsis or a hyphen;
/// a caret passes through it to the text under it, as Blink's does
/// (`IsGeneratedText`).
#[derive(Copy, Clone)]
pub(super) enum Leaf<'a> {
    /// A text run.
    Text(TextRun<'a>),
    /// An atomic inline.
    Atomic(Atomic<'a>),
}

impl<'a> Leaf<'a> {
    /// Returns the leaf `item` is, where it is one.
    fn from_item(item: Item<'a>) -> Option<Self> {
        match item {
            Item::Text(run) => Some(Self::Text(run)),
            Item::Atomic(atomic) => Some(Self::Atomic(atomic)),
            Item::Generated(_) | Item::Box(_) => None,
        }
    }

    /// Returns where its text starts, as a byte offset into the layout's text.
    pub(super) fn start(&self) -> usize {
        match self {
            Self::Text(run) => run.text_range().start,
            Self::Atomic(atomic) => atomic.text_range().start,
        }
    }

    /// Returns where its text ends.
    pub(super) fn end(&self) -> usize {
        match self {
            Self::Text(run) => run.text_range().end,
            Self::Atomic(atomic) => atomic.text_range().end,
        }
    }

    /// Returns where it starts and ends along its line, from the line box's left.
    pub(super) fn inline(&self) -> InlineExtents {
        match self {
            Self::Text(run) => run.inline(),
            Self::Atomic(atomic) => atomic.inline(),
        }
    }

    /// Returns its bidi level.
    pub(super) fn level(&self) -> BidiLevel {
        match self {
            Self::Text(run) => run.level(),
            Self::Atomic(atomic) => atomic.level(),
        }
    }

    /// Returns whether it reads right to left.
    pub(super) fn is_rtl(&self) -> bool {
        self.level().is_rtl()
    }

    /// Returns the text run it is, where it is one.
    pub(super) fn run(&self) -> Option<&TextRun<'a>> {
        match self {
            Self::Text(run) => Some(run),
            Self::Atomic(_) => None,
        }
    }

    /// Returns whether it is combined text, whose carets run along the line.
    ///
    /// Combined text is set across a vertical line in one em.
    pub(super) fn is_combined(&self) -> bool {
        self.run().is_some_and(TextRun::is_combined)
    }

    /// Returns where its boundary `at` stands across the line, for combined text only.
    ///
    /// It is the start of the cluster starting at `at`, or the end of the one
    /// ending there, from `TextRun::across_places`.
    fn across(&self, layout: &Layout, at: usize) -> Option<f32> {
        let clusters = &layout.analysis().clusters;
        let mut last = None;
        for (cluster, from, to) in self.run()?.across_places()? {
            if clusters.start(cluster).get() == at {
                return Some(from);
            }
            last = Some((cluster, to));
        }
        let (cluster, to) = last?;
        (clusters.range(cluster).end.get() == at).then_some(to)
    }

    /// Returns the boundary of its combined text that `y` across the line hits, with the cluster starting there.
    ///
    /// It is the nearer edge of the cluster there, a tie going to its start,
    /// as Chrome's click in a unit's horizontal text finds it. Past the text
    /// on either side it is the unit's start or end. Chrome's click there
    /// leaves the unit for the text's start, a bug not matched.
    pub(super) fn hit_across(&self, layout: &Layout, y: f32) -> Option<(usize, ClusterId)> {
        let clusters = &layout.analysis().clusters;
        let mut end = None;
        for (cluster, from, to) in self.run()?.across_places()? {
            let range = clusters.range(cluster);
            let start = (range.start.get(), cluster);
            let after = (range.end.get(), ClusterId::new(cluster.get() + 1));
            // The text reads from its under side, the larger `y`. Past its
            // start there, the hit is the start.
            if end.is_none() && y >= from {
                return Some(start);
            }
            if to < y && y <= from {
                return Some(if from - y <= y - to { start } else { after });
            }
            end = Some(after);
        }
        end
    }

    /// Returns the offset at its right edge where `right`, else at its left edge.
    pub(super) fn edge_offset(&self, right: bool) -> usize {
        if right != self.is_rtl() {
            self.end()
        } else {
            self.start()
        }
    }

    /// Returns where the caret at its boundary `at` stands along the line.
    ///
    /// It is at its start's edge, its end's, or between two clusters, which
    /// share a ligature evenly (`TextRun::places`).
    pub(super) fn caret_x(&self, layout: &Layout, at: usize) -> f32 {
        let InlineExtents { left, right } = self.inline();
        let rtl = self.is_rtl();
        let start = self.start();
        // The start's edge or the end's; an atomic inline has only these.
        let edge = |end: bool| if end != rtl { right } else { left };
        let Self::Text(run) = self else {
            return edge(at != start);
        };
        let clusters = &layout.analysis().clusters;
        for (cluster, from, to) in run.places() {
            let range = clusters.range(cluster);
            let (start, end) = (range.start.get(), range.end.get());
            if end == at {
                return if rtl { from.to_px() } else { to.to_px() };
            }
            if start == at {
                return if rtl { to.to_px() } else { from.to_px() };
            }
        }
        edge(at != start)
    }

    /// Returns the caret boundary in it that `x` along the line hits, with the cluster starting there where found.
    ///
    /// It is the nearer edge of the cluster under `x`, a tie going left, as
    /// Blink's `ShapeResult::CaretOffsetForHitTest` finds it. Where no cluster
    /// is under `x`, it is the nearer end, whose cluster is not known.
    ///
    /// Combined text is hit along the line as an atomic inline is, before or
    /// after by its halves. A point across the line picks among its
    /// characters ([`hit_across`](Self::hit_across)).
    pub(super) fn hit(&self, layout: &Layout, x: f32) -> (usize, Option<ClusterId>) {
        let InlineExtents { left, right } = self.inline();
        let middle = left + (right - left) / 2.0;
        let edge = (self.edge_offset(x > middle), None);
        let Self::Text(run) = self else {
            return edge;
        };
        if run.is_combined() {
            return edge;
        }
        let rtl = self.is_rtl();
        let clusters = &layout.analysis().clusters;
        for (cluster, from, to) in run.places() {
            let (from, to) = (from.to_px(), to.to_px());
            if from <= x && x < to {
                let range = clusters.range(cluster);
                let left_half = x - from <= (to - from) / 2.0;
                return if left_half != rtl {
                    (range.start.get(), Some(cluster))
                } else {
                    (range.end.get(), Some(ClusterId::new(cluster.get() + 1)))
                };
            }
        }
        edge
    }

    /// Returns where a caret beside it spans across `line`, from the line box's top.
    ///
    /// In text it spans the font's ascent and descent around the baseline, as
    /// Chrome sizes a caret. Beside an atomic inline it spans the line box,
    /// as Chrome's `ComputeLocalCaretRectByBoxSide` does.
    fn caret_block(&self, line: &Line<'_>) -> CrossExtents {
        match self {
            Self::Text(run) => run.block(),
            Self::Atomic(_) => CrossExtents {
                over: 0.0,
                under: line.metrics().height(),
            },
        }
    }

    /// Returns the caret at its boundary `at` on `line`.
    ///
    /// The caret stands across the line. In combined text it runs along the
    /// line over the unit's em instead.
    fn caret(&self, layout: &Layout, line: &Line<'_>, at: usize) -> Caret {
        let (inline, block) = match self.across(layout, at) {
            Some(across) => (
                self.inline(),
                CrossExtents {
                    over: across,
                    under: across,
                },
            ),
            None => {
                let x = self.caret_x(layout, at);
                (InlineExtents { left: x, right: x }, self.caret_block(line))
            }
        };
        Caret {
            line: line.index(),
            inline,
            block,
            rtl: self.is_rtl(),
        }
    }
}

/// Returns the leaves of `line` in visual order.
///
/// They include the text an ellipsis hides, which keeps its carets and hits
/// as in Chrome.
pub(super) fn leaves<'a>(line: Line<'a>) -> impl Iterator<Item = Leaf<'a>> {
    line.all_items().filter_map(Leaf::from_item)
}

/// Returns the text runs of `line`'s ruby annotations, column by column.
pub(super) fn annotation_leaves<'a>(line: Line<'a>) -> impl Iterator<Item = Leaf<'a>> {
    line.annotations()
        .flat_map(|annotation| annotation.runs())
        .map(Leaf::Text)
}

/// A position and the cluster its offset is in, so no reader searches for the cluster again.
///
/// Snapping keeps the cluster in step with the offset. Functions that need a
/// caret stop take one named `snapped`.
///
/// The cluster is the one [`Clusters::at`] finds for the offset: the one
/// starting there at a boundary, and the end id at or past the text's end.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct ClusteredPosition {
    /// The position.
    pub(super) position: Position,
    /// The cluster holding its offset.
    pub(super) cluster: ClusterId,
}

impl ClusteredPosition {
    /// Returns `position` with the cluster its offset is in, found by search.
    pub(super) fn new(layout: &Layout, position: Position) -> Self {
        let clusters = &layout.analysis().clusters;
        let at = position.offset.min(layout.text().len());
        Self {
            position,
            cluster: clusters.at(TextOffset::new(at)),
        }
    }

    /// Returns the position at `cluster`'s start, with `affinity`.
    pub(super) fn from_cluster(
        clusters: &Clusters,
        cluster: ClusterId,
        affinity: Affinity,
    ) -> Self {
        Self {
            position: Position::new(clusters.start(cluster).get(), affinity),
            cluster,
        }
    }

    /// Returns `position` with its cluster, walked to from `near` a cluster at a time.
    ///
    /// It costs the clusters between `near` and the offset, so `near` is a
    /// cluster the caller holds close by.
    pub(super) fn from_walk(layout: &Layout, near: ClusterId, position: Position) -> Self {
        let clusters = &layout.analysis().clusters;
        let at = TextOffset::new(position.offset.min(layout.text().len()));
        Self {
            position,
            cluster: walk_to(clusters, near, at),
        }
    }

    /// Returns `position` with `cluster` where the caller holds it, else found by search.
    pub(super) fn with_cluster(
        layout: &Layout,
        position: Position,
        cluster: Option<ClusterId>,
    ) -> Self {
        match cluster {
            Some(cluster) => Self { position, cluster },
            None => Self::new(layout, position),
        }
    }

    /// Returns its byte offset.
    pub(super) fn offset(&self) -> usize {
        self.position.offset
    }
}

/// Returns the cluster holding byte `at`, as [`Clusters::at`] finds it, walked to from `near`.
fn walk_to(clusters: &Clusters, near: ClusterId, at: TextOffset) -> ClusterId {
    let end = clusters.end_id();
    let mut cluster = near.min(end);
    // Back to the cluster starting at or before `at`.
    while cluster.get() > 0 && clusters.start(cluster) > at {
        work::step();
        cluster = ClusterId::new(cluster.get() - 1);
    }
    // On to the first ending past it.
    while cluster < end && clusters.range(cluster).end <= at {
        work::step();
        cluster = ClusterId::new(cluster.get() + 1);
    }
    cluster
}

/// Returns `unsnapped` snapped to a caret stop, moving toward the affinity's side.
///
/// The stop is in the text and on a cluster boundary. It is outside a
/// grapheme split by a style boundary, unless a box's edges part it, and
/// outside a character a text transform changed the length of.
pub(super) fn snap(layout: &Layout, unsnapped: ClusteredPosition) -> ClusteredPosition {
    let affinity = unsnapped.position.affinity;
    let clusters = &layout.analysis().clusters;
    if clusters.is_empty() {
        return ClusteredPosition {
            position: Position::new(0, affinity),
            cluster: ClusterId::new(0),
        };
    }
    let at = unsnapped.position.offset.min(layout.text().len());
    // Onto a boundary: the cluster starting there.
    let mut cluster = unsnapped.cluster;
    if affinity == Affinity::Upstream && clusters.start(cluster).get() != at {
        cluster = ClusterId::new(cluster.get() + 1);
    }
    // Out of a split grapheme, a cluster at a time.
    loop {
        work::step();
        if !divides_grapheme(layout, cluster) {
            break;
        }
        cluster = match affinity {
            Affinity::Downstream => match cluster.get().checked_sub(1) {
                Some(before) => ClusterId::new(before),
                None => break,
            },
            Affinity::Upstream => ClusterId::new(cluster.get() + 1),
        };
    }
    let start = clusters.start(cluster);
    // Out of a character a text transform changed the length of.
    if let Some(map) = layout.content().offset_map()
        && let Some(whole) = map.variable_around(start)
    {
        let at = match affinity {
            Affinity::Downstream => whole.start,
            Affinity::Upstream => whole.end,
        };
        return ClusteredPosition {
            position: Position::new(at.get(), affinity),
            cluster: walk_to(clusters, cluster, at),
        };
    }
    ClusteredPosition {
        position: Position::new(start.get(), affinity),
        cluster,
    }
}

/// Returns whether the boundary before `cluster` is inside a grapheme and no stop.
///
/// That is where `cluster` continues a grapheme split at an item boundary,
/// or is the marks after a space, and no box's margin, border or padding
/// takes room there. Chrome makes such an edge a stop of its own.
fn divides_grapheme(layout: &Layout, cluster: ClusterId) -> bool {
    layout
        .analysis()
        .clusters
        .continues_grapheme(layout.text(), cluster)
        && !is_box_edge(layout, cluster)
}

/// Returns whether a box opening or closing before `cluster` has margin, border or padding.
fn is_box_edge(layout: &Layout, cluster: ClusterId) -> bool {
    let content = layout.content();
    let (nodes, facts) = (&content.nodes, &content.facts);
    layout
        .analysis()
        .item_clusters
        .empty_items(cluster)
        .any(|item| {
            work::step();
            content.items.get(item).is_some_and(|item| {
                let own = nodes.box_facts(item.node, FirstLineVariant::Standard);
                matches!(item.kind, ItemKind::Open | ItemKind::Close)
                    && facts.box_facts(own).has(BoxFlags::HAS_EDGES)
            })
        })
}

/// Returns whether a caret can stop at `cluster`'s start.
///
/// A stop is not inside a grapheme split by a style boundary, and not inside
/// a character a text transform changed the length of. The text's end is a
/// stop.
pub(super) fn is_stop(layout: &Layout, cluster: ClusterId) -> bool {
    if divides_grapheme(layout, cluster) {
        return false;
    }
    let at = layout.analysis().clusters.start(cluster);
    layout
        .content()
        .offset_map()
        .is_none_or(|map| map.variable_around(at).is_none())
}

/// Returns the line `snapped`, a snapped position, is on by its affinity alone.
///
/// Downstream it is the line holding the offset, upstream the one ending at
/// it, and past the text's end the last. It does not look at what the line
/// placed. `None` where the layout has no lines.
///
/// Where the caller expects line `near`, it checks that line before it
/// searches.
pub(super) fn line(
    layout: &Layout,
    snapped: ClusteredPosition,
    near: Option<LineId>,
) -> Option<LineId> {
    let lines = layout.line_records();
    let last = lines.lines.last_id()?;
    let cluster = snapped.cluster;
    let upstream = snapped.position.affinity == Affinity::Upstream;
    let columns = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .ruby_columns();
    if let Some(line) = lines
        .ruby
        .as_ref()
        .and_then(|ruby| ruby.annotation_line(lines, columns, cluster, upstream))
    {
        return Some(line.min(last));
    }
    if upstream && cluster.get() == 0 {
        return Some(LineId::new(0));
    }
    // A line's end and the one before's bound the clusters it holds.
    let end = |line: LineId| lines.lines.get(line).map(|record| record.clusters().end);
    let is_on = |line: LineId| {
        let before = line
            .get()
            .checked_sub(1)
            .and_then(|before| end(LineId::new(before)));
        let after = if line < last { end(line) } else { None };
        if upstream {
            before.is_none_or(|before| before < cluster) && after.is_none_or(|end| end >= cluster)
        } else {
            before.is_none_or(|before| before <= cluster) && after.is_none_or(|end| end > cluster)
        }
    };
    if let Some(near) = near.filter(|&near| near <= last && is_on(near)) {
        return Some(near);
    }
    let line = if upstream {
        lines.first_reaching(cluster)
    } else {
        lines.at(cluster)
    };
    Some(line.min(last))
}

/// Returns the carets of `snapped`, a snapped position, on line `near` where the caller expects it.
pub(super) fn carets(
    layout: &Layout,
    snapped: ClusteredPosition,
    near: Option<LineId>,
) -> Option<Carets> {
    let id = line(layout, snapped, near)?;
    let line = Line::new(layout, id)?;
    let position = snapped.position;
    if let Some(found) = line_carets(layout, &line, position) {
        return Some(found);
    }
    let at = position.offset;
    // Upstream past what the line placed, the caret is on the next line.
    if position.affinity == Affinity::Upstream
        && let Some(next) = Line::new(layout, LineId::new(id.get() + 1))
        && let Some(found) = line_carets(layout, &next, position)
    {
        return Some(found);
    }
    Some(Carets {
        strong: past_content(layout, &line, at),
        weak: None,
    })
}

/// Returns the strong caret of `snapped`, a snapped position, as [`carets`] finds it.
pub(super) fn caret(
    layout: &Layout,
    snapped: ClusteredPosition,
    near: Option<LineId>,
) -> Option<Caret> {
    carets(layout, snapped, near).map(|carets| carets.strong)
}

/// Returns the line the caret of `snapped`, a snapped position, is drawn on, as [`carets`] finds it.
///
/// It reads which leaves meet the offset, not where their carets are.
pub(super) fn caret_line(
    layout: &Layout,
    snapped: ClusteredPosition,
    near: Option<LineId>,
) -> Option<Line<'_>> {
    let id = line(layout, snapped, near)?;
    let line = Line::new(layout, id)?;
    let at = snapped.offset();
    if snapped.position.affinity == Affinity::Upstream
        && !touches(&line, at)
        && let Some(next) = Line::new(layout, LineId::new(id.get() + 1))
        && touches(&next, at)
    {
        return Some(next);
    }
    Some(line)
}

/// Returns whether a leaf of `line`, in its base text or annotations, holds or meets at offset `at`.
fn touches(line: &Line<'_>, at: usize) -> bool {
    leaves(*line).chain(annotation_leaves(*line)).any(|leaf| {
        work::step();
        let (start, end) = (leaf.start(), leaf.end());
        (start < at && at < end) || (start < end && (start == at || end == at))
    })
}

/// Returns the carets of `position` among `line`'s leaves that hold or meet at its offset.
///
/// It searches the base text first, then the annotations. `None` where no
/// leaf does.
fn line_carets(layout: &Layout, line: &Line<'_>, position: Position) -> Option<Carets> {
    touching(layout, line, leaves(*line), position)
        .or_else(|| touching(layout, line, annotation_leaves(*line), position))
}

/// Returns the carets of `position` among `leaves` of `line`.
///
/// The strong caret is in the leaf holding the offset. At a boundary
/// between two leaves, it is in the one nearer the paragraph's level, the
/// side Chrome draws the caret on. At equal levels it is in the one before,
/// which Blink resolves first. Where the two leaves read in different
/// directions, the other gives the weak caret.
///
/// At the edge of combined text, the affinity chooses: downstream the leaf
/// after, upstream the one before. A click in a unit then draws its caret in
/// the unit, and a click beside it beside it, as in Chrome 153. Chrome has
/// two DOM positions at a unit's edge, one inside and one outside.
fn touching<'a>(
    layout: &Layout,
    line: &Line<'_>,
    leaves: impl Iterator<Item = Leaf<'a>>,
    position: Position,
) -> Option<Carets> {
    let at = position.offset;
    let (mut before, mut after) = (None, None);
    for leaf in leaves {
        work::step();
        let (start, end) = (leaf.start(), leaf.end());
        if start < at && at < end {
            return Some(Carets {
                strong: leaf.caret(layout, line, at),
                weak: None,
            });
        }
        if start < end && end == at {
            before = Some(leaf);
        }
        if start < end && start == at {
            after = Some(leaf);
        }
    }
    let (strong, weak) = match (before, after) {
        (Some(before), Some(after)) if after.level() < before.level() => (after, Some(before)),
        (Some(before), Some(after))
            if after.level() == before.level()
                && position.affinity == Affinity::Downstream
                && (before.is_combined() || after.is_combined()) =>
        {
            (after, Some(before))
        }
        (Some(before), after) => (before, after),
        (None, after) => (after?, None),
    };
    Some(Carets {
        strong: strong.caret(layout, line, at),
        weak: weak
            .filter(|weak| weak.is_rtl() != strong.is_rtl())
            .map(|weak| weak.caret(layout, line, at)),
    })
}

/// Returns the caret at `at` on `line` where no leaf holds or meets at `at`.
///
/// It is at the logical end of what the line placed, past a final forced
/// break or white space a wrap removed. On an empty line it is at the start,
/// as tall as the line box.
fn past_content(layout: &Layout, line: &Line<'_>, at: usize) -> Caret {
    let last = leaves(*line)
        .filter(|leaf| leaf.end() <= at)
        .max_by_key(Leaf::end);
    match last {
        Some(leaf) => leaf.caret(layout, line, leaf.end()),
        None => {
            let rtl = line.level().is_rtl();
            let metrics = line.metrics();
            let x = if rtl { metrics.width } else { 0.0 };
            Caret {
                line: line.index(),
                inline: InlineExtents { left: x, right: x },
                block: CrossExtents {
                    over: 0.0,
                    under: metrics.height(),
                },
                rtl,
            }
        }
    }
}
