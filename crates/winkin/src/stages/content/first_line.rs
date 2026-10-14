//! The first paragraph's text as the first line draws it, when
//! `::first-line` gives some node a different `text-transform`.
//!
//! The content's text holds each node's own transform. [`FirstLineSource`]
//! holds the first paragraph with first-line transforms applied, and a map
//! from content offsets to its own, so first-line font selection, shaping
//! and measurement read a cluster's characters by content offsets.
//!
//! The writer starts it at the first text whose first-line transform
//! differs, copying the text so far, and from then on mirrors every write
//! and every collapsing edit (a space inserted where its run began, the
//! block's leading and trailing trim). It stops at the end of the first
//! paragraph. It stays empty when no first-line transform differs.
//!
//! **The map** is a list of [`MapEntry`] sorted by content offset, each
//! running to the next. Before the first entry the texts are identical.
//! - An aligned entry matches byte for byte.
//! - An unaligned entry is a source character the two transforms made of
//!   different lengths (`ß` is `SS` in capitals). It maps as a whole: an
//!   offset inside it maps to its end, so its first cluster draws all of it,
//!   as a ligature does.
//! - When the transforms don't map characters one for one (a Greek accent
//!   dropped in context), the whole run is one unaligned entry.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

use super::transform::{Transforms, align_counts};
use crate::data::{TextOffset, heap_bytes};
use crate::work;

/// The first paragraph's text with first-line transforms applied.
///
/// It lives in the content's boxed extras and is empty unless some
/// first-line transform differs.
#[derive(Default)]
pub(super) struct FirstLineSource {
    /// The text, from the content's start through its first paragraph.
    pub(super) text: String,
    /// Entries mapping content offsets into `text`, sorted.
    pub(super) map: Vec<MapEntry>,
}

heap_bytes! {
    FirstLineSource { text, map }
}

/// One map entry: where a piece starts in the content's text and in the
/// first line's, and whether the two match byte for byte. 12 bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct MapEntry {
    content: TextOffset,
    first_line: TextOffset,
    aligned: bool,
}

/// How many entries a cursor steps back before it binary searches.
const BACK_STEPS: usize = 8;

impl FirstLineSource {
    /// Creates an empty source, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            text: String::new(),
            map: Vec::new(),
        }
    }

    /// Empties it, keeping the allocations.
    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.map.clear();
    }

    /// Maps content offset `at` into the first line's text.
    ///
    /// In an aligned entry the offset keeps its distance from the entry's
    /// start. In an unaligned entry the start maps to the start and any
    /// offset inside to the end. Offsets past the text clamp to its end.
    /// Offsets before the first entry or in the last map directly; others
    /// are binary searched.
    pub(super) fn offset(&self, at: TextOffset) -> TextOffset {
        let count = self.edge_count(at).unwrap_or_else(|| {
            work::seek();
            self.map.partition_point(|entry| entry.content <= at)
        });
        self.mapped(at, count)
    }

    /// Maps content offset `at` as [`offset`](Self::offset) does, using
    /// `after` as a cursor.
    ///
    /// `after` counts the entries starting at or before the last offset
    /// asked. It steps toward `at`, so offsets asked in order cost no search.
    /// Stepping back more than [`BACK_STEPS`] binary searches. Offsets before
    /// the first entry or in the last leave `after` unchanged.
    pub(super) fn offset_stepped(&self, at: TextOffset, after: &Cell<usize>) -> TextOffset {
        if let Some(count) = self.edge_count(at) {
            return self.mapped(at, count);
        }
        let map = &self.map;
        // `count` is right when entries before it start at or before `at`
        // and the rest start after it.
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

    /// Returns how many entries start at or before `at` when `at` is before
    /// the first entry or in the last; `None` otherwise.
    fn edge_count(&self, at: TextOffset) -> Option<usize> {
        if self.map.first().is_none_or(|first| first.content > at) {
            Some(0)
        } else if self.map.last().is_some_and(|last| last.content <= at) {
            Some(self.map.len())
        } else {
            None
        }
    }

    /// Maps content offset `at`, given that `after` entries start at or
    /// before it.
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

    /// Starts the source with `text`, the content's text so far.
    pub(super) fn start(&mut self, text: &str) {
        self.clear();
        self.text.push_str(text);
    }

    /// Whether it holds no text.
    pub(super) fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Appends `text`, written to the content at `at` and identical on the
    /// first line.
    pub(super) fn push_same(&mut self, at: TextOffset, text: &str) {
        self.align_here(at, true);
        self.text.push_str(text);
    }

    /// Opens an entry at content offset `at` and the current text end,
    /// unless it is aligned and the open entry is too.
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

    /// Appends source text `src` with the first-line transform applied.
    ///
    /// The content wrote `written` for `src` at `at`; the two are aligned
    /// one source character at a time. `before` is the source character
    /// before `src`, for capitalize; `scratch` is segmenter scratch.
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

    /// Maps `content`, written to the content at `at`, onto this text from
    /// `from` to its end.
    ///
    /// Both come from one source run; `counts` gives each source
    /// character's output length in each. If the counts don't add up, the
    /// whole run becomes one unaligned entry.
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

    /// Inserts the collapsed space the content inserted at `at`, shifting
    /// later entries by one byte.
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

    /// Drops what the content's first `cut` bytes map to, for the block's
    /// leading trim.
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

    /// Truncates at content offset `cut`, for the block's trailing trim.
    pub(super) fn truncate(&mut self, cut: TextOffset) {
        let here = self.offset(cut).get();
        if here < self.text.len() && self.text.is_char_boundary(here) {
            self.text.truncate(here);
        }
        self.map.retain(|entry| entry.content < cut);
    }
}
