//! A vector only its own id indexes, and making room in one.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
use core::mem;
#[cfg(test)]
use core::ops::Index;
use core::ops::Range;

use super::{HeapBytes, Id, IdRange};
use crate::work;

/// The widest record, in bytes, whose table grows as a plain vector does.
/// See [`Table::push`].
pub(super) const NARROW_RECORD: usize = 4;

/// Makes room in `items` for `count` items in all, as [`Table::reserve`]
/// does.
///
/// An unused vector gets exactly `count`. A used one doubles where `count`
/// does not fit and is left alone where it does. An allocation failure is
/// ignored; pushes then grow the vector as usual.
///
/// Shaping's advances and measurement's sums use it beside the tables.
pub(crate) fn make_room<T>(items: &mut Vec<T>, count: usize) {
    let additional = count.saturating_sub(items.len());
    let _refused = if items.capacity() == 0 {
        items.try_reserve_exact(additional)
    } else {
        items.try_reserve(additional)
    };
}

/// A vector indexed only by its own id type.
///
/// Most per-layout tables are one. A stage builds its tables by pushing and
/// freezes them by returning. [`clear`] keeps the capacity, so a rebuild
/// allocates nothing.
///
/// [`clear`]: Table::clear
pub(crate) struct Table<I, T> {
    items: Vec<T>,
    id: PhantomData<fn(I) -> I>,
}

impl<I: Id, T> Table<I, T> {
    /// An empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            items: Vec::new(),
            id: PhantomData,
        }
    }

    /// Appends `item` and returns its id, or returns `None` and keeps nothing
    /// if the table is full.
    ///
    /// A table holds at most `I::MAX` items, one fewer than `I` can name, so
    /// its length is always an id too.
    ///
    /// Never panics. The owner decides what a full table means: the builder
    /// drops content and reports it, and font selection reuses a font it
    /// already has. Input cannot tell in advance whether collapsed and
    /// transformed text crosses a limit, so crossing one is never fatal.
    ///
    /// A full table of records wider than [`NARROW_RECORD`] doubles its room,
    /// starting from one, where a vector starts from four. A small layout
    /// often holds one style, one font and one paragraph, and keeps that room.
    ///
    /// Narrower records, such as per-cluster tables, grow as a vector does.
    /// The size test is a constant, so their pushes test nothing more.
    /// Testing every push slows a warm rebuild by about 4%.
    #[inline]
    #[must_use]
    pub(crate) fn push(&mut self, item: T) -> Option<I> {
        let len = self.items.len();
        if len >= I::MAX {
            return None;
        }
        if size_of::<T>() > NARROW_RECORD && len == self.items.capacity() {
            self.grow();
        }
        self.items.push(item);
        Some(I::new(len))
    }

    /// Doubles the room of a full table, starting from one, up to `I::MAX`.
    /// On allocation failure the push grows the table as a vector would.
    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        let len = self.items.len();
        let _refused = self
            .items
            .try_reserve_exact(len.max(1).min(I::MAX.saturating_sub(len)));
    }

    /// Appends `item` to a table whose length another table's ids bound.
    ///
    /// `invariant` says why the item fits. Overflow is a bug in the crate: it
    /// fails in debug builds and keeps nothing in release.
    #[inline]
    pub(crate) fn push_bounded(&mut self, item: T, invariant: &'static str) {
        if self.push(item).is_none() {
            debug_assert!(false, "{invariant}");
        }
    }

    /// Appends all of `items` and returns their ids, or returns `None` and
    /// keeps none of them if they do not all fit.
    #[must_use]
    pub(crate) fn extend_from_slice(&mut self, items: &[T]) -> Option<Range<I>>
    where
        T: Clone,
    {
        if items.len() > self.remaining() {
            return None;
        }
        let start = self.next_id();
        self.items.extend_from_slice(items);
        Some(start..self.next_id())
    }

    /// Appends every item `items` yields and returns their ids, or returns
    /// `None` and keeps none of them if they do not all fit.
    #[must_use]
    pub(crate) fn extend<It>(&mut self, items: It) -> Option<Range<I>>
    where
        It: IntoIterator<Item = T>,
        It::IntoIter: ExactSizeIterator,
    {
        let items = items.into_iter();
        let room = self.remaining();
        if items.len() > room {
            return None;
        }
        let start = self.next_id();
        // An iterator may yield more than its length says.
        self.items.extend(items.take(room));
        Some(start..self.next_id())
    }

    /// Sets the item at `id` to `item`, first padding the table with `fill`
    /// up to `id`.
    ///
    /// Font selection's memo beside a font list grows this way. Does nothing
    /// for an id at or past `I::MAX`.
    pub(crate) fn set_growing(&mut self, id: I, item: T, fill: T)
    where
        T: Clone,
    {
        let at = id.get();
        if at >= I::MAX {
            return;
        }
        if self.items.len() <= at {
            self.items.resize(at + 1, fill);
        }
        if let Some(slot) = self.items.get_mut(at) {
            *slot = item;
        }
    }

    /// Removes and returns the last item, or `None` if the table is empty.
    #[inline]
    pub(crate) fn pop(&mut self) -> Option<T> {
        self.items.pop()
    }

    /// The number of items the table can still take.
    #[inline]
    pub(crate) fn remaining(&self) -> usize {
        I::MAX - self.items.len()
    }

    /// The id the next [`push`](Self::push) returns: the end of every id
    /// handed out so far.
    #[inline]
    pub(crate) fn next_id(&self) -> I {
        I::new(self.items.len())
    }

    /// The number of items.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the table holds nothing.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The item `id` names, or `None` past the end.
    #[inline]
    pub(crate) fn get(&self, id: I) -> Option<&T> {
        self.items.get(id.get())
    }

    /// The item `id` names, mutably, or `None` past the end.
    #[inline]
    pub(crate) fn get_mut(&mut self, id: I) -> Option<&mut T> {
        self.items.get_mut(id.get())
    }

    /// The items `range` names.
    ///
    /// # Panics
    ///
    /// If the range is out of order or past the end.
    #[inline]
    pub(crate) fn slice(&self, range: Range<I>) -> &[T] {
        &self.items[range.start.get()..range.end.get()]
    }

    /// The items `range` names, or `None` if it is out of order or runs past
    /// the end.
    #[inline]
    pub(crate) fn get_slice(&self, range: Range<I>) -> Option<&[T]> {
        self.items.get(range.start.get()..range.end.get())
    }

    /// The items `range` names, mutably, or `None` if it is out of order or
    /// runs past the end.
    #[inline]
    pub(crate) fn get_slice_mut(&mut self, range: Range<I>) -> Option<&mut [T]> {
        self.items.get_mut(range.start.get()..range.end.get())
    }

    /// The last item, or `None` if the table is empty.
    #[inline]
    pub(crate) fn last(&self) -> Option<&T> {
        self.items.last()
    }

    /// The last item, mutably, or `None` if the table is empty.
    #[inline]
    pub(crate) fn last_mut(&mut self) -> Option<&mut T> {
        self.items.last_mut()
    }

    /// The id of the last item, or `None` if the table is empty.
    #[inline]
    pub(crate) fn last_id(&self) -> Option<I> {
        self.items.len().checked_sub(1).map(I::new)
    }

    /// Finds, by binary search, the id of the last item `before` accepts, or
    /// `None` if it accepts none.
    ///
    /// `before` must accept a prefix of the items, as
    /// [`slice::partition_point`] requires. In a table ordered by start, this
    /// finds the entry containing a position.
    #[inline]
    pub(crate) fn last_where(&self, before: impl FnMut(&T) -> bool) -> Option<I> {
        work::seek();
        self.items
            .partition_point(before)
            .checked_sub(1)
            .map(I::new)
    }

    /// Every item, in id order.
    #[inline]
    pub(crate) fn as_slice(&self) -> &[T] {
        &self.items
    }

    /// Every item, mutably, in id order.
    #[inline]
    pub(crate) fn as_mut_slice(&mut self) -> &mut [T] {
        &mut self.items
    }

    /// Every id, in order.
    pub(crate) fn ids(&self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator {
        (I::new(0)..self.next_id()).ids()
    }

    /// Every item with its id, in id order.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = (I, &T)> + ExactSizeIterator {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| (I::new(index), item))
    }

    /// Removes every item and keeps the allocation.
    #[inline]
    pub(crate) fn clear(&mut self) {
        self.items.clear();
    }

    /// Makes room for `count` items in all.
    ///
    /// An unused table takes exactly `count`; doubling would leave it about a
    /// third empty and allocate once per doubling. A used table that lacks
    /// room doubles, so growing content still takes few allocations. One with
    /// room keeps its capacity.
    ///
    /// Never reserves past `I::MAX` and never panics. On allocation failure
    /// pushes grow the table as usual.
    pub(crate) fn reserve(&mut self, count: usize) {
        make_room(&mut self.items, count.min(I::MAX));
    }

    /// Removes every item from `to` on and keeps the allocation.
    ///
    /// Line breaking rewinds trial lines and reshaped edges this way. Does
    /// nothing if `to` is past the end.
    #[inline]
    pub(crate) fn truncate(&mut self, to: I) {
        self.items.truncate(to.get());
    }
}

impl<I, T> HeapBytes for Table<I, T> {
    /// The table's capacity in bytes. Heap memory its items own is not
    /// counted; their owner counts it.
    fn heap_bytes(&self) -> usize {
        self.items.heap_bytes()
    }
}

impl<I: Id, T> Default for Table<I, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<I: Id, T: Clone> Clone for Table<I, T> {
    fn clone(&self) -> Self {
        Self {
            items: self.items.clone(),
            id: PhantomData,
        }
    }

    /// Reuses this table's allocation, which a derived `clone_from` does not.
    fn clone_from(&mut self, source: &Self) {
        self.items.clone_from(&source.items);
    }
}

impl<I: Id, T: fmt::Debug> fmt::Debug for Table<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Panicking indexing, for tests only. The crate reads rows with
/// [`get`](Table::get).
#[cfg(test)]
impl<I: Id, T> Index<I> for Table<I, T> {
    type Output = T;

    #[inline]
    fn index(&self, id: I) -> &T {
        &self.items[id.get()]
    }
}

/// The most bytes a [`Staging`] room keeps between builds.
///
/// A larger room saves allocations for large tables but stays on the context.
/// At 64 KB the context keeps about 200 KB more over the bench documents, for
/// 20 to 30 fewer allocations per new article.
const STAGING_LIMIT: usize = 16384;

/// A table that a [`Staging`] room can be lent to.
pub(crate) trait Staged {
    /// Returns the bytes its capacity takes.
    fn capacity_bytes(&self) -> usize;

    /// Returns a copy with no spare capacity, or `None` where the allocator
    /// refuses.
    fn exact_copy(&self) -> Option<Self>
    where
        Self: Sized;

    /// Removes everything and keeps the allocation.
    fn clear(&mut self);
}

/// A context-owned allocation that a stage writes a layout's table into.
///
/// A new layout's table has no capacity. [`lend`](Self::lend) swaps the room
/// in, and [`settle`](Self::settle) copies the result to an exact-length
/// allocation and takes the room back. A table with capacity, as in a rebuilt
/// layout, is written in place and copied only if the stage grew it.
pub(crate) struct Staging<V> {
    room: V,
    /// Whether a table holds the room.
    lent: bool,
    /// The table's capacity in bytes when the stage began.
    before: usize,
}

impl<V> Staging<V> {
    /// Returns a staging that lends `room`, which holds nothing.
    pub(crate) const fn new(room: V) -> Self {
        Self {
            room,
            lent: false,
            before: 0,
        }
    }
}

impl<V: Staged> Staging<V> {
    /// Lends the room to `table` if it has no capacity, and notes its
    /// capacity otherwise. Does nothing while the room is already lent.
    #[inline]
    pub(crate) fn lend(&mut self, table: &mut V) {
        if self.lent {
            return;
        }
        self.before = table.capacity_bytes();
        if self.before == 0 {
            mem::swap(table, &mut self.room);
            self.lent = true;
        }
    }

    /// Copies `table` to an exact-length allocation if it was lent the room
    /// or grew, and takes the room back.
    ///
    /// A room grown past [`STAGING_LIMIT`] is dropped. If the copy fails to
    /// allocate, the table keeps the room and the staging starts a new one.
    #[inline]
    pub(crate) fn settle(&mut self, table: &mut V) {
        let lent = mem::take(&mut self.lent);
        if !lent && table.capacity_bytes() <= self.before {
            return;
        }
        let Some(copy) = table.exact_copy() else {
            return;
        };
        let room = mem::replace(table, copy);
        if lent && room.capacity_bytes() <= STAGING_LIMIT {
            self.room = room;
            self.room.clear();
        }
    }
}

impl<V: HeapBytes> HeapBytes for Staging<V> {
    fn heap_bytes(&self) -> usize {
        self.room.heap_bytes()
    }
}

impl<I: Id, T: Clone> Staged for Table<I, T> {
    fn capacity_bytes(&self) -> usize {
        self.items.heap_bytes()
    }

    fn exact_copy(&self) -> Option<Self> {
        let mut items = Vec::new();
        items.try_reserve_exact(self.items.len()).ok()?;
        items.extend_from_slice(&self.items);
        Some(Self {
            items,
            id: PhantomData,
        })
    }

    fn clear(&mut self) {
        self.items.clear();
    }
}

impl Staged for String {
    fn capacity_bytes(&self) -> usize {
        self.capacity()
    }

    fn exact_copy(&self) -> Option<Self> {
        let mut text = String::new();
        text.try_reserve_exact(self.len()).ok()?;
        text.push_str(self);
        Some(text)
    }

    fn clear(&mut self) {
        String::clear(self);
    }
}
