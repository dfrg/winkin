//! The clusters of each content item.

use core::ops::Range;

use crate::data::{Id, IdRange, Run, RunCursor, Runs, heap_bytes};
use crate::stages::content::ItemId;
use crate::work;

use super::ClusterId;

/// The clusters each item holds, and the items at a cluster or boundary.
///
/// Each item keeps its first cluster; its clusters end where the next
/// item's begin. An item with no clusters sits at the boundary before its
/// first cluster. Later stages walk items and clusters together through
/// this table.
#[derive(Debug)]
pub(crate) struct ItemClusters {
    /// Each item's first cluster, or for an empty item, the cluster after
    /// its boundary.
    pub(super) firsts: Runs<ItemId, ClusterId>,
    /// The end of the last item's clusters: the text's end, written after
    /// the last cluster.
    pub(super) end: ClusterId,
}

impl Run for ClusterId {
    type Position = ClusterId;

    /// Returns the cluster itself, an item's first.
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

    /// Makes room for `count` items.
    pub(super) fn reserve(&mut self, count: usize) {
        self.firsts.reserve(count);
    }

    /// Appends the next item, whose first cluster is `first`.
    ///
    /// Returns `None` past the last `ItemId`, which a content never reaches.
    pub(super) fn push(&mut self, first: ClusterId) -> Option<ItemId> {
        self.firsts.push(first)
    }

    /// Returns the number of items.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.firsts.len()
    }

    /// Returns `item`'s first cluster, or for an empty item the cluster
    /// after its boundary. Returns `None` past the last item.
    pub(crate) fn get(&self, item: ItemId) -> Option<ClusterId> {
        self.firsts.get(item).copied()
    }

    /// Returns where `item`'s clusters start, as [`get`](Self::get) does,
    /// or the text's end past the last item.
    #[inline]
    pub(crate) fn start(&self, item: ItemId) -> ClusterId {
        self.firsts.start(item, self.end)
    }

    /// Returns `item`'s clusters, up to the next item's first.
    ///
    /// The range is empty, at its boundary, for an empty item, and empty at
    /// the text's end past the last item.
    #[inline]
    pub(crate) fn range(&self, item: ItemId) -> Range<ClusterId> {
        self.range_to(item, self.end)
    }

    /// Returns `item`'s clusters while the writer runs, with `end` standing
    /// for the text's end.
    #[inline]
    pub(super) fn range_to(&self, item: ItemId, end: ClusterId) -> Range<ClusterId> {
        self.firsts.span(item, end)
    }

    /// Returns the clusters of the consecutive `items`. An end past the last
    /// item is the text's end.
    pub(crate) fn span(&self, items: Range<ItemId>) -> Range<ClusterId> {
        self.start(items.start)..self.start(items.end)
    }

    /// Returns the empty items at boundary `at`, in order: box edges,
    /// floats and ruby marks. Searches for the first.
    pub(crate) fn empty_items(&self, at: ClusterId) -> impl Iterator<Item = ItemId> + '_ {
        let from = self.firsts.first_from(at);
        let end = ItemId::new(self.firsts.len());
        (from..end).ids().take_while(move |&item| {
            let clusters = self.range(item);
            clusters.start == at && clusters.is_empty()
        })
    }
}

/// Cursors for walks over the items ([`Segments`]). An empty item is a run
/// of no clusters, and a cursor stands on it like any other.
///
/// [`Segments`]: crate::stages::Segments
impl ItemClusters {
    /// Returns a cursor at `item`, such as a line's first item.
    #[inline]
    pub(crate) fn cursor(&self, item: ItemId) -> RunCursor<ItemId, ClusterId> {
        self.firsts.cursor(item, self.end)
    }

    /// Returns the cursor a walk from boundary `at` starts at, by search.
    ///
    /// That is the first item starting at `at`, empty items first, or else
    /// the item holding the cluster across `at`. At the text's end the cursor
    /// is past the last item.
    pub(crate) fn cursor_containing(&self, at: ClusterId) -> RunCursor<ItemId, ClusterId> {
        let from = self.firsts.first_from(at);
        if self.firsts.get(from).is_some_and(|&first| first == at) {
            return self.firsts.cursor(from, self.end);
        }
        // No item starts at `at`, so the item before `from` holds it, unless
        // that item ends there at the text's end.
        let before = from
            .get()
            .checked_sub(1)
            .map(|before| self.firsts.cursor(ItemId::new(before), self.end))
            .filter(|before| before.end() > at);
        before.unwrap_or_else(|| self.firsts.cursor(from, self.end))
    }

    /// Moves `cursor` to the next item, empty or not. Past the last item it
    /// stands at the text's end.
    #[inline]
    pub(crate) fn step(&self, cursor: &mut RunCursor<ItemId, ClusterId>) {
        self.firsts.step(cursor, self.end);
    }

    /// Returns the item a walk from boundary `at` starts at, as
    /// [`cursor_containing`](Self::cursor_containing) finds it, by stepping
    /// from `from` forward or back.
    ///
    /// Costs one step per item between, and no search, so a walk near `at`
    /// uses it.
    pub(crate) fn walk_to(&self, from: ItemId, at: ClusterId) -> ItemId {
        // An item reaches `at` if it starts at or after it, or holds the
        // cluster across it. Past the last item, the start is the text's end,
        // which reaches.
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

    /// Returns the item holding `cluster`, stepping from `from` as
    /// [`walk_to`](Self::walk_to) does and then past empty items.
    ///
    /// Past the last item's clusters, returns the last item.
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
