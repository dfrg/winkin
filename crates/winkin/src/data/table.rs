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

/// The widest record whose table grows as a plain vector does: see
/// [`Table::push`].
pub(super) const NARROW_RECORD: usize = 4;

/// Makes room in `items` for `count` in all, as [`Table::reserve`] does.
///
/// A vector that has never held any gets exactly that many. One that has
/// grows by doubling where they do not fit, and is left alone where they do.
/// Where the allocator refuses, nothing happens, and pushing grows it as it
/// would have.
///
/// The tables use it, and so do buffers kept beside them, such as the
/// advances that shaping hands to measurement.
pub(crate) fn make_room<T>(items: &mut Vec<T>, count: usize) {
    let additional = count.saturating_sub(items.len());
    let _refused = if items.capacity() == 0 {
        items.try_reserve_exact(additional)
    } else {
        items.try_reserve(additional)
    };
}

/// A vector that only its own id can index.
///
/// It is how every per-layout table is stored: one per node column, one for
/// the items, one for the clusters, and so on. A stage builds its tables by
/// pushing and freezes them by returning; later stages read them. [`clear`]
/// keeps the capacity, which is how a rebuild allocates nothing.
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

    /// Appends `item` and returns its id, or keeps nothing and returns `None`
    /// if the table is full.
    ///
    /// A table holds at most `I::MAX` items, one short of what `I` can name, so
    /// that its length, the id one past the end, is always an id too.
    ///
    /// Nothing here panics, however large the input. The table's owner
    /// decides what running out means. The builder drops content past its
    /// limits and reports what it dropped. The font stage falls back to a
    /// font it already has. A caller cannot know in advance whether its text,
    /// once collapsed and transformed, crosses a limit. So crossing one is
    /// never fatal, and this returns an `Option` rather than asserting.
    ///
    /// A full table of records wider than 4 bytes doubles its room, from one
    /// at its first push, where a vector's first growth makes room for four.
    /// A small layout most often holds one style, one font and one
    /// paragraph, and keeps the room it grew to for good. A table that goes
    /// on to hold more takes one or two more allocations to reach four.
    ///
    /// Narrower records, the per-cluster tables among them, grow as a vector
    /// does: the size test is a constant, so their pushes test nothing more.
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

    /// Doubles the room of a full table, from one, never past what the
    /// table can hold. Where the allocator refuses, the push grows the table
    /// as a vector would.
    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        let len = self.items.len();
        let _refused = self
            .items
            .try_reserve_exact(len.max(1).min(I::MAX.saturating_sub(len)));
    }

    /// Appends `item`, which the pushing code knows fits.
    ///
    /// Another table's ids already bound this one's length. For example, a
    /// paragraph's flags are one per paragraph, which a `ParagraphId` names.
    /// `invariant` says why at the push. Overflow is a bug in the crate, never
    /// something a caller did. It fails in debug builds and keeps nothing in
    /// release.
    #[inline]
    pub(crate) fn push_bounded(&mut self, item: T, invariant: &'static str) {
        if self.push(item).is_none() {
            debug_assert!(false, "{invariant}");
        }
    }

    /// Appends every item of `items` and returns the range of their ids, or
    /// `None` if they do not all fit.
    ///
    /// Where they do not fit it keeps none of them. A table interning lists
    /// uses it to store each list whole or not at all.
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

    /// Appends every item `items` yields and returns the range of their ids,
    /// or `None` if they do not all fit.
    ///
    /// It works as [`extend_from_slice`](Self::extend_from_slice) does, for
    /// items made as they are stored, such as a list converted on its way in.
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
        // Bounded by the room as well, so an iterator that says less than
        // it yields cannot take the table past what its ids name.
        self.items.extend(items.take(room));
        Some(start..self.next_id())
    }

    /// Sets the item `id` names to `item`, first filling the table with
    /// `fill` up to it where it is shorter.
    ///
    /// A table kept beside another table's ids grows this way as later ids
    /// are reached. The font stage keeps a memo beside a list of fonts like
    /// this. Nothing happens for an id past what the table can hold.
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

    /// Removes the last item and returns it, or `None` where the table is
    /// empty.
    ///
    /// A table used as a stack pops this way, as the builder's open
    /// containers do.
    #[inline]
    pub(crate) fn pop(&mut self) -> Option<T> {
        self.items.pop()
    }

    /// The number of items the table can still take before
    /// [`push`](Self::push) refuses.
    #[inline]
    pub(crate) fn remaining(&self) -> usize {
        I::MAX - self.items.len()
    }

    /// The id the next [`push`](Self::push) will return, which is also the end
    /// of the range of every id handed out so far.
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

    /// The item `id` names, for the stage building the table, or `None` past
    /// the end.
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

    /// The items `range` names, or `None` where it is out of order or runs
    /// past the end.
    ///
    /// It is [`slice`](Self::slice) for a range a reader cannot vouch for,
    /// as [`get`](Self::get) is for indexing.
    #[inline]
    pub(crate) fn get_slice(&self, range: Range<I>) -> Option<&[T]> {
        self.items.get(range.start.get()..range.end.get())
    }

    /// The items `range` names, for the stage building the table, or `None`
    /// where it is out of order or runs past the end.
    #[inline]
    pub(crate) fn get_slice_mut(&mut self, range: Range<I>) -> Option<&mut [T]> {
        self.items.get_mut(range.start.get()..range.end.get())
    }

    /// The last item, or `None` where the table is empty.
    #[inline]
    pub(crate) fn last(&self) -> Option<&T> {
        self.items.last()
    }

    /// The last item, for the stage building the table, or `None` where it
    /// is empty: the top of a table used as a stack.
    #[inline]
    pub(crate) fn last_mut(&mut self) -> Option<&mut T> {
        self.items.last_mut()
    }

    /// The id of the last item, or `None` where the table is empty.
    #[inline]
    pub(crate) fn last_id(&self) -> Option<I> {
        self.items.len().checked_sub(1).map(I::new)
    }

    /// The id of the last item `before` holds of, by halving, or `None`
    /// where it holds of none.
    ///
    /// `before` must hold of every item up to some point and of none after,
    /// as [`slice::partition_point`] requires. A table ordered by where its
    /// entries start uses it to find the entry holding a position, the last
    /// starting at or before it. Examples are the paragraph, run or item
    /// holding a cluster.
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

    /// Every item, in id order, for the stage building the table.
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

    /// Makes room for `count` items in all, for a stage that knows how many
    /// it will push.
    ///
    /// Analysis knows the clusters once it has found them, and the builder
    /// knows the items once it has written them.
    ///
    /// A table that has never held any takes exactly that many. Growing by
    /// doubling would leave it about a third empty for good and allocate once
    /// per doubling on the way. A table that has held items and lacks room
    /// grows by doubling, so a rebuild of growing content still takes few
    /// allocations. One with the room keeps what it has, so a rebuild of
    /// less content keeps the larger capacity.
    ///
    /// It never reserves past what the table can hold, and never panics.
    /// Where the allocator refuses, the pushes grow the table as they would
    /// have.
    pub(crate) fn reserve(&mut self, count: usize) {
        make_room(&mut self.items, count.min(I::MAX));
    }

    /// Removes every item from `to` on, keeping those before it and the
    /// allocation.
    ///
    /// A stage takes back what it wrote on trial this way, as the breaker
    /// takes back a reshaped line edge it did not keep. Nothing happens where
    /// `to` is past the end.
    #[inline]
    pub(crate) fn truncate(&mut self, to: I) {
        self.items.truncate(to.get());
    }
}

impl<I, T> HeapBytes for Table<I, T> {
    /// Its capacity, whose items hold nothing on the heap themselves: a
    /// table whose items do counts theirs on its owner's behalf.
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

    /// Reuses this table's allocation, which a derived `clone_from` would
    /// not.
    fn clone_from(&mut self, source: &Self) {
        self.items.clone_from(&source.items);
    }
}

impl<I: Id, T: fmt::Debug> fmt::Debug for Table<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Reads a row by its id, for tests. The crate reads rows through
/// [`get`](Table::get), which never panics.
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
/// A larger room saves a large table's allocations as it grows, and the
/// context keeps it for good: at 64 KB the context keeps about 200 KB more
/// over the bench documents, for 20 to 30 fewer allocations a new article.
const STAGING_LIMIT: usize = 16384;

/// A table a [`Staging`] room stands in for.
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

/// Room in the context that a stage writes a layout's table in, and the
/// table's capacity before the stage.
///
/// A new layout's table has no capacity. [`lend`](Self::lend) swaps the
/// room in, so the stage writes in the context's allocation, and
/// [`settle`](Self::settle) copies what it wrote to an allocation of its
/// exact length and takes the room back. A table that already has capacity,
/// as a rebuilt layout's does, is written in place, and copied to its exact
/// length only where the stage grew it.
pub(crate) struct Staging<V> {
    room: V,
    /// Whether a table holds the room.
    lent: bool,
    /// The bytes the table's capacity took when the stage began.
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
    /// Lends the room to `table` where the table has no capacity of its own,
    /// and notes the capacity it has otherwise.
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

    /// Leaves `table` its rows at their exact length where it was lent the
    /// room or grew, and takes the room back.
    ///
    /// A room grown past [`STAGING_LIMIT`] is dropped, and grows again in
    /// the next table lent it. Where the allocator refuses the copy, the
    /// table keeps what it has, and the room is made again.
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
