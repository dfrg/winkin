//! `text-transform`, applied as text is written.
//!
//! **Case** uses the crate's full case mapping (`unicode::uppercase` and the
//! like) with the language's tailoring, as CSS Text 3, section 2.1 asks. So
//! `ß` uppercases to `SS`, a final sigma lowercases to `ς`, and Turkish,
//! Lithuanian, Greek and Dutch get their own rules; no other language is
//! tailored. Uppercase makes no Georgian Mtavruli.
//!
//! **Capitalize** titlecases the first character of every word.
//! - Words are UAX #29's, found with the context's dictionaries, as word
//!   motion finds them.
//! - The `en_US_POSIX` tailoring applies: a full stop joins only digits,
//!   so `x.y` is two words and `3.14` one.
//! - The character before the written text counts, so a word split by a span
//!   stays one word. Across calls only the previous call's last character is
//!   known, as with Blink's `LayoutText::PreviousCharacter`.
//! - The titlecase is the full mapping with the language's tailoring, as CSS
//!   asks, so `ß` and `ﬁ` titlecase in full and Dutch gives `IJsland`.
//!
//! **`full-width`** maps printable ASCII, half-width katakana and Hangul and
//! a few signs to their full-width forms, by Blink's
//! `Character::FullwidthVariant` table. A space becomes U+3000 only where white
//! space is kept; a collapsible space is left for collapsing, as CSS Text 3
//! says. It applies after the case.
//!
//! **`full-size-kana`** maps the 58 small kana of CSS Text 3's table
//! (Appendix G, Blink's `Character::FullSizeKanaVariant`) to full size. It
//! applies last, so a half-width small katakana becomes full-width and then
//! full-size.
//!
//! **`math-auto`** maps a single-character text node to mathematical italic
//! by MathML Core's italic table. It excludes every other transform.
//!
//! **Lengths.** A transform may grow or shrink the text (`ŉ` to `ʼN`, a
//! full-width letter, a dropped Greek accent). [`Transforms::counts`] gives
//! each source character's output length in characters; the writer turns
//! those into offset map entries.

use alloc::string::String;
use core::iter;
use core::ops::Range;
use core::str::CharIndices;

use crate::data::TextOffset;
use crate::style::{TextCase, TextTransform};
use cleft::{Dictionaries, Utf8Scalars, WordBreaks, WordOptions};

use crate::unicode::{CaseLanguage, lowercase, titlecase, uppercase};
use crate::work;

/// The `text-transform` of one node, with its case language.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct TextTransformer {
    transform: TextTransform,
    /// The language whose case tailoring applies.
    language: CaseLanguage,
    /// White space is kept (`preserve`, `break-spaces`, `preserve-spaces`),
    /// so `full-width` maps spaces to U+3000.
    keeps_spaces: bool,
}

impl TextTransformer {
    /// Creates a transformer. Any `math-auto` transform becomes plain
    /// `math-auto`, which excludes the others.
    pub(super) fn new(
        transform: TextTransform,
        language: CaseLanguage,
        keeps_spaces: bool,
    ) -> Self {
        let transform = if transform.is_math_auto() {
            TextTransform::MATH_AUTO
        } else {
            transform
        };
        Self {
            transform,
            language,
            keeps_spaces,
        }
    }

    /// Writes `src` transformed onto `out`.
    ///
    /// `before` is the character before `src`; only capitalize reads it,
    /// finding words with `dictionaries`. The output fits in
    /// [`max_growth`](Self::max_growth) times the length of `src`.
    pub(super) fn write(
        &self,
        src: &str,
        before: char,
        dictionaries: &Dictionaries,
        out: &mut String,
    ) {
        let TextTransform {
            full_width,
            full_size_kana,
            ..
        } = self.transform;
        if full_width || full_size_kana {
            self.write_case(src, before, dictionaries, &mut |mut ch| {
                if full_width {
                    ch = self::full_width(ch, self.keeps_spaces);
                }
                if full_size_kana {
                    ch = self::full_size_kana(ch);
                }
                out.push(ch);
            });
        } else if self.transform.is_math_auto() {
            Self::write_math(src, out);
        } else if matches!(self.transform.case, TextCase::None | TextCase::MathAuto) {
            out.push_str(src);
        } else {
            self.write_case(src, before, dictionaries, &mut |ch| out.push(ch));
        }
    }

    /// Whether it capitalizes, which reads the words around a character.
    pub(super) fn capitalizes(&self) -> bool {
        self.transform.case == TextCase::Capitalize
    }

    /// Returns the most bytes the transform makes of one byte of text.
    pub(super) fn max_growth(&self) -> usize {
        if self.transform.is_math_auto() {
            MATH_GROWTH
        } else {
            CASE_GROWTH
        }
    }

    /// Writes the mathematical italic form of each character to `out`.
    #[cold]
    #[inline(never)]
    fn write_math(src: &str, out: &mut String) {
        for ch in src.chars() {
            out.push(math_italic(ch).unwrap_or(ch));
        }
    }

    /// Calls `push` with each character of the case part of
    /// [`write`](Self::write).
    fn write_case(
        &self,
        src: &str,
        before: char,
        dictionaries: &Dictionaries,
        push: &mut dyn FnMut(char),
    ) {
        let language = self.language;
        match self.transform.case {
            TextCase::None | TextCase::MathAuto => src.chars().for_each(push),
            TextCase::Uppercase => uppercase(src, language, push),
            TextCase::Lowercase => lowercase(src, language, push),
            TextCase::Capitalize => words(src, before, dictionaries, |segment, starts| {
                if starts {
                    titlecase(segment, language, push);
                } else {
                    segment.chars().for_each(&mut *push);
                }
            }),
        }
    }

    /// Counts the characters the transform makes of `ch` alone.
    ///
    /// `starts` says whether `ch` starts a word; only capitalize reads it.
    /// [`write`](Self::write) maps in context, so the two can disagree
    /// (a Greek accent, a Lithuanian or Turkish dot). Callers of
    /// [`Transforms::counts`] detect that by the totals disagreeing.
    fn count(&self, ch: char, starts: bool) -> usize {
        // Every case mapping makes one character of an ASCII one, the
        // Turkic `i` included. `full-width` and `full-size-kana` always
        // make one.
        if ch.is_ascii() {
            return 1;
        }
        let language = self.language;
        let mut buffer = [0u8; 4];
        let one = ch.encode_utf8(&mut buffer);
        let mut count = 0;
        let mut counted = |_| count += 1;
        match self.transform.case {
            TextCase::None | TextCase::MathAuto => return 1,
            TextCase::Uppercase => uppercase(one, language, &mut counted),
            TextCase::Lowercase => lowercase(one, language, &mut counted),
            TextCase::Capitalize if starts => titlecase(one, language, &mut counted),
            TextCase::Capitalize => return 1,
        }
        count
    }
}

/// The transforms for a text node: its own, and the first line's.
#[derive(Copy, Clone, Debug)]
pub(super) struct Transforms {
    /// The node's own transform; `None` for none.
    pub(super) own: Option<TextTransformer>,
    /// The first line's transform where it differs from `own`; the inner
    /// `None` means none.
    first_line: Option<Option<TextTransformer>>,
    /// The `-webkit-text-security` mask character for each grapheme, if
    /// any. `::first-line` cannot change it.
    pub(super) mask: Option<char>,
}

impl Transforms {
    /// No transform on any line.
    pub(super) const NONE: Self = Self {
        own: None,
        first_line: None,
        mask: None,
    };

    /// Creates the pair, with no mask.
    pub(super) fn new(
        own: Option<TextTransformer>,
        first_line: Option<Option<TextTransformer>>,
    ) -> Self {
        Self {
            own,
            first_line,
            mask: None,
        }
    }

    /// Returns the pair with `mask` as its mask.
    pub(super) fn with_mask(self, mask: Option<char>) -> Self {
        Self { mask, ..self }
    }

    /// Returns the pair with the first line using the node's own transform.
    pub(super) fn without_first_line(self) -> Self {
        Self {
            first_line: None,
            ..self
        }
    }

    /// Whether either transform is `math-auto`, which depends on the whole
    /// text node.
    pub(super) fn has_math_auto(&self) -> bool {
        [self.own, self.first_line()]
            .into_iter()
            .any(|transformer| transformer.is_some_and(|t| t.transform.is_math_auto()))
    }

    /// Drops `math-auto` unless the node is `single` character with a
    /// mathematical italic form.
    pub(super) fn with_node(self, single: bool, text: &str) -> Self {
        let eligible = || single && text.chars().next().and_then(math_italic).is_some();
        let resolve =
            |t: Option<TextTransformer>| t.filter(|t| !t.transform.is_math_auto() || eligible());
        Self::new(resolve(self.own), self.first_line.map(resolve)).with_mask(self.mask)
    }

    /// Returns the most bytes either transform makes of one byte of text.
    pub(super) fn max_growth(&self) -> usize {
        let growth =
            |transformer: Option<TextTransformer>| transformer.map_or(1, |t| t.max_growth());
        growth(self.own).max(growth(self.first_line()))
    }

    /// Whether the first line's transform differs from the node's own.
    pub(super) fn first_line_differs(&self) -> bool {
        self.first_line.is_some()
    }

    /// Returns the first line's transform; `None` for none.
    pub(super) fn first_line(&self) -> Option<TextTransformer> {
        self.first_line.unwrap_or(self.own)
    }

    /// Calls `each` with every character of `src` and the number of
    /// characters the own and first-line transforms make of it alone.
    ///
    /// `before` is the character before `src`, for capitalize, which finds
    /// words with `dictionaries`. The offset map aligns by the own count, and the
    /// first-line text by both. The first-line count equals the own count
    /// unless [`first_line_differs`](Self::first_line_differs).
    pub(super) fn counts(
        &self,
        src: &str,
        before: char,
        dictionaries: &Dictionaries,
        each: &mut dyn FnMut(char, usize, usize),
    ) {
        let (ours, theirs) = (self.own, self.first_line());
        let differs = self.first_line_differs();
        let mut counted = |ch: char, starts: bool| {
            work::step();
            let count = |transformer: Option<TextTransformer>| {
                transformer.map_or(1, |transformer| transformer.count(ch, starts))
            };
            let own = count(ours);
            each(ch, own, if differs { count(theirs) } else { own });
        };
        // Only capitalize asks whether a character starts a word.
        let capitalizes = |transformer: Option<TextTransformer>| {
            transformer
                .is_some_and(|transformer| transformer.transform.case == TextCase::Capitalize)
        };
        if !capitalizes(ours) && !capitalizes(theirs) {
            src.chars().for_each(|ch| counted(ch, false));
            return;
        }
        words(src, before, dictionaries, |segment, starts| {
            for (at, ch) in segment.chars().enumerate() {
                counted(ch, starts && at == 0);
            }
        });
    }
}

/// The outputs of two transforms for one source character.
///
/// Ranges are byte offsets from the start of each transform's output for
/// the run; `counts` are their lengths in characters.
#[derive(Clone, Debug)]
pub(super) struct Made {
    pub(super) ours: Range<TextOffset>,
    pub(super) theirs: Range<TextOffset>,
    counts: (usize, usize),
}

impl Made {
    /// Whether both outputs are one character of the same byte length.
    pub(super) fn is_aligned(&self) -> bool {
        let len = |range: &Range<TextOffset>| range.end.get() - range.start.get();
        self.counts == (1, 1) && len(&self.ours) == len(&self.theirs)
    }
}

/// Walks `ours` and `theirs`, two transforms of one source run, one source
/// character at a time.
///
/// `counts` reports per-character output counts, as [`Transforms::counts`]
/// does; `each` gets each character's [`Made`]. Returns whether the counts
/// walk both texts exactly to their ends. When they don't (context changed a
/// mapping), `each` stops at the first character that doesn't fit and the
/// caller maps the run as a whole.
pub(super) fn align_counts(
    ours: &str,
    theirs: &str,
    counts: impl FnOnce(&mut dyn FnMut(char, usize, usize)),
    mut each: impl FnMut(Made),
) -> bool {
    let mut ours_chars = ours.char_indices();
    let mut theirs_chars = theirs.char_indices();
    // Byte positions in `ours` and `theirs`.
    let (mut here, mut there) = (0, 0);
    let mut whole = true;
    // Takes `count` characters of `chars`, moving `to` past them.
    let take = |chars: &mut CharIndices<'_>, count: usize, to: &mut usize| {
        for _ in 0..count {
            let Some((at, ch)) = chars.next() else {
                return false;
            };
            *to = at + ch.len_utf8();
        }
        true
    };
    counts(&mut |_, ours_count, theirs_count| {
        if !whole {
            return;
        }
        let (was_here, was_there) = (here, there);
        whole = take(&mut ours_chars, ours_count, &mut here)
            && take(&mut theirs_chars, theirs_count, &mut there);
        if whole {
            each(Made {
                ours: TextOffset::new(was_here)..TextOffset::new(here),
                theirs: TextOffset::new(was_there)..TextOffset::new(there),
                counts: (ours_count, theirs_count),
            });
        }
    });
    whole && ours_chars.next().is_none() && theirs_chars.next().is_none()
}

/// The most bytes a case mapping, `full-width` or `full-size-kana` makes of
/// one byte: a full-width ASCII form is three bytes, and no case mapping
/// does more.
const CASE_GROWTH: usize = 3;

/// The most bytes `math-auto` makes of one byte: a mathematical italic
/// letter is four bytes for one ASCII byte.
const MATH_GROWTH: usize = 4;

/// MathML Core's italic mappings.
/// <https://w3c.github.io/mathml-core/#italic-mappings>
fn math_italic(ch: char) -> Option<char> {
    let codepoint = match ch {
        'A'..='Z' => ch as u32 + 0x1D3F3,
        'h' => 0x210E,
        'a'..='z' => ch as u32 + 0x1D3ED,
        '\u{131}' => 0x1D6A4,
        '\u{237}' => 0x1D6A5,
        '\u{391}'..='\u{3A1}' | '\u{3A3}'..='\u{3A9}' => ch as u32 + 0x1D351,
        '\u{3B1}'..='\u{3C9}' => ch as u32 + 0x1D34B,
        '\u{3F4}' => 0x1D6F3,
        '\u{2207}' => 0x1D6FB,
        '\u{2202}' => 0x1D715,
        '\u{3F5}' => 0x1D716,
        '\u{3D1}' => 0x1D717,
        '\u{3F0}' => 0x1D718,
        '\u{3D5}' => 0x1D719,
        '\u{3F1}' => 0x1D71A,
        '\u{3D6}' => 0x1D71B,
        _ => return None,
    };
    char::from_u32(codepoint)
}

/// Calls `each` with the UAX #29 word segments of `src` and whether each
/// starts a word (rather than continuing the word `before` is in).
///
/// - Words are found with `dictionaries` and the `en_US_POSIX` full
///   stop, as word motion finds them, so `x.y` is three words and `3.14`
///   one.
/// - The dictionaries split Thai, ideographs and kana into words.
fn words(src: &str, before: char, dictionaries: &Dictionaries, mut each: impl FnMut(&str, bool)) {
    // A word never continues past white space or a line end, so `before`
    // joins the text only when it could be inside a word.
    let joined = !matches!(
        before,
        ' ' | '\t' | '\n' | '\r' | '\x0C' | '\u{200B}' | '\u{FFFC}'
    );
    let skip = if joined { before.len_utf8() } else { 0 };
    let scalars = joined
        .then_some((0, before))
        .into_iter()
        .chain(Utf8Scalars::new(src).map(move |(at, ch)| (at + skip, ch)));
    let options = WordOptions {
        posix_full_stop: true,
    };
    let breaks = WordBreaks::new(scalars, options).with_dictionaries(dictionaries);
    let mut start = 0;
    for end in breaks.chain(iter::once(skip + src.len())) {
        work::step();
        if end > skip && end > start {
            // The segment's part in `src`, past `before`.
            let from = start.max(skip);
            if let Some(segment) = src.get(from - skip..end - skip) {
                each(segment, start >= skip);
            }
        }
        start = end;
    }
}

/// Returns the full-size form of a small kana, or `ch` unchanged.
///
/// The table is CSS Text 3's (Appendix G).
pub(super) fn full_size_kana(ch: char) -> char {
    match ch {
        // Hiragana.
        '\u{3041}' => '\u{3042}',
        '\u{3043}' => '\u{3044}',
        '\u{3045}' => '\u{3046}',
        '\u{3047}' => '\u{3048}',
        '\u{3049}' => '\u{304A}',
        '\u{3095}' => '\u{304B}',
        '\u{3096}' => '\u{3051}',
        '\u{1B132}' => '\u{3053}',
        '\u{3063}' => '\u{3064}',
        '\u{3083}' => '\u{3084}',
        '\u{3085}' => '\u{3086}',
        '\u{3087}' => '\u{3088}',
        '\u{308E}' => '\u{308F}',
        '\u{1B150}' => '\u{3090}',
        '\u{1B151}' => '\u{3091}',
        '\u{1B152}' => '\u{3092}',
        // Katakana.
        '\u{30A1}' => '\u{30A2}',
        '\u{30A3}' => '\u{30A4}',
        '\u{30A5}' => '\u{30A6}',
        '\u{30A7}' => '\u{30A8}',
        '\u{30A9}' => '\u{30AA}',
        '\u{30F5}' => '\u{30AB}',
        '\u{31F0}' => '\u{30AF}',
        '\u{30F6}' => '\u{30B1}',
        '\u{1B155}' => '\u{30B3}',
        '\u{31F1}' => '\u{30B7}',
        '\u{31F2}' => '\u{30B9}',
        '\u{30C3}' => '\u{30C4}',
        '\u{31F3}' => '\u{30C8}',
        '\u{31F4}' => '\u{30CC}',
        '\u{31F5}' => '\u{30CF}',
        '\u{31F6}' => '\u{30D2}',
        '\u{31F7}' => '\u{30D5}',
        '\u{31F8}' => '\u{30D8}',
        '\u{31F9}' => '\u{30DB}',
        '\u{31FA}' => '\u{30E0}',
        '\u{30E3}' => '\u{30E4}',
        '\u{30E5}' => '\u{30E6}',
        '\u{30E7}' => '\u{30E8}',
        '\u{31FB}' => '\u{30E9}',
        '\u{31FC}' => '\u{30EA}',
        '\u{31FD}' => '\u{30EB}',
        '\u{31FE}' => '\u{30EC}',
        '\u{31FF}' => '\u{30ED}',
        '\u{30EE}' => '\u{30EF}',
        '\u{1B164}' => '\u{30F0}',
        '\u{1B165}' => '\u{30F1}',
        '\u{1B166}' => '\u{30F2}',
        '\u{1B167}' => '\u{30F3}',
        // Half-width katakana.
        '\u{FF67}' => '\u{FF71}',
        '\u{FF68}' => '\u{FF72}',
        '\u{FF69}' => '\u{FF73}',
        '\u{FF6A}' => '\u{FF74}',
        '\u{FF6B}' => '\u{FF75}',
        '\u{FF6F}' => '\u{FF82}',
        '\u{FF6C}' => '\u{FF94}',
        '\u{FF6D}' => '\u{FF95}',
        '\u{FF6E}' => '\u{FF96}',
        _ => ch,
    }
}

/// Returns the full-width form of `ch`, or `ch` unchanged, as Blink's
/// `Character::FullwidthVariant` does. A space maps only if `keeps_spaces`.
fn full_width(ch: char, keeps_spaces: bool) -> char {
    const KATAKANA: [u16; 63] = [
        0x3002, 0x300C, 0x300D, 0x3001, 0x30FB, 0x30F2, 0x30A1, 0x30A3, 0x30A5, 0x30A7, 0x30A9,
        0x30E3, 0x30E5, 0x30E7, 0x30C3, 0x30FC, 0x30A2, 0x30A4, 0x30A6, 0x30A8, 0x30AA, 0x30AB,
        0x30AD, 0x30AF, 0x30B1, 0x30B3, 0x30B5, 0x30B7, 0x30B9, 0x30BB, 0x30BD, 0x30BF, 0x30C1,
        0x30C4, 0x30C6, 0x30C8, 0x30CA, 0x30CB, 0x30CC, 0x30CD, 0x30CE, 0x30CF, 0x30D2, 0x30D5,
        0x30D8, 0x30DB, 0x30DE, 0x30DF, 0x30E0, 0x30E1, 0x30E2, 0x30E4, 0x30E6, 0x30E8, 0x30E9,
        0x30EA, 0x30EB, 0x30EC, 0x30ED, 0x30EF, 0x30F3, 0x3099, 0x309A,
    ];
    // Full-width forms of half-width Hangul from U+FFA0; 0 marks the
    // block's unassigned gaps.
    const HANGUL: [u16; 61] = [
        0x3164, 0x3131, 0x3132, 0x3133, 0x3134, 0x3135, 0x3136, 0x3137, 0x3138, 0x3139, 0x313A,
        0x313B, 0x313C, 0x313D, 0x313E, 0x313F, 0x3140, 0x3141, 0x3142, 0x3143, 0x3144, 0x3145,
        0x3146, 0x3147, 0x3148, 0x3149, 0x314A, 0x314B, 0x314C, 0x314D, 0x314E, 0, 0, 0, 0x314F,
        0x3150, 0x3151, 0x3152, 0x3153, 0x3154, 0, 0, 0x3155, 0x3156, 0x3157, 0x3158, 0x3159,
        0x315A, 0, 0, 0x315B, 0x315C, 0x315D, 0x315E, 0x315F, 0x3160, 0, 0, 0x3161, 0x3162, 0x3163,
    ];
    let code = u32::from(ch);
    let mapped = match code {
        0x20 if keeps_spaces => 0x3000,
        0x21..=0x7E => code + 0xFEE0,
        0xFF61..=0xFF9F => usize::try_from(code - 0xFF61)
            .ok()
            .and_then(|at| KATAKANA.get(at))
            .map_or(code, |&to| u32::from(to)),
        0xFFA0..=0xFFDC => match usize::try_from(code - 0xFFA0)
            .ok()
            .and_then(|at| HANGUL.get(at))
        {
            Some(&to) if to != 0 => u32::from(to),
            _ => code,
        },
        0xA2 => 0xFFE0,
        0xA3 => 0xFFE1,
        0xAC => 0xFFE2,
        0xAF => 0xFFE3,
        0xA6 => 0xFFE4,
        0xA5 => 0xFFE5,
        0x20A9 => 0xFFE6,
        0x2985 => 0xFF5F,
        0x2986 => 0xFF60,
        0xFFE8 => 0x2502,
        0xFFE9 => 0x2190,
        0xFFEA => 0x2191,
        0xFFEB => 0x2192,
        0xFFEC => 0x2193,
        0xFFED => 0x25A0,
        0xFFEE => 0x25CB,
        _ => code,
    };
    char::from_u32(mapped).unwrap_or(ch)
}
