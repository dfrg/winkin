//! The line segmenters' keys, and the line stream that asks one where lines
//! may break.
//!
//! ICU4X's line segmenter takes its options when it is made and never after.
//! An iterator over text can neither start with context nor change its
//! options. So the context keeps one segmenter per resolved set of options
//! (see `context`).
//!
//! When a paragraph's stream meets a style whose options differ, it
//! **resumes**. It runs the new segmenter from the last opportunity before
//! the seam and discards what that says before the seam. Under one set of
//! options, UAX #14 decides nothing after an opportunity by what came before
//! it. So resuming at an opportunity reproduces the uncut stream, which a
//! test in `tests` checks. The options after the seam govern the pair at the
//! seam.
//!
//! The stream restarts only at a change of options and a paragraph's start.
//! It does not restart at objects (U+FFFC is in the text, and context runs
//! through it) or at box edges.
//!
//! Most text never reaches ICU:
//! - The writer asks Chrome's own ASCII rules (`ascii`) first, and the stream
//!   only where they do not decide, as Blink asks its iterator. Under
//!   `break-all` they take Blink's break-all table too. So ASCII text runs
//!   no segmenter at all, except under `anywhere`.
//! - Where the options let it, the stream reads Latin-1 opportunities off a
//!   pair table of ICU's own answers (`pairs`). It hands over to ICU and
//!   back at opportunities, where both go on as from the text's start.
//!
//! The opportunities are ICU's either way. The tests hold the table to ICU
//! for every pair it covers. They hold the stream to ICU over UAX #14's test
//! strings and text that switches back and forth.

use icu_locale_core::{LanguageIdentifier, langid};
use icu_segmenter::LineSegmenterBorrowed;
use icu_segmenter::iterators::LineBreakIterator;
use icu_segmenter::options::{LineBreakOptions, LineBreakStrictness, LineBreakWordOption};
use icu_segmenter::scaffold::Utf8;

use crate::style::{LineBreak, WordBreak};
use crate::work;

/// A language the line segmenter tailors for: only whether the language is
/// Japanese or Chinese matters to it, and both mean the same.
static JAPANESE: LanguageIdentifier = langid!("ja");

/// The options a line segmenter is made with, resolved from a style: 24
/// values.
///
/// Two styles that break alike resolve to one key, so a stream resumes only
/// where the breaks can differ.
/// - `line-break: anywhere` makes the word option and the language
///   irrelevant. `keep-all` with `anywhere` is anywhere, as CSS says, not
///   ICU's keep-all.
/// - The language matters only under `normal` and `loose`.
/// - A value the style does not set is CSS's initial one, never ICU's
///   `strict` default.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct LineKey {
    strictness: LineBreak,
    word: WordBreak,
    /// Whether the content language is Japanese or Chinese.
    ja_zh: bool,
}

impl LineKey {
    /// How many keys there are, and segmenters in the context.
    pub(super) const COUNT: usize = 24;

    /// The key text breaks under with `line-break: strictness` and
    /// `word-break: word`.
    ///
    /// `cjk` says the language is Japanese or Chinese (the builder's
    /// `TextFlags::CJK_LINE_BREAKS`).
    pub(super) fn new(strictness: LineBreak, word: WordBreak, cjk: bool) -> Self {
        match strictness {
            LineBreak::Anywhere => Self {
                strictness,
                word: WordBreak::Normal,
                ja_zh: false,
            },
            LineBreak::Strict => Self {
                strictness,
                word,
                ja_zh: false,
            },
            LineBreak::Loose | LineBreak::Normal => Self {
                strictness,
                word,
                ja_zh: cjk,
            },
        }
    }

    /// Whether the pair table (`pairs`) answers for text breaking under the
    /// key.
    ///
    /// It does for `loose`, `normal` and `strict` with `word-break: normal`,
    /// in any language: ICU breaks the table's characters alike under all of
    /// them. `break-all` and `keep-all` change their classes, and `anywhere`
    /// breaks between graphemes.
    pub(super) fn breaks_by_pairs(self) -> bool {
        self.word == WordBreak::Normal && self.strictness != LineBreak::Anywhere
    }

    /// Returns Chrome's answer between `before` and `after` where its ASCII
    /// rules decide it under the key, or `None` where it asks ICU.
    ///
    /// `before_before` finds the character before `before`, which only a
    /// hyphen before a digit reads.
    /// - Under `line-break: anywhere`, none: Blink breaks between graphemes
    ///   (`kBreakCharacter`).
    /// - Under `word-break: break-all`, Blink's break-all table over its
    ///   ASCII rules (`ascii::breaks_all`).
    /// - Otherwise its ASCII rules (`ascii::breaks`), under every strictness
    ///   and `keep-all`. Blink asks its iterator only where they do not
    ///   decide.
    pub(super) fn ascii_answer(
        self,
        before_before: impl FnOnce() -> Option<char>,
        before: char,
        after: char,
    ) -> Option<bool> {
        match (self.strictness, self.word) {
            (LineBreak::Anywhere, _) => None,
            (_, WordBreak::BreakAll) => super::ascii::breaks_all(before_before, before, after),
            _ => super::ascii::breaks(before_before, before, after),
        }
    }

    /// Whether text breaking under the key is `line-break: normal`, under
    /// which a small kana may start a line unless the config holds it.
    pub(super) fn is_normal(self) -> bool {
        self.strictness == LineBreak::Normal
    }

    /// The key's slot among the context's segmenters.
    pub(super) fn index(self) -> usize {
        let strictness = match self.strictness {
            LineBreak::Loose => 0,
            LineBreak::Normal => 1,
            LineBreak::Strict => 2,
            LineBreak::Anywhere => 3,
        };
        let word = match self.word {
            WordBreak::Normal => 0,
            WordBreak::BreakAll => 1,
            WordBreak::KeepAll => 2,
        };
        strictness * 6 + word * 2 + usize::from(self.ja_zh)
    }

    /// The key in slot `index`, the other way from [`index`](Self::index).
    pub(super) fn from_index(index: usize) -> Self {
        let strictness = match index / 6 {
            0 => LineBreak::Loose,
            1 => LineBreak::Normal,
            2 => LineBreak::Strict,
            _ => LineBreak::Anywhere,
        };
        let word = match (index % 6) / 2 {
            0 => WordBreak::Normal,
            1 => WordBreak::BreakAll,
            _ => WordBreak::KeepAll,
        };
        Self {
            strictness,
            word,
            ja_zh: index % 2 == 1,
        }
    }

    /// The options ICU takes for the key.
    pub(super) fn options(self) -> LineBreakOptions<'static> {
        // Non-exhaustive, so made from its default and filled in.
        let mut options = LineBreakOptions::default();
        options.strictness = Some(self.strictness.into());
        options.word_option = Some(word_option(self.word));
        options.content_locale = self.ja_zh.then_some(&JAPANESE);
        options
    }
}

impl From<LineBreak> for LineBreakStrictness {
    /// The strictness ICU breaks at under `line-break: value`, the four CSS
    /// values being ICU's four.
    fn from(value: LineBreak) -> Self {
        match value {
            LineBreak::Loose => Self::Loose,
            LineBreak::Normal => Self::Normal,
            LineBreak::Strict => Self::Strict,
            LineBreak::Anywhere => Self::Anywhere,
        }
    }
}

/// ICU's word option for `word-break: word`, the three CSS values being
/// ICU's three.
///
/// This is a function rather than a `From`, since both types are other
/// crates'.
fn word_option(word: WordBreak) -> LineBreakWordOption {
    match word {
        WordBreak::Normal => LineBreakWordOption::Normal,
        WordBreak::BreakAll => LineBreakWordOption::BreakAll,
        WordBreak::KeepAll => LineBreakWordOption::KeepAll,
    }
}

/// One paragraph's line-break opportunities, asked for in text order.
///
/// Holds one opportunity ahead of what was asked, and the last opportunity
/// behind it, which is where a resume starts. Every position is a byte
/// offset into the whole text.
///
/// The opportunities come from the pair table where it covers the text
/// (`pairs`), and from ICU's iterator elsewhere. Both go on from an
/// opportunity as from the text's start, so each hands over to the other at
/// one. When the table meets a character it does not cover, it starts ICU
/// at the opportunity it went on from. At each opportunity ICU finds, the
/// table is tried again.
///
/// The opportunities are ICU's either way. ICU never starts inside text
/// that needs its dictionaries or its LSTM, since the table covers none of
/// it.
pub(super) struct LineStream<'t> {
    text: &'t str,
    segmenter: LineSegmenterBorrowed<'static>,
    /// ICU's iterator where it is answering, from `base`; `None` where the
    /// table is.
    iter: Option<LineBreakIterator<'static, 't, Utf8>>,
    /// Where `iter`'s text starts in the whole text.
    base: usize,
    key: LineKey,
    /// Whether the table answers for text under `key`.
    pairs: bool,
    /// The last opportunity before the last position asked about, or where
    /// the stream started.
    last: usize,
    /// The next opportunity at or after the last position asked about, or
    /// `usize::MAX` once there are none.
    ahead: usize,
}

impl<'t> LineStream<'t> {
    /// A stream over `text` from `at`, which starts a paragraph, under `key`.
    pub(super) fn new(
        text: &'t str,
        at: usize,
        segmenter: LineSegmenterBorrowed<'static>,
        key: LineKey,
    ) -> Self {
        let mut stream = Self {
            text,
            segmenter,
            iter: None,
            base: at,
            key,
            pairs: key.breaks_by_pairs(),
            last: at,
            ahead: at,
        };
        stream.start(at);
        stream
    }

    /// The key the stream breaks under.
    pub(super) fn key(&self) -> LineKey {
        self.key
    }

    /// Goes on from `from` as from the text's start, by the table where the
    /// key lets it and by ICU otherwise. `from` becomes `ahead`, and says
    /// nothing.
    fn start(&mut self, from: usize) {
        self.ahead = from;
        self.iter = None;
        if !self.pairs {
            self.start_icu(from);
        }
    }

    /// Asks ICU from `from`, which is `ahead`.
    fn start_icu(&mut self, from: usize) {
        let mut iter = self
            .segmenter
            .segment_str(self.text.get(from..).unwrap_or_default());
        // ICU says 0 first, which is `from` and says nothing.
        iter.next();
        self.iter = Some(iter);
        self.base = from;
    }

    /// Finds the next opportunity after `ahead`.
    fn advance(&mut self) {
        if self.ahead >= self.text.len() {
            self.ahead = usize::MAX;
            return;
        }
        // The table first, from every opportunity: where it finds the next,
        // ICU is done with until it cannot.
        if self.pairs
            && super::pairs::may_cover(self.text, self.ahead)
            && let Some(at) = super::pairs::next_break(self.text, self.ahead)
        {
            self.ahead = at;
            self.iter = None;
            return;
        }
        // ICU, from here where the table was answering, or on from where it
        // is, so that text the table leaves again and again, a Latin word
        // among ideographs, costs no new iterator each time.
        if self.iter.is_none() {
            self.start_icu(self.ahead);
        }
        let Some(iter) = &mut self.iter else {
            return;
        };
        self.ahead = match iter.next() {
            Some(at) => self.base.saturating_add(at),
            None => usize::MAX,
        };
    }

    /// Whether a line may break at `at`. Positions are asked about in
    /// increasing order.
    pub(super) fn at(&mut self, at: usize) -> bool {
        while self.ahead < at {
            self.last = self.ahead;
            self.advance();
        }
        self.ahead == at
    }

    /// Goes on under `key` from `seam`, where a style breaking under it
    /// starts.
    ///
    /// `segmenter` runs from the last opportunity before the seam, and what
    /// it says before the seam is dropped. So the pair at the seam is
    /// decided under `key`, with its context, and nothing before it moves.
    pub(super) fn resume(
        &mut self,
        segmenter: LineSegmenterBorrowed<'static>,
        key: LineKey,
        seam: usize,
    ) {
        while self.ahead < seam {
            self.last = self.ahead;
            self.advance();
        }
        let from = self.last;
        self.segmenter = segmenter;
        self.key = key;
        self.pairs = key.breaks_by_pairs();
        self.start(from);
        while self.ahead < seam {
            self.advance();
        }
    }
}

/// Whether a line may break at byte `at` of `text` under `segmenter`.
///
/// This is one short run of the segmenter over text a caller puts together.
/// Chrome's `CanBreakAfterRubyColumn` asks its break iterator the same way,
/// over a ruby base and what follows the column. The text's start and end
/// say nothing of a pair, so neither breaks.
pub(super) fn is_break_opportunity(
    segmenter: LineSegmenterBorrowed<'static>,
    text: &str,
    at: usize,
) -> bool {
    if at == 0 || at >= text.len() {
        return false;
    }
    for found in segmenter.segment_str(text) {
        work::step();
        if found >= at {
            return found == at;
        }
    }
    false
}

#[cfg(test)]
impl LineKey {
    /// The key UAX #14's own conformance tests assume: `strict`, `normal`
    /// words, no language.
    pub(super) const UAX14: Self = Self {
        strictness: LineBreak::Strict,
        word: WordBreak::Normal,
        ja_zh: false,
    };

    /// Every key's slot is its own, and the slots are all used.
    pub(super) fn round_trips() -> bool {
        (0..Self::COUNT).all(|index| Self::from_index(index).index() == index)
    }
}
