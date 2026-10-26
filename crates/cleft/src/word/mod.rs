//! Word boundaries (UAX #29), found by a forward scan.
//!
//! [`WordBreaks`] reads a stream of `(offset, char)` and yields the offsets
//! where a word boundary falls. The rules (`rules`) are those of the Unicode
//! version [`UNICODE_VERSION`](crate::UNICODE_VERSION) names, WB1 to WB999.
//!
//! The scan keeps a state of two bytes. WB6, WB7b and WB12 read the next
//! character past any that attach to it (WB4), by cloning the stream, so
//! nothing is buffered. A boundary is a safe place to start again: no rule
//! reads across one.
//!
//! Runs of Thai, Lao, Khmer and Myanmar, and of Han, Hiragana and Katakana,
//! break at the words of a dictionary, as line breaking does inside SA runs.

mod rules;
#[cfg(test)]
mod tests;

use crate::dictionary::{self, Dictionaries, DictionaryScript, Run, RunScript};
use crate::{GraphemeProps, GraphemeState, grapheme_props};

/// The word class bits of a [`WordProps`].
const CLASS_MASK: u8 = 0x1F;
/// The Extended_Pictographic bit of a [`WordProps`].
const PICTOGRAPHIC: u8 = 0x20;
/// The shift of a [`WordProps`]'s run bits.
const RUN_SHIFT: u32 = 6;
/// The run bits of a Line_Break SA character.
const RUN_SOUTHEAST_ASIAN: u8 = 1;
/// The run bits of a Han, Hiragana or Katakana character.
const RUN_CHINESE_JAPANESE: u8 = 2;
/// The class the lookahead reads at the end of the stream.
const END: u8 = u8::MAX;

// The classes the options resolve are the last two, in this order.
const _: () = assert!(
    rules::CHINESE_JAPANESE_LETTER == rules::FULL_STOP + 1
        && rules::CLASSES == rules::CHINESE_JAPANESE_LETTER as usize + 1
);

/// What UAX #29 reads of a character for words, packed in a byte: its word
/// class, whether it is Extended_Pictographic, and the run of a dictionary
/// it can be in.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct WordProps(u8);

impl WordProps {
    /// Creates the properties from their bits: the word class in bits 0 to
    /// 4, Extended_Pictographic in bit 5, the run in bits 6 and 7.
    #[inline]
    pub(crate) const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the word class, as `rules` numbers it.
    #[inline]
    const fn class(self) -> u8 {
        self.0 & CLASS_MASK
    }

    /// Returns whether the character is Extended_Pictographic.
    #[inline]
    const fn is_pictographic(self) -> bool {
        self.0 & PICTOGRAPHIC != 0
    }

    /// Returns the run bits: zero outside any run.
    #[inline]
    const fn run(self) -> u8 {
        self.0 >> RUN_SHIFT
    }
}

/// Returns the script of the dictionary run `ch` can be in: its own for a
/// Line_Break SA character, Chinese and Japanese for Han, Hiragana and
/// Katakana.
#[inline]
pub(crate) fn run_script(ch: char) -> Option<RunScript> {
    match crate::props::word_props(ch).run() {
        RUN_SOUTHEAST_ASIAN => Some(RunScript::from_sa_char(ch)),
        RUN_CHINESE_JAPANESE => Some(RunScript::ChineseJapanese),
        _ => None,
    }
}

/// The tailorings a [`WordBreaks`] applies. The default is plain UAX #29.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct WordOptions {
    /// Whether U+002E FULL STOP joins only digits, as in ICU's `en_US_POSIX`
    /// word rules: it reads as MidNum rather than MidNumLet. Then `x.y` is
    /// three words, and `3.14` stays one.
    pub posix_full_stop: bool,
}

impl WordOptions {
    /// Creates the options of plain UAX #29.
    #[inline]
    pub const fn new() -> Self {
        Self {
            posix_full_stop: false,
        }
    }
}

/// The offsets of the word boundaries in a stream of characters.
///
/// The stream is any cloneable iterator of `(offset, char)`, such as
/// [`Utf8Scalars`](crate::Utf8Scalars) or
/// [`Utf16Scalars`](crate::Utf16Scalars). Each offset yielded is that of a
/// character a word starts at. The start of the stream and its end are
/// boundaries (WB1, WB2) and are not yielded.
///
/// Offsets must increase. At the first one that does not, the iterator
/// ends, as if the text ended there. Nothing it does allocates or panics.
///
/// The words of Thai, Lao, Khmer and Myanmar (Line_Break SA), and of Han
/// and Hiragana, follow no rule. With
/// [`with_dictionaries`](Self::with_dictionaries), a run of them breaks at
/// the words of its script's dictionary: the longest word at each position,
/// ending only where a grapheme cluster does. Where the script changes
/// inside a run, a boundary falls if either script has a dictionary. A run
/// of a Southeast Asian script with no dictionary is one word. Han and
/// Hiragana with no dictionary follow the rules: each ideograph is a word.
/// With one, no Han or Hiragana character is a letter to the rules, so `々`
/// does not join the Latin after it.
///
/// Katakana (Word_Break Katakana but the halfwidth forms) joins the run of
/// Han and Hiragana where there is a Chinese and Japanese dictionary, and
/// otherwise follows the rules, which keep a run of it whole. With the
/// dictionary, a run of at most eight katakana that the dictionary's words
/// do not cover is one word, so `グーグルマップ` stays whole and
/// `ゲームソフト` parts into `ゲーム|ソフト`.
///
/// A boundary is a safe place to start again: an iterator over the text
/// from a boundary finds the same boundaries after it as one over the whole
/// text.
///
/// # Examples
///
/// ```
/// use cleft::{Utf8Scalars, WordBreaks, WordOptions};
///
/// let text = "Can't stop, 3.14!";
/// let breaks: Vec<usize> = WordBreaks::new(Utf8Scalars::new(text), WordOptions::new()).collect();
/// assert_eq!(breaks, [5, 6, 10, 11, 12, 16]);
/// ```
#[derive(Clone, Debug)]
pub struct WordBreaks<'d, I> {
    scalars: I,
    /// The rules' state after the last character.
    state: u8,
    /// Whether the last character is U+200D ZERO WIDTH JOINER (WB3c).
    after_zwj: bool,
    /// The classes U+002E FULL STOP and the letters of Han and Hiragana read
    /// as.
    resolved: [u8; 2],
    /// The least offset the next character may have, or `None` once the
    /// stream has ended.
    least: Option<usize>,
    /// Whether the first character has been read.
    started: bool,
    /// The dictionaries that find the words of runs.
    dictionaries: Option<&'d Dictionaries>,
    /// Whether Han, Hiragana and Katakana form runs: where there is a
    /// Chinese and Japanese dictionary.
    cj_runs: bool,
    /// Where the scan stands in a run.
    run: Run,
    /// Whether the last character of a Chinese and Japanese run is
    /// katakana, which a short katakana run's word reads.
    after_katakana: bool,
    /// What the tests read in place of the crate's data and rules.
    #[cfg(test)]
    test: tests::TestState,
}

impl<'d, I: Iterator<Item = (usize, char)> + Clone> WordBreaks<'d, I> {
    /// Creates an iterator over the word boundaries of `scalars`, which
    /// starts a text.
    #[inline]
    pub fn new(scalars: I, options: WordOptions) -> Self {
        Self {
            scalars,
            state: rules::START,
            after_zwj: false,
            resolved: [
                if options.posix_full_stop {
                    rules::MID_NUMBER
                } else {
                    rules::MID_NUM_LETTER
                },
                rules::LETTER,
            ],
            least: Some(0),
            started: false,
            dictionaries: None,
            cj_runs: false,
            run: Run::default(),
            after_katakana: false,
            #[cfg(test)]
            test: tests::TestState::default(),
        }
    }

    /// Returns the iterator, breaking runs at the words of `dictionaries`.
    ///
    /// Call it before reading any boundary.
    ///
    /// # Examples
    ///
    /// ```
    /// use cleft::{Dictionaries, Utf8Scalars, WordBreaks, WordOptions};
    ///
    /// // With no dictionary, a Thai run is one word, and each ideograph is
    /// // a word of its own.
    /// let none = Dictionaries::new();
    /// let text = "กินข้าว 日本語";
    /// let breaks: Vec<usize> = WordBreaks::new(Utf8Scalars::new(text), WordOptions::new())
    ///     .with_dictionaries(&none)
    ///     .collect();
    /// assert_eq!(breaks, [21, 22, 25, 28]);
    /// ```
    #[inline]
    pub fn with_dictionaries(mut self, dictionaries: &'d Dictionaries) -> Self {
        self.dictionaries = Some(dictionaries);
        self.cj_runs = dictionaries
            .get(DictionaryScript::ChineseJapanese)
            .is_some();
        // A dictionary finds the words of Han and Hiragana, so none of them
        // is a letter that joins the letters beside it, as in ICU4C's word
        // rules.
        if self.cj_runs {
            self.resolved[1] = rules::OTHER;
        }
        self
    }

    /// Returns the word byte of `ch`.
    #[inline(always)]
    fn props(&self, ch: char) -> WordProps {
        #[cfg(test)]
        if let Some(classes) = self.test.classes {
            return WordProps(classes.get(ch as usize).copied().unwrap_or_default());
        }
        crate::props::word_props(ch)
    }

    /// Returns the class of a character whose word byte is `props`, under
    /// the options.
    #[inline(always)]
    fn class(&self, props: WordProps) -> u8 {
        #[cfg(test)]
        if self.test.legacy & tests::Legacy::COMPLEX_CLASS != 0 && self.is_complex_props(props) {
            return rules::OTHER;
        }
        let class = props.class();
        if class < rules::FULL_STOP {
            class
        } else {
            self.resolved
                .get(usize::from(class - rules::FULL_STOP))
                .copied()
                .unwrap_or(rules::OTHER)
        }
    }

    /// Moves past `ch`, at `offset`, and returns whether a word boundary
    /// falls before it, asked only where `decides`.
    #[inline(always)]
    fn step(&mut self, offset: usize, ch: char, decides: bool) -> bool {
        let props = self.props(ch);
        let class = self.class(props);
        let run = if props.run() != 0 {
            self.run_step(offset, ch, props.run(), class)
        } else {
            self.run.end();
            None
        };
        #[cfg(test)]
        let run = if self.test.legacy != 0 {
            self.model_step(offset, ch, run)
        } else {
            run
        };
        let (answer, next) = rules::pair(self.state, class);
        #[cfg(test)]
        let answer = self.icu_answer(answer, class);
        let breaks = decides
            && match run {
                Some(breaks) => breaks,
                None => self.answer(answer, props),
            };
        self.state = next;
        self.after_zwj = class == rules::ZWJ;
        breaks
    }

    /// Returns whether the rules' `answer` before a character whose word
    /// byte is `props` is a boundary, reading ahead where it waits.
    #[inline(always)]
    fn answer(&self, answer: u8, props: WordProps) -> bool {
        // WB3c: a pictograph joins a zero width joiner.
        if self.after_zwj && props.is_pictographic() {
            return false;
        }
        match answer {
            rules::KEEP => false,
            rules::BREAK => true,
            waits => !rules::holds(waits, self.next_class()),
        }
    }

    /// Returns the class of the next character that does not attach to the
    /// one before it, or [`END`] where the stream ends first.
    #[cold]
    #[inline(never)]
    fn next_class(&self) -> u8 {
        let mut least = self.least.unwrap_or(usize::MAX);
        for (offset, ch) in self.scalars.clone() {
            if offset < least {
                break;
            }
            least = offset.saturating_add(1);
            let class = self.class(self.props(ch));
            if !rules::is_attached(class) {
                return class;
            }
        }
        END
    }

    /// Moves the run past `ch`, at `offset`, of word class `class` and with
    /// the run bits `bits`. Returns whether a boundary falls before it where
    /// the run decides, or `None` where the rules do.
    ///
    /// Out of line: Southeast Asian, Chinese and Japanese text alone reach
    /// it.
    #[inline(never)]
    fn run_step(&mut self, offset: usize, ch: char, bits: u8, class: u8) -> Option<bool> {
        let script = if bits == RUN_SOUTHEAST_ASIAN {
            RunScript::from_sa_char(ch)
        } else if self.cj_runs {
            RunScript::ChineseJapanese
        } else {
            self.run.end();
            return None;
        };
        let mut run = self.run;
        let katakana_start = !(self.after_katakana && run.script() == Some(script));
        let cj = script == RunScript::ChineseJapanese;
        let breaks = run.step(
            offset,
            script,
            rules::is_attached(class),
            self.dictionaries,
            |dictionary| {
                let continues = |next| self.continues_run(next, script);
                let props = |ch| self.grapheme_props(ch);
                let rest = self.scalars.clone();
                let start = self.grapheme_start();
                if cj {
                    dictionary::find_next_cj_word(
                        dictionary,
                        offset,
                        ch,
                        rest,
                        continues,
                        props,
                        start,
                        katakana_start,
                    )
                } else {
                    dictionary::find_next_word(
                        dictionary, offset, ch, rest, continues, props, start,
                    )
                }
            },
        );
        self.run = run;
        self.after_katakana = cj && dictionary::is_katakana(ch);
        breaks
    }

    /// Returns whether `ch` continues a run of `script`.
    #[inline]
    fn continues_run(&self, ch: char, script: RunScript) -> bool {
        let bits = self.props(ch).run();
        if script == RunScript::ChineseJapanese {
            bits == RUN_CHINESE_JAPANESE
        } else {
            bits == RUN_SOUTHEAST_ASIAN && RunScript::from_sa_char(ch) == script
        }
    }

    /// Returns the grapheme properties of `ch`.
    #[inline(always)]
    fn grapheme_props(&self, ch: char) -> GraphemeProps {
        #[cfg(test)]
        if let Some(classes) = self.test.graphemes {
            return classes.get(ch as usize).copied().unwrap_or_default();
        }
        grapheme_props(ch)
    }

    /// Returns the grapheme state at the start of a text.
    #[inline(always)]
    fn grapheme_start(&self) -> GraphemeState {
        #[cfg(test)]
        if self.test.graphemes_17 {
            return GraphemeState::with_unicode_17_conjuncts();
        }
        GraphemeState::new()
    }
}

impl<I: Iterator<Item = (usize, char)> + Clone> Iterator for WordBreaks<'_, I> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        if !self.started {
            self.started = true;
            match self.scalars.next() {
                Some((offset, ch)) => {
                    self.least = Some(offset.saturating_add(1));
                    self.step(offset, ch, false);
                }
                None => self.least = None,
            }
        }
        let mut least = self.least?;
        while let Some((offset, ch)) = self.scalars.next() {
            if offset < least {
                break;
            }
            least = offset.saturating_add(1);
            self.least = Some(least);
            if self.step(offset, ch, true) {
                return Some(offset);
            }
        }
        self.least = None;
        None
    }
}

impl<I: Iterator<Item = (usize, char)> + Clone> core::iter::FusedIterator for WordBreaks<'_, I> {}
