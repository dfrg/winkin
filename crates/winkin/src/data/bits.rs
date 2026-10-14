//! One bit per id of a table, with or without a rank count per word.

use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
use core::ops::Range;

use super::{HeapBytes, Id, make_room};

/// One bit per id of a table: bit `i % 64` of word `i / 64`.
///
/// Searches for set bits read 64 ids per word; the line breaker finds break
/// opportunities this way. The owner pushes each bit with its item, so the
/// two stay in step.
pub(crate) struct BitTable<I> {
    words: Vec<u64>,
    id: PhantomData<fn(I) -> I>,
}

impl<I: Id> BitTable<I> {
    /// An empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            words: Vec::new(),
            id: PhantomData,
        }
    }

    /// Removes every bit and keeps the allocation.
    pub(crate) fn clear(&mut self) {
        self.words.clear();
    }

    /// Makes room for the bits of `count` ids in all, as
    /// [`Table::reserve`](super::Table::reserve) does.
    pub(crate) fn reserve(&mut self, count: usize) {
        make_room(&mut self.words, count.min(I::MAX).div_ceil(64));
    }

    /// Appends the bit for `id`, set if `set`.
    ///
    /// `id` must be the next id after every one pushed so far.
    pub(crate) fn push(&mut self, id: I, set: bool) {
        let at = id.get();
        if at.is_multiple_of(64) {
            self.words.push(0);
        }
        if set && let Some(word) = self.words.last_mut() {
            *word |= 1u64 << (at % 64);
        }
    }

    /// The last id in `range` whose bit is set, or `None`.
    pub(crate) fn last(&self, range: Range<I>) -> Option<I> {
        let (from, below) = (range.start.get(), range.end.get());
        if below <= from {
            return None;
        }
        let mut at = below - 1;
        loop {
            let index = at / 64;
            let word = self.words.get(index).copied().unwrap_or(0);
            // The bits at and below `at` in its word.
            let masked = word & (u64::MAX >> (63 - at % 64));
            if masked != 0 {
                let found = index * 64 + (63 - masked.leading_zeros() as usize);
                return (found >= from).then(|| I::new(found));
            }
            at = (index * 64).checked_sub(1)?;
            if at < from {
                return None;
            }
        }
    }

    /// The first id in `range` whose bit is set, or `None`.
    pub(crate) fn first(&self, range: Range<I>) -> Option<I> {
        let (from, below) = (range.start.get(), range.end.get());
        let mut at = from;
        while at < below {
            let index = at / 64;
            let word = self.words.get(index)?;
            // The bits at and above `at` in its word.
            let masked = word & (u64::MAX << (at % 64));
            if masked != 0 {
                let found = index * 64 + masked.trailing_zeros() as usize;
                return (found < below).then(|| I::new(found));
            }
            at = (index + 1) * 64;
        }
        None
    }

    /// The bytes the bits take, excluding spare capacity.
    #[cfg(test)]
    pub(crate) fn bytes(&self) -> usize {
        self.words.len() * size_of::<u64>()
    }
}

impl<I> HeapBytes for BitTable<I> {
    fn heap_bytes(&self) -> usize {
        self.words.heap_bytes()
    }
}

impl<I: Id> Default for BitTable<I> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I> fmt::Debug for BitTable<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.words).finish()
    }
}

/// A [`BitTable`] set in increasing order, with a count of the bits set
/// before each 64-bit word.
///
/// Counting the set bits through an id takes one read and one popcount.
/// When each record starts at a set bit, the record containing an id is that
/// count less one, found without a search. Shaping's run starts use it.
pub(crate) struct RankedBitTable<I> {
    bits: BitTable<I>,
    /// For each word, the number of bits set in the words before it.
    before: Vec<u32>,
}

impl<I: Id> RankedBitTable<I> {
    /// An empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            bits: BitTable::new(),
            before: Vec::new(),
        }
    }

    /// Removes every bit and keeps the allocation.
    pub(crate) fn clear(&mut self) {
        self.bits.clear();
        self.before.clear();
    }

    /// Makes room for the bits of `count` ids in all and their counts, as
    /// [`Table::reserve`](super::Table::reserve) does.
    pub(crate) fn reserve(&mut self, count: usize) {
        self.bits.reserve(count);
        make_room(&mut self.before, count.min(I::MAX).div_ceil(64));
    }

    /// Sets the bit for `id`, leaving the ids since the last one set unset.
    ///
    /// Ids come in increasing order, so a word's count is recorded when an id
    /// past it is set.
    pub(crate) fn set(&mut self, id: I) {
        let at = id.get();
        let index = at / 64;
        let words = &mut self.bits.words;
        while words.len() <= index {
            let counted = match (self.before.last(), words.last()) {
                (Some(&before), Some(word)) => before.saturating_add(word.count_ones()),
                _ => 0,
            };
            words.push(0);
            self.before.push(counted);
        }
        if let Some(word) = words.get_mut(index) {
            *word |= 1u64 << (at % 64);
        }
    }

    /// The number of ids at or before `id` whose bit is set.
    #[inline]
    pub(crate) fn count_through(&self, id: I) -> usize {
        let at = id.get();
        let index = at / 64;
        let words = &self.bits.words;
        match (words.get(index), self.before.get(index)) {
            (Some(&word), Some(&before)) => {
                // The bits at and below `at` in its word.
                let held = word & (u64::MAX >> (63 - at % 64));
                usize::try_from(before)
                    .unwrap_or(usize::MAX)
                    .saturating_add(held.count_ones() as usize)
            }
            // Past the last word: every set bit counts.
            _ => match (words.last(), self.before.last()) {
                (Some(&word), Some(&before)) => usize::try_from(before)
                    .unwrap_or(usize::MAX)
                    .saturating_add(word.count_ones() as usize),
                _ => 0,
            },
        }
    }
}

impl<I> HeapBytes for RankedBitTable<I> {
    fn heap_bytes(&self) -> usize {
        self.bits.heap_bytes() + self.before.heap_bytes()
    }
}
