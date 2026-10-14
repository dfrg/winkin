//! The crate's data primitives: typed ids, the tables they index, and the
//! text position type.
//!
//! An `XId` indexes a table of `X` and nothing else. An id is a newtype over
//! `u32` (`u16` or `u8` for small tables), never `usize` and never an alias,
//! so it cannot index the wrong table or pass for a count. A byte position in
//! the text is a [`TextOffset`], which indexes no table.
//!
//! A [`Table`] hands out its own ids and is indexed only by them. The casts
//! between ids, [`TextOffset`] and `usize` live in this module; the casts
//! between floats and integers live in `unit`.
//!
//! - `id`: the [`Id`] trait, [`define_id!`] and the id-index casts.
//! - `offset`: [`TextOffset`].
//! - `table`: [`Table`], [`Staging`] and [`make_room`].
//! - `bits`: [`BitTable`] and [`RankedBitTable`], one bit per id.
//! - `flags`: [`define_flags!`], a set of flags over an integer.
//! - `index`: [`HashIndex`], ids found by the hash of their records.
//! - `lru`: [`LruCache`], a bounded cache that drops the least recently used.
//! - `sorted`: [`SortedTable`], a sparse table found by key, and
//!   [`find_sorted`], the one search by key.
//! - `runs`: [`Runs`], runs tiling a range of positions, and [`RunCursor`].
//! - `hash`: [`FxHasher`], the one hash.
//! - `heap`: [`HeapBytes`] and [`heap_bytes!`], heap usage of tables.
//! - `sort`: [`sort_by_key`] and [`stable_sort_by_key`].
//! - `sync`: [`Mutex`], blocking with `std` and spinning without.

mod bits;
mod flags;
mod hash;
mod heap;
mod id;
mod index;
mod lru;
mod offset;
mod runs;
mod sort;
mod sorted;
mod sync;
mod table;
#[cfg(test)]
mod tests;

pub(crate) use bits::{BitTable, RankedBitTable};
pub(crate) use flags::define_flags;
pub(crate) use hash::{FxHasher, hash_one};
pub(crate) use heap::{HeapBytes, heap_bytes};
pub(crate) use id::{Id, IdRange, define_id, index_to_u32, u32_to_index};
pub(crate) use index::HashIndex;
pub(crate) use lru::LruCache;
pub(crate) use offset::TextOffset;
pub(crate) use runs::{Run, RunCursor, Runs};
pub(crate) use sort::{sort_by_key, stable_sort_by_key};
pub(crate) use sorted::{Keyed, SortedCursor, SortedTable, find_sorted};
pub(crate) use sync::Mutex;
pub(crate) use table::{Staging, Table, make_room};
