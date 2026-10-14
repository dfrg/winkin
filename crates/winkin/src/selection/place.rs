//! Caret stops, the line and leaf a position is drawn in, and its caret.
//!
//! - A caret stop is a grapheme cluster boundary. A grapheme split at an item
//!   boundary is one stop, unless a box edge takes room inside it; Chrome's
//!   caret then stops at the edge. A character whose length a text transform
//!   changed is one stop.
//! - Downstream, the line is the one holding the offset; upstream, the one
//!   ending at it. Upstream at a soft wrap after removed white space, the
//!   caret stays on the line before, past that space, as if it hung. Chrome
//!   puts it on the next line, finding no item to place it by
//!   (`CanResolveInlineCaretPositionAfterFragment`); keeping it on the line
//!   before keeps a wrap's two places apart. Upstream after a forced break is
//!   on the next line.
//! - The leaf holding the offset draws the caret. Where leaves of opposite
//!   directions meet, the affinity picks the side: upstream the leaf before,
//!   downstream the one after. Chrome picks the side nearer the paragraph's
//!   level whatever the affinity. At other level changes the leaf nearer the
//!   paragraph's level wins, and at equal levels the one before, as Blink's
//!   `ComputeInlineCaretPosition` resolves it. Ruby annotation text is
//!   searched where the base text has no match.
//! - A box edge that takes room along the line, with a leaf on each side on
//!   one line, has two positions: upstream is drawn in the leaf before,
//!   downstream in the leaf after, each on its own side of the room, where
//!   typed text goes. Chrome keeps one position there, in the text before;
//!   this is a deliberate difference. Firefox has the two positions.
//! - A soft hyphen differs from Chrome. Blink's line ends with the generated
//!   hyphen, so the position after the soft hyphen stays on that line even
//!   downstream. Here downstream is the next line's start, as at any wrap.

use super::{Affinity, Caret, Carets, Position};
use crate::data::{Id, TextOffset};
use crate::layout::Layout;
use crate::layout::{Atomic, CrossExtents, Item, Line, TextRun};
use crate::stages::analysis::{BidiLevel, ClusterClass, ClusterId, Clusters};
use crate::stages::content::{BoxFlags, ContentFlags, ItemKind};
use crate::stages::lines::{InlineExtents, LineId};
use crate::style::FirstLineVariant;
use crate::unit::LayoutUnit;
use crate::work;

/// A leaf of a line as a caret sees it: a text run or an atomic inline.
///
/// Boxes are not leaves. Neither is generated text (an ellipsis or a
/// hyphen): a caret passes through it to the text under it, as Blink's does
/// (`IsGeneratedText`).
#[derive(Copy, Clone)]
pub(super) struct Leaf<'a> {
    /// The byte offset in the layout's text where its text starts.
    pub(super) start: usize,
    /// The byte offset where its text ends.
    pub(super) end: usize,
    /// Its extent along the line, from the line box's left.
    pub(super) inline: InlineExtents,
    /// The text run or atomic inline it is.
    pub(super) item: LeafItem<'a>,
    /// The line it is on.
    pub(super) line: Line<'a>,
}

/// What a leaf is.
#[derive(Copy, Clone)]
pub(super) enum LeafItem<'a> {
    /// A text run.
    Text(TextRun<'a>),
    /// An atomic inline.
    Atomic(Atomic<'a>),
}

impl<'a> Leaf<'a> {
    /// Returns `item` of `line` as a leaf, or `None` for a box or generated
    /// text.
    fn from_item(line: Line<'a>, item: Item<'a>) -> Option<Self> {
        match item {
            Item::Text(run) => Some(Self::from_run(line, run)),
            Item::Atomic(atomic) => {
                let range = atomic.text_range();
                Some(Self {
                    start: range.start,
                    end: range.end,
                    inline: atomic.inline(),
                    item: LeafItem::Atomic(atomic),
                    line,
                })
            }
            Item::Generated(_) | Item::Box(_) => None,
        }
    }

    /// Returns the text run `run` of `line` as a leaf.
    fn from_run(line: Line<'a>, run: TextRun<'a>) -> Self {
        let range = run.text_range();
        Self {
            start: range.start,
            end: range.end,
            inline: run.inline(),
            item: LeafItem::Text(run),
            line,
        }
    }

    /// Returns its bidi level.
    pub(super) fn level(&self) -> BidiLevel {
        match &self.item {
            LeafItem::Text(run) => run.level(),
            LeafItem::Atomic(atomic) => atomic.level(),
        }
    }

    /// Returns whether it reads right to left.
    pub(super) fn is_rtl(&self) -> bool {
        self.level().is_rtl()
    }

    /// Returns its text run, or `None` for an atomic inline.
    pub(super) fn run(&self) -> Option<&TextRun<'a>> {
        match &self.item {
            LeafItem::Text(run) => Some(run),
            LeafItem::Atomic(_) => None,
        }
    }

    /// Returns whether it is combined text, whose carets run along the line.
    ///
    /// Combined text is set across a vertical line in one em.
    pub(super) fn is_combined(&self) -> bool {
        self.run().is_some_and(TextRun::is_combined)
    }

    /// Returns where the boundary `at` stands across the line, for combined
    /// text only.
    ///
    /// Uses the start of the cluster starting at `at`, or the end of the one
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

    /// Returns the boundary of its combined text that `y` across the line
    /// hits, with the cluster starting there.
    ///
    /// The boundary is the nearer edge of the cluster at `y`, a tie going to
    /// its start, as a click in Chrome's combined text finds it. Past the text
    /// on either side it is the unit's start or end. Chrome there jumps to the
    /// text's start, a bug not matched.
    pub(super) fn hit_across(&self, layout: &Layout, y: f32) -> Option<(usize, ClusterId)> {
        let clusters = &layout.analysis().clusters;
        let mut end = None;
        for (cluster, from, to) in self.run()?.across_places()? {
            let range = clusters.range(cluster);
            let start = (range.start.get(), cluster);
            let after = (range.end.get(), ClusterId::new(cluster.get() + 1));
            // The text starts at its under side, the larger `y`; a hit past
            // that is the start.
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
            self.end
        } else {
            self.start
        }
    }

    /// Returns where the caret at the boundary `at` stands along the line.
    ///
    /// The caret is at the leaf's start or end edge, or between two clusters;
    /// clusters share a ligature evenly (`TextRun::places`).
    pub(super) fn caret_x(&self, layout: &Layout, at: usize) -> f32 {
        let InlineExtents { left, right } = self.inline;
        let rtl = self.is_rtl();
        let start = self.start;
        // The start's edge or the end's; an atomic inline has only these.
        let edge = |end: bool| if end != rtl { right } else { left };
        let LeafItem::Text(run) = &self.item else {
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

    /// Returns the boundary that `x` along the line hits, with the cluster
    /// starting there where known.
    ///
    /// The boundary is the nearer edge of the cluster under `x`, a tie going
    /// left, as Blink's `ShapeResult::CaretOffsetForHitTest` finds it. Where
    /// no cluster is under `x`, it is the nearer end, with no cluster.
    ///
    /// Combined text is hit along the line as an atomic inline is, before or
    /// after by its halves. A point across the line picks among its
    /// characters ([`hit_across`](Self::hit_across)).
    pub(super) fn hit(&self, layout: &Layout, x: f32) -> (usize, Option<ClusterId>) {
        let InlineExtents { left, right } = self.inline;
        let middle = left + (right - left) / 2.0;
        let edge = (self.edge_offset(x > middle), None);
        let LeafItem::Text(run) = &self.item else {
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

    /// Returns the extent across the line of a caret in it, from the line
    /// box's top.
    ///
    /// In text it spans the font's ascent and descent around the baseline, as
    /// Chrome sizes a caret. Beside an atomic inline it spans the line box,
    /// as Chrome's `ComputeLocalCaretRectByBoxSide` does.
    fn caret_block(&self) -> CrossExtents {
        match &self.item {
            LeafItem::Text(run) => run.block(),
            LeafItem::Atomic(_) => CrossExtents::from_line_height(self.line.metrics().height()),
        }
    }

    /// Returns the caret at the boundary `at`.
    ///
    /// The caret stands across the line. In combined text it runs along the
    /// line over the unit's em instead.
    fn caret(&self, layout: &Layout, at: usize) -> Caret {
        let (inline, block) = match self.across(layout, at) {
            Some(across) => (
                self.inline,
                CrossExtents {
                    over: across,
                    under: across,
                },
            ),
            None => {
                let x = self.caret_x(layout, at);
                (InlineExtents { left: x, right: x }, self.caret_block())
            }
        };
        Caret {
            line: self.line.index(),
            inline,
            block,
            rtl: self.is_rtl(),
        }
    }
}

/// Returns the leaves of `line` in visual order.
///
/// Includes text an ellipsis hides, which keeps its carets and hits as in
/// Chrome.
pub(super) fn leaves<'a>(line: Line<'a>) -> impl DoubleEndedIterator<Item = Leaf<'a>> + Clone {
    line.leaf_items()
        .filter_map(move |item| Leaf::from_item(line, item))
}

/// Returns the text runs of `line`'s ruby annotations, column by column.
pub(super) fn annotation_leaves<'a>(line: Line<'a>) -> impl Iterator<Item = Leaf<'a>> {
    line.annotations()
        .flat_map(|annotation| annotation.runs())
        .map(move |run| Leaf::from_run(line, run))
}

/// A position with the cluster its offset is in, found once.
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
    /// Returns `position` with its cluster, found by search.
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

    /// Returns `position` with its cluster, walked to from `near`.
    ///
    /// Costs the clusters between `near` and the offset, so `near` should be
    /// close.
    pub(super) fn from_walk(layout: &Layout, near: ClusterId, position: Position) -> Self {
        let clusters = &layout.analysis().clusters;
        let at = TextOffset::new(position.offset.min(layout.text().len()));
        Self {
            position,
            cluster: clusters.walk_to(near, at),
        }
    }

    /// Returns `position` with `cluster` where given, else found by search.
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

/// Returns `unsnapped` snapped to a caret stop, moving toward the affinity's side.
///
/// The stop is in the text, on a cluster boundary, outside a grapheme split
/// by a style boundary (unless a box edge parts it), and outside a
/// character whose length a text transform changed.
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
    // Out of a character whose length a text transform changed.
    if let Some(map) = layout.content().offset_map()
        && let Some(whole) = map.variable_around(start)
    {
        let at = match affinity {
            Affinity::Downstream => whole.start,
            Affinity::Upstream => whole.end,
        };
        return ClusteredPosition {
            position: Position::new(at.get(), affinity),
            cluster: clusters.walk_to(cluster, at),
        };
    }
    ClusteredPosition {
        position: Position::new(start.get(), affinity),
        cluster,
    }
}

/// Returns whether the boundary before `cluster` is inside a grapheme and
/// so no stop.
///
/// It is where `cluster` continues a grapheme split at an item boundary, or
/// holds the marks after a space, and no box's margin, border or padding
/// takes room there. Chrome makes such a box edge a stop of its own.
fn divides_grapheme(layout: &Layout, cluster: ClusterId) -> bool {
    layout
        .analysis()
        .clusters
        .continues_grapheme(layout.text(), cluster)
        && !is_box_edge(layout, cluster)
}

/// Returns whether a box opening or closing before `cluster` takes room
/// along the line.
///
/// The room is the margin, border and padding on the side facing the
/// boundary: a box's start where it opens, its end where it closes.
fn is_box_edge(layout: &Layout, cluster: ClusterId) -> bool {
    let content = layout.content();
    if !content.flags.contains(ContentFlags::BOXES_WITH_EDGES) {
        return false;
    }
    let (nodes, facts) = (&content.nodes, &content.facts);
    layout
        .analysis()
        .item_clusters
        .empty_items(cluster)
        .any(|item| {
            work::step();
            content.items.get(item).is_some_and(|item| {
                let row = facts.box_facts(nodes.box_facts(item.node, FirstLineVariant::Standard));
                let (left, right) = row.room;
                let rtl = row.has(BoxFlags::RTL);
                let room = match item.kind {
                    ItemKind::Open if rtl => right,
                    ItemKind::Open => left,
                    ItemKind::Close if rtl => left,
                    ItemKind::Close => right,
                    _ => return false,
                };
                room > LayoutUnit::ZERO
            })
        })
}

/// Returns whether `snapped` is at a box edge whose two sides are two
/// positions.
///
/// The edge takes room along the line and has a leaf on each side on one
/// line, so the two affinities draw the caret at two places. Returns false
/// at a line's start or end, where only the side inside the line exists.
pub(super) fn is_sided(layout: &Layout, snapped: ClusteredPosition) -> bool {
    if !is_box_edge(layout, snapped.cluster) {
        return false;
    }
    let side = |affinity: Affinity| {
        let position = Position::new(snapped.offset(), affinity);
        caret(
            layout,
            ClusteredPosition {
                position,
                ..snapped
            },
            None,
        )
    };
    match (side(Affinity::Upstream), side(Affinity::Downstream)) {
        (Some(up), Some(down)) => up.line == down.line && up.inline != down.inline,
        _ => false,
    }
}

/// Returns whether a line after the first starts at `cluster`, so upstream
/// there is on the line before.
///
/// True at a wrap or a forced break. The text's end starts no line.
pub(super) fn starts_line(layout: &Layout, cluster: ClusterId) -> bool {
    let lines = &layout.line_records().lines;
    let line = layout.line_records().first_reaching(cluster);
    lines
        .get(line)
        .is_some_and(|record| record.clusters().end == cluster)
        && lines.get(LineId::new(line.get() + 1)).is_some()
}

/// Returns whether a forced break directly precedes `cluster`, so a line
/// starting there has one place.
pub(super) fn follows_break(layout: &Layout, cluster: ClusterId) -> bool {
    let clusters = &layout.analysis().clusters;
    cluster.get().checked_sub(1).is_some_and(|before| {
        clusters
            .class(ClusterId::new(before))
            .is_some_and(ClusterClass::is_forced_break)
    })
}

/// Returns whether `line` ends at a soft wrap: another line follows and no
/// forced break ends it.
pub(super) fn wraps(layout: &Layout, line: &Line<'_>) -> bool {
    Line::new(layout, LineId::new(line.index() + 1)).is_some()
        && !follows_break(layout, line.record().clusters().end)
}

/// Returns the upstream caret at the end of `line` where it wraps after
/// removed white space.
///
/// The caret stands past that space, in the line's direction, as if the
/// space hung there. Returns `None` where the line does not wrap, its leaves
/// reach its end, or the clusters after them hold no space.
pub(super) fn wrap_end(layout: &Layout, line: &Line<'_>) -> Option<Caret> {
    if !wraps(layout, line) {
        return None;
    }
    let (mut left, mut right, mut placed) = (f32::INFINITY, f32::NEG_INFINITY, None);
    for leaf in leaves(*line) {
        work::step();
        left = left.min(leaf.inline.left);
        right = right.max(leaf.inline.right);
        placed = Some(placed.map_or(leaf.end, |at: usize| at.max(leaf.end)));
    }
    let placed = placed?;
    let clusters = &layout.analysis().clusters;
    let stages = line.stages();
    let (mut cluster, mut room, mut removed) = (line.record().clusters().end, 0.0, false);
    // The clusters after the last leaf, walked back from the line's end.
    while let Some(before) = cluster.get().checked_sub(1).map(ClusterId::new) {
        work::step();
        if clusters.start(before).get() < placed {
            break;
        }
        removed |= clusters.class(before) == Some(ClusterClass::Space);
        room += stages.cluster_advance(before).to_px();
        cluster = before;
    }
    if !removed {
        return None;
    }
    let base = past_content(layout, line, placed);
    let rtl = line.level().is_rtl();
    let x = if rtl { left - room } else { right + room };
    Some(Caret {
        inline: InlineExtents { left: x, right: x },
        rtl,
        ..base
    })
}

/// Returns whether a caret can stop at `cluster`'s start.
///
/// A stop is outside a grapheme split by a style boundary and outside a
/// character whose length a text transform changed. The text's end is a
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

/// Returns the line `snapped` is on by its affinity alone.
///
/// Downstream it is the line holding the offset, upstream the one ending at
/// it, and past the text's end the last. Ignores what the line placed.
/// Returns `None` where the layout has no lines.
///
/// Checks the expected line `near` before searching.
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
    // A line holds the clusters between the previous line's end and its own.
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

/// Returns the carets of `snapped`, checking the expected line `near` first.
pub(super) fn carets(
    layout: &Layout,
    snapped: ClusteredPosition,
    near: Option<LineId>,
) -> Option<Carets> {
    let id = line(layout, snapped, near)?;
    let line = Line::new(layout, id)?;
    if let Some(found) = line_carets(layout, &line, snapped) {
        return Some(found);
    }
    let at = snapped.offset();
    if snapped.position.affinity == Affinity::Upstream {
        // Upstream at a wrap after removed white space: past that space.
        if line.record().clusters().end == snapped.cluster
            && let Some(strong) = wrap_end(layout, &line)
        {
            return Some(Carets { strong, weak: None });
        }
        // Elsewhere past the line's leaves, the caret is on the next line.
        if let Some(next) = Line::new(layout, LineId::new(id.get() + 1))
            && let Some(found) = line_carets(layout, &next, snapped)
        {
            return Some(found);
        }
    }
    Some(Carets {
        strong: past_content(layout, &line, at),
        weak: None,
    })
}

/// Returns the strong caret of `snapped`, as [`carets`] finds it.
pub(super) fn caret(
    layout: &Layout,
    snapped: ClusteredPosition,
    near: Option<LineId>,
) -> Option<Caret> {
    carets(layout, snapped, near).map(|carets| carets.strong)
}

/// Returns the line `snapped`'s caret is drawn on, as [`carets`] finds it.
///
/// Reads which leaves meet the offset without computing their carets.
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
        && !(line.record().clusters().end == snapped.cluster && wrap_end(layout, &line).is_some())
        && let Some(next) = Line::new(layout, LineId::new(id.get() + 1))
        && touches(&next, at)
    {
        return Some(next);
    }
    Some(line)
}

/// Returns whether a leaf of `line`, in its base text or annotations, holds
/// or meets offset `at`.
fn touches(line: &Line<'_>, at: usize) -> bool {
    leaves(*line).chain(annotation_leaves(*line)).any(|leaf| {
        work::step();
        let (start, end) = (leaf.start, leaf.end);
        (start < at && at < end) || (start < end && (start == at || end == at))
    })
}

/// Returns the carets of `snapped` among the leaves of `line` that hold or
/// meet its offset.
///
/// Searches the base text first, then the annotations. Returns `None` where
/// no leaf does.
fn line_carets(layout: &Layout, line: &Line<'_>, snapped: ClusteredPosition) -> Option<Carets> {
    touching(layout, leaves(*line), snapped)
        .or_else(|| touching(layout, annotation_leaves(*line), snapped))
}

/// Returns the carets of `snapped` among `leaves`, all on one line.
///
/// The strong caret is in the leaf holding the offset. Between two leaves of
/// opposite directions, the affinity picks the side (upstream the leaf
/// before, downstream the one after) and the other leaf gives the weak
/// caret; Chrome draws the strong caret in the leaf nearer the paragraph's
/// level. At other level changes the strong caret is in the leaf nearer the
/// paragraph's level, and at equal levels in the one before, which Blink
/// resolves first.
///
/// At a box edge taking room between the two, the affinity picks the side,
/// with no weak caret.
///
/// At the edge of combined text, the affinity picks the side: downstream the
/// leaf after, upstream the one before. A click in a unit draws its caret in
/// the unit and a click beside it draws it outside, as in Chrome 153, which
/// has two DOM positions there.
fn touching<'a>(
    layout: &Layout,
    leaves: impl Iterator<Item = Leaf<'a>>,
    snapped: ClusteredPosition,
) -> Option<Carets> {
    let position = snapped.position;
    let at = position.offset;
    let (mut before, mut after) = (None, None);
    for leaf in leaves {
        work::step();
        let (start, end) = (leaf.start, leaf.end);
        if start < at && at < end {
            return Some(Carets {
                strong: leaf.caret(layout, at),
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
    if let (Some(before), Some(after)) = (before, after)
        && is_box_edge(layout, snapped.cluster)
    {
        let side = match position.affinity {
            Affinity::Upstream => before,
            Affinity::Downstream => after,
        };
        return Some(Carets {
            strong: side.caret(layout, at),
            weak: None,
        });
    }
    let (strong, weak) = match (before, after) {
        (Some(before), Some(after)) if before.is_rtl() != after.is_rtl() => {
            match position.affinity {
                Affinity::Upstream => (before, Some(after)),
                Affinity::Downstream => (after, Some(before)),
            }
        }
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
        strong: strong.caret(layout, at),
        weak: weak
            .filter(|weak| weak.is_rtl() != strong.is_rtl())
            .map(|weak| weak.caret(layout, at)),
    })
}

/// Returns the caret at `at` on `line` where no leaf holds or meets `at`.
///
/// The caret is at the end of the last leaf ending at or before `at`, as past
/// a final forced break. On a line with no such leaf it is at the line's
/// start, as tall as the line box.
fn past_content(layout: &Layout, line: &Line<'_>, at: usize) -> Caret {
    let last = leaves(*line)
        .filter(|leaf| leaf.end <= at)
        .max_by_key(|leaf| leaf.end);
    match last {
        Some(leaf) => leaf.caret(layout, leaf.end),
        None => {
            let rtl = line.level().is_rtl();
            let metrics = line.metrics();
            let x = if rtl { metrics.width } else { 0.0 };
            Caret {
                line: line.index(),
                inline: InlineExtents { left: x, right: x },
                block: CrossExtents::from_line_height(metrics.height()),
                rtl,
            }
        }
    }
}
