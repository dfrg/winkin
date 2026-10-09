//! `text-transform`, applied as text is written.
//!
//! **Case** is ICU4X's full case mapping (`icu_casemap`), as CSS Text 3,
//! section 2.1 asks: the full mappings, the conditional ones included, with
//! the language's tailoring. So `ß` uppercases to `SS`, a final sigma
//! lowercases to `ς`, Turkish and Azerbaijani case their dotted and dotless
//! `i` as their own capitals, Lithuanian keeps its dots, Greek drops its
//! accents in capitals, and Dutch titlecases `ij` whole. Chrome maps upper
//! and lower case the same way, through ICU with the text's locale
//! (`CaseMap`).
//!
//! **Capitalize** titlecases the first character of every word, and nothing
//! else.
//! - The words are Unicode's (UAX #29), found by the build's ICU4X word
//!   segmenter, the same one word motion uses (`unicode::segmenters`). It
//!   has dictionaries or an LSTM for Thai and the like. Chrome finds words
//!   with ICU's word break iterator.
//! - Chrome's one tailoring applies: Blink breaks words in `en_US_POSIX`,
//!   whose rules make a full stop join only digits. So `x.y` is two words
//!   and `3.14` one.
//! - The character before what is written counts, so a word a span divides
//!   is one word. Within one call, an apostrophe or a mark inside a word is
//!   passed over, as the segmenter passes over them. Across calls only the
//!   last character of the call before is known, as Chrome knows only the
//!   last character of the text before (`LayoutText::PreviousCharacter`).
//! - The titlecase is the full mapping with the language's tailoring, as
//!   CSS asks and Chrome's `ICUCapitalization` does. Chrome as it ships
//!   titlecases one UTF-16 unit by the simple mapping and no language. It
//!   leaves `ß` and `ﬁ` as they are and sets `ijsland` as `Ijsland` in
//!   Dutch. Chrome is moving off that limitation, so this crate does not
//!   copy it.
//!
//! **`full-width`** maps each character to its full-width form, as Blink's
//! `Character::FullwidthVariant` does (behind Chrome's
//! `CSSTextTransformFullWidth` flag). It maps printable ASCII to U+FF01
//! onward, half-width katakana and Hangul to their full-width forms, and a
//! few signs. A space becomes U+3000 only where white space is kept. A space
//! that collapses is left for collapsing, as CSS Text 3 says and Blink's
//! `ApplyFullwidthTransform` does. It applies after the case, as CSS orders
//! them.
//!
//! **`full-size-kana`** maps each small kana to its full-size form, by CSS
//! Text 3's table (Appendix G). The table holds the small hiragana and
//! katakana, those of the Kana Extended and Small Kana Extension blocks, and
//! the half-width small katakana: 58 in all, the same table as Blink's
//! `Character::FullSizeKanaVariant`. It applies last, after `full-width`, as
//! CSS orders them and Blink's `ComputedStyle::ApplyTextTransform` does. So
//! a half-width small katakana that `full-width` made full-width is then
//! made full-size.
//!
//! **`math-auto`** maps single-character text nodes to mathematical italic
//! by MathML Core's italic table. It excludes every other transform.
//!
//! **Lengths.** A transform may change how long the text is: `ß` to `SS`
//! keeps its bytes, `ŉ` to `ʼN` grows by one, a full-width letter by two, and
//! a dropped Greek accent shrinks it. Nothing here stores where. The offset
//! map needs each source character's own output, which
//! [`Transforms::counts`] gives in characters. The writer turns those into
//! map units where it writes the text.

use alloc::string::String;
use core::fmt;

use icu_casemap::options::{LeadingAdjustment, TitlecaseOptions, TrailingCase};
use icu_casemap::{CaseMapper, CaseMapperBorrowed};
use icu_locale_core::{LanguageIdentifier, langid};
use writeable::Writeable;

use crate::style::{Language, TextCase, TextTransform};
use crate::unicode::{has_stand_ins, segmenter_char, word_boundaries, word_segmenter};
use crate::work;

static ROOT: LanguageIdentifier = LanguageIdentifier::UNKNOWN;
static TURKISH: LanguageIdentifier = langid!("tr");
static LITHUANIAN: LanguageIdentifier = langid!("lt");
static GREEK: LanguageIdentifier = langid!("el");
static DUTCH: LanguageIdentifier = langid!("nl");
static ARMENIAN: LanguageIdentifier = langid!("hy");

/// Returns the language ICU4X maps the case of text in `language` by.
///
/// The primary subtag decides. It is one of the languages whose mapping
/// ICU4X tailors away from the root's (`CaseMapLocale`), or the root for
/// every other language and for none.
pub(super) fn case_langid(language: &Language) -> &'static LanguageIdentifier {
    match language.language() {
        // `i` and `ı` are two letters.
        "tr" | "az" => &TURKISH,
        // An `i` keeps its dot under an accent.
        "lt" => &LITHUANIAN,
        // Capitals drop their accents.
        "el" => &GREEK,
        // `ij` titlecases whole.
        "nl" => &DUTCH,
        // The ligature `և` uppercases to two letters its own way.
        "hy" => &ARMENIAN,
        _ => &ROOT,
    }
}

/// How one node's text is transformed.
///
/// It holds the node's `text-transform`, the language its case is mapped
/// in, and whether its white space is kept, which decides whether
/// `full-width` maps its spaces.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct TextTransformer {
    transform: TextTransform,
    /// The language whose tailoring maps its case ([`case_langid`]).
    langid: &'static LanguageIdentifier,
    /// White space is kept (`preserve`, `break-spaces`, `preserve-spaces`).
    /// Its spaces are then content, and `full-width` makes them U+3000.
    keeps_spaces: bool,
}

/// The case mapper, with its data compiled in, so it is free to make.
const CASE: CaseMapperBorrowed<'static> = CaseMapper::new();

/// Returns options that titlecase the first character of a word and leave
/// the rest as it is.
fn title_options() -> TitlecaseOptions {
    let mut options = TitlecaseOptions::default();
    options.leading_adjustment = Some(LeadingAdjustment::None);
    options.trailing_case = Some(TrailingCase::Unchanged);
    options
}

impl TextTransformer {
    /// Creates a transformer for `transform`, mapping case as `langid`
    /// tailors it, with white space kept or not (`keeps_spaces`).
    pub(super) fn new(
        transform: TextTransform,
        langid: &'static LanguageIdentifier,
        keeps_spaces: bool,
    ) -> Self {
        let transform = if matches!(transform.case, TextCase::MathAuto) {
            TextTransform::MATH_AUTO
        } else {
            transform
        };
        Self {
            transform,
            langid,
            keeps_spaces,
        }
    }

    /// Writes `src` transformed onto `out`.
    ///
    /// `before` is the character before `src`, whose word `src` may carry
    /// on. Only capitalize reads it. `scratch` holds the two together where
    /// needed.
    ///
    /// It writes nothing past what the transform makes, so a caller that
    /// left room for [`MAX_TRANSFORM_GROWTH`] times `src` has room for all
    /// of it.
    pub(super) fn write(&self, src: &str, before: char, scratch: &mut String, out: &mut String) {
        if self.transform.full_width || self.transform.full_size_kana {
            let mut mapped = CharForms {
                out,
                full_width: self.transform.full_width,
                full_size_kana: self.transform.full_size_kana,
                keeps_spaces: self.keeps_spaces,
            };
            self.write_case(src, before, scratch, &mut mapped);
        } else if matches!(self.transform.case, TextCase::MathAuto) {
            Self::write_math(src, out);
        } else {
            self.write_case(src, before, scratch, out);
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

    /// Writes the case part of [`write`](Self::write) into `sink`.
    fn write_case<W: fmt::Write>(
        &self,
        src: &str,
        before: char,
        scratch: &mut String,
        sink: &mut W,
    ) {
        let langid = self.langid;
        // Writing into a string never fails.
        let _ = match self.transform.case {
            TextCase::None | TextCase::MathAuto => sink.write_str(src),
            TextCase::Uppercase => CASE.uppercase(src, langid).write_to(sink),
            TextCase::Lowercase => CASE.lowercase(src, langid).write_to(sink),
            TextCase::Capitalize => {
                let mut result = Ok(());
                words(src, before, scratch, |segment, starts| {
                    result = result.and_then(|()| {
                        if starts {
                            CASE.titlecase_segment_with_only_case_data(
                                segment,
                                langid,
                                title_options(),
                            )
                            .write_to(sink)
                        } else {
                            sink.write_str(segment)
                        }
                    });
                });
                result
            }
        };
    }

    /// Counts the characters the transform makes of `ch` on its own.
    ///
    /// `starts` says whether `ch` starts a word, which only capitalize
    /// reads. [`write`](Self::write) maps a character in its context, so
    /// the two disagree for the few mappings whose length a context changes
    /// (a Greek accent dropped under a capital, a Lithuanian or Turkish
    /// dot). [`Transforms::counts`]'s callers find those by the totals
    /// disagreeing.
    fn count(&self, ch: char, starts: bool) -> usize {
        // Every case mapping makes one character of an ASCII one, the
        // Turkic `i` included. `full-width` and `full-size-kana` make one
        // character of any.
        if ch.is_ascii() {
            return 1;
        }
        let langid = self.langid;
        let mut buffer = [0u8; 4];
        let one = ch.encode_utf8(&mut buffer);
        let mut count = Count(0);
        // Counting never fails.
        let _ = match self.transform.case {
            TextCase::None | TextCase::MathAuto => return 1,
            TextCase::Uppercase => CASE.uppercase(one, langid).write_to(&mut count),
            TextCase::Lowercase => CASE.lowercase(one, langid).write_to(&mut count),
            TextCase::Capitalize if starts => CASE
                .titlecase_segment_with_only_case_data(one, langid, title_options())
                .write_to(&mut count),
            TextCase::Capitalize => return 1,
        };
        count.0
    }
}

/// The writer's pair of transformers for a text node's text.
///
/// One follows the node's own style. The other applies on the block's
/// first line, where the first-line style says otherwise.
#[derive(Copy, Clone, Debug)]
pub(super) struct Transforms {
    /// How the text's own style transforms it, or `None` for not at all.
    pub(super) own: Option<TextTransformer>,
    /// How the block's first line transforms the text (`None` for not at
    /// all), where that differs from the node's own style.
    first_line: Option<Option<TextTransformer>>,
}

impl Transforms {
    /// Text written as given, on every line.
    pub(super) const NONE: Self = Self {
        own: None,
        first_line: None,
    };

    /// Creates the pair: `own` for every line (`None` for not at all), and
    /// `first_line` for the first line where it says otherwise.
    pub(super) fn new(
        own: Option<TextTransformer>,
        first_line: Option<Option<TextTransformer>>,
    ) -> Self {
        Self { own, first_line }
    }

    /// Whether either variant asks for the whole text node's length.
    pub(super) fn has_math_auto(&self) -> bool {
        [self.own, self.first_line()]
            .into_iter()
            .any(|transformer| {
                transformer.is_some_and(|t| matches!(t.transform.case, TextCase::MathAuto))
            })
    }

    /// Disables `math-auto` where the source node is not one mapped character.
    pub(super) fn with_node(self, single: bool, text: &str) -> Self {
        let eligible = || single && text.chars().next().and_then(math_italic).is_some();
        let resolve = |t: Option<TextTransformer>| {
            t.filter(|t| !matches!(t.transform.case, TextCase::MathAuto) || eligible())
        };
        Self::new(resolve(self.own), self.first_line.map(resolve))
    }

    /// Whether the first line transforms the text otherwise than its own
    /// style does.
    pub(super) fn first_line_differs(&self) -> bool {
        self.first_line.is_some()
    }

    /// Returns how the first line transforms the text, or `None` for not at
    /// all. It is the own style's transform unless the first-line style
    /// says otherwise.
    pub(super) fn first_line(&self) -> Option<TextTransformer> {
        self.first_line.unwrap_or(self.own)
    }

    /// Calls `each` with every character of `src` in order, and how many
    /// characters the text's own transform and the first line's each make
    /// of it on its own.
    ///
    /// `before` is the character before `src`, which capitalize reads, and
    /// `scratch` is where it joins it to `src`. The counts align outputs a
    /// caller's character at a time. The offset map aligns the content's
    /// text with the caller's by the own count. The first line's side text
    /// aligns with the content's by both. The first line's count is made
    /// only where it transforms otherwise, and equals the own count where
    /// it does not.
    pub(super) fn counts(
        &self,
        src: &str,
        before: char,
        scratch: &mut String,
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
        words(src, before, scratch, |segment, starts| {
            for (at, ch) in segment.chars().enumerate() {
                counted(ch, starts && at == 0);
            }
        });
    }
}

/// The most bytes a transform makes of one byte of text.
///
/// A mathematical italic letter is four bytes of one of ASCII, a full-width
/// form three, and no case mapping does more. The writer leaves this much
/// room before it writes a transform.
pub(super) const MAX_TRANSFORM_GROWTH: usize = 4;

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

/// Calls `each` with the word segments of `src` in order, and whether the
/// segment starts a word or carries on the one `before` is in.
///
/// - The segments are UAX #29's. They are found with `before` in front where
///   it could carry a word on, copied into `scratch` with `src`.
/// - Chrome's `en_US_POSIX` tailoring applies: a full stop between two
///   letters, or a letter and a digit, ends a word, since only digits join
///   across one there (`unicode::word_boundaries`, as word motion finds
///   them).
/// - The word segmenter is the build's, the same as word motion's
///   (`unicode::word_segmenter`). With the `dictionaries` feature, ICU's
///   dictionaries part a run of Thai or of ideographs and kana into words,
///   as ICU4C parts it for Chrome.
/// - Without that feature, each ideograph and hiragana is copied into
///   `scratch` as a stand-in character (`unicode::segmenter_char`). UAX #29's
///   rules segment the stand-in as they would the original, and it is as
///   long, so the segments are read from `src` at the copy's offsets.
fn words(src: &str, before: char, scratch: &mut String, mut each: impl FnMut(&str, bool)) {
    let segmenter = word_segmenter();
    // A word never carries on past white space or a line's end, so only a
    // character that could be inside one is looked at with the text.
    let joined = !matches!(
        before,
        ' ' | '\t' | '\n' | '\r' | '\x0C' | '\u{200B}' | '\u{FFFC}'
    );
    let skip = if joined { before.len_utf8() } else { 0 };
    let text = if joined || has_stand_ins(src) {
        scratch.clear();
        let mut push = |ch: char| scratch.push(segmenter_char(ch).unwrap_or(ch));
        if joined {
            push(before);
        }
        src.chars().for_each(push);
        scratch.as_str()
    } else {
        src
    };
    let mut start = 0;
    for end in word_boundaries(segmenter, text, skip) {
        work::step();
        if end > skip && end > start {
            // Takes the segment's part in `src`, and whether it starts there.
            let from = start.max(skip);
            if let Some(segment) = src.get(from.saturating_sub(skip)..end.saturating_sub(skip)) {
                each(segment, start >= skip);
            }
        }
        start = end;
    }
}

/// Counts the characters written to it.
struct Count(usize);

impl fmt::Write for Count {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 += s.chars().count();
        Ok(())
    }
}

/// A writer that maps each character to the forms the transform asks for.
///
/// It applies full-width under `full-width`, then full-size under
/// `full-size-kana`, as CSS orders them.
struct CharForms<'a> {
    out: &'a mut String,
    full_width: bool,
    full_size_kana: bool,
    keeps_spaces: bool,
}

impl fmt::Write for CharForms<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for ch in s.chars() {
            let ch = if self.full_width {
                full_width(ch, self.keeps_spaces)
            } else {
                ch
            };
            let ch = if self.full_size_kana {
                full_size_kana(ch)
            } else {
                ch
            };
            self.out.push(ch);
        }
        Ok(())
    }
}

/// Returns `ch`'s full-size form where it is a small kana, or `ch`.
///
/// The table is CSS Text 3's (Appendix G), which is Blink's
/// `Character::FullSizeKanaVariant`.
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

/// Returns `ch`'s full-width form, or `ch` where it has none, as Blink's
/// `Character::FullwidthVariant` does.
///
/// A space has one only where white space is kept (`keeps_spaces`).
fn full_width(ch: char, keeps_spaces: bool) -> char {
    const KATAKANA: [u16; 63] = [
        0x3002, 0x300C, 0x300D, 0x3001, 0x30FB, 0x30F2, 0x30A1, 0x30A3, 0x30A5, 0x30A7, 0x30A9,
        0x30E3, 0x30E5, 0x30E7, 0x30C3, 0x30FC, 0x30A2, 0x30A4, 0x30A6, 0x30A8, 0x30AA, 0x30AB,
        0x30AD, 0x30AF, 0x30B1, 0x30B3, 0x30B5, 0x30B7, 0x30B9, 0x30BB, 0x30BD, 0x30BF, 0x30C1,
        0x30C4, 0x30C6, 0x30C8, 0x30CA, 0x30CB, 0x30CC, 0x30CD, 0x30CE, 0x30CF, 0x30D2, 0x30D5,
        0x30D8, 0x30DB, 0x30DE, 0x30DF, 0x30E0, 0x30E1, 0x30E2, 0x30E4, 0x30E6, 0x30E8, 0x30E9,
        0x30EA, 0x30EB, 0x30EC, 0x30ED, 0x30EF, 0x30F3, 0x3099, 0x309A,
    ];
    // The full-width forms of half-width Hangul, from U+FFA0. The gaps
    // Unicode leaves in the half-width block hold 0, for no mapping.
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
