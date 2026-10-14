//! The grapheme boundaries the cluster writer starts clusters at.

use core::mem;

use icu_segmenter::GraphemeClusterSegmenterBorrowed;
use icu_segmenter::iterators::GraphemeClusterBreakIterator;
use icu_segmenter::scaffold::Utf8;

use crate::unicode::RareProps;

/// The grapheme boundaries of a text, queried one character at a time in
/// order.
///
/// Two characters whose Grapheme_Cluster_Break stands alone (Other, a
/// control, CR or LF) always have a boundary between them, except inside a
/// CRLF: every UAX #29 rule that joins two characters names another class.
/// Every Latin-1 character stands alone, so Latin-1 text reads no table.
///
/// Only a character that may join a neighbour, or the one after it, asks
/// the segmenter. A segmenter that has fallen behind restarts at the last
/// known boundary, which segments the rest as it would from the text's start.
pub(super) struct GraphemeCursor<'a> {
    text: &'a str,
    segmenter: GraphemeClusterSegmenterBorrowed<'static>,
    /// The segmenter's iterator over the text from byte `from`.
    iter: GraphemeClusterBreakIterator<'static, 'a, Utf8>,
    from: usize,
    /// The next boundary from the segmenter, at or after the last position
    /// queried.
    next: usize,
    /// The last known boundary, where a restarted segmenter starts.
    known: usize,
    /// The previous character, or [`char::MAX`] at the text's start.
    before: char,
    /// Whether the previous character stands alone; false at the text's
    /// start, so the first query asks the segmenter.
    before_alone: bool,
}

impl<'a> GraphemeCursor<'a> {
    /// Starts at the start of `text`, segmenting with `segmenter`.
    pub(super) fn new(text: &'a str, segmenter: GraphemeClusterSegmenterBorrowed<'static>) -> Self {
        let mut iter = segmenter.segment_str(text);
        // The first boundary is 0, the text's start.
        let next = iter.next().unwrap_or(usize::MAX);
        Self {
            text,
            segmenter,
            iter,
            from: 0,
            next,
            known: 0,
            before: char::MAX,
            before_alone: false,
        }
    }

    /// Returns whether there is a grapheme boundary at byte `at`, before
    /// `ch`.
    ///
    /// `rare` is `ch`'s rare word, or `None` for Latin-1. Every character is
    /// queried in order, except those [`skip_to`](Self::skip_to) passes.
    pub(super) fn boundary(&mut self, at: usize, ch: char, rare: Option<RareProps>) -> bool {
        let before = mem::replace(&mut self.before, ch);
        let alone = rare.is_none_or(|rare| rare.grapheme_cluster_break().stands_alone());
        let before_alone = mem::replace(&mut self.before_alone, alone);
        if alone && before_alone && !(before == '\r' && ch == '\n') {
            self.known = at;
            return true;
        }
        if self.next < self.known {
            let from = self.known;
            self.iter = self
                .segmenter
                .segment_str(self.text.get(from..).unwrap_or_default());
            // Skips the first boundary, `from` itself.
            self.iter.next();
            self.from = from;
            self.next = from;
        }
        while self.next < at {
            self.next = match self.iter.next() {
                Some(next) => self.from.saturating_add(next),
                None => usize::MAX,
            };
        }
        let boundary = self.next == at;
        if boundary {
            self.known = at;
        }
        boundary
    }

    /// Resumes at `ch`, a Latin-1 character starting a grapheme at byte `at`,
    /// after characters that were not queried.
    pub(super) fn skip_to(&mut self, at: usize, ch: char) {
        self.before = ch;
        self.before_alone = true;
        self.known = at;
    }
}
