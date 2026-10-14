//! Sparse tables sorted by key, the one search by key, and a cursor for
//! walks in key order.

use alloc::vec::Vec;
use core::ops::RangeInclusive;

use super::{HeapBytes, sort_by_key};
use crate::work;

/// An entry that holds the key its table is sorted and searched by.
pub(crate) trait Keyed {
    /// The key type.
    type Key: Copy + Ord;

    /// The entry's key.
    fn key(&self) -> Self::Key;
}

impl<K: Copy + Ord, V> Keyed for (K, V) {
    type Key = K;

    #[inline]
    fn key(&self) -> K {
        self.0
    }
}

/// Finds, by binary search, the entry whose key is `key`, or `None`.
///
/// `entries` are sorted by key, each key once. This is the one search by key
/// for a [`SortedTable`], a [`Table`](super::Table) in key order, or a slice
/// of either.
#[inline]
pub(crate) fn find_sorted<T: Keyed>(entries: &[T], key: T::Key) -> Option<&T> {
    work::seek();
    let found = entries.partition_point(|entry| entry.key() < key);
    entries.get(found).filter(|entry| entry.key() == key)
}

/// A sparse table sorted by key, searched by binary search.
///
/// Holds entries for the few keys that have a value, such as the content's
/// keyed boxes and the texts ruby spreads room into. The owner pushes entries
/// in key order, or pushes them unsorted and calls
/// [`sort_from`](Self::sort_from).
pub(crate) struct SortedTable<T> {
    entries: Vec<T>,
}

impl<T: Keyed> Default for SortedTable<T> {
    fn default() -> Self {
        Self::new()
    }
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

    /// Appends `entry` out of order, to be sorted by
    /// [`sort_from`](Self::sort_from).
    pub(crate) fn push_unsorted(&mut self, entry: T) {
        self.entries.push(entry);
    }

    /// The number of entries, a mark for [`sort_from`](Self::sort_from).
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Sorts the entries from `mark` on by key. Their keys must be past every
    /// key before `mark`.
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

    /// The entry whose key is `key`, or `None`.
    #[inline]
    pub(crate) fn get(&self, key: T::Key) -> Option<&T> {
        find_sorted(&self.entries, key)
    }

    /// Whether the table holds no entry.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A cursor at `key`, for a walk that asks keys near one another.
    #[inline]
    pub(crate) fn cursor(&self, key: T::Key) -> SortedCursor<'_, T> {
        SortedCursor::new(&self.entries, key)
    }

    /// Returns the entries whose keys lie in `keys`, both ends included.
    pub(crate) fn range(&self, keys: RangeInclusive<T::Key>) -> &[T] {
        work::seek();
        let (first, last) = (*keys.start(), *keys.end());
        let start = self.entries.partition_point(|entry| entry.key() < first);
        let rest = self.entries.get(start..).unwrap_or_default();
        let count = rest.partition_point(|entry| entry.key() <= last);
        rest.get(..count).unwrap_or_default()
    }

    /// Every entry, in key order. Tests only.
    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[T] {
        &self.entries
    }

    /// Every entry, in key order. Tests only.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> + ExactSizeIterator {
        self.entries.iter()
    }
}

/// A position in a walk over entries sorted by key.
///
/// It seeks once by binary search; each later key moves it one entry at a
/// time in either direction, so nearby keys cost the entries between them.
/// The line breaker walks its line-edge costs this way.
#[derive(Copy, Clone, Debug)]
pub(crate) struct SortedCursor<'a, T> {
    entries: &'a [T],
    /// The entries from the key last asked: a suffix of `entries`.
    rest: &'a [T],
}

impl<'a, T: Keyed> SortedCursor<'a, T> {
    /// A cursor over `entries`, sorted by key, at `key`.
    pub(crate) fn new(entries: &'a [T], key: T::Key) -> Self {
        work::seek();
        let found = entries.partition_point(|entry| entry.key() < key);
        Self {
            entries,
            rest: entries.get(found..).unwrap_or_default(),
        }
    }

    /// Moves the cursor to `key` and returns its entry, or `None`.
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
