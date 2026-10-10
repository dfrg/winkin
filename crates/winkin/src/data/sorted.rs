//! Sparse tables sorted by a key, the one search that finds an entry by its
//! key, and the cursor a walk in key order moves instead of searching.

use alloc::vec::Vec;

use super::{HeapBytes, sort_by_key};
use crate::work;

/// An entry a table is sorted by, and found by: its key, which it holds.
pub(crate) trait Keyed {
    /// What the entries are sorted by.
    type Key: Copy + Ord;

    /// Its key.
    fn key(&self) -> Self::Key;
}

impl<K: Copy + Ord, V> Keyed for (K, V) {
    type Key = K;

    #[inline]
    fn key(&self) -> K {
        self.0
    }
}

/// The entry of `entries` whose key is `key`, by halving, or `None` where
/// none has it.
///
/// `entries` are sorted by their keys, each key once. This is the one search
/// by key, whatever holds the entries: a [`SortedTable`], a
/// [`Table`](super::Table) whose items are in key order, or a slice of one.
/// A line's settled shifts, for example, are a slice of the whole table.
#[inline]
pub(crate) fn find_sorted<T: Keyed>(entries: &[T], key: T::Key) -> Option<&T> {
    work::seek();
    let found = entries.partition_point(|entry| entry.key() < key);
    entries.get(found).filter(|entry| entry.key() == key)
}

/// A sparse table sorted by key: an entry for each key that has one, in the
/// order of their keys, found by halving.
///
/// A stage keeps one where only a few ids of an earlier stage's table have
/// a value. Examples are the line-edge costs by boundary, the baseline shifts
/// by node, the emphasis boxes by style, the fits of combined text by run,
/// and the justified lines. The owner pushes the entries in key order as it
/// walks them.
pub(crate) struct SortedTable<T> {
    entries: Vec<T>,
}

impl<T: Keyed> SortedTable<T> {
    /// An empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Removes every entry and keeps the allocation.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Appends `entry`, whose key is past every key so far.
    pub(crate) fn push(&mut self, entry: T) {
        debug_assert!(
            self.entries
                .last()
                .is_none_or(|last| last.key() < entry.key()),
            "a sorted table's keys increase"
        );
        self.entries.push(entry);
    }

    /// Appends `entry`, whose key may come before others pushed since the
    /// last [`sort_from`](Self::sort_from).
    pub(crate) fn push_unsorted(&mut self, entry: T) {
        self.entries.push(entry);
    }

    /// How many entries it holds, which marks where [`sort_from`](Self::sort_from)
    /// starts.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Sorts the entries pushed from `mark` on by key. Their keys are past
    /// every key before `mark`.
    pub(crate) fn sort_from(&mut self, mark: usize) {
        if let Some(tail) = self.entries.get_mut(mark..) {
            sort_by_key(tail, Keyed::key);
        }
        debug_assert!(
            self.entries
                .get(mark.saturating_sub(1)..)
                .unwrap_or_default()
                .is_sorted_by(|a, b| a.key() < b.key()),
            "a sorted table's keys increase"
        );
    }

    /// The entry whose key is `key`, by halving, or `None` where none has
    /// it.
    #[inline]
    pub(crate) fn get(&self, key: T::Key) -> Option<&T> {
        find_sorted(&self.entries, key)
    }

    /// Whether it holds no entry.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A cursor at `key`, found by halving, for a walk that asks keys near
    /// one another.
    #[inline]
    pub(crate) fn cursor(&self, key: T::Key) -> SortedCursor<'_, T> {
        SortedCursor::new(&self.entries, key)
    }

    /// Every entry, in the order of their keys, for the tests to check.
    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[T] {
        &self.entries
    }

    /// Every entry, in the order of their keys, for the tests to check.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator {
        self.entries.iter()
    }
}

/// Where a walk over a sorted table stands: the entries before the key it
/// last asked, and those from it.
///
/// It is sought once by halving. Each later key moves it an entry at a time,
/// forward or back, so a walk asking keys close together pays for the
/// entries between them, not for a search. The breaker's line-edge costs
/// move one with a line's candidates.
#[derive(Copy, Clone, Debug)]
pub(crate) struct SortedCursor<'a, T> {
    entries: &'a [T],
    /// The entries from the key last asked: a suffix of `entries`.
    rest: &'a [T],
}

impl<'a, T: Keyed> SortedCursor<'a, T> {
    /// A cursor over `entries`, sorted by their keys, at `key`, by halving.
    pub(crate) fn new(entries: &'a [T], key: T::Key) -> Self {
        work::seek();
        let found = entries.partition_point(|entry| entry.key() < key);
        Self {
            entries,
            rest: entries.get(found..).unwrap_or_default(),
        }
    }

    /// The entry whose key is `key`, or `None` where none has it.
    ///
    /// Moves the cursor to `key`, an entry at a time.
    #[inline]
    pub(crate) fn get(&mut self, key: T::Key) -> Option<&'a T> {
        while let Some((first, rest)) = self.rest.split_first()
            && first.key() < key
        {
            work::step();
            self.rest = rest;
        }
        loop {
            let taken = self.entries.len() - self.rest.len();
            let Some(before) = taken.checked_sub(1) else {
                break;
            };
            if self
                .entries
                .get(before)
                .is_none_or(|entry| entry.key() < key)
            {
                break;
            }
            work::step();
            self.rest = self.entries.get(before..).unwrap_or_default();
        }
        self.rest.first().filter(|entry| entry.key() == key)
    }
}

impl<T> HeapBytes for SortedTable<T> {
    fn heap_bytes(&self) -> usize {
        self.entries.heap_bytes()
    }
}
