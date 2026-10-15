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
//! - **Curly double quotes** under the Japanese and Chinese options are read
//!   as brackets, as ICU4C's rules for those languages read them
//!   ([`fill_cj_text`]).
//!
//! The opportunities are ICU's either way. The tests hold the table to ICU
//! for every pair it covers, and the stream to ICU over UAX #14's test
//! strings and text that switches back and forth. The writer adds one
//! tailoring over them: ICU4X keeps the LB19 of Unicode 15.0, so the breaks
//! LB19a allows beside a quotation mark in East Asian text come from
//! [`is_quotation_break`].

use core::str;

use alloc::string::String;

use icu_locale_core::{LanguageIdentifier, langid};
use icu_segmenter::LineSegmenterBorrowed;
use icu_segmenter::iterators::LineBreakIterator;
use icu_segmenter::options::{LineBreakOptions, LineBreakStrictness, LineBreakWordOption};
use icu_segmenter::scaffold::Utf8;

use crate::style::{LineBreak, WordBreak};
use crate::{unicode, work};

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
    /// (`TextFlags::CJK_LINE_BREAKS`). `auto-phrase` segments as `normal`,
    /// as Blink's `kPhrase` does; the writer then keeps only the phrase
    /// boundaries between ideographs and kana.
    pub(super) fn new(strictness: LineBreak, word: WordBreak, cjk: bool) -> Self {
        let word = match word {
            WordBreak::AutoPhrase => WordBreak::Normal,
            word => word,
        };
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

    /// Returns whether ICU reads the text with U+201C as an opening bracket
    /// and U+201D as a closing one ([`fill_cj_text`]).
    ///
    /// It does under the Japanese and Chinese tailoring, as ICU's `_cj` line
    /// rules do. Chrome 155 breaks before `“` and after `”` beside ideographs
    /// wherever it also breaks before U+301C `〜`: in Chinese under `normal`
    /// and `loose`, and in no language under `strict`.
    pub(super) fn reads_cj_quotes(self) -> bool {
        self.ja_zh
    }

    /// Returns whether the key is `line-break: normal`, under which a small
    /// kana may start a line unless the config holds it.
    pub(super) fn is_normal(self) -> bool {
        self.strictness == LineBreak::Normal
    }

    /// Returns the key's slot among the context's segmenters.
    pub(super) fn index(self) -> usize {
        let word = match self.word {
            WordBreak::Normal | WordBreak::AutoPhrase => 0,
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

/// Returns ICU's word option for `word-break: word`. Three values match one
/// to one, and `auto-phrase` segments as `normal`.
///
/// A function rather than a `From`, since both types are foreign.
fn word_option(word: WordBreak) -> LineBreakWordOption {
    match word {
        WordBreak::Normal | WordBreak::AutoPhrase => LineBreakWordOption::Normal,
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
    /// The text as ICU reads it under a key that reads CJ quotes
    /// ([`LineKey::reads_cj_quotes`]): `text` with its curly double quotes
    /// swapped, or `text` itself where it has none.
    cj_text: &'t str,
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
    ///
    /// `cj_text` is `text` as [`fill_cj_text`] swaps it, or `text` itself.
    pub(super) fn new(
        text: &'t str,
        cj_text: &'t str,
        at: usize,
        segmenter: LineSegmenterBorrowed<'static>,
        key: LineKey,
    ) -> Self {
        let mut stream = Self {
            text,
            cj_text,
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
        let text = if self.key.reads_cj_quotes() {
            self.cj_text
        } else {
            self.text
        };
        let mut iter = self
            .segmenter
            .segment_str(text.get(from..).unwrap_or_default());
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

/// Writes `text` into `out` with U+201C swapped for U+2045 `⁅`, an opening
/// bracket, and U+201D for U+2046 `⁆`, a closing one. Returns whether it
/// swapped any; where it swapped none, `out` is left empty.
///
/// ICU's Japanese and Chinese line rules (`line_cj.txt` and its `normal`
/// and `loose` variants) class the two quotes so, which lets a line break
/// before `“` and after `”`. ICU4X's segmenter does not, so a stream under
/// such a key reads the swapped text. Each stand-in is three bytes long, as
/// the quote is, and not East Asian wide, as the quote is not, so every
/// offset and every other rule stay as they were.
pub(super) fn fill_cj_text(text: &str, out: &mut String) -> bool {
    out.clear();
    // Each single-character search runs `memchr`; the two-character pattern
    // would decode every character.
    if !text.contains('\u{201C}') && !text.contains('\u{201D}') {
        return false;
    }
    let mut from = 0;
    for (at, quote) in text.match_indices(['\u{201C}', '\u{201D}']) {
        work::step();
        out.push_str(text.get(from..at).unwrap_or_default());
        out.push(if quote == "\u{201C}" {
            '\u{2045}'
        } else {
            '\u{2046}'
        });
        from = at + quote.len();
    }
    out.push_str(text.get(from..).unwrap_or_default());
    true
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

/// Returns whether a cluster starting with `before_first` and one starting
/// with `after_first` may meet at a quotation mark UAX #14's LB19a breaks
/// beside: an initial quotation mark after, or a final one before.
///
/// A cheap test before [`is_quotation_break`], which decides.
#[inline]
pub(super) fn may_break_beside_quotation(before_first: char, after_first: char) -> bool {
    is_initial_quotation(after_first) || is_final_quotation(before_first)
}

/// Returns whether UAX #14 breaks between `before` and `after` beside a
/// quotation mark East Asian characters surround, where the segmenter
/// holds the pair together.
///
/// `before` is the text up to the boundary and `after` the text from it.
/// ICU4X's line segmenter keeps the LB19 of Unicode 15.0: no break on either
/// side of any quotation mark. Unicode 15.1 narrows LB19 to the side a
/// quotation mark faces and adds LB19a, which breaks on that side where East
/// Asian characters (East_Asian_Width F, W or H) stand either side of the
/// mark, as Chrome's ICU does:
/// - before an initial (Pi) mark, where the character before it and the one
///   after it are East Asian;
/// - after a final (Pf) mark, where the character before it and the one
///   after it are East Asian.
///
/// Marks and joiners after a character go with it (LB9). The rules before
/// and after LB19 still hold the pair where they apply (`「“`, `”。`). They
/// read the mark as a class with no rules of its own, so the segmenter
/// answers for the East Asian neighbour beside U+2014 (B2), which none of
/// them names next to an East Asian character.
#[inline(never)]
pub(super) fn is_quotation_break(
    segmenter: LineSegmenterBorrowed<'static>,
    before: &str,
    after: &str,
) -> bool {
    /// Stands in for the quotation mark: B2, ruled only beside B2.
    const STAND_IN: char = '\u{2014}';
    let mut back = before.chars().rev().filter(|&ch| !is_attached(ch));
    let mut ahead = after.chars();
    let (Some(near), Some(first)) = (back.next(), ahead.next()) else {
        return false;
    };
    let mut ahead = ahead.filter(|&ch| !is_attached(ch));
    let mut pair = [0u8; 8];
    let (left, right, at) = if is_initial_quotation(first) {
        // [^EA] × QU and × QU ([^EA] | eot).
        if !is_uax14_east_asian(near) || !ahead.next().is_some_and(is_uax14_east_asian) {
            return false;
        }
        (near, STAND_IN, near.len_utf8())
    } else if is_final_quotation(near) {
        // (sot | [^EA]) QU × and QU × [^EA].
        if !back.next().is_some_and(is_uax14_east_asian) || !is_uax14_east_asian(first) {
            return false;
        }
        (STAND_IN, first, STAND_IN.len_utf8())
    } else {
        return false;
    };
    let length = left.encode_utf8(&mut pair).len();
    let length = length + right.encode_utf8(&mut pair[length..]).len();
    let text = str::from_utf8(&pair[..length]).unwrap_or_default();
    is_break_opportunity(segmenter, text, at)
}

/// Returns whether `ch` goes with the character before it under UAX #14's
/// LB9: a mark, a variation selector or a zero width joiner.
fn is_attached(ch: char) -> bool {
    ch == '\u{200D}' || unicode::is_mark(ch) || unicode::is_variation_selector(ch)
}

/// Returns whether `ch` is East_Asian_Width Fullwidth, Wide or Halfwidth,
/// UAX #14's `$EastAsian`.
fn is_uax14_east_asian(ch: char) -> bool {
    unicode::rare_props(ch).is_wide()
        || matches!(
            ch,
            '\u{20A9}'
                | '\u{FF61}'..='\u{FFBE}'
                | '\u{FFC2}'..='\u{FFC7}'
                | '\u{FFCA}'..='\u{FFCF}'
                | '\u{FFD2}'..='\u{FFD7}'
                | '\u{FFDA}'..='\u{FFDC}'
                | '\u{FFE8}'..='\u{FFEE}'
        )
}

/// Returns whether `ch` is an initial quotation mark: Line_Break QU and
/// General_Category Pi.
#[inline]
fn is_initial_quotation(ch: char) -> bool {
    matches!(
        ch,
        '\u{AB}'
            | '\u{2018}'
            | '\u{201B}'
            | '\u{201C}'
            | '\u{201F}'
            | '\u{2039}'
            | '\u{2E02}'
            | '\u{2E04}'
            | '\u{2E09}'
            | '\u{2E0C}'
            | '\u{2E1C}'
            | '\u{2E20}'
    )
}

/// Returns whether `ch` is a final quotation mark: Line_Break QU and
/// General_Category Pf.
#[inline]
fn is_final_quotation(ch: char) -> bool {
    matches!(
        ch,
        '\u{BB}'
            | '\u{2019}'
            | '\u{201D}'
            | '\u{203A}'
            | '\u{2E03}'
            | '\u{2E05}'
            | '\u{2E0A}'
            | '\u{2E0D}'
            | '\u{2E1D}'
            | '\u{2E21}'
    )
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

/// Returns whether `ch` is an initial quotation mark, a final one, and East
/// Asian, as [`is_quotation_break`] reads them.
#[cfg(test)]
pub(super) fn quotation_classes(ch: char) -> (bool, bool, bool) {
    (
        is_initial_quotation(ch),
        is_final_quotation(ch),
        is_uax14_east_asian(ch),
    )
}
