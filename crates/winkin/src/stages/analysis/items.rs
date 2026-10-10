//! Stored items from text analysis.

use core::ops::Range;

use crate::data::{Id, IdRange, Run, RunCursor, Runs, heap_bytes};
use crate::stages::content::ItemId;
use crate::work;

use super::ClusterId;

/// Where each item's clusters are: the clusters an item holds, and the
/// items at a cluster or a boundary.
///
/// Later stages walk items and clusters together through this, without
/// searching the text. Each item keeps its first cluster, and its clusters
/// end where the next item's begin. An item that holds none sits at the
/// boundary its first cluster names.
#[derive(Debug)]
pub(crate) struct ItemClusters {
    /// Per item: its first cluster, or for an item with no text, the cluster
    /// after the boundary it sits at.
    pub(super) firsts: Runs<ItemId, ClusterId>,
    /// Where the last item's clusters end: the text's end, which the writer
    /// writes when it has written every cluster.
    pub(super) end: ClusterId,
}

impl Run for ClusterId {
    type Position = ClusterId;

    /// Itself: an item's first cluster, where its clusters start.
    #[inline]
    fn start(&self) -> ClusterId {
        *self
    }
}

impl ItemClusters {
    pub(super) const fn new() -> Self {
        Self {
            firsts: Runs::new(),
            end: ClusterId(0),
        }
    }

    pub(super) fn clear(&mut self) {
        self.firsts.clear();
        self.end = ClusterId(0);
    }

    /// Makes room for `count` items, the content's.
    pub(super) fn reserve(&mut self, count: usize) {
        self.firsts.reserve(count);
    }

    /// Appends the next item, whose first cluster is `first`; `None` past
    /// what an `ItemId` names, which the content's items never reach.
    pub(super) fn push(&mut self, first: ClusterId) -> Option<ItemId> {
        self.firsts.push(first)
    }

    /// How many items there are, for tests to count.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.firsts.len()
    }

    /// `item`'s first cluster, or the cluster after the boundary it sits at
    /// where it holds none; `None` past the last item.
    pub(crate) fn get(&self, item: ItemId) -> Option<ClusterId> {
        self.firsts.get(item).copied()
    }

    /// Where `item`'s clusters start: its first cluster, or the cluster
    /// after the boundary it sits at where it holds none; the text's end past
    /// the last item.
    #[inline]
    pub(crate) fn start(&self, item: ItemId) -> ClusterId {
        self.firsts.start(item, self.end)
    }

    /// `item`'s clusters: from its first to the next item's first, or the
    /// text's end. Empty where it holds none, at the boundary it sits at;
    /// the text's end, empty, past the last item.
    #[inline]
    pub(crate) fn range(&self, item: ItemId) -> Range<ClusterId> {
        self.range_to(item, self.end)
    }

    /// `item`'s clusters, where the clusters written so far end at `end`:
    /// what the writer reads while it runs, before the text's end is known.
    #[inline]
    pub(super) fn range_to(&self, item: ItemId, end: ClusterId) -> Range<ClusterId> {
        self.firsts.span(item, end)
    }

    /// The clusters of the consecutive `items`, from the first's first
    /// cluster to the last's end. Past the last item, each end is the
    /// text's end.
    pub(crate) fn span(&self, items: Range<ItemId>) -> Range<ClusterId> {
        self.start(items.start)..self.start(items.end)
    }

    /// The items at boundary `at` that hold no cluster, in order, by search.
    ///
    /// They are the box edges, floats and ruby marks there. The walk starts
    /// at the first item whose first cluster is not before `at`.
    pub(crate) fn empty_items(&self, at: ClusterId) -> impl Iterator<Item = ItemId> + '_ {
        let from = self.firsts.first_from(at);
        let end = ItemId::new(self.firsts.len());
        (from..end).ids().take_while(move |&item| {
            let clusters = self.range(item);
            clusters.start == at && clusters.is_empty()
        })
    }
}

/// The cursor a walk over the items holds ([`Segments`]): an item holding
/// no cluster is a run of none, which the cursor stands on as on any other.
///
/// [`Segments`]: crate::stages::Segments
impl ItemClusters {
    /// A cursor at `item`, from a link to it: a line's first item.
    #[inline]
    pub(crate) fn cursor(&self, item: ItemId) -> RunCursor<ItemId, ClusterId> {
        self.firsts.cursor(item, self.end)
    }

    /// The cursor a walk from boundary `at` starts at, by search.
    ///
    /// It is the first item sitting at `at` or starting there, with items
    /// holding no cluster first. Otherwise it is the item holding the
    /// cluster across `at`. At the text's end it is past the last item.
    pub(crate) fn cursor_containing(&self, at: ClusterId) -> RunCursor<ItemId, ClusterId> {
        let from = self.firsts.first_from(at);
        if self.firsts.get(from).is_some_and(|&first| first == at) {
            return self.firsts.cursor(from, self.end);
        }
        // None starts there: the item before the first starting past it
        // holds it, unless it ends there, at the text's end.
        let before = from
            .get()
            .checked_sub(1)
            .map(|before| self.firsts.cursor(ItemId::new(before), self.end))
            .filter(|before| before.end() > at);
        before.unwrap_or_else(|| self.firsts.cursor(from, self.end))
    }

    /// Moves `cursor` to the next item, whether or not it holds a cluster:
    /// past the last, it stands at the text's end.
    #[inline]
    pub(crate) fn step(&self, cursor: &mut RunCursor<ItemId, ClusterId>) {
        self.firsts.step(cursor, self.end);
    }

    /// Returns the item a walk from boundary `at` starts at, as
    /// [`cursor_containing`](Self::cursor_containing) decides it, walked to
    /// from `from` an item at a time, forward or back.
    ///
    /// A walk that holds the item at a boundary near `at` uses it. It costs
    /// a step per item between, and no search.
    pub(crate) fn walk_to(&self, from: ItemId, at: ClusterId) -> ItemId {
        // The items from the one a walk from `at` starts at: those sitting
        // at `at` or after it, and the one holding the cluster across it.
        // Past the last item, the start is the text's end, which reaches.
        let reaches = |item: ItemId| self.start(item) >= at || self.range(item).end > at;
        let mut item = from;
        while !reaches(item) {
            work::step();
            item = ItemId::new(item.get() + 1);
        }
        while let Some(before) = item.get().checked_sub(1).map(ItemId::new)
            && reaches(before)
        {
            work::step();
            item = before;
        }
        item
    }

    /// Returns the item holding `cluster`, walked to from `from` an item at a
    /// time: the item [`walk_to`](Self::walk_to) finds, stepped past those
    /// at the cluster's start that hold none.
    ///
    /// Past the last item's clusters, it is the last item.
    pub(crate) fn walk_to_cluster(&self, from: ItemId, cluster: ClusterId) -> ItemId {
        let last = self.firsts.len().saturating_sub(1);
        let mut item = self.walk_to(from, cluster);
        while self.range(item).end <= cluster && item.get() < last {
            work::step();
            item = ItemId::new(item.get() + 1);
        }
        item
    }
}

heap_bytes! {
    ItemClusters { firsts; end }
}
