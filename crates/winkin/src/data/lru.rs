//! A bounded cache that keeps its most recently used values.
//!
//! The context's font and shaping caches and the shared fonts use it.

use super::{HashIndex, HeapBytes, Id, IdRange, Table};

/// How many recently found or added values a [`LruCache`] tries before
/// hashing: the scripts a paragraph of Japanese alternates between, plus one.
const RECENT: usize = 4;

/// A cache whose values are found by the hash of their content, trimmed to a
/// capacity.
///
/// The owner hashes a value's content and says which value matches. The
/// cache keeps a hash index, the few values last used, and a list of
/// entries in order of use.
///
/// Adding never evicts, so an id stays valid until [`trim`](Self::trim). The
/// owner trims between calls, when no id is held, dropping the least
/// recently used values down to the capacity. Dropped entries are reused, so
/// a cache churning at capacity allocates nothing for its entries.
pub(crate) struct LruCache<I, T> {
    entries: Table<I, Entry<I, T>>,
    /// Each live value's id under the hash of its content.
    index: HashIndex,
    /// The value used last.
    newest: Option<I>,
    /// The value used longest ago, which a trim drops first.
    oldest: Option<I>,
    /// The first vacant entry, which links the next through `older`.
    vacant: Option<I>,
    /// The number of entries holding a value.
    len: usize,
    /// The number of values a trim keeps.
    capacity: usize,
    /// The values last found or added, which
    /// [`find_recent`](Self::find_recent) tries before hashing. A paragraph's
    /// runs use a font or two and alternate scripts, so a lookup usually
    /// repeats one of the last few.
    recent: [Option<I>; RECENT],
    /// Where in `recent` the next value found or added goes.
    next_recent: usize,
}

/// An entry of a [`LruCache`]: a value and its place in the order of use, or
/// a vacant entry linking the next vacant one.
struct Entry<I, T> {
    /// The hash the value is indexed under.
    hash: u64,
    /// The value, or `None` for a vacant entry.
    value: Option<T>,
    /// The value used just before this one, or the next vacant entry.
    older: Option<I>,
    /// The value used just after this one.
    newer: Option<I>,
}

impl<I: Id, T> LruCache<I, T> {
    /// Returns an empty cache that keeps `capacity` values, allocating
    /// nothing.
    pub(crate) const fn new(capacity: usize) -> Self {
        Self {
            entries: Table::new(),
            index: HashIndex::new(),
            newest: None,
            oldest: None,
            vacant: None,
            len: 0,
            capacity,
            recent: [None; RECENT],
            next_recent: 0,
        }
    }

    /// Drops every value, keeping the entries' capacity.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.index.clear();
        self.newest = None;
        self.oldest = None;
        self.vacant = None;
        self.len = 0;
        self.recent = [None; RECENT];
        self.next_recent = 0;
    }

    /// Returns the number of values. Tests only.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Whether every id is taken, so [`insert`](Self::insert) refuses.
    pub(crate) fn is_full(&self) -> bool {
        self.vacant.is_none() && self.entries.remaining() == 0
    }

    /// Sets how many values a trim keeps.
    pub(crate) fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
    }

    /// Returns the value `id` names, or `None` if the cache does not hold it.
    #[inline]
    pub(crate) fn get(&self, id: I) -> Option<&T> {
        self.entries.get(id)?.value.as_ref()
    }

    /// Returns the value `id` names mutably, or `None` if the cache does not
    /// hold it.
    pub(crate) fn get_mut(&mut self, id: I) -> Option<&mut T> {
        self.entries.get_mut(id)?.value.as_mut()
    }

    /// Returns every value with its id, in id order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (I, &T)> {
        self.entries
            .iter()
            .filter_map(|(id, entry)| Some((id, entry.value.as_ref()?)))
    }

    /// Returns the value hashing to `hash` that `matches` accepts, without
    /// counting it as used.
    #[inline]
    pub(crate) fn peek(&self, hash: u64, matches: impl Fn(&T) -> bool) -> Option<I> {
        self.index
            .find(hash, |id| self.get(id).is_some_and(&matches))
    }

    /// Returns the value hashing to `hash` that `matches` accepts, and
    /// counts it as used.
    #[inline]
    pub(crate) fn find(&mut self, hash: u64, matches: impl Fn(&T) -> bool) -> Option<I> {
        let found = self.peek(hash, matches)?;
        self.touch(found);
        self.remember(found);
        Some(found)
    }

    /// Finds the value `matches` accepts, trying recent values before
    /// hashing, and counts it as used.
    ///
    /// Calls `hash` only on a miss among recent values. If no value matches,
    /// returns the hash as the error, to insert the value under.
    pub(crate) fn find_recent(
        &mut self,
        hash: impl FnOnce() -> u64,
        matches: impl Fn(&T) -> bool,
    ) -> Result<I, u64> {
        // Newest first.
        let newest = (1..=RECENT).map(|back| (self.next_recent + RECENT - back) % RECENT);
        let held = |&id: &I| self.get(id).is_some_and(&matches);
        if let Some(id) = newest
            .filter_map(|at| self.recent.get(at).copied().flatten())
            .find(held)
        {
            self.touch(id);
            return Ok(id);
        }
        let hash = hash();
        self.find(hash, matches).ok_or(hash)
    }

    /// Adds `value`, which hashes to `hash`, as the newest, and returns its
    /// id.
    ///
    /// Never evicts; reuses a vacant entry if there is one. Returns `None`
    /// and keeps nothing if every id is taken.
    pub(crate) fn insert(&mut self, hash: u64, value: T) -> Option<I> {
        let entry = Entry {
            hash,
            value: Some(value),
            older: None,
            newer: None,
        };
        let id = match self.vacant {
            Some(id) => {
                let reused = self.entries.get_mut(id)?;
                self.vacant = reused.older;
                *reused = entry;
                id
            }
            None => self.entries.push(entry)?,
        };
        self.link_newest(id);
        self.index.insert(hash, id);
        self.len += 1;
        self.remember(id);
        Some(id)
    }

    /// Removes and returns the value `id` names, or `None` if the cache does
    /// not hold it.
    pub(crate) fn remove(&mut self, id: I) -> Option<T> {
        let entry = self.entries.get_mut(id)?;
        let value = entry.value.take()?;
        let hash = entry.hash;
        self.unlink(id);
        if let Some(entry) = self.entries.get_mut(id) {
            entry.older = self.vacant;
            entry.newer = None;
        }
        self.vacant = Some(id);
        self.index.remove(hash, id);
        self.len -= 1;
        for recent in &mut self.recent {
            if *recent == Some(id) {
                *recent = None;
            }
        }
        Some(value)
    }

    /// Drops the least recently used values down to the capacity, passing
    /// each to `evicted` with its id.
    ///
    /// Call only when no id is held.
    #[inline]
    pub(crate) fn trim(&mut self, mut evicted: impl FnMut(I, T)) {
        while self.len > self.capacity {
            let Some(id) = self.oldest else { break };
            let Some(value) = self.remove(id) else { break };
            evicted(id, value);
        }
    }

    /// Drops every value `keep` refuses, passing each to `dropped` with its
    /// id.
    pub(crate) fn retain(
        &mut self,
        mut keep: impl FnMut(I, &T) -> bool,
        mut dropped: impl FnMut(I, T),
    ) {
        for id in (I::new(0)..self.entries.next_id()).ids() {
            let refused = self.get(id).is_some_and(|value| !keep(id, value));
            if refused && let Some(value) = self.remove(id) {
                dropped(id, value);
            }
        }
    }

    /// Moves `id` to the newest end of the order of use.
    #[inline]
    fn touch(&mut self, id: I) {
        if self.newest != Some(id) {
            self.unlink(id);
            self.link_newest(id);
        }
    }

    /// Takes `id` out of the order of use, joining its neighbours.
    fn unlink(&mut self, id: I) {
        let Some(entry) = self.entries.get(id) else {
            return;
        };
        let (older, newer) = (entry.older, entry.newer);
        match older.and_then(|older| self.entries.get_mut(older)) {
            Some(entry) => entry.newer = newer,
            None => self.oldest = newer,
        }
        match newer.and_then(|newer| self.entries.get_mut(newer)) {
            Some(entry) => entry.older = older,
            None => self.newest = older,
        }
    }

    /// Puts `id`, which is out of the order of use, at its newest end.
    fn link_newest(&mut self, id: I) {
        let older = self.newest;
        if let Some(entry) = self.entries.get_mut(id) {
            entry.older = older;
            entry.newer = None;
        }
        match older.and_then(|older| self.entries.get_mut(older)) {
            Some(entry) => entry.newer = Some(id),
            None => self.oldest = Some(id),
        }
        self.newest = Some(id);
    }

    /// Records `id` as recent, replacing the oldest recent value.
    fn remember(&mut self, id: I) {
        if let Some(recent) = self.recent.get_mut(self.next_recent) {
            *recent = Some(id);
        }
        self.next_recent = (self.next_recent + 1) % RECENT;
    }
}

impl<I, T> HeapBytes for LruCache<I, T> {
    /// The entries and the index. The owner counts heap memory its values
    /// own.
    fn heap_bytes(&self) -> usize {
        let Self {
            entries,
            index,
            newest: _,
            oldest: _,
            vacant: _,
            len: _,
            capacity: _,
            recent: _,
            next_recent: _,
        } = self;
        entries.heap_bytes() + index.heap_bytes()
    }
}
