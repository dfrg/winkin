//! The line segmenters' keys, and the line stream that asks one where lines
//! may break.
//!
//! An ICU4X line segmenter takes its options when it is made, and its
//! iterator can neither start with context nor change options. So the
//! context keeps one segmenter per resolved set of options (`context`).
//!
//! - **Resuming.** Where a style's options differ, the paragraph's stream
//!   runs the new segmenter from the last opportunity before the seam and
//!   drops what it says before the seam. Under one set of options, UAX #14
//!   decides nothing after an opportunity by what came before it, so this
//!   reproduces the uncut stream (checked in `tests`). The new options decide
//!   the pair at the seam.
//! - **Restarts** happen only at a change of options and a paragraph's
//!   start, not at objects (U+FFFC is in the text) or box edges.
//! - **Most text never reaches ICU.** The writer asks Chrome's ASCII rules
//!   (`ascii`) first and the stream only where they do not decide, as Blink
//!   asks its iterator, with Blink's break-all table under `break-all`.
//!   ASCII text runs no segmenter except under `anywhere`. Where the options allow, the stream reads Latin-1
//!   opportunities from a pair table of ICU's answers (`pairs`), handing
//!   over to ICU and back at opportunities.
//!
//! The opportunities are ICU's either way. The tests hold the table to ICU
//! for every pair it covers, and the stream to ICU over UAX #14's test
//! strings and text that switches back and forth.

use icu_locale_core::{LanguageIdentifier, langid};
use icu_segmenter::LineSegmenterBorrowed;
use icu_segmenter::iterators::LineBreakIterator;
use icu_segmenter::options::{LineBreakOptions, LineBreakStrictness, LineBreakWordOption};
use icu_segmenter::scaffold::Utf8;

use crate::style::{LineBreak, WordBreak};
use crate::work;

/// The language passed for Japanese or Chinese text. The segmenter tailors
/// only for those two, alike.
static JAPANESE: LanguageIdentifier = langid!("ja");

/// The options a line segmenter is made with, resolved from a style: one of
/// 16 values.
///
/// Styles that break alike resolve to one key, so a stream resumes only
/// where the breaks can differ.
/// - `line-break: anywhere` ignores the word option and the language, so
///   `keep-all` with `anywhere` is anywhere, as CSS says.
/// - The language matters only under `normal` and `loose`.
/// - An unset value is CSS's initial one, never ICU's `strict` default.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct LineKey {
    strictness: LineBreak,
    word: WordBreak,
    /// Whether the content language is Japanese or Chinese.
    ja_zh: bool,
}

impl LineKey {
    /// The number of keys, and of segmenters in the context.
    pub(super) const COUNT: usize = 16;

    /// Returns the key for `line-break: strictness` and `word-break: word`.
    ///
    /// `cjk` says the language is Japanese or Chinese
    /// (`TextFlags::CJK_LINE_BREAKS`).
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

    /// Returns whether the pair table (`pairs`) answers under the key.
    ///
    /// It does for `loose`, `normal` and `strict` with `word-break: normal`,
    /// in any language, since ICU breaks the table's characters alike under
    /// all of them. `break-all` and `keep-all` change their classes, and
    /// `anywhere` breaks between graphemes.
    pub(super) fn breaks_by_pairs(self) -> bool {
        self.word == WordBreak::Normal && self.strictness != LineBreak::Anywhere
    }

    /// Returns Chrome's answer between `before` and `after` where its ASCII
    /// rules decide it under the key, or `None` where it asks ICU.
    ///
    /// `before_before` finds the character before `before`; only a hyphen
    /// before a digit reads it.
    /// - Under `line-break: anywhere`, always `None`: Blink breaks between
    ///   graphemes (`kBreakCharacter`).
    /// - Under `word-break: break-all`, Blink's break-all table over its
    ///   ASCII rules (`ascii::breaks_all`).
    /// - Otherwise its ASCII rules (`ascii::breaks`), under every strictness
    ///   and `keep-all`.
    fn ascii_answer(
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

    /// Returns whether a line may break between `before` and `after`, by
    /// Chrome's ASCII rules ([`ascii_answer`](Self::ascii_answer)) where they
    /// decide and by `fallback` elsewhere.
    #[inline]
    pub(super) fn ascii_or(
        self,
        before_before: impl FnOnce() -> Option<char>,
        before: char,
        after: char,
        fallback: impl FnOnce() -> bool,
    ) -> bool {
        self.ascii_answer(before_before, before, after)
            .unwrap_or_else(fallback)
    }

    /// Returns whether the key is `line-break: normal`, under which a small
    /// kana may start a line unless the config holds it.
    pub(super) fn is_normal(self) -> bool {
        self.strictness == LineBreak::Normal
    }

    /// Returns the key's slot among the context's segmenters.
    pub(super) fn index(self) -> usize {
        let word = match self.word {
            WordBreak::Normal => 0,
            WordBreak::BreakAll => 1,
            WordBreak::KeepAll => 2,
        };
        match self.strictness {
            LineBreak::Loose => word * 2 + usize::from(self.ja_zh),
            LineBreak::Normal => 6 + word * 2 + usize::from(self.ja_zh),
            LineBreak::Strict => 12 + word,
            LineBreak::Anywhere => 15,
        }
    }

    /// Returns the key in slot `index`, the inverse of
    /// [`index`](Self::index).
    pub(super) fn from_index(index: usize) -> Self {
        let word = |at| match at {
            0 => WordBreak::Normal,
            1 => WordBreak::BreakAll,
            _ => WordBreak::KeepAll,
        };
        match index {
            0..6 => Self::new(LineBreak::Loose, word(index / 2), index % 2 == 1),
            6..12 => Self::new(LineBreak::Normal, word(index / 2 - 3), index % 2 == 1),
            12..15 => Self::new(LineBreak::Strict, word(index - 12), false),
            _ => Self::new(LineBreak::Anywhere, WordBreak::Normal, false),
        }
    }

    /// Returns ICU's options for the key.
    pub(super) fn options(self) -> LineBreakOptions<'static> {
        // Non-exhaustive, so built from its default.
        let mut options = LineBreakOptions::default();
        options.strictness = Some(self.strictness.into());
        options.word_option = Some(word_option(self.word));
        options.content_locale = self.ja_zh.then_some(&JAPANESE);
        options
    }
}

impl From<LineBreak> for LineBreakStrictness {
    /// Returns ICU's strictness for `line-break: value`; the four values
    /// match one to one.
    fn from(value: LineBreak) -> Self {
        match value {
            LineBreak::Loose => Self::Loose,
            LineBreak::Normal => Self::Normal,
            LineBreak::Strict => Self::Strict,
            LineBreak::Anywhere => Self::Anywhere,
        }
    }
}

/// Returns ICU's word option for `word-break: word`; the three values match
/// one to one.
///
/// A function rather than a `From`, since both types are foreign.
fn word_option(word: WordBreak) -> LineBreakWordOption {
    match word {
        WordBreak::Normal => LineBreakWordOption::Normal,
        WordBreak::BreakAll => LineBreakWordOption::BreakAll,
        WordBreak::KeepAll => LineBreakWordOption::KeepAll,
    }
}

/// One paragraph's line-break opportunities, asked for in text order.
///
/// Holds the next opportunity at or after the last position asked about,
/// and the one before it, where a resume starts. Positions are byte offsets
/// into the whole text.
///
/// Opportunities come from the pair table (`pairs`) where it covers the
/// text and from ICU's iterator elsewhere. Both go on from an opportunity
/// as from the text's start, so they hand over to each other there. ICU
/// starts at the last opportunity the table found, and the table is tried
/// again at each opportunity ICU finds. ICU never starts inside text that
/// needs its dictionaries or LSTM, since the table covers none of it.
pub(super) struct LineStream<'t> {
    text: &'t str,
    segmenter: LineSegmenterBorrowed<'static>,
    /// ICU's iterator, over the text from `base`; `None` while the table
    /// answers.
    iter: Option<LineBreakIterator<'static, 't, Utf8>>,
    /// The byte offset of `iter`'s text in the whole text.
    base: usize,
    key: LineKey,
    /// Whether the table answers for text under `key`.
    pairs: bool,
    /// The last opportunity before the last position asked about, or the
    /// stream's start.
    last: usize,
    /// The next opportunity at or after the last position asked about, or
    /// `usize::MAX` once there are none.
    ahead: usize,
}

impl<'t> LineStream<'t> {
    /// Creates a stream over `text` under `key`, from `at`, a paragraph's
    /// start.
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

    /// Returns the key the stream breaks under.
    pub(super) fn key(&self) -> LineKey {
        self.key
    }

    /// Goes on from `from` as from the text's start, by the table where the
    /// key allows and by ICU otherwise. `from` becomes `ahead` but is not an
    /// opportunity itself.
    fn start(&mut self, from: usize) {
        self.ahead = from;
        self.iter = None;
        if !self.pairs {
            self.start_icu(from);
        }
    }

    /// Starts ICU at `from`, which is `ahead`.
    fn start_icu(&mut self, from: usize) {
        let mut iter = self
            .segmenter
            .segment_str(self.text.get(from..).unwrap_or_default());
        // Skips ICU's first answer, 0, which is `from`.
        iter.next();
        self.iter = Some(iter);
        self.base = from;
    }

    /// Moves `ahead` to the next opportunity, or `usize::MAX` if none.
    fn advance(&mut self) {
        if self.ahead >= self.text.len() {
            self.ahead = usize::MAX;
            return;
        }
        // The table first, at every opportunity. Where it answers, ICU's
        // iterator is dropped.
        if self.pairs
            && super::pairs::may_cover(self.text, self.ahead)
            && let Some(at) = super::pairs::next_break(self.text, self.ahead)
        {
            self.ahead = at;
            self.iter = None;
            return;
        }
        // ICU starts here if the table was answering, or goes on where it
        // is, so text the table keeps declining costs no new iterator at
        // each opportunity.
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

    /// Returns whether a line may break at byte `at`. Positions must be asked
    /// in increasing order.
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
    /// `segmenter` runs from the last opportunity before the seam, and its
    /// answers before the seam are dropped. So `key` decides the pair at the
    /// seam, with its context, and nothing before it changes.
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

/// Returns whether a line may break at byte `at` of `text` under
/// `segmenter`; never at the text's start or end.
///
/// Runs the segmenter once over assembled text, as Chrome's
/// `CanBreakAfterRubyColumn` asks its break iterator over a ruby base and
/// what follows the column.
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
    /// The key UAX #14's conformance tests assume: `strict`, `normal` words,
    /// no language.
    pub(super) const UAX14: Self = Self {
        strictness: LineBreak::Strict,
        word: WordBreak::Normal,
        ja_zh: false,
    };

    /// Returns whether every key's slot maps back to it, covering all slots.
    pub(super) fn round_trips() -> bool {
        (0..Self::COUNT).all(|index| Self::from_index(index).index() == index)
    }
}
