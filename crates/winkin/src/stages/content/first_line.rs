//! The first paragraph as its first line draws it, where a first-line
//! transform differs from a node's own.
//!
//! The content's text holds each node's own transform, which every line but
//! the first shows and every stage reads. Where the block's `::first-line`
//! gives a node another transform, the first line draws other characters.
//! `FirstLineSource` holds them: the first paragraph's text with each such
//! node's text transformed as its first-line style says. It also holds a
//! map from the content's offsets to its own. The first-line measurements
//! use the map to read each cluster's characters by its content offsets.
//!
//! The writer keeps it in step with the text as it writes. From the first
//! text a first-line transform changes, it writes everything here too: the
//! same text where no transform differs, and the first line's transform of
//! the caller's text where one does. It also makes the same edits
//! collapsing makes: a space written where its run began, and the block's
//! leading and trailing trim. It stops where the first paragraph ends, since
//! no first line reaches past it. A document whose first line transforms
//! nothing its own way keeps nothing here.
//!
//! **The map** is a list of entries ([`MapEntry`]), sorted by content offset,
//! each running to the next. Before the first entry the two texts are the
//! same.
//! - In an aligned entry the two texts match byte for byte. They do
//!   wherever the transforms agree, or change a character for one of the
//!   same length.
//! - An entry that is not aligned is one of the caller's characters that
//!   the two transforms made differently long (`ß` in capitals is `SS` in
//!   the text and itself on the first line). It maps as a whole: an offset
//!   inside it goes to its end. So the first cluster it holds draws all of
//!   it and the rest draw nothing, as a ligature's continuations do.
//! - Where the two transforms do not map the caller's characters one for
//!   one, as a Greek accent dropped in context can, the whole run of text
//!   is one entry that is not aligned.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

use super::transform::{Transforms, align_counts};
use crate::data::{TextOffset, heap_bytes};
use crate::work;

/// The first paragraph as its first line draws it, where some node's
/// `::first-line` transform differs from its own.
///
/// The first-line variant selects fonts for this text and shapes it. It is
/// empty unless the first line transforms some text its own way.
#[derive(Default)]
pub(super) struct FirstLineSource {
    /// The text, from the content's start through its first paragraph.
    pub(super) text: String,
    /// Where the content's offsets are in `text`, one entry a piece, sorted.
    pub(super) map: Vec<MapEntry>,
}

heap_bytes! {
    FirstLineSource { text, map }
}

/// One entry of [`FirstLineSource`]'s map, for a piece of text. 12 bytes.
///
/// It records where the piece starts in the content's text and in the first
/// line's. It also records whether the two match byte for byte through it,
/// or map only as wholes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct MapEntry {
    content: TextOffset,
    first_line: TextOffset,
    aligned: bool,
}

/// How many entries a cursor steps back one at a time before it halves.
const BACK_STEPS: usize = 8;

impl FirstLineSource {
    /// No text, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            text: String::new(),
            map: Vec::new(),
        }
    }

    /// Empties it for a new build, keeping the allocations.
    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.map.clear();
    }

    /// Maps content offset `at` into the first line's text.
    ///
    /// In an aligned entry it keeps its distance from the entry's start. In
    /// an entry that is not aligned, its start maps to the entry's start
    /// and an offset inside maps to its end. An offset past what the text
    /// reaches maps to the text's end, since no first line reaches past the
    /// first paragraph.
    ///
    /// An offset before the first entry or from the last on maps at once.
    /// Any other is found by halving.
    pub(super) fn offset(&self, at: TextOffset) -> TextOffset {
        let count = self.edge_count(at).unwrap_or_else(|| {
            work::seek();
            self.map.partition_point(|entry| entry.content <= at)
        });
        self.mapped(at, count)
    }

    /// Maps content offset `at` as [`offset`](Self::offset) does, moving
    /// `after` to it.
    ///
    /// `after` counts the entries that start at or before the offset asked
    /// last. It steps an entry at a time to `at`, so offsets asked in order
    /// search nothing. Back past a few entries it halves. An offset before
    /// the first entry or from the last on maps at once and leaves `after`.
    pub(super) fn offset_stepped(&self, at: TextOffset, after: &Cell<usize>) -> TextOffset {
        if let Some(count) = self.edge_count(at) {
            return self.mapped(at, count);
        }
        let map = &self.map;
        // Whether the first `count` entries start at or before `at`, and
        // whether the rest start after it.
        let through = |count: usize| {
            count
                .checked_sub(1)
                .is_none_or(|last| map.get(last).is_some_and(|entry| entry.content <= at))
        };
        let before = |count: usize| map.get(count).is_none_or(|entry| entry.content > at);
        let mut count = after.get().min(map.len());
        let mut back = 0;
        while !(through(count) && before(count)) {
            work::step();
            if through(count) {
                count += 1;
            } else if back < BACK_STEPS {
                back += 1;
                count = count.saturating_sub(1);
            } else {
                work::seek();
                count = map.partition_point(|entry| entry.content <= at);
                break;
            }
        }
        after.set(count);
        self.mapped(at, count)
    }

    /// How many entries start at or before content offset `at`, where it is
    /// before the first entry or from the last on: `None` between.
    fn edge_count(&self, at: TextOffset) -> Option<usize> {
        if self.map.first().is_none_or(|first| first.content > at) {
            Some(0)
        } else if self.map.last().is_some_and(|last| last.content <= at) {
            Some(self.map.len())
        } else {
            None
        }
    }

    /// Maps content offset `at`, which `after` entries start at or before.
    fn mapped(&self, at: TextOffset, after: usize) -> TextOffset {
        let here = match after.checked_sub(1).and_then(|last| self.map.get(last)) {
            None => at.get(),
            Some(entry) if entry.aligned => {
                entry.first_line.get() + (at.get() - entry.content.get())
            }
            Some(entry) if entry.content == at => entry.first_line.get(),
            Some(_) => self
                .map
                .get(after)
                .map_or(self.text.len(), |next| next.first_line.get()),
        };
        TextOffset::new(here.min(self.text.len()))
    }

    /// Returns the first line's text for the content's `range`.
    pub(super) fn slice(&self, range: Range<TextOffset>) -> &str {
        let (start, end) = (self.offset(range.start), self.offset(range.end));
        self.text.get(start.get()..end.get()).unwrap_or_default()
    }

    /// Starts keeping the first line's text, from a copy of the content's
    /// text so far, `text`.
    pub(super) fn start(&mut self, text: &str) {
        self.clear();
        self.text.push_str(text);
    }

    /// Whether it holds anything.
    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Appends `text`, which the content's text took at `at` and the first
    /// line draws the same.
    pub(super) fn push_same(&mut self, at: TextOffset, text: &str) {
        self.align_here(at, true);
        self.text.push_str(text);
    }

    /// Opens an entry at content offset `at` and this text's end, aligned or
    /// not, unless an aligned one already runs on there.
    fn align_here(&mut self, at: TextOffset, aligned: bool) {
        let open_aligned = self.map.last().is_none_or(|last| last.aligned);
        if aligned && open_aligned {
            return;
        }
        self.map.push(MapEntry {
            content: at,
            first_line: TextOffset::new(self.text.len()),
            aligned,
        });
    }

    /// Appends the caller's `src` as the first line draws it.
    ///
    /// The content's text took `written` for `src` at `at`. This appends
    /// `src` as `transforms` has the first line transform it, and aligns it
    /// with `written` a caller's character at a time. `before` is the
    /// caller's character before `src`, and `scratch` is where capitalize
    /// joins the two.
    pub(super) fn push_transformed(
        &mut self,
        at: TextOffset,
        written: &str,
        src: &str,
        before: char,
        transforms: Transforms,
        scratch: &mut String,
    ) {
        let from = TextOffset::new(self.text.len());
        match transforms.first_line() {
            Some(transformer) => transformer.write(src, before, scratch, &mut self.text),
            None => self.text.push_str(src),
        }
        self.align(at, written, from, |each| {
            transforms.counts(src, before, scratch, each);
        });
    }

    /// Aligns `content`, which the content's text took at `at`, with what
    /// this text took from `from` to its end.
    ///
    /// The two transforms made both of one run of the caller's text.
    /// `counts` gives, for each of the caller's characters in order, how
    /// many characters each made of it. Where those do not add up to what
    /// was written, the whole run becomes one entry that is not aligned.
    fn align(
        &mut self,
        at: TextOffset,
        content: &str,
        from: TextOffset,
        counts: impl FnOnce(&mut dyn FnMut(char, usize, usize)),
    ) {
        let Self { text, map } = self;
        let mark = map.len();
        let first = text.get(from.get()..).unwrap_or_default();
        let whole = align_counts(content, first, counts, |made| {
            work::step();
            let aligned = made.is_aligned();
            let open_aligned = map.last().is_none_or(|last| last.aligned);
            if !(aligned && open_aligned) {
                map.push(MapEntry {
                    content: TextOffset::new(at.get() + made.ours.start.get()),
                    first_line: TextOffset::new(from.get() + made.theirs.start.get()),
                    aligned,
                });
            }
        });
        if !whole {
            map.truncate(mark);
            map.push(MapEntry {
                content: at,
                first_line: from,
                aligned: false,
            });
        }
    }

    /// Inserts the space the content's text took at `at`, where a run of
    /// white space began.
    ///
    /// The same space goes where `at` maps here, and every entry after it
    /// moves along by one.
    pub(super) fn insert_space(&mut self, at: TextOffset) {
        let here = self.offset(at).get();
        if here > self.text.len() || !self.text.is_char_boundary(here) {
            debug_assert!(false, "a space goes on a boundary of the first line's text");
            return;
        }
        self.text.insert(here, ' ');
        for entry in self.map.iter_mut().rev() {
            if entry.content <= at {
                break;
            }
            entry.content = TextOffset::new(entry.content.get() + 1);
            entry.first_line = TextOffset::new(entry.first_line.get() + 1);
        }
    }

    /// Drops what the content's first `cut` bytes map to here, for the
    /// block's leading trim.
    pub(super) fn drain_front(&mut self, cut: TextOffset) {
        let here = self.offset(cut).get().min(self.text.len());
        if !self.text.is_char_boundary(here) {
            debug_assert!(false, "a trim cuts on a boundary of the first line's text");
            return;
        }
        self.text.drain(..here);
        self.map.retain(|entry| entry.content >= cut);
        for entry in &mut self.map {
            entry.content = TextOffset::new(entry.content.get() - cut.get());
            entry.first_line = TextOffset::new(entry.first_line.get().saturating_sub(here));
        }
    }

    /// Cuts this text where content offset `cut` maps, for the block's
    /// trailing trim.
    pub(super) fn truncate(&mut self, cut: TextOffset) {
        let here = self.offset(cut).get();
        if here < self.text.len() && self.text.is_char_boundary(here) {
            self.text.truncate(here);
        }
        self.map.retain(|entry| entry.content < cut);
    }
}
