//! A vector only its own id indexes, and making room in one.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::marker::PhantomData;
#[cfg(test)]
use core::ops::Index;
use core::ops::Range;

use super::{HeapBytes, Id, IdRange};
use crate::work;

/// The record size from which a table makes room for only one record at its
/// first push.
///
/// A plain vector makes room for four of anything up to a kilobyte. See
/// [`Table::push`].
pub(super) const LARGE_RECORD: usize = 64;

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

/// Makes room in `text` for `bytes` more, as [`make_room`] does in a vector.
///
/// The content's text uses it as a call hands text in, and so does a family
/// list as it interns names.
pub(crate) fn make_text_room(text: &mut String, bytes: usize) {
    let _refused = if text.capacity() == 0 {
        text.try_reserve_exact(bytes)
    } else {
        text.try_reserve(bytes)
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
    /// A table of large records makes room for one at its first push, where
    /// a vector's first growth makes room for four. Styles, used fonts,
    /// lines, font instances and shape plans are such records. A small
    /// layout, or a context that has set text in one font, most often holds
    /// one, and keeps its room for good. A table that goes on to hold more
    /// grows to four from there, one allocation more.
    #[inline]
    #[must_use]
    pub(crate) fn push(&mut self, item: T) -> Option<I> {
        let len = self.items.len();
        if len >= I::MAX {
            return None;
        }
        if size_of::<T>() >= LARGE_RECORD && self.items.capacity() == 0 {
            let _refused = self.items.try_reserve_exact(1);
        }
        self.items.push(item);
        Some(I::new(len))
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
