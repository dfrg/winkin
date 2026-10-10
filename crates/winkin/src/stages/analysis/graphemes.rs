//! The grapheme boundaries the cluster writer starts clusters at.

use core::mem;

use icu_segmenter::GraphemeClusterSegmenterBorrowed;
use icu_segmenter::iterators::GraphemeClusterBreakIterator;
use icu_segmenter::scaffold::Utf8;

use crate::unicode::RareProps;

/// The grapheme boundaries of a text, asked about a character at a time, in
/// order.
///
/// Between two characters whose Grapheme_Cluster_Break stands alone (Other,
/// a control, CR or LF) there is always a boundary, except inside a CRLF.
/// Every rule of UAX #29 that keeps two characters together names another
/// class on one side. Every Latin-1 character stands alone, so Latin-1 text
/// reads no table for it. Ideographs, kana and most letters beyond Latin-1
/// read their rare word.
///
/// So only a character that may join a neighbour, or the one after it, asks
/// the segmenter. Where the segmenter has fallen behind, it restarts at the
/// last known boundary. Like the line segmenter, it goes on from a boundary
/// as from the text's start.
pub(super) struct GraphemeCursor<'a> {
    text: &'a str,
    segmenter: GraphemeClusterSegmenterBorrowed<'static>,
    /// The segmenter's iterator, over the text from `from`.
    iter: GraphemeClusterBreakIterator<'static, 'a, Utf8>,
    from: usize,
    /// The next boundary the segmenter found, at or after the last position
    /// it was asked about.
    next: usize,
    /// The last boundary known, from the segmenter or between two characters
    /// that stand alone: where the segmenter starts again.
    known: usize,
    /// The character before the one being asked about, or [`char::MAX`] at
    /// the text's start.
    before: char,
    /// Whether that character's Grapheme_Cluster_Break stands alone: false at
    /// the text's start, where the segmenter is asked.
    before_alone: bool,
}

impl<'a> GraphemeCursor<'a> {
    /// Starts at the start of `text`, segmenting with `segmenter`.
    pub(super) fn new(text: &'a str, segmenter: GraphemeClusterSegmenterBorrowed<'static>) -> Self {
        let mut iter = segmenter.segment_str(text);
        // The segmenter says 0 first: the text's start is a boundary.
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
    /// `rare` is `ch`'s rare word past Latin-1. Every character is asked
    /// about, in order, except those [`skip_to`](Self::skip_to) passes.
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
            // It says 0 first, which is `from`.
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

    /// Goes on from `ch`, a Latin-1 character starting a grapheme at byte
    /// `at`, past characters that were not asked about.
    pub(super) fn skip_to(&mut self, at: usize, ch: char) {
        self.before = ch;
        self.before_alone = true;
        self.known = at;
    }
}
