//! Tables of runs that tile a range of positions, and cursors that walk them.

use core::fmt;
use core::ops::Range;

use super::{HeapBytes, Id, Table};
use crate::work;

/// A record of [`Runs`], known by its start.
pub(crate) trait Run {
    /// The positions the runs tile. Every table of runs in the crate tiles
    /// clusters.
    type Position: Id;

    /// The run's first position.
    fn start(&self) -> Self::Position;
}

/// Runs that tile a range of positions in order, each ending where the next
/// starts.
///
/// [`containing`](Self::containing) finds one position's run by binary
/// search. A forward walk uses a [`RunCursor`], which passes each run once.
///
/// Paragraphs, item clusters, script runs, font runs and shaped runs are
/// tables of this shape. The table does not store where the last run ends;
/// the owner passes that `end`: the text end, or where its variant stops.
pub(crate) struct Runs<I, T> {
    runs: Table<I, T>,
}

impl<I: Id, T: Run> Runs<I, T> {
    /// An empty table, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self { runs: Table::new() }
    }

    /// Removes every run, keeping the allocation.
    pub(crate) fn clear(&mut self) {
        self.runs.clear();
    }

    /// Makes room for `count` runs in all.
    pub(crate) fn reserve(&mut self, count: usize) {
        self.runs.reserve(count);
    }

    /// Appends `run`, which starts at or after the last run, and returns its
    /// id, or `None` if the table is full.
    pub(crate) fn push(&mut self, run: T) -> Option<I> {
        debug_assert!(
            self.runs
                .last()
                .is_none_or(|last| last.start() <= run.start()),
            "runs start in order"
        );
        self.runs.push(run)
    }

    /// The number of runs.
    #[inline]
    pub(crate) fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether there are no runs.
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Run `id`, or `None` past the last.
    #[inline]
    pub(crate) fn get(&self, id: I) -> Option<&T> {
        self.runs.get(id)
    }

    /// The last run, or `None` if there are none.
    pub(crate) fn last(&self) -> Option<&T> {
        self.runs.last()
    }

    /// Every run with its id, in order.
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = (I, &T)> + ExactSizeIterator {
        self.runs.iter()
    }

    /// Every run, in order. Tests only.
    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[T] {
        self.runs.as_slice()
    }

    /// Every run from `from` on, in order.
    pub(crate) fn rest(&self, from: I) -> &[T] {
        self.runs
            .get_slice(from..self.runs.next_id())
            .unwrap_or_default()
    }

    /// Where run `id` starts, or `end` past the last run.
    #[inline]
    pub(crate) fn start(&self, id: I, end: T::Position) -> T::Position {
        self.runs.get(id).map_or(end, Run::start)
    }

    /// Where the run after `id` starts, which is where `id` ends, or `None`
    /// if `id` is the last.
    #[inline]
    pub(crate) fn next_start(&self, id: I) -> Option<T::Position> {
        self.runs.get(I::new(id.get() + 1)).map(Run::start)
    }

    /// Run `id`'s positions, ending at the next run's start or `end`.
    ///
    /// Past the last run the range is empty, at `end`.
    #[inline]
    pub(crate) fn span(&self, id: I, end: T::Position) -> Range<T::Position> {
        self.start(id, end)..self.next_start(id).unwrap_or(end)
    }

    /// Finds, by binary search, the run containing `pos`: the last that
    /// starts at or before it. `None` if no run does.
    #[inline]
    pub(crate) fn containing(&self, pos: T::Position) -> Option<I> {
        self.last_where(|run| run.start() <= pos)
    }

    /// Finds, by binary search, the last run `before` accepts, or `None`.
    ///
    /// `before` must accept a prefix of the runs. Owners use it to seek by a
    /// position on another axis that run starts map to in order.
    #[inline]
    pub(crate) fn last_where(&self, before: impl FnMut(&T) -> bool) -> Option<I> {
        self.runs.last_where(before)
    }

    /// Finds, by binary search, the first run that starts at or after `at`,
    /// or one past the last if none does.
    pub(crate) fn first_from(&self, at: T::Position) -> I {
        work::seek();
        I::new(self.runs.as_slice().partition_point(|run| run.start() < at))
    }
}

/// A position in a walk over [`Runs`]: the current run and where it ends.
///
/// Holding the end costs the walk one comparison per run and none per
/// position. Walks over shaped runs need it: finding each end from the start
/// bits makes relayout 3 to 5% slower.
///
/// `Copy`, so a walk that looks ahead copies it. The fields are private
/// because only the table keeps `end` equal to the next run's start.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct RunCursor<I, P> {
    id: I,
    end: P,
}

impl<I: Copy, P: Copy> RunCursor<I, P> {
    /// The current run, or one past the last once the walk passes every run.
    #[inline]
    pub(crate) fn id(self) -> I {
        self.id
    }

    /// Where the current run ends: the next run's start, or the range end
    /// for the last run and past it.
    #[inline]
    pub(crate) fn end(self) -> P {
        self.end
    }
}

impl<I: Id, T: Run> Runs<I, T> {
    /// A cursor at run `id`, without a search.
    ///
    /// `end` is where the last run ends. Past the last run the cursor holds
    /// no run and ends at `end`.
    #[inline]
    pub(crate) fn cursor(&self, id: I, end: T::Position) -> RunCursor<I, T::Position> {
        let end = if id.get() < self.runs.len() {
            self.next_start(id).unwrap_or(end)
        } else {
            end
        };
        RunCursor { id, end }
    }

    /// A cursor at the run containing `at`, found by binary search, or `None`
    /// if no run contains it.
    ///
    /// `end` is where the last run ends. A walk with no link to start from
    /// seeks once with this.
    pub(crate) fn cursor_containing(
        &self,
        at: T::Position,
        end: T::Position,
    ) -> Option<RunCursor<I, T::Position>> {
        self.containing(at).map(|id| self.cursor(id, end))
    }

    /// A cursor at the run containing `at`, stepping from run `near` in
    /// either direction, or `None` if no run contains `at`.
    ///
    /// Finds what [`cursor_containing`](Self::cursor_containing) finds, at a
    /// cost of one step per run between `near` and `at`.
    pub(crate) fn cursor_from(
        &self,
        near: I,
        at: T::Position,
        end: T::Position,
    ) -> Option<RunCursor<I, T::Position>> {
        let last = self.runs.len().checked_sub(1)?;
        let mut id = near.get().min(last);
        while id > 0 && self.start(I::new(id), end) > at {
            work::step();
            id -= 1;
        }
        if self.start(I::new(id), end) > at {
            return None;
        }
        let mut cursor = self.cursor(I::new(id), end);
        self.step_to(&mut cursor, at, end);
        Some(cursor)
    }

    /// Moves `cursor` to the next run, even an empty one.
    ///
    /// `end` is where the last run ends. A cursor past the last run stays
    /// there.
    #[inline]
    pub(crate) fn step(&self, cursor: &mut RunCursor<I, T::Position>, end: T::Position) {
        if cursor.id.get() < self.runs.len() {
            *cursor = self.cursor(I::new(cursor.id.get() + 1), end);
        }
    }

    /// Moves `cursor` forward to the run containing `at`.
    ///
    /// Passes each run ending at or before `at`, but never the last run.
    /// Costs one comparison if the cursor already contains `at`.
    #[inline]
    pub(crate) fn step_to(
        &self,
        cursor: &mut RunCursor<I, T::Position>,
        at: T::Position,
        end: T::Position,
    ) {
        while cursor.end <= at && cursor.id.get() + 1 < self.runs.len() {
            *cursor = self.cursor(I::new(cursor.id.get() + 1), end);
        }
    }
}

impl<I: Id, T: fmt::Debug> fmt::Debug for Runs<I, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.runs.fmt(f)
    }
}

impl<I, T> HeapBytes for Runs<I, T> {
    fn heap_bytes(&self) -> usize {
        self.runs.heap_bytes()
    }
}
