//! The line-break options a style resolves to, and the line stream that asks
//! the line engine where lines may break.
//!
//! The engine is `cleft`'s UAX #14 scan, with the dictionaries of
//! the [`Context`](crate::Context) for Thai, Lao, Khmer and Myanmar. It takes
//! its options per scan and can start after any text, so one stream serves
//! a paragraph whatever its styles.
//!
//! - **Resuming.** Where a style's options differ, the paragraph's stream
//!   starts the engine again under the new options at the last opportunity
//!   before the seam, reading the text before it, and drops what it says
//!   before the seam. An opportunity is a safe place to start again (checked
//!   in `tests`), so the new options decide the pair at the seam and nothing
//!   before it changes.
//! - **Restarts** happen only at a change of options and a paragraph's
//!   start, not at objects (U+FFFC is in the text) or box edges.
//! - **Most text never reaches the engine.** The writer asks Blink's ASCII
//!   rules (`ascii`) first and the stream only where they do not decide. ASCII
//!   text runs no scan except under `anywhere`. Except under `anywhere`, the
//!   stream reads Latin-1 opportunities from a pair table of the engine's
//!   answers (`pairs`), handing over to the engine and back at opportunities.
//! - **`keep-all` and `break-all` are Blink's own rules**, applied to each
//!   pair before the stream is asked. The engine runs under `normal` words for
//!   both: Blink never passes them to ICU. `keep-all` holds two letters or
//!   numbers by General_Category; `break-all` adds opportunities by Blink's
//!   table of Line_Break classes.
//!
//! The opportunities are the engine's either way. The tests hold the table to
//! the engine for every pair it covers, and the stream to the engine over
//! UAX #14's test strings and text that switches back and forth.

use cleft::{Dictionaries, LineBreaks, LineOptions, LineStrictness, LineWordOption, Utf8Scalars};

use crate::config::LooseHyphens;
use crate::style::{LineBreak, WordBreak};
use crate::unicode::{self, LineBreak as Class};
use crate::work;

/// The options the line engine runs under, resolved from a style and the
/// config: one of 19 values.
///
/// Styles that break alike resolve to one key, so a stream resumes only
/// where the breaks can differ.
/// - `line-break: anywhere` ignores the word option and the language, so
///   `keep-all` with `anywhere` is anywhere, as CSS says.
/// - The language matters only under `normal` and `loose`.
/// - The config's hyphen rule matters only under `loose` outside Japanese
///   and Chinese.
/// - An unset value is CSS's initial one, never UAX #14's `strict` default.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct LineKey {
    strictness: LineBreak,
    word: WordBreak,
    /// Whether the engine tailors for Japanese and Chinese.
    ja_zh: bool,
    /// Whether `loose` breaks before U+2010 and U+2013 after an ideograph
    /// outside Japanese and Chinese, as [`LooseHyphens::AnyLanguage`] says.
    loose_hyphens: bool,
}

impl LineKey {
    /// Returns the key for `line-break: strictness` and `word-break: word`.
    ///
    /// `cjk` says the engine tailors for Japanese and Chinese
    /// (`TextFlags::CJK_LINE_BREAKS`). `auto` breaks as `normal`, its
    /// tailoring already resolved into `cjk`. `auto-phrase` segments as
    /// `normal`, as Blink's `kPhrase` does; the writer then keeps only the
    /// phrase boundaries between ideographs and kana. `hyphens` is the
    /// config's rule for hyphens under `loose`.
    pub(super) fn new(
        strictness: LineBreak,
        word: WordBreak,
        cjk: bool,
        hyphens: LooseHyphens,
    ) -> Self {
        let word = match word {
            WordBreak::AutoPhrase => WordBreak::Normal,
            word => word,
        };
        match strictness {
            LineBreak::Auto => Self::new(LineBreak::Normal, word, cjk, hyphens),
            LineBreak::Anywhere => Self {
                strictness,
                word: WordBreak::Normal,
                ja_zh: false,
                loose_hyphens: false,
            },
            LineBreak::Strict | LineBreak::Normal => Self {
                strictness,
                word,
                ja_zh: cjk && strictness == LineBreak::Normal,
                loose_hyphens: false,
            },
            LineBreak::Loose => Self {
                strictness,
                word,
                ja_zh: cjk,
                loose_hyphens: !cjk && hyphens == LooseHyphens::AnyLanguage,
            },
        }
    }

    /// Returns whether the pair table (`pairs`) answers under the key.
    ///
    /// It does under every strictness but `anywhere`, in any language and
    /// with any word option, since the engine breaks the table's characters
    /// alike under all of them and runs under `normal` words. `anywhere`
    /// breaks between graphemes.
    pub(super) fn breaks_by_pairs(self) -> bool {
        self.strictness != LineBreak::Anywhere
    }

    /// Returns Blink's answer between `before` and `after` where its own
    /// rules decide it under the key, or `None` where it asks ICU.
    ///
    /// `earlier` yields the characters before `before`, nearest first.
    /// - Under `line-break: anywhere`, always `None`: Blink breaks between
    ///   graphemes (`kBreakCharacter`).
    /// - Otherwise its ASCII rules (`ascii::breaks`) first, with its
    ///   break-all table between ASCII characters under `break-all`
    ///   (`ascii::breaks_all`).
    /// - Then, past ASCII, its `keep-all` and `break-all` rules
    ///   ([`keeps_all`], [`breaks_all`]).
    fn blink_answer<I: Iterator<Item = char>>(
        self,
        earlier: impl Fn() -> I,
        before: char,
        after: char,
    ) -> Option<bool> {
        let before_before = || earlier().next();
        match (self.strictness, self.word) {
            (LineBreak::Anywhere, _) => None,
            (_, WordBreak::BreakAll) => super::ascii::breaks_all(before_before, before, after)
                .or_else(|| {
                    let loose = self.strictness == LineBreak::Loose;
                    breaks_all(before, after, loose).then_some(true)
                }),
            (_, WordBreak::KeepAll) => super::ascii::breaks(before_before, before, after)
                .or_else(|| keeps_all(&earlier, before, after).then_some(false)),
            _ => super::ascii::breaks(before_before, before, after),
        }
    }

    /// Returns whether a line may break between `before` and `after`, by
    /// Blink's own rules ([`blink_answer`](Self::blink_answer)) where they
    /// decide and by `fallback` elsewhere.
    ///
    /// `earlier` yields the characters before `before`, nearest first; only
    /// a hyphen before a digit and a mark under `keep-all` read it.
    #[inline]
    pub(super) fn blink_or<I: Iterator<Item = char>>(
        self,
        earlier: impl Fn() -> I,
        before: char,
        after: char,
        fallback: impl FnOnce() -> bool,
    ) -> bool {
        self.blink_answer(earlier, before, after)
            .unwrap_or_else(fallback)
    }

    /// Returns whether the key is `line-break: normal` or `auto`, under which
    /// a small kana may start a line unless the config holds it.
    pub(super) fn is_normal(self) -> bool {
        self.strictness == LineBreak::Normal
    }

    /// Returns the engine's options for the key.
    ///
    /// Under the Japanese and Chinese tailoring, U+201C reads as an opening
    /// bracket and U+201D as a closing one, as ICU4C's `_cj` rules read
    /// them. So a line breaks before `“` and after `”` beside ideographs
    /// wherever it also breaks before U+301C `〜`: in Chinese under `normal`
    /// and `loose`, and in no language under `strict`.
    pub(super) fn options(self) -> LineOptions {
        LineOptions {
            strictness: match self.strictness {
                LineBreak::Loose => LineStrictness::Loose,
                LineBreak::Auto | LineBreak::Normal => LineStrictness::Normal,
                LineBreak::Strict => LineStrictness::Strict,
                LineBreak::Anywhere => LineStrictness::Anywhere,
            },
            // Blink applies `keep-all` and `break-all` itself
            // (`blink_answer`), over ICU's `normal` words.
            word: LineWordOption::Normal,
            ja_zh: self.ja_zh,
            cj_quotes: self.ja_zh,
            loose_hyphens: self.loose_hyphens,
        }
    }
}

/// Returns whether Blink's `keep-all` holds a line from breaking between
/// `before` and `after`, two characters past ASCII or beside one.
///
/// Blink holds two letters or numbers (General_Category L or N), but not
/// Line_Break SA, and reads the character before a mark in its place
/// (`ShouldKeepAfterKeepAll`). `earlier` yields the characters before
/// `before`, nearest first. Blink reads UTF-16 code units, so a character
/// past the BMP, a surrogate pair to it, is never held: `𠀀|𠀀` breaks
/// under `keep-all`.
#[cold]
#[inline(never)]
fn keeps_all<I: Iterator<Item = char>>(earlier: impl Fn() -> I, before: char, after: char) -> bool {
    let held = |ch: char| {
        u32::from(ch) <= 0xFFFF
            && unicode::rare_props(ch).is_letter_or_number()
            && !unicode::core_props(ch).is_complex_context()
    };
    if u32::from(before) > 0xFFFF {
        return false;
    }
    let before = if unicode::core_props(before).is_mark() {
        match earlier().next() {
            Some(ch) => ch,
            None => return false,
        }
    } else {
        before
    };
    held(before) && held(after)
}

/// Returns whether Blink's `break-all` adds a line break opportunity between
/// `before` and `after`, two characters past ASCII or beside one.
///
/// Blink reads the Line_Break classes of the two, with `+` read as AL, and
/// adds an opportunity where its table says (`kBreakAllLineBreakClassTable`,
/// `ShouldBreakAfterBreakAll`). Under `loose`, it also breaks before U+2010
/// HYPHEN and U+2013 EN DASH after AL, NU, SA or ID. Where it adds nothing,
/// ICU's `normal` answer stands.
///
/// Blink finds a line's break by searching back from where the line
/// overflows (`PreviousBreakOpportunity`). That search reads only the
/// character before each boundary and one UTF-16 code unit after it. So a
/// combining mark (CM) before the boundary adds nothing, and nothing is added
/// before a character past the BMP: `a\u{301}|b`, `क\u{93F}|क` and `a|𝐀`
/// hold in the middle of a line. Only where nothing fits a line does Blink
/// scan forward, stepping back over marks and reading surrogate pairs.
#[cold]
#[inline(never)]
fn breaks_all(before: char, after: char, loose: bool) -> bool {
    if u32::from(after) > 0xFFFF {
        return false;
    }
    let class = |ch: char| match ch {
        '+' => Class::ALPHABETIC,
        ch => unicode::line_break(ch),
    };
    let last = class(before);
    if loose
        && matches!(after, '\u{2010}' | '\u{2013}')
        && matches!(
            last,
            Class::NUMERIC | Class::ALPHABETIC | Class::COMPLEX_CONTEXT | Class::IDEOGRAPHIC
        )
    {
        return true;
    }
    let next = class(after);
    // Blink holds before BA but under `loose`, U+007C `|` aside.
    if next == Class::BREAK_AFTER && after != '|' && !loose {
        return false;
    }
    break_all_adds(last, next)
}

/// Returns whether Blink's break-all table adds an opportunity between a
/// character of class `last` and one of class `next`.
///
/// After AI, AL, BA, HL, NU, SA and SY, before AI, AL, BA, HL, HY, NU, OP, PR
/// and SA; after CP the same but OP; after CL the same but OP and SA; after
/// EX and PO the same as after CL, and PO; after IS before AI, AL, BA, HL,
/// HY and NU; after HY before NU; after PR before PO.
fn break_all_adds(last: Class, next: Class) -> bool {
    let common = matches!(
        next,
        Class::AMBIGUOUS
            | Class::ALPHABETIC
            | Class::HEBREW_LETTER
            | Class::BREAK_AFTER
            | Class::HYPHEN
            | Class::NUMERIC
    );
    match last {
        Class::AMBIGUOUS
        | Class::ALPHABETIC
        | Class::HEBREW_LETTER
        | Class::BREAK_AFTER
        | Class::NUMERIC
        | Class::COMPLEX_CONTEXT
        | Class::BREAK_SYMBOLS => {
            common
                || matches!(
                    next,
                    Class::OPEN_PUNCTUATION | Class::PREFIX_NUMERIC | Class::COMPLEX_CONTEXT
                )
        }
        Class::CLOSE_PARENTHESIS => {
            common || matches!(next, Class::PREFIX_NUMERIC | Class::COMPLEX_CONTEXT)
        }
        Class::CLOSE_PUNCTUATION => common || next == Class::PREFIX_NUMERIC,
        Class::EXCLAMATION | Class::POSTFIX_NUMERIC => {
            common || matches!(next, Class::PREFIX_NUMERIC | Class::POSTFIX_NUMERIC)
        }
        Class::INFIX_NUMERIC => common,
        Class::HYPHEN => next == Class::NUMERIC,
        Class::PREFIX_NUMERIC => next == Class::POSTFIX_NUMERIC,
        _ => false,
    }
}

/// The engine's scan over a text, from a position on.
type Scan<'t> = LineBreaks<'t, Utf8Scalars<'t>>;

/// One paragraph's line-break opportunities, asked for in text order.
///
/// Holds the next opportunity at or after the last position asked about,
/// and the one before it, where a resume starts. Positions are byte offsets
/// into the whole text.
///
/// Opportunities come from the pair table (`pairs`) where it covers the
/// text and from the engine elsewhere. Both go on from an opportunity as
/// from the text's start, so they hand over to each other there. The engine
/// starts at the last opportunity the table found, and the table is tried
/// again at each opportunity the engine finds. The table covers no SA text,
/// so the engine always reads an SA run from its start.
pub(super) struct LineStream<'t> {
    text: &'t str,
    dictionaries: &'t Dictionaries,
    /// The engine's scan; `None` while the table answers.
    scan: Option<Scan<'t>>,
    /// The stream's start, a paragraph's, where the engine reads no text
    /// before.
    origin: usize,
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
    /// start, breaking SA runs at the words of `dictionaries`.
    pub(super) fn new(
        text: &'t str,
        at: usize,
        dictionaries: &'t Dictionaries,
        key: LineKey,
    ) -> Self {
        let mut stream = Self {
            text,
            dictionaries,
            scan: None,
            origin: at,
            key,
            pairs: key.breaks_by_pairs(),
            last: at,
            ahead: at,
        };
        stream.start(at, false);
        stream
    }

    /// Returns the key the stream breaks under.
    pub(super) fn key(&self) -> LineKey {
        self.key
    }

    /// Goes on from `from`, by the table where the key allows and by the
    /// engine otherwise. `from` becomes `ahead` but is not an opportunity
    /// itself. `after_text` says the engine reads the text before `from`,
    /// where `from` is an opportunity rather than a paragraph's start.
    fn start(&mut self, from: usize, after_text: bool) {
        self.ahead = from;
        self.scan = None;
        if !self.pairs {
            self.start_engine(from, after_text);
        }
    }

    /// Starts the engine at `from`, which is `ahead`, reading the text
    /// before it where `after_text`.
    fn start_engine(&mut self, from: usize, after_text: bool) {
        let scalars = Utf8Scalars::with_start(self.text, from);
        let options = self.key.options();
        let scan = if after_text {
            let before = self.text.get(..from).unwrap_or_default();
            LineBreaks::with_preceding(scalars, options, before.chars().rev())
        } else {
            LineBreaks::new(scalars, options)
        };
        self.scan = Some(scan.with_dictionaries(self.dictionaries));
    }

    /// Moves `ahead` to the next opportunity, or `usize::MAX` if none.
    fn advance(&mut self) {
        if self.ahead >= self.text.len() {
            self.ahead = usize::MAX;
            return;
        }
        // The table first, at every opportunity. Where it answers, the
        // engine's scan is dropped.
        if self.pairs
            && super::pairs::may_cover(self.text, self.ahead)
            && let Some(at) = super::pairs::next_break(self.text, self.ahead)
        {
            self.ahead = at;
            self.scan = None;
            return;
        }
        // The engine starts here if the table was answering, or goes on where
        // it is, so text the table keeps declining costs no new scan at
        // each opportunity.
        if self.scan.is_none() {
            self.start_engine(self.ahead, self.ahead != self.origin);
        }
        let Some(scan) = &mut self.scan else {
            return;
        };
        // The engine yields no opportunity at the text's end, which the
        // stream reports, as the table does.
        self.ahead = scan.next().unwrap_or(self.text.len());
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
    /// The engine runs from the last opportunity before the seam, and its
    /// answers before the seam are dropped. So `key` decides the pair at the
    /// seam, with its context, and nothing before it changes.
    pub(super) fn resume(&mut self, key: LineKey, seam: usize) {
        while self.ahead < seam {
            self.last = self.ahead;
            self.advance();
        }
        let from = self.last;
        self.key = key;
        self.pairs = key.breaks_by_pairs();
        // Where `from` is the paragraph's start, the text before ends in a
        // mandatory break, after which the engine reads as at a text's start.
        self.start(from, true);
        while self.ahead < seam {
            self.advance();
        }
    }
}

/// Returns whether a line may break at byte `at` of `text` under `key` and
/// `dictionaries`; never at the text's start or end.
///
/// Runs the engine once over assembled text, as Blink's
/// `CanBreakAfterRubyColumn` asks its break iterator over a ruby base and
/// what follows the column.
pub(super) fn is_break_opportunity(
    text: &str,
    at: usize,
    dictionaries: &Dictionaries,
    key: LineKey,
) -> bool {
    if at == 0 || at >= text.len() {
        return false;
    }
    let scan =
        LineBreaks::new(Utf8Scalars::new(text), key.options()).with_dictionaries(dictionaries);
    for found in scan {
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
        loose_hyphens: false,
    };
}
