//! Ids found by the hash of their records.

use hashbrown::HashTable;

use super::{HeapBytes, Id, index_to_u32, u32_to_index};

/// Ids found by the hash of their records.
///
/// Interning tables and caches keep one beside their records to find a
/// record by content. It stores ids as plain `u32`s, so hashbrown's table,
/// a kilobyte or more of code per type, compiles once for every id type.
///
/// Each call names its id type, so one index may hold several tables' ids.
/// A find's `same` compares within its own table and passes over another
/// table's entry with the same hash.
pub(crate) struct HashIndex {
    entries: HashTable<(u64, u32)>,
}

impl HashIndex {
    /// An empty index, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            entries: HashTable::new(),
        }
    }

    /// Forgets every id, keeping the capacity.
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// The id noted under `hash` that `same` accepts, or `None`.
    ///
    /// Skips entries no `I` can name; they belong to another table.
    #[inline]
    pub(crate) fn find<I: Id>(&self, hash: u64, mut same: impl FnMut(I) -> bool) -> Option<I> {
        self.entries
            .find(hash, |&(noted, id)| {
                noted == hash && I::try_new(u32_to_index(id)).is_some_and(&mut same)
            })
            .and_then(|&(_, id)| I::try_new(u32_to_index(id)))
    }

    /// Notes `id` under `hash`.
    pub(crate) fn insert<I: Id>(&mut self, hash: u64, id: I) {
        self.entries
            .insert_unique(hash, (hash, index_to_u32(id.get())), noted_hash);
    }

    /// Forgets `id`, noted under `hash`, if it is there.
    pub(crate) fn remove<I: Id>(&mut self, hash: u64, id: I) {
        let packed = index_to_u32(id.get());
        if let Ok(entry) = self
            .entries
            .find_entry(hash, |&(noted, held)| noted == hash && held == packed)
        {
            entry.remove();
        }
    }
}

/// The hash an entry of a [`HashIndex`] was noted under. One function serves
/// every index, so growing compiles once.
fn noted_hash(entry: &(u64, u32)) -> u64 {
    entry.0
}

impl HeapBytes for HashIndex {
    fn heap_bytes(&self) -> usize {
        self.entries.heap_bytes()
    }
}
