//! Line break opportunities (UAX #14), found by a forward scan.
//!
//! [`LineBreaks`] reads a stream of `(offset, char)` and yields the offsets
//! where a line may break. The rules (`rules`) are those of the Unicode
//! version [`UNICODE_VERSION`](crate::UNICODE_VERSION) names, LB1 to LB31,
//! with SA resolved by LB1 alone: no dictionary breaks Thai, Lao, Khmer or
//! Myanmar text inside a run.
//!
//! The scan keeps a state of two bytes and looks at most two classes ahead,
//! by cloning the stream. A break opportunity is a safe place to start
//! again: [`LineBreaks::with_preceding`] reads only the character before it.
//!
//! [`LineOptions`] selects the CSS tailorings: the strictness of
//! `line-break`, the `word-break` option, the Japanese and Chinese
//! tailoring, and the reading of curly double quotes as brackets. The
//! strictnesses follow ICU4C's rules. The default is plain UAX #14.

mod rules;
#[cfg(test)]
mod tests;

use crate::dictionary::{self, Dictionaries, Run, RunScript};
use crate::{GraphemeProps, GraphemeState, LineBreak, grapheme_props};
use rules::{
    AFTER_ZWJ, AL, BEFORE_EAST_ASIAN, BREAK, CL, CM, EOT, KEEP, LEFT_EAST_ASIAN, NEEDS_NEXT,
    NEXT_EAST_ASIAN, OP, Pair, QU_PF, QU_PI, RESOLVE, RI, RI_EVEN, RIGHT_EAST_ASIAN, Rules, SA,
    SA_MARK, SOT, Tailoring, UNREAD, ZWJ,
};

/// The line class bits of a [`LineProps`].
const CLASS_MASK: u8 = 0x3F;
/// The East Asian bit of a [`LineProps`]: East_Asian_Width F, W or H.
const EAST_ASIAN: u8 = 0x40;
/// The bit of a [`LineProps`] that says the character is
/// Extended_Pictographic and unassigned.
const UNASSIGNED_PICTOGRAPHIC: u8 = 0x80;

/// What UAX #14 reads of a character, packed in a byte: its line class,
/// whether it is East Asian, and whether it is an unassigned pictograph.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct LineProps(u8);

impl LineProps {
    /// Creates the properties from their bits: the line class in bits 0 to
    /// 5, East Asian in bit 6, Extended_Pictographic and unassigned in bit 7.
    #[inline]
    pub(crate) const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the line class, as `rules` numbers it.
    #[inline]
    const fn class(self) -> u8 {
        self.0 & CLASS_MASK
    }

    /// Returns the East Asian and unassigned pictograph bits.
    #[inline]
    const fn flags(self) -> u8 {
        self.0 & (EAST_ASIAN | UNASSIGNED_PICTOGRAPHIC)
    }

    /// Returns the Line_Break value the class stands for.
    #[inline]
    pub(crate) const fn line_break(self) -> LineBreak {
        let class = self.class() as usize;
        LineBreak::new(if class < rules::DATA_CLASS_COUNT {
            rules::LINE_BREAK[class]
        } else {
            0
        })
    }
}

/// How strictly lines break, as CSS's `line-break` sets it.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum LineStrictness {
    /// Fewer restrictions than `Normal`: a line may also break before
    /// iteration marks and between ellipses. With [`LineOptions::ja_zh`],
    /// also before U+301C `〜` and U+30A0 `゠`, centred punctuation,
    /// fullwidth `!` and `?` and East Asian postfixes, and after East Asian
    /// prefixes. With [`LineOptions::ja_zh`], [`LineOptions::loose_hyphens`]
    /// or [`LineWordOption::BreakAll`], also after an ideograph before U+2010
    /// HYPHEN and U+2013 EN DASH.
    ///
    /// The characters a line may break before still do not start a line
    /// after an opening bracket, a quotation mark, a word joiner or a
    /// no-break space.
    Loose,
    /// The common restrictions: small kana and the prolonged sound mark (CJ)
    /// may start a line. With [`LineOptions::ja_zh`], a line may also break
    /// before U+301C `〜` and U+30A0 `゠`, the dashes of Japanese text, unless
    /// what precedes holds them, as under `Loose`.
    Normal,
    /// UAX #14's rules alone: CJ may not start a line.
    #[default]
    Strict,
    /// A break between any two extended grapheme clusters, whatever the
    /// characters.
    Anywhere,
}

/// Which breaks within words are allowed, as CSS's `word-break` sets it.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum LineWordOption {
    /// UAX #14's rules.
    #[default]
    Normal,
    /// Breaks between letters and digits too: AL, NU, SA and CJ break as
    /// ideographs (ID).
    BreakAll,
    /// No breaks between letters, digits, ideographs, Hangul and CJ (AI, AL,
    /// ID, NU, H2, H3, JL, JV, JT and CJ).
    KeepAll,
}

/// The tailorings a [`LineBreaks`] applies. The default is plain UAX #14.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct LineOptions {
    /// How strictly lines break. Under [`LineStrictness::Anywhere`], the word
    /// option and the language are not read.
    pub strictness: LineStrictness,
    /// Which breaks within words are allowed.
    pub word: LineWordOption,
    /// Whether the text is Japanese or Chinese, which `normal` and `loose`
    /// tailor for, as [`LineStrictness`] says.
    pub ja_zh: bool,
    /// Whether U+201C LEFT DOUBLE QUOTATION MARK reads as opening punctuation
    /// (OP) and U+201D RIGHT DOUBLE QUOTATION MARK as closing punctuation
    /// (CL), as ICU4C's rules for Japanese and Chinese read them. A line may
    /// then break before `“` and after `”` beside ideographs.
    pub cj_quotes: bool,
    /// Whether [`LineStrictness::Loose`] breaks before U+2010 HYPHEN and
    /// U+2013 EN DASH after an ideograph in any language, as CSS Text 3
    /// allows.
    ///
    /// Without it, `Loose` breaks there only with [`LineOptions::ja_zh`] or
    /// [`LineWordOption::BreakAll`], as ICU4C does.
    pub loose_hyphens: bool,
}

impl LineOptions {
    /// Creates the options of plain UAX #14.
    #[inline]
    pub const fn new() -> Self {
        Self {
            strictness: LineStrictness::Strict,
            word: LineWordOption::Normal,
            ja_zh: false,
            cj_quotes: false,
            loose_hyphens: false,
        }
    }

    /// Returns the rules the options select.
    const fn rules(self) -> Rules {
        let tailoring = match self.strictness {
            LineStrictness::Loose => Tailoring::Loose,
            LineStrictness::Normal if self.ja_zh => Tailoring::NormalJapaneseChinese,
            _ => Tailoring::None,
        };
        Rules {
            tailoring,
            ja_zh: self.ja_zh,
            keep_all: matches!(self.word, LineWordOption::KeepAll),
            loose_hyphens: matches!(self.strictness, LineStrictness::Loose)
                && (self.ja_zh
                    || self.loose_hyphens
                    || matches!(self.word, LineWordOption::BreakAll)),
            #[cfg(test)]
            legacy: 0,
        }
    }

    /// Returns the table that resolves classes under the options (LB1).
    const fn resolve(self) -> &'static [u8; 64] {
        &RESOLVE[self.resolve_index()]
    }

    /// Returns the index of the options' table among the resolve tables.
    const fn resolve_index(self) -> usize {
        let strictness = match self.strictness {
            LineStrictness::Strict | LineStrictness::Anywhere => 0,
            LineStrictness::Normal => 1 + self.ja_zh as usize,
            LineStrictness::Loose => 3 + self.ja_zh as usize,
        };
        let break_all = matches!(self.word, LineWordOption::BreakAll);
        strictness * 2 + break_all as usize
    }
}

/// The state of a scan: the state before the next boundary, and the flags
/// that say what East Asian characters and joiners it follows.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct ScanState {
    /// The state after the last character that is not a joining mark.
    left: u8,
    /// [`LEFT_EAST_ASIAN`], [`BEFORE_EAST_ASIAN`] and [`AFTER_ZWJ`].
    flags: u8,
}

/// The offsets where a line may break in a stream of characters.
///
/// The stream is any cloneable iterator of `(offset, char)`, such as
/// [`Utf8Scalars`](crate::Utf8Scalars) or
/// [`Utf16Scalars`](crate::Utf16Scalars). Each offset yielded is that of a
/// character a line may break before. The start of the stream and its end
/// are not yielded: a line never breaks at the start (LB2) and always at
/// the end (LB3).
///
/// Offsets must increase. At the first one that does not, the iterator
/// ends, as if the text ended there. Nothing it does allocates or panics.
///
/// Thai, Lao, Khmer and Myanmar (Line_Break SA) have no rules for breaks
/// between their words. A line breaks inside a run of one of them only at
/// the words of its dictionary, given by
/// [`with_dictionaries`](Self::with_dictionaries): the longest word at each
/// position, as ICU4X finds them, ending only where a grapheme cluster
/// does. A run of a script with no dictionary does not break inside, and a
/// line may break where a run changes script, if either has a dictionary.
/// Under
/// [`LineWordOption::BreakAll`] SA reads as ID instead.
///
/// # Examples
///
/// ```
/// use cleft::{LineBreaks, LineOptions, Utf8Scalars};
///
/// let text = "The quick (\"brown\") fox";
/// let breaks: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), LineOptions::new()).collect();
/// assert_eq!(breaks, [4, 10, 20]);
/// ```
#[derive(Clone, Debug)]
pub struct LineBreaks<'d, I> {
    scalars: I,
    state: ScanState,
    /// The grapheme state, read under [`LineStrictness::Anywhere`].
    graphemes: GraphemeState,
    anywhere: bool,
    cj_quotes: bool,
    rules: Rules,
    resolve: &'static [u8; 64],
    /// The least offset the next character may have, or `None` once the
    /// stream has ended.
    least: Option<usize>,
    /// Whether the first character has been read.
    started: bool,
    /// Whether SA runs break at the words of a dictionary: everywhere but
    /// under `break-all`, which reads SA as ID.
    reads_runs: bool,
    /// The dictionaries that find the words of SA runs.
    dictionaries: Option<&'d Dictionaries>,
    /// Where the scan stands in an SA run.
    run: Run,
    /// What the tests read in place of the crate's data and rules.
    #[cfg(test)]
    test: TestState,
}

/// What the tests read in place of the crate's data and rules, and the
/// state of the model of ICU4X's SA runs.
#[cfg(test)]
#[derive(Clone, Debug, Default)]
struct TestState {
    /// The line classes of an older Unicode version.
    classes: Option<&'static [u8]>,
    /// The grapheme properties of an older Unicode version.
    grapheme_classes: Option<&'static [crate::GraphemeProps]>,
    /// Whether graphemes follow Unicode 17.0's GB9c.
    graphemes_17: bool,
    /// The breaks still ahead in the SA run the model segmented, with the
    /// run's end where they cover the run to its end.
    model: Option<(std::vec::Vec<usize>, Option<usize>)>,
    /// The last character, with its offset and whether it is SA.
    last: Option<(usize, char, bool)>,
}

impl<'d, I: Iterator<Item = (usize, char)> + Clone> LineBreaks<'d, I> {
    /// Creates an iterator over the break opportunities of `scalars`, which
    /// starts a text.
    #[inline]
    pub fn new(scalars: I, options: LineOptions) -> Self {
        Self::from_options(scalars, options)
    }

    /// Returns the iterator, breaking SA runs at the words of
    /// `dictionaries`.
    ///
    /// A run of a script `dictionaries` has no dictionary for does not break
    /// inside. Call it before reading any break.
    ///
    /// # Examples
    ///
    /// ```
    /// use cleft::{Dictionaries, LineBreaks, LineOptions, Utf8Scalars};
    ///
    /// // With no dictionary, a Thai run does not break inside.
    /// let none = Dictionaries::new();
    /// let text = "กินข้าว";
    /// let breaks: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), LineOptions::new())
    ///     .with_dictionaries(&none)
    ///     .collect();
    /// assert!(breaks.is_empty());
    /// ```
    #[inline]
    pub fn with_dictionaries(mut self, dictionaries: &'d Dictionaries) -> Self {
        self.dictionaries = Some(dictionaries);
        self
    }

    /// Creates an iterator over the break opportunities of `scalars`, which
    /// starts at a break opportunity of a longer text, after the characters
    /// of `preceding`, nearest first.
    ///
    /// The iterator finds the same opportunities after the start as one
    /// that reads the whole text. It reads `preceding` back past any
    /// combining marks to the character they join, so passing the text
    /// before the start reversed reads one or a few characters. A start that
    /// is not an opportunity gives answers that may differ from the whole
    /// text's after it.
    ///
    /// This is how the options change partway through a text: an iterator
    /// with the new options starts at the last opportunity before the
    /// change.
    ///
    /// # Examples
    ///
    /// ```
    /// use cleft::{LineBreaks, LineOptions, Utf8Scalars};
    ///
    /// let text = "a b c";
    /// let preceding = text[..2].chars().rev();
    /// let breaks: Vec<usize> =
    ///     LineBreaks::with_preceding(Utf8Scalars::with_start(text, 2), LineOptions::new(), preceding)
    ///         .collect();
    /// assert_eq!(breaks, [4]);
    /// ```
    pub fn with_preceding(
        scalars: I,
        options: LineOptions,
        preceding: impl IntoIterator<Item = char>,
    ) -> Self {
        let mut breaks = Self::from_options(scalars, options);
        let mut marked = false;
        for ch in preceding {
            let (class, bits) = breaks.classify(ch);
            if class == CM || class == ZWJ {
                marked = true;
                continue;
            }
            breaks.state = if marked && rules::is_unmarkable(class) {
                // The marks after it read as AL (LB10).
                ScanState { left: AL, flags: 0 }
            } else {
                ScanState {
                    // A line breaks between regional indicators only after
                    // an even run of them (LB30a).
                    left: if class == RI { RI_EVEN } else { class },
                    flags: rules::left_flags(bits),
                }
            };
            marked = false;
            break;
        }
        if marked {
            breaks.state.left = AL;
        }
        breaks
    }

    /// Creates the iterator before its first character.
    #[inline]
    fn from_options(scalars: I, options: LineOptions) -> Self {
        Self {
            scalars,
            state: ScanState {
                left: SOT,
                flags: 0,
            },
            graphemes: GraphemeState::new(),
            anywhere: matches!(options.strictness, LineStrictness::Anywhere),
            cj_quotes: options.cj_quotes,
            rules: options.rules(),
            resolve: options.resolve(),
            least: Some(0),
            started: false,
            reads_runs: !matches!(options.word, LineWordOption::BreakAll),
            dictionaries: None,
            run: Run::default(),
            #[cfg(test)]
            test: TestState::default(),
        }
    }

    /// Moves past the first character, before which no line breaks.
    #[inline]
    fn start(&mut self) {
        self.started = true;
        if let Some((offset, ch)) = self.scalars.next() {
            self.least = Some(offset.saturating_add(1));
            self.step(offset, ch, false);
        } else {
            self.least = None;
        }
    }

    /// Returns the line class of `ch` under the options, and its East Asian
    /// and unassigned pictograph bits, as [`LineProps`] holds them.
    #[inline(always)]
    fn classify(&self, ch: char) -> (u8, u8) {
        self.classify_props(ch, self.props(ch))
    }

    /// Returns the line class of `ch`, whose line byte is `props`, under the
    /// options, and its East Asian and unassigned pictograph bits.
    #[inline(always)]
    fn classify_props(&self, ch: char, props: LineProps) -> (u8, u8) {
        let class = self.resolve[usize::from(props.class())];
        if self.cj_quotes && (class == QU_PI || class == QU_PF) {
            match ch {
                '\u{201C}' => return (OP, 0),
                '\u{201D}' => return (CL, 0),
                _ => {}
            }
        }
        (class, props.flags())
    }

    /// Returns the line byte of `ch`.
    #[inline(always)]
    fn props(&self, ch: char) -> LineProps {
        #[cfg(test)]
        if let Some(classes) = self.test.classes {
            return LineProps(classes.get(ch as usize).copied().unwrap_or_default());
        }
        crate::props::line_props(ch)
    }

    /// Moves past `ch`, at `offset`, and returns whether a line may break
    /// before it, asked only where `decides`.
    #[inline(always)]
    fn step(&mut self, offset: usize, ch: char, decides: bool) -> bool {
        if self.anywhere {
            let props = self.grapheme_props(ch);
            return self.graphemes.advance(props) && decides;
        }
        let props = self.props(ch);
        let raw = props.class();
        // A run of SA decides the boundaries inside it.
        let sa = (raw == SA || raw == SA_MARK) && self.reads_runs;
        let run = if sa {
            self.run_step(offset, ch, raw == SA_MARK)
        } else {
            self.run.end();
            None
        };
        #[cfg(test)]
        let run = if self.rules.legacy & rules::Legacy::DICTIONARY_RUNS != 0 {
            self.model_run_step(offset, ch, sa)
        } else {
            run
        };
        let (mut class, mut bits) = self.classify_props(ch, props);
        let ScanState { left, flags } = self.state;
        let joiner = if class == ZWJ { AFTER_ZWJ } else { 0 };
        if class == CM || class == ZWJ {
            // LB9: a mark joins the character before it, which it reads as.
            // A run decides only where a word starts at the mark, which ends
            // no grapheme cluster outside tests.
            if !rules::is_unmarkable(left) {
                self.state.flags = (flags & !AFTER_ZWJ) | joiner;
                return decides && run == Some(true);
            }
            // LB10.
            class = AL;
            #[cfg(test)]
            if self.rules.legacy & rules::Legacy::MARKS_APART != 0 {
                class = rules::MARK_AL;
            }
            bits = 0;
        }
        let breaks = decides
            && match run {
                Some(breaks) => breaks,
                None => self.breaks(left, class, flags, bits, ch),
            };
        let mut next = joiner | rules::left_flags(bits);
        if flags & LEFT_EAST_ASIAN != 0 {
            next |= BEFORE_EAST_ASIAN;
        }
        #[cfg(test)]
        let left = if decides && self.icu_forgets(left, class, flags, bits, ch, breaks) {
            SOT
        } else {
            left
        };
        let left_next = rules::next_left(left, class, ch, &self.rules);
        #[cfg(test)]
        if self.rules.legacy & rules::Legacy::TAILORING_OUTSIDE_RULES != 0 {
            next |= rules::icu_inner_flags(left, class, left_next);
        }
        self.state = ScanState {
            left: left_next,
            flags: next,
        };
        breaks
    }

    /// Moves the SA run past `ch`, at `offset`, which is SA, and a mark where
    /// `mark`. Returns whether a line may break before it where the run
    /// decides, or `None` where the rules do.
    ///
    /// - **Inside a run** a line breaks only where the next word starts.
    /// - **Where the run's script changes** a line breaks if either script
    ///   has a dictionary, unless `ch` is a mark, which starts a run but
    ///   never a line. Without a dictionary the rules decide, reading SA as
    ///   AL (LB1).
    /// - **Where a run starts after other text** the rules decide, with SA
    ///   read as AL (LB1).
    ///
    /// Out of line: SA text alone reaches it.
    #[inline(never)]
    fn run_step(&mut self, offset: usize, ch: char, mark: bool) -> Option<bool> {
        let script = RunScript::from_sa_char(ch);
        let mut run = self.run;
        let breaks = run.step(offset, script, mark, self.dictionaries, |dictionary| {
            dictionary::find_next_word(
                dictionary,
                offset,
                ch,
                self.scalars.clone(),
                |next| self.continues_run(next, script),
                |ch| self.grapheme_props(ch),
                self.grapheme_start(),
            )
        });
        self.run = run;
        breaks
    }

    /// Returns whether `ch` continues a run of SA in `script`.
    #[inline]
    fn continues_run(&self, ch: char, script: RunScript) -> bool {
        let raw = self.props(ch).class();
        (raw == SA || raw == SA_MARK) && RunScript::from_sa_char(ch) == script
    }

    /// Returns the grapheme properties of `ch`.
    #[inline(always)]
    fn grapheme_props(&self, ch: char) -> GraphemeProps {
        #[cfg(test)]
        if let Some(classes) = self.test.grapheme_classes {
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

    /// Moves the model of ICU4X's SA runs past `ch`, at `offset`, which is
    /// SA where `sa`. Returns whether a line may break before it where the
    /// model decides, or `None` where the rules do.
    ///
    /// As ICU4X's line segmenter does, a pair of SA characters with no
    /// segmented run ahead segments the run from the first of them, and the
    /// breaks found answer until they are used up.
    #[cfg(test)]
    fn model_run_step(&mut self, offset: usize, ch: char, sa: bool) -> Option<bool> {
        let previous = self.test.last.replace((offset, ch, sa));
        if let Some((breaks, end)) = &mut self.test.model {
            let found = breaks.first() == Some(&offset);
            breaks.retain(|&at| at > offset);
            let covered = end.is_some_and(|end| offset < end);
            if breaks.is_empty() && !covered {
                self.test.model = None;
            }
            if found {
                return Some(true);
            }
            if !sa {
                self.test.model = None;
                return None;
            }
            if self.test.model.is_some() {
                return Some(false);
            }
        }
        let (start, first, true) = previous? else {
            return None;
        };
        if !sa {
            return None;
        }
        let mut run = std::vec![(start, first), (offset, ch)];
        let mut least = offset.saturating_add(1);
        for (at, next) in self.scalars.clone() {
            let raw = self.props(next).class();
            if at < least || !(raw == SA || raw == SA_MARK) {
                break;
            }
            run.push((at, next));
            least = at.saturating_add(1);
        }
        let legacy = self.rules.legacy;
        let lao = legacy & rules::Legacy::LAO_LO_LING != 0;
        let quirks = crate::tests::RunQuirks {
            slice_ends: legacy & rules::Legacy::RUN_END_BREAKS != 0,
            leaves: legacy & rules::Legacy::DICTIONARY_LEAVES != 0,
            misses: legacy & rules::Legacy::DICTIONARY_MISSES != 0,
            rest: legacy & rules::Legacy::DICTIONARY_REST != 0,
        };
        let (mut breaks, covers) = crate::tests::icu_run_breaks(
            &run,
            self.dictionaries,
            |ch| {
                // ICU4X's Lao range leaves out U+0EA3.
                if lao && ch == '\u{EA3}' {
                    RunScript::Other
                } else {
                    RunScript::from_sa_char(ch)
                }
            },
            quirks,
            |ch| self.grapheme_props(ch),
            self.grapheme_start(),
            |ch| self.props(ch).class() == SA_MARK,
        );
        let found = breaks.first() == Some(&offset);
        breaks.retain(|&at| at > offset);
        let end = run
            .last()
            .map(|&(at, ch)| at + ch.len_utf8())
            .filter(|_| covers);
        if !breaks.is_empty() || end.is_some() {
            self.test.model = Some((breaks, end));
        }
        Some(found)
    }

    /// Returns whether ICU4X's segmenter reads the character after this
    /// pair with no context: where the pair breaks, or where a tailoring
    /// rather than its rules decides it.
    #[cfg(test)]
    fn icu_forgets(
        &self,
        left: u8,
        right: u8,
        flags: u8,
        bits: u8,
        ch: char,
        breaks: bool,
    ) -> bool {
        if self.rules.legacy & rules::Legacy::CONTEXT_FORGOTTEN == 0 {
            return false;
        }
        let pair = Pair {
            left,
            right,
            flags: flags
                | if bits & EAST_ASIAN != 0 {
                    RIGHT_EAST_ASIAN
                } else {
                    0
                },
            flags_unread: false,
            ch,
            next: UNREAD,
            second: UNREAD,
        };
        breaks || rules::icu_tailors(&pair, &self.rules)
    }

    /// Returns whether a line may break between the state `left` and a
    /// character `ch` of class `right`: from the pair table where it
    /// answers, and from the rules otherwise.
    #[inline(always)]
    fn breaks(&self, left: u8, right: u8, flags: u8, bits: u8, ch: char) -> bool {
        let tailors = matches!(self.rules.tailoring, Tailoring::Loose);
        // The table holds the current rules alone.
        #[cfg(test)]
        if self.rules.legacy != 0 {
            return self.decide(left, right, flags, bits, ch);
        }
        if flags & AFTER_ZWJ == 0 && !(tailors && rules::may_tailor(left, right)) {
            let entry = rules::PAIRS
                .get(usize::from(left))
                .and_then(|row| row.get(usize::from(right)));
            if let Some(&entry) = entry
                && entry & rules::PAIR_READS_MORE == 0
            {
                let answer = if self.rules.keep_all {
                    rules::PAIR_BREAKS_KEEP_ALL
                } else {
                    rules::PAIR_BREAKS
                };
                return entry & answer != 0;
            }
        }
        self.decide(left, right, flags, bits, ch)
    }

    /// Returns whether a line may break between the state `left` and a
    /// character `ch` of class `right`, reading ahead where the rules ask.
    #[inline(never)]
    fn decide(&self, left: u8, right: u8, flags: u8, bits: u8, ch: char) -> bool {
        let mut pair = Pair {
            left,
            right,
            flags: flags
                | if bits & EAST_ASIAN != 0 {
                    RIGHT_EAST_ASIAN
                } else {
                    0
                },
            flags_unread: false,
            ch,
            next: UNREAD,
            second: UNREAD,
        };
        loop {
            match rules::decide(&pair, &self.rules) {
                KEEP => return false,
                BREAK => return true,
                NEEDS_NEXT => self.read_ahead(&mut pair, false),
                _ => self.read_ahead(&mut pair, true),
            }
        }
    }

    /// Reads the class after the right character into `pair.next`, and
    /// where `second`, the one after that into `pair.second`. The marks
    /// that join a character are passed over (LB9).
    #[cold]
    #[inline(never)]
    fn read_ahead(&self, pair: &mut Pair, second: bool) {
        let mut scalars = self.scalars.clone();
        let mut least = self.least.unwrap_or(usize::MAX);
        let mut found = [EOT; 2];
        let mut east_asian = false;
        let wanted = if second { 2 } else { 1 };
        let mut count = 0;
        while count < wanted {
            let Some((offset, ch)) = scalars.next() else {
                break;
            };
            if offset < least {
                break;
            }
            least = offset.saturating_add(1);
            let (class, bits) = self.classify(ch);
            if class == CM || class == ZWJ {
                continue;
            }
            if count == 0 {
                east_asian = bits & EAST_ASIAN != 0;
            }
            found[count] = class;
            count += 1;
        }
        pair.next = found[0];
        pair.second = if second { found[1] } else { UNREAD };
        if east_asian {
            pair.flags |= NEXT_EAST_ASIAN;
        }
    }
}

/// The data and rules of an older Unicode version, and ICU4X's quirks, in
/// place of the crate's.
#[cfg(test)]
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Config {
    /// Each code point's line byte, in place of the crate's.
    pub(super) classes: Option<&'static [u8]>,
    /// The [`Legacy`](rules::Legacy) bits.
    pub(super) legacy: u32,
    /// Whether `anywhere` reads Unicode 17.0's GB9c.
    pub(super) graphemes_17: bool,
    /// Each code point's grapheme properties, in place of the crate's.
    pub(super) graphemes: Option<&'static [crate::GraphemeProps]>,
}

#[cfg(test)]
impl<I: Iterator<Item = (usize, char)> + Clone> LineBreaks<'_, I> {
    /// Creates an iterator over the break opportunities of `scalars`, which
    /// starts a text, under `config`.
    pub(super) fn with_config(scalars: I, options: LineOptions, config: Config) -> Self {
        let mut breaks = Self::from_options(scalars, options);
        breaks.test.classes = config.classes;
        breaks.rules.legacy = config.legacy;
        let sa_marks = config.legacy & rules::Legacy::SA_MARKS != 0;
        let icu = config.legacy & rules::Legacy::ICU_TAILORING != 0;
        if sa_marks || icu {
            let tables = &rules::RESOLVE_ICU[usize::from(sa_marks) | usize::from(icu) << 1];
            breaks.resolve = &tables[options.resolve_index()];
        }
        if config.graphemes_17 {
            breaks.graphemes = GraphemeState::with_unicode_17_conjuncts();
            breaks.test.graphemes_17 = true;
        }
        breaks.test.grapheme_classes = config.graphemes;
        breaks
    }
}

impl<I: Iterator<Item = (usize, char)> + Clone> Iterator for LineBreaks<'_, I> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        if !self.started {
            self.start();
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

impl<I: Iterator<Item = (usize, char)> + Clone> core::iter::FusedIterator for LineBreaks<'_, I> {}
