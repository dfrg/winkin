//! The crate's data primitives: typed ids, the tables they index, the text
//! position type, and the shapes every stage builds its data from.
//!
//! One rule has no exceptions: an `XId` indexes a table of `X`, and nothing
//! else. An id is a newtype over `u32` (`u16` or `u8` where the table is
//! small), never `usize` and never an alias. So an id cannot index the wrong
//! table, and it cannot pass for a count or an offset. A byte position in the
//! text is a [`TextOffset`], which is not an id: it indexes no table.
//!
//! A [`Table`] hands out its own ids and can only be indexed by them. The `as`
//! casts between ids and `usize` live in this module alone, and so do the
//! casts of [`TextOffset`]. A type change then touches one place, not every
//! call site. The lengths, and the only casts between a float and an integer,
//! are in `unit`.
//!
//! Each submodule holds one primitive, re-exported here, so a stage names
//! what it uses from `data` alone:
//!
//! - `id`: the [`Id`] trait, [`define_id!`], and the casts between an id
//!   and an index;
//! - `offset`: [`TextOffset`];
//! - `table`: [`Table`], and making room in a vector as a table does;
//! - `bits`: [`BitTable`] and [`RankedBitTable`], a bit per id;
//! - `flags`: [`define_flags!`], a set of flags over an integer;
//! - `index`: [`HashIndex`], tables' ids by the hash of what each names;
//! - `lru`: [`LruCache`], a bounded context cache whose entries are found
//!   by hash or among those found last, and which drops those used longest
//!   ago;
//! - `sorted`: [`SortedTable`], a sparse table found by a key, and the one
//!   search by key, [`find_sorted`];
//! - `runs`: [`Runs`], a table of runs tiling a range of positions, each
//!   found by its start, and [`RunCursor`], where a walk over one stands;
//! - `hash`: the one hash, [`FxHasher`];
//! - `heap`: [`HeapBytes`], what a table keeps on the heap, and
//!   [`heap_bytes!`], which sums a struct's tables;
//! - `sort`: the crate's sorts, [`sort_by_key`] and [`stable_sort_by_key`].

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
pub(crate) use table::{Table, make_room, make_text_room};
