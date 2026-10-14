//! Generates `crates/winkin/src/unicode/tables.rs`.
//!
//! Run it from anywhere in the workspace, then format what it wrote:
//!
//! ```text
//! cargo run --release -p unicode_gen
//! cargo fmt
//! ```
//!
//! The file is committed. Running both again reproduces it byte for byte;
//! winkin's sweep test (`unicode/tests.rs`) checks it against `icu_properties`
//! for every character.
//!
//! # Why a packed table
//!
//! Analysis asks for a handful of properties of every character. Asking
//! `icu_properties` means one trie or inversion list per property, and above
//! U+0FFF an out-of-line call and four to six dependent loads for each trie:
//! about ten times what one packed lookup costs (research `icu4x-text.md` §6).
//! So everything is packed into two `u32` words per code point, each a
//! three-level packtab lookup with no slow path: three dependent loads in
//! every plane.
//!
//! # The core word
//!
//! Read for every character by the analysis loop.
//!
//! | Bits | Field |
//! |---:|---|
//! | 0..5 | bidi class, in ICU4C's `UCharDirection` numbering |
//! | 5..15 | script, or a Script_Extensions set id plus `SCRIPT_COUNT` |
//! | 15..22 | bracket pair id plus one, or zero for none |
//! | 22 | the bracket opens its pair |
//! | 23 | Regional_Indicator |
//! | 24 | Extended_Pictographic |
//! | 25 | Emoji_Presentation |
//! | 26 | paragraph separator |
//! | 27 | White_Space |
//! | 28 | Line_Break = SA |
//! | 29 | General_Category Mn, Mc or Me: a mark |
//! | 30 | Emoji |
//! | 31 | zero |
//!
//! # The rare word
//!
//! Read per cluster, or behind a gate a paragraph's flags open.
//!
//! | Bits | Field |
//! |---:|---|
//! | 0 | Joining_Type = Join_Causing |
//! | 1 | East_Asian_Width = Wide or Fullwidth |
//! | 2 | Vertical_Orientation = U, Tu or Tr: upright in a vertical line under `text-orientation: mixed` |
//! | 3 | Line_Break = CJ |
//! | 4 | Line_Break = GL, WJ or ZWJ: glue |
//! | 5 | Emoji_Modifier_Base |
//! | 6 | Emoji_Component |
//! | 7 | Default_Ignorable_Code_Point |
//! | 8..12 | Grapheme_Cluster_Break, ICU4C's numbering with ZWJ moved to 13 |
//! | 12..14 | Indic_Conjunct_Break, ICU4C's numbering |
//! | 14..17 | East Asian spacing: 0 other, 1 narrow, 2 narrow digit, 3 conditional, 4 wide, 5 a mark |
//! | 17..21 | the text-spacing class: Blink's `HanKerningCharType`, 0 to 10 |
//! | 21..24 | the emphasis class: 0 marked, 1 a space, 2 punctuation, 3 a symbol, 4 never marked |
//! | 24..26 | the first-letter class: 0 other, 1 a space, 2 opening punctuation or a dash, 3 other punctuation |
//! | 26 | Cased |
//! | 27 | Case_Ignorable |
//! | 28..32 | zero |
//!
//! Joining_Type, East_Asian_Width, Vertical_Orientation and the glue classes
//! were each a field of their own until the simplification wave
//! (2026-09-29), ten bits in all, where every stage reads each as the one
//! question its bit now answers: a join-causing character that letter
//! spacing sees through, a wide character `text-emphasis-skip: narrow`
//! marks, an upright one under `text-orientation: mixed`, and a character
//! that glues an atomic inline to its neighbour.
//!
//! Emoji_Modifier_Base, Emoji_Component and Indic_Conjunct_Break are
//! reserved for an in-house grapheme segmenter, written only if measurement
//! argues for one. They are filled now so that the table does
//! not change shape when it is. Until a stage reads them only the sweep test
//! does, so the constants of the three are compiled for the tests alone.
//! Line_Break = SA is read by word motion, which
//! segments a run of it whole, as ICU's word segmenter hands
//! such a run to its dictionary or model. The mark bit is read by font
//! coverage, which replays harfrust's normalizer against a font: a mark is
//! what that normalizer clusters with the character before it, and what it
//! composes onto a starter. The Emoji bit is read by font
//! selection, which applies `font-variant-emoji` to an Emoji character as
//! Chrome does (`Character::IsEmoji`), and to no other.
//!
//! # East Asian spacing
//!
//! What `text-autospace` reads: UTR #59's East_Asian_Spacing,
//! derived here as Chrome 153 derives it for its own table
//! (`character_property_data_generator.cc`, `SetEastAsianSpacing`), since
//! ICU carries no such property yet. Wide: Script Bopomofo, Han, Hangul,
//! Hiragana, Katakana, Khitan Small Script, Nushu, Tangut or Yi, or a
//! Script_Extensions naming one of them where East_Asian_Width is neither
//! Neutral nor Narrow; less what is East_Asian_Width Halfwidth, punctuation,
//! Other_Number, or a symbol other than a modifier symbol; plus U+3013.
//! Conditional: Other_Punctuation not Fullwidth, Halfwidth or Wide, less
//! eight listed characters. Narrow: letters, marks and decimal digits not
//! Fullwidth, Halfwidth or Wide, less what is Wide. Unassigned code points are
//! other. Two values are this crate's own: a Narrow decimal digit is kept
//! apart from a letter, for `ideograph-numeric` beside `ideograph-alpha`; and
//! every mark (General_Category M), which Chrome's autospace loop steps over
//! before it classifies anything, is 5 whatever the derivation says.
//!
//! # Text-spacing classes
//!
//! What `text-spacing-trim` reads: CSS Text 4's text spacing
//! classes as Blink numbers them (`HanKerningCharType`, set in
//! `SetHanKerning`): other 0, opening 1, closing 2, middle 3, a narrow opening
//! bracket 4, a narrow closing one 5, and the five whose class depends on the
//! font -- the dots 6, the colon 7, the semicolon 8, the opening quotes 9 and
//! the closing quotes 10. An opening or closing bracket (General_Category Ps
//! or Pe) is full width in the CJK Symbols and Punctuation block
//! (U+3000-303F) or where it is East_Asian_Width Fullwidth, and narrow
//! otherwise.
//!
//! # Emphasis classes
//!
//! What `text-emphasis-skip` reads: which kinds of
//! character a mark is not set over. 4, never marked: controls (Cc), format
//! characters (Cf), unassigned code points (Cn) and the line and paragraph
//! separators (Zl, Zp). 1, a space: the space separators (Zs) and the word
//! separators Chrome lists beside them (`Character::CanReceiveTextEmphasis`:
//! U+1361, U+10100, U+10101, U+1039F, U+0F0B, U+0F0C). 2, punctuation:
//! General_Category P, less the characters CSS Text Decoration 4 marks
//! anyway (`#`, `%`, `&`, `@`, `§`, `¶`, the per mille and per ten thousand
//! signs, the Arabic percent signs, U+204A, U+204B, U+2053, U+303D, and their
//! small and fullwidth forms), which Chrome lists too. 3, a symbol:
//! General_Category S. Everything else, 0, takes a mark. Skipping the
//! initial `spaces punctuation` is Chrome's rule exactly.
//!
//! # First-letter classes
//!
//! What `::first-letter` reads to find where its text ends,
//! CSS Pseudo-Elements 4 §2.2.1's categories as Blink's
//! `FirstLetterPseudoElement` tests them: 1, a space separator (Zs) other
//! than U+3000 IDEOGRAPHIC SPACE, which may come between the punctuation and
//! the letter; 2, opening punctuation (Ps) or a dash (Pd), which may come
//! before the letter only; 3, the rest of the punctuation (Pc, Pe, Pi, Pf,
//! Po), which may come on either side of it. Everything else is 0.
//!
//! # Numbering
//!
//! The numbers are ICU4C's, baked into the generated file, except where a
//! field is too narrow for them. ICU4X is removing the conversions that read
//! ICU4C's values, so each property is mapped here by a `match` on its named
//! values instead. The match is also the check that winkin's enums agree:
//! a value added to Unicode fails here, loudly, rather than being packed as
//! something else. Script values are read from ICU's conversion while it
//! exists; the generated file records the answer, so its removal costs a new
//! mapping, never correctness.
//!
//! Grapheme_Cluster_Break is the one field that departs. ICU4C numbers ZWJ 17,
//! after four values Unicode has assigned to no character since 11.0. Four
//! bits hold 0 to 15, so ZWJ is 13 here and the four are refused.
//!
//! # Case mapping
//!
//! What `text-transform` maps case by (`unicode/case.rs`), read from
//! icu_casemap: each character's full lowercase, uppercase and titlecase in
//! the root language, alone. The conditional mappings and the language
//! tailorings are code there, as ICU4C writes them.
//!
//! - `CASE_RUNS` holds every run of neighbours that map alike, binary
//!   searched: each the run's first character, its length less one and its
//!   kind. A run of lowercase letters maps up by one delta, a run of capitals
//!   down by one, and a run of pairs alternates capital and small letter.
//!   338 runs hold every mapped character of Unicode 17.
//! - `CASE_RUN_DATA` names each run's delta in `CASE_DELTAS`, or its first
//!   exception.
//! - An exception is a character one delta cannot map: a mapping longer than
//!   one character (`ß`, `ﬁ`, `ŉ`), or a titlecase that is not the uppercase
//!   (`ǆ`, `ǅ`). `CASE_EXCEPTIONS` spells its three mappings in
//!   `CASE_STRINGS`.
//! - Uppercase leaves Georgian Mkhedruli as it is, as Chrome's
//!   `DisableNewGeorgianCapitalLetters` does, so it needs no exception.
//! - `SOFT_DOTTED` is the Soft_Dotted ranges, which Lithuanian reads.
//! - `GREEK_LETTERS` is what ICU4C's `GreekUpper` reads of each character of
//!   U+0370 to U+03FF and U+1F00 to U+1FFF: the capital of its base letter,
//!   from its canonical decomposition, and whether it is a vowel and carries
//!   an accent, a dialytika or an iota subscript. A character that is not a
//!   Greek letter with Greek diacritics only is zero.
//!
//! Cased and Case_Ignorable, which the final sigma and Greek uppercasing read
//! of the characters beside them, are two bits of the rare word, which they
//! grow by 2.4 KB: Case_Ignorable's 400-odd ranges would take more.
//!
//! # The paragraph separator
//!
//! Line_Break BK, CR, LF or NL: LF, VT, FF, CR, NEL, U+2028 and U+2029. These
//! are UAX #14's mandatory breaks, so a forced line break and a bidi paragraph
//! end coincide. CRLF is one break, but that is segmentation's
//! business. U+001C to U+001E are bidi class B but not line breaks, and are
//! left out until a Chrome probe says otherwise.
//!
//! # Scripts
//!
//! The ten-bit field copies ICU's `ScriptWithExtensions` encoding, narrowed.
//! ICU spends two more bits saying whether the script is Common, Inherited or
//! another; here there are none to spend, so:
//!
//! - a value below `SCRIPT_COUNT` is a script, and the character's
//!   Script_Extensions is that script alone;
//! - a value at or above it names set `value - SCRIPT_COUNT`, whose run in
//!   `SCRIPT_RUNS` is the character's Script and then its Script_Extensions.
//!   ICU stores the script first in the same way for its "other" case; here
//!   every set does, Common and Inherited included.
//!
//! One set exists per distinct pair of Script and Script_Extensions.
//! `SCRIPT_RUNS` begins with every script alone, in value order, so that a
//! plain script's extensions are a one-element slice of it.
//!
//! # Brackets
//!
//! A pair id and an "opens" bit rather than the closing character: Unicode 17
//! has 64 bracket pairs, 63 once the equivalent ones below are merged, so
//! seven bits hold an id plus one with room to double, where a character would
//! need sixteen and a presence bit. `BRACKET_CLOSINGS` maps the id back.
//!
//! Rule N0 matches a closing bracket to an opener by Bidi_Paired_Bracket "or
//! its canonical equivalent" (UAX #9 BD16). The only brackets with canonical
//! decompositions are U+2329 and U+232A, to U+3008 and U+3009. So both pairs
//! get one id, whose closing character is U+3009, and N0 compares ids with no
//! special case.
//!
//! # The lookups
//!
//! packtab's three-lookup solution, rendered here rather than by packtab:
//! packtab reads its arrays with `[]`, and winkin allows no panic that a
//! caller's input could reach. Every read goes through `at`, which answers
//! zero past the end of its array. No index the lookups compute is past the
//! end; the sweep test visits every character to show it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use icu_properties::props::{
    BidiClass, BidiMirroringGlyph, BidiPairedBracketType, CaseIgnorable, Cased,
    DefaultIgnorableCodePoint, EastAsianWidth, Emoji, EmojiComponent, EmojiModifierBase,
    EmojiPresentation, ExtendedPictographic, GeneralCategory, GraphemeClusterBreak,
    IndicConjunctBreak, JoiningType, LineBreak, RegionalIndicator, Script, SoftDotted,
    VerticalOrientation, WhiteSpace,
};
use icu_properties::script::ScriptWithExtensions;
use icu_properties::{CodePointMapData, CodePointSetData, PropertyNamesShort};

/// Where the generated file goes, from this crate's manifest.
const OUT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/winkin/src/unicode/tables.rs"
);

/// A field of one of the two words.
struct Field {
    /// The constant's name in the generated file, and in errors.
    name: &'static str,
    shift: u32,
    bits: u32,
    /// Filled so that the table keeps its shape, and read by the sweep test
    /// alone until a stage asks for it: its constants are `#[cfg(test)]`.
    reserved: bool,
}

impl Field {
    const fn new(name: &'static str, shift: u32, bits: u32) -> Self {
        Self {
            name,
            shift,
            bits,
            reserved: false,
        }
    }

    /// A field no stage reads yet (the module's documentation says which).
    const fn reserved(name: &'static str, shift: u32, bits: u32) -> Self {
        Self {
            reserved: true,
            ..Self::new(name, shift, bits)
        }
    }

    /// Stores `value` in `word`, refusing a value too wide for the field.
    fn put(&self, word: &mut u32, value: u32) {
        assert!(
            value < 1 << self.bits,
            "{} = {value} does not fit in {} bits",
            self.name,
            self.bits
        );
        *word |= value << self.shift;
    }

    /// The constants the reader uses: a mask in place for a flag, a shift and
    /// a mask for anything wider; for the tests alone where it is reserved.
    fn emit(&self, code: &mut String) {
        let gate = if self.reserved { "#[cfg(test)]\n" } else { "" };
        if self.bits == 1 {
            writeln!(
                code,
                "{gate}pub(super) const {}: u32 = 1 << {};",
                self.name, self.shift
            )
            .unwrap();
        } else {
            writeln!(
                code,
                "{gate}pub(super) const {}_SHIFT: u32 = {};",
                self.name, self.shift
            )
            .unwrap();
            writeln!(
                code,
                "{gate}pub(super) const {}_MASK: u32 = 0x{:X};",
                self.name,
                (1u32 << self.bits) - 1
            )
            .unwrap();
        }
    }
}

// The core word.
const BIDI_CLASS: Field = Field::new("BIDI_CLASS", 0, 5);
const SCRIPT: Field = Field::new("SCRIPT", 5, 10);
const BRACKET_PAIR: Field = Field::new("BRACKET_PAIR", 15, 7);
const OPENS_BRACKET: Field = Field::new("OPENS_BRACKET", 22, 1);
const REGIONAL_INDICATOR: Field = Field::new("REGIONAL_INDICATOR", 23, 1);
const EXTENDED_PICTOGRAPHIC: Field = Field::new("EXTENDED_PICTOGRAPHIC", 24, 1);
const EMOJI_PRESENTATION: Field = Field::new("EMOJI_PRESENTATION", 25, 1);
const PARAGRAPH_SEPARATOR: Field = Field::new("PARAGRAPH_SEPARATOR", 26, 1);
const WHITE_SPACE: Field = Field::new("WHITE_SPACE", 27, 1);
const COMPLEX_CONTEXT: Field = Field::new("COMPLEX_CONTEXT", 28, 1);
const MARK: Field = Field::new("MARK", 29, 1);
const EMOJI: Field = Field::new("EMOJI", 30, 1);

const CORE: [&Field; 12] = [
    &BIDI_CLASS,
    &SCRIPT,
    &BRACKET_PAIR,
    &OPENS_BRACKET,
    &REGIONAL_INDICATOR,
    &EXTENDED_PICTOGRAPHIC,
    &EMOJI_PRESENTATION,
    &PARAGRAPH_SEPARATOR,
    &WHITE_SPACE,
    &COMPLEX_CONTEXT,
    &MARK,
    &EMOJI,
];

// The rare word.
const JOIN_CAUSING: Field = Field::new("JOIN_CAUSING", 0, 1);
const WIDE: Field = Field::new("WIDE", 1, 1);
const UPRIGHT_IN_MIXED: Field = Field::new("UPRIGHT_IN_MIXED", 2, 1);
const CONDITIONAL_JAPANESE_STARTER: Field = Field::new("CONDITIONAL_JAPANESE_STARTER", 3, 1);
const GLUE: Field = Field::new("GLUE", 4, 1);
const EMOJI_MODIFIER_BASE: Field = Field::reserved("EMOJI_MODIFIER_BASE", 5, 1);
const EMOJI_COMPONENT: Field = Field::reserved("EMOJI_COMPONENT", 6, 1);
const DEFAULT_IGNORABLE: Field = Field::new("DEFAULT_IGNORABLE", 7, 1);
const GRAPHEME_CLUSTER_BREAK: Field = Field::new("GRAPHEME_CLUSTER_BREAK", 8, 4);
const INDIC_CONJUNCT_BREAK: Field = Field::reserved("INDIC_CONJUNCT_BREAK", 12, 2);
const EAST_ASIAN_SPACING: Field = Field::new("EAST_ASIAN_SPACING", 14, 3);
const TEXT_SPACING: Field = Field::new("TEXT_SPACING", 17, 4);
const EMPHASIS: Field = Field::new("EMPHASIS", 21, 3);
const FIRST_LETTER: Field = Field::new("FIRST_LETTER", 24, 2);
const CASED: Field = Field::new("CASED", 26, 1);
const CASE_IGNORABLE: Field = Field::new("CASE_IGNORABLE", 27, 1);

const RARE: [&Field; 16] = [
    &JOIN_CAUSING,
    &WIDE,
    &UPRIGHT_IN_MIXED,
    &CONDITIONAL_JAPANESE_STARTER,
    &GLUE,
    &EMOJI_MODIFIER_BASE,
    &EMOJI_COMPONENT,
    &DEFAULT_IGNORABLE,
    &GRAPHEME_CLUSTER_BREAK,
    &INDIC_CONJUNCT_BREAK,
    &EAST_ASIAN_SPACING,
    &TEXT_SPACING,
    &EMPHASIS,
    &FIRST_LETTER,
    &CASED,
    &CASE_IGNORABLE,
];

// The kinds of case run, in a run's low bits.
/// Each character's lowercase is itself, and its uppercase and titlecase
/// are it plus the run's delta.
const RUN_LOWER: u32 = 0;
/// Each character's uppercase and titlecase are itself, and its lowercase
/// is it plus the run's delta.
const RUN_UPPER: u32 = 1;
/// Capital and small letters alternate, the capital first: each capital's
/// lowercase is the next character, and each small letter's uppercase and
/// titlecase the one before.
const RUN_PAIRS: u32 = 2;
/// Each character is an exception, in order from the run's first index.
const RUN_EXCEPTIONS: u32 = 3;
/// The bits of a run's kind, below its length.
const RUN_KIND_BITS: u32 = 3;
/// The bits of a run's length less one, below its start.
const RUN_LENGTH_BITS: u32 = 8;

// The Greek letter flags, above the ten bits of the capital.
const GREEK_UPPER_MASK: u32 = 0x3FF;
const GREEK_VOWEL: u32 = 1 << 10;
const GREEK_YPOGEGRAMMENI: u32 = 1 << 11;
const GREEK_ACCENT: u32 = 1 << 12;
const GREEK_DIALYTIKA: u32 = 1 << 13;

/// The Greek diacritics ICU's Greek uppercasing reads, and what each says
/// (`GreekUpper::getDiacriticData`): an accent, a dialytika, both, an iota
/// subscript, or another diacritic, which says nothing.
fn greek_diacritic(ch: char) -> Option<u32> {
    match ch {
        '\u{300}' | '\u{301}' | '\u{342}' | '\u{302}' | '\u{303}' | '\u{311}' => Some(GREEK_ACCENT),
        '\u{308}' => Some(GREEK_DIALYTIKA),
        '\u{344}' => Some(GREEK_DIALYTIKA | GREEK_ACCENT),
        '\u{345}' => Some(GREEK_YPOGEGRAMMENI),
        '\u{304}' | '\u{306}' | '\u{313}' | '\u{314}' | '\u{343}' => Some(0),
        _ => None,
    }
}

/// The Greek uppercasing entry of `cp`: its base letter's capital and the
/// flags its diacritics set, or zero for a character that is not a Greek
/// letter, possibly with Greek diacritics.
fn greek_letter(
    cp: u32,
    nfd: &icu_normalizer::DecomposingNormalizerBorrowed<'_>,
    categories: &icu_properties::CodePointMapDataBorrowed<'_, GeneralCategory>,
    case: &icu_casemap::CaseMapperBorrowed<'_>,
) -> u32 {
    let Some(ch) = char::from_u32(cp) else {
        return 0;
    };
    let decomposed: Vec<char> = nfd.normalize_iter([ch].into_iter()).collect();
    let (&base, marks) = decomposed
        .split_first()
        .expect("a decomposition is never empty");
    if !(0x370..=0x3FF).contains(&u32::from(base)) || !is_letter(categories.get(base)) {
        return 0;
    }
    let mut flags = 0;
    for &mark in marks {
        match greek_diacritic(mark) {
            Some(flag) => flags |= flag,
            None => return 0,
        }
    }
    let upper = u32::from(case.simple_uppercase(base));
    assert!(
        upper <= GREEK_UPPER_MASK,
        "U+{cp:04X}'s capital is past U+03FF"
    );
    if [0x391, 0x395, 0x397, 0x399, 0x39F, 0x3A5, 0x3A9].contains(&upper) {
        flags |= GREEK_VOWEL;
    }
    upper | flags
}

/// ICU4C's `UCharDirection` numbering, which is the bidi resolver's.
fn bidi_class(class: BidiClass) -> u32 {
    match class {
        BidiClass::LeftToRight => 0,
        BidiClass::RightToLeft => 1,
        BidiClass::EuropeanNumber => 2,
        BidiClass::EuropeanSeparator => 3,
        BidiClass::EuropeanTerminator => 4,
        BidiClass::ArabicNumber => 5,
        BidiClass::CommonSeparator => 6,
        BidiClass::ParagraphSeparator => 7,
        BidiClass::SegmentSeparator => 8,
        BidiClass::WhiteSpace => 9,
        BidiClass::OtherNeutral => 10,
        BidiClass::LeftToRightEmbedding => 11,
        BidiClass::LeftToRightOverride => 12,
        BidiClass::ArabicLetter => 13,
        BidiClass::RightToLeftEmbedding => 14,
        BidiClass::RightToLeftOverride => 15,
        BidiClass::PopDirectionalFormat => 16,
        BidiClass::NonspacingMark => 17,
        BidiClass::BoundaryNeutral => 18,
        BidiClass::FirstStrongIsolate => 19,
        BidiClass::LeftToRightIsolate => 20,
        BidiClass::RightToLeftIsolate => 21,
        BidiClass::PopDirectionalIsolate => 22,
        other => panic!("bidi class {other:?} is new; the bidi resolver needs a value for it"),
    }
}

/// Whether a character causes a join on both sides with no forms of its
/// own: Joining_Type Join_Causing, the tatweel and U+200D.
fn is_join_causing(joining: JoiningType) -> bool {
    match joining {
        JoiningType::JoinCausing => true,
        JoiningType::NonJoining
        | JoiningType::DualJoining
        | JoiningType::LeftJoining
        | JoiningType::RightJoining
        | JoiningType::Transparent => false,
        other => panic!("joining type {other:?} is new; decide whether it causes a join"),
    }
}

/// Whether a character is wide: East_Asian_Width Wide or Fullwidth.
fn is_wide(width: EastAsianWidth) -> bool {
    match width {
        EastAsianWidth::Wide | EastAsianWidth::Fullwidth => true,
        EastAsianWidth::Neutral
        | EastAsianWidth::Ambiguous
        | EastAsianWidth::Halfwidth
        | EastAsianWidth::Narrow => false,
        other => panic!("East_Asian_Width {other:?} is new; decide whether it is wide"),
    }
}

/// Whether a character stands upright in a vertical line under
/// `text-orientation: mixed`: Vertical_Orientation U, Tu or Tr, as Blink's
/// `Character::IsUprightInMixedVertical` takes them.
fn is_mixed_upright(orientation: VerticalOrientation) -> bool {
    match orientation {
        VerticalOrientation::Rotated => false,
        VerticalOrientation::TransformedRotated
        | VerticalOrientation::TransformedUpright
        | VerticalOrientation::Upright => true,
        other => panic!("Vertical_Orientation {other:?} is new; decide whether it stands up"),
    }
}

/// Whether a character glues its neighbours together, which the object-edge
/// rule reads: Line_Break GL, WJ or ZWJ.
fn is_glue(line_break: LineBreak) -> bool {
    matches!(
        line_break,
        LineBreak::Glue | LineBreak::WordJoiner | LineBreak::ZWJ
    )
}

/// ICU4C's numbering up to 12, and ZWJ at 13 rather than 17.
fn grapheme_cluster_break(class: GraphemeClusterBreak) -> u32 {
    match class {
        GraphemeClusterBreak::Other => 0,
        GraphemeClusterBreak::Control => 1,
        GraphemeClusterBreak::CR => 2,
        GraphemeClusterBreak::Extend => 3,
        GraphemeClusterBreak::L => 4,
        GraphemeClusterBreak::LF => 5,
        GraphemeClusterBreak::LV => 6,
        GraphemeClusterBreak::LVT => 7,
        GraphemeClusterBreak::T => 8,
        GraphemeClusterBreak::V => 9,
        GraphemeClusterBreak::SpacingMark => 10,
        GraphemeClusterBreak::Prepend => 11,
        GraphemeClusterBreak::RegionalIndicator => 12,
        GraphemeClusterBreak::ZWJ => 13,
        other => panic!(
            "Grapheme_Cluster_Break {other:?} is assigned; four bits only hold it if an \
             unused value makes room"
        ),
    }
}

/// ICU4C's numbering, which is `winkin::unicode::IndicConjunctBreak`'s.
fn indic_conjunct_break(class: IndicConjunctBreak) -> u32 {
    match class {
        IndicConjunctBreak::None => 0,
        IndicConjunctBreak::Consonant => 1,
        IndicConjunctBreak::Extend => 2,
        IndicConjunctBreak::Linker => 3,
        other => panic!("Indic_Conjunct_Break {other:?} is new"),
    }
}

/// The scripts UTR #59 calls ideographic for East_Asian_Spacing.
const WIDE_SCRIPTS: [Script; 9] = [
    Script::Bopomofo,
    Script::Han,
    Script::Hangul,
    Script::Hiragana,
    Script::Katakana,
    Script::KhitanSmallScript,
    Script::Nushu,
    Script::Tangut,
    Script::Yi,
];

/// Whether `gc` is a mark: General_Category Mn, Mc or Me.
fn is_mark(gc: GeneralCategory) -> bool {
    matches!(
        gc,
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    )
}

/// Whether `gc` is a letter.
fn is_letter(gc: GeneralCategory) -> bool {
    matches!(
        gc,
        GeneralCategory::UppercaseLetter
            | GeneralCategory::LowercaseLetter
            | GeneralCategory::TitlecaseLetter
            | GeneralCategory::ModifierLetter
            | GeneralCategory::OtherLetter
    )
}

/// Whether `gc` is punctuation.
fn is_punctuation(gc: GeneralCategory) -> bool {
    matches!(
        gc,
        GeneralCategory::ConnectorPunctuation
            | GeneralCategory::DashPunctuation
            | GeneralCategory::OpenPunctuation
            | GeneralCategory::ClosePunctuation
            | GeneralCategory::InitialPunctuation
            | GeneralCategory::FinalPunctuation
            | GeneralCategory::OtherPunctuation
    )
}

/// The East Asian spacing value of `cp` (see the module documentation): 0
/// other, 1 narrow, 2 a narrow decimal digit, 3 conditional, 4 wide, 5 a
/// mark. `in_scripts` says whether its Script_Extensions names a script.
fn east_asian_spacing(
    cp: u32,
    gc: GeneralCategory,
    width: EastAsianWidth,
    script: Script,
    in_scripts: impl Fn(Script) -> bool,
) -> u32 {
    if is_mark(gc) {
        return 5;
    }
    if gc == GeneralCategory::Unassigned {
        return 0;
    }
    let wide_width = matches!(
        width,
        EastAsianWidth::Fullwidth | EastAsianWidth::Halfwidth | EastAsianWidth::Wide
    );
    let mut wide = WIDE_SCRIPTS.contains(&script)
        || (WIDE_SCRIPTS.iter().any(|&wide| in_scripts(wide))
            && !matches!(width, EastAsianWidth::Neutral | EastAsianWidth::Narrow));
    wide &= width != EastAsianWidth::Halfwidth
        && !is_punctuation(gc)
        && gc != GeneralCategory::OtherNumber
        && !matches!(
            gc,
            GeneralCategory::MathSymbol
                | GeneralCategory::CurrencySymbol
                | GeneralCategory::OtherSymbol
        );
    if wide || cp == 0x3013 {
        return 4;
    }
    let listed = [0x22, 0x27, 0x2A, 0x2F, 0xB7, 0x2020, 0x2021, 0x2026];
    if gc == GeneralCategory::OtherPunctuation && !wide_width && !listed.contains(&cp) {
        return 3;
    }
    if !wide_width && gc == GeneralCategory::DecimalNumber {
        return 2;
    }
    if !wide_width && is_letter(gc) {
        return 1;
    }
    0
}

/// The text-spacing class of `cp` (see the module documentation), Blink's
/// `HanKerningCharType` numbering.
fn text_spacing(cp: u32, gc: GeneralCategory, width: EastAsianWidth) -> u32 {
    match cp {
        0x2018 | 0x201C => return 9,
        0x2019 | 0x201D => return 10,
        0x3000 | 0x00B7 | 0x2027 | 0x30FB => return 3,
        0x3001 | 0x3002 | 0xFF0C | 0xFF0E => return 6,
        0xFF1A => return 7,
        0xFF1B => return 8,
        _ => {}
    }
    let full = (0x3000..=0x303F).contains(&cp) || width == EastAsianWidth::Fullwidth;
    match gc {
        GeneralCategory::OpenPunctuation if full => 1,
        GeneralCategory::ClosePunctuation if full => 2,
        GeneralCategory::OpenPunctuation => 4,
        GeneralCategory::ClosePunctuation => 5,
        _ => 0,
    }
}

/// The emphasis class of `cp` (see the module documentation): 0 marked, 1 a
/// space, 2 punctuation, 3 a symbol, 4 never marked. In Chrome's order
/// (`Character::CanReceiveTextEmphasis`): the categories never marked, the
/// word separators, the punctuation CSS marks anyway, and then the rest of
/// the punctuation.
fn emphasis(cp: u32, gc: GeneralCategory) -> u32 {
    match gc {
        GeneralCategory::Control
        | GeneralCategory::Format
        | GeneralCategory::Unassigned
        | GeneralCategory::LineSeparator
        | GeneralCategory::ParagraphSeparator => return 4,
        GeneralCategory::SpaceSeparator => return 1,
        _ => {}
    }
    if matches!(cp, 0x1361 | 0x10100 | 0x10101 | 0x1039F | 0x0F0B | 0x0F0C) {
        return 1;
    }
    let marked_anyway = [
        0x23, 0x25, 0x26, 0x40, 0xA7, 0xB6, 0x609, 0x60A, 0x66A, 0x2030, 0x2031, 0x204A, 0x204B,
        0x2053, 0x303D, 0xFE5F, 0xFE60, 0xFE6A, 0xFE6B, 0xFF03, 0xFF05, 0xFF06, 0xFF20,
    ];
    if marked_anyway.contains(&cp) {
        return 0;
    }
    if is_punctuation(gc) {
        return 2;
    }
    if matches!(
        gc,
        GeneralCategory::MathSymbol
            | GeneralCategory::CurrencySymbol
            | GeneralCategory::ModifierSymbol
            | GeneralCategory::OtherSymbol
    ) {
        return 3;
    }
    0
}

/// The first-letter class of `cp` (see the module documentation): 0 other,
/// 1 a space separator other than U+3000, 2 opening punctuation or a dash, 3
/// other punctuation.
fn first_letter(cp: u32, gc: GeneralCategory) -> u32 {
    match gc {
        GeneralCategory::SpaceSeparator if cp != 0x3000 => 1,
        GeneralCategory::OpenPunctuation | GeneralCategory::DashPunctuation => 2,
        GeneralCategory::ConnectorPunctuation
        | GeneralCategory::ClosePunctuation
        | GeneralCategory::InitialPunctuation
        | GeneralCategory::FinalPunctuation
        | GeneralCategory::OtherPunctuation => 3,
        _ => 0,
    }
}

/// Whether a character forces a line break, and so ends a bidi paragraph.
fn is_paragraph_separator(line_break: LineBreak) -> bool {
    matches!(
        line_break,
        LineBreak::MandatoryBreak
            | LineBreak::CarriageReturn
            | LineBreak::LineFeed
            | LineBreak::NextLine
    )
}

/// ICU4C's script value.
#[allow(deprecated, reason = "the values are stable; baking them is the point")]
fn script_value(script: Script) -> u16 {
    script.to_icu4c_value()
}

/// The script ICU4C numbers `value`.
#[allow(deprecated, reason = "the values are stable; baking them is the point")]
fn script_from_value(value: u16) -> Script {
    Script::from_icu4c_value(value)
}

/// The one member of a canonical equivalence class of brackets that the table
/// keeps (UAX #9 BD16).
fn canonical_bracket(ch: char) -> char {
    match ch {
        '\u{2329}' => '\u{3008}',
        '\u{232A}' => '\u{3009}',
        other => other,
    }
}

/// A character's bracket pair, named by its canonical closing character, and
/// whether it opens the pair.
fn paired_bracket(cp: u32, mirroring: BidiMirroringGlyph) -> Option<(char, bool)> {
    let ch = char::from_u32(cp)?;
    match mirroring.paired_bracket_type {
        BidiPairedBracketType::Open => {
            let closing = mirroring
                .mirroring_glyph
                .unwrap_or_else(|| panic!("U+{cp:04X} opens a pair with no closing bracket"));
            Some((canonical_bracket(closing), true))
        }
        BidiPairedBracketType::Close => Some((canonical_bracket(ch), false)),
        _ => None,
    }
}

/// What a code point's script field holds, before the number of scripts is
/// known and a set's value can be worked out.
enum ScriptField {
    /// A script, which is the whole of the character's Script_Extensions.
    Script(u16),
    /// A Script_Extensions set, by id.
    Set(u16),
}

/// A packed lookup, rendered.
struct Lookup {
    code: String,
    bytes: usize,
    shape: String,
}

/// Packs `values` into packtab's three-lookup solution and renders it as
/// `fn {name}(ch: char) -> u32`, reading every array through `at`.
fn pack(name: &str, values: &[i64]) -> Lookup {
    let info = packtab::pack_table_all(values, Some(0));
    let mut three = info
        .solutions
        .iter()
        .filter(|solution| solution.n_lookups() == 3);
    let solution = three
        .next()
        .unwrap_or_else(|| panic!("packtab found no three-lookup solution for {name}"));
    assert!(
        three.next().is_none(),
        "packtab found two three-lookup solutions for {name}; choose between them"
    );
    assert!(
        !solution.is_palette(),
        "{name}: packtab chose a palette, which this rendering has not been checked with"
    );
    let language = packtab::codegen::Language::Rust {
        unsafe_access: false,
    };
    let ir = packtab::codegen::generate(solution, &info, name, language);
    assert!(
        ir.accessors.is_empty(),
        "{name}: sub-byte accessors are not rendered here"
    );
    let [function] = ir.functions.as_slice() else {
        panic!("{name}: expected one lookup function");
    };
    assert_eq!(function.arg_name, "u", "{name}: unexpected argument name");

    let arrays: Vec<(String, String)> = ir
        .arrays
        .iter()
        .map(|array| (array.name.clone(), array.name.to_uppercase()))
        .collect();
    let body = guard_reads(&function.body, &arrays);
    assert!(
        !body.contains('[') && !body.contains(']') && !body.contains(".."),
        "{name}: a read escaped `at`: {body}"
    );

    let mut code = String::new();
    let mut bytes = 0;
    for array in &ir.arrays {
        let ty = array.typ.abbr();
        assert!(
            array.typ.is_unsigned(),
            "{name}: signed arrays are not expected"
        );
        bytes += array.values.len() * usize::from(array.typ.bits() / 8);
        writeln!(
            code,
            "static {}: [{ty}; {}] = [",
            array.name.to_uppercase(),
            array.values.len()
        )
        .unwrap();
        for value in &array.values {
            write!(code, "{value},").unwrap();
        }
        code.push_str("];\n\n");
    }

    let comment = function.comment.as_deref().unwrap_or_default();
    let shape = comment.strip_prefix("packtab: ").unwrap_or(comment);
    writeln!(
        code,
        "/// The {name} word of `ch`, from packtab's {shape} layout: three reads.\n\
         #[allow(\n\
             clippy::unnecessary_cast,\n\
             reason = \"packtab casts every index to usize, whatever its type\"\n\
         )]\n\
         #[inline]\n\
         pub(super) fn {name}_word(ch: char) -> u32 {{\n\
             let u = ch as usize;\n\
             {body}\n\
         }}\n"
    )
    .unwrap();

    Lookup {
        code,
        bytes,
        shape: shape.to_owned(),
    }
}

/// Renders `values` as `pub(super) static {name}: [{ty}; _]`, with `doc`.
fn emit_array<T: std::fmt::Display>(
    code: &mut String,
    doc: &str,
    name: &str,
    ty: &str,
    values: &[T],
) {
    writeln!(
        code,
        "\n/// {doc}\npub(super) static {name}: [{ty}; {}] = [",
        values.len()
    )
    .unwrap();
    for value in values {
        write!(code, "{value},").unwrap();
    }
    code.push_str("];\n");
}

/// Rewrites every `array[index]` in packtab's expression as
/// `at(&ARRAY, index)`, and refuses any other use of an array.
fn guard_reads(body: &str, arrays: &[(String, String)]) -> String {
    let chars: Vec<char> = body.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let Some((_, upper)) = arrays.iter().find(|(name, _)| *name == word) else {
                out.push_str(&word);
                continue;
            };
            assert_eq!(chars.get(i), Some(&'['), "{word} is used other than read");
            let open = i;
            let mut depth = 0;
            let close = (open..chars.len())
                .find(|&j| {
                    match chars[j] {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {}
                    }
                    depth == 0
                })
                .unwrap_or_else(|| panic!("unbalanced brackets in {body}"));
            let index: String = chars[open + 1..close].iter().collect();
            write!(out, "at(&{upper}, {})", guard_reads(&index, arrays)).unwrap();
            i = close + 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

const HEADER: &str = "\
//! winkin's Unicode character properties, packed: two words per code point
//! and the side tables they name.
//!
//! Generated from icu_properties 2.3 (Unicode 17.0) by
//! `cargo run --release -p unicode_gen`, then `cargo fmt`. Do not edit: change
//! the generator and run it again. The generator's documentation
//! (`support/unicode_gen/src/main.rs`) says what each field holds and why, and
//! `super` reads them.

use super::ScriptId;
";

const AT: &str = "\
/// `table[i]`, or zero past the end of `table`.
///
/// No index a lookup computes is past the end: the sweep test visits every
/// character. The zero is there so that no input can reach a panic.
#[inline(always)]
fn at<T: Copy + Default>(table: &[T], i: usize) -> T {
    match table.get(i) {
        Some(value) => *value,
        None => T::default(),
    }
}
";

fn main() {
    let bidi = CodePointMapData::<BidiClass>::new();
    let line_break = CodePointMapData::<LineBreak>::new();
    let mirroring = CodePointMapData::<BidiMirroringGlyph>::new();
    let joining = CodePointMapData::<JoiningType>::new();
    let width = CodePointMapData::<EastAsianWidth>::new();
    let orientation = CodePointMapData::<VerticalOrientation>::new();
    let grapheme = CodePointMapData::<GraphemeClusterBreak>::new();
    let conjunct = CodePointMapData::<IndicConjunctBreak>::new();
    let regional = CodePointSetData::new::<RegionalIndicator>();
    let pictographic = CodePointSetData::new::<ExtendedPictographic>();
    let presentation = CodePointSetData::new::<EmojiPresentation>();
    let emoji = CodePointSetData::new::<Emoji>();
    let white_space = CodePointSetData::new::<WhiteSpace>();
    let modifier_base = CodePointSetData::new::<EmojiModifierBase>();
    let component = CodePointSetData::new::<EmojiComponent>();
    let ignorable = CodePointSetData::new::<DefaultIgnorableCodePoint>();
    let cased = CodePointSetData::new::<Cased>();
    let case_ignorable = CodePointSetData::new::<CaseIgnorable>();
    let soft_dotted = CodePointSetData::new::<SoftDotted>();
    let scripts = ScriptWithExtensions::new();
    let script_names = PropertyNamesShort::<Script>::new();
    let categories = CodePointMapData::<GeneralCategory>::new();
    let script_map = CodePointMapData::<Script>::new();

    // --- scripts ---------------------------------------------------------
    // Every code point's Script, and its Script_Extensions where they are more
    // than the script alone. A set is named by the pair, in the order the
    // sweep first meets it.
    let mut script_count = 0;
    let mut sets: BTreeMap<Vec<u16>, u16> = BTreeMap::new();
    let mut set_runs: Vec<Vec<u16>> = Vec::new();
    let mut script_fields = Vec::with_capacity(0x110000);
    for cp in 0..=0x10FFFF_u32 {
        let script = script_value(scripts.get_script_val32(cp));
        let extensions: Vec<u16> = scripts
            .get_script_extensions_val32(cp)
            .iter()
            .map(script_value)
            .collect();
        for value in extensions.iter().chain([&script]) {
            script_count = script_count.max(value + 1);
        }
        if extensions == [script] {
            script_fields.push(ScriptField::Script(script));
        } else {
            let mut run = vec![script];
            run.extend(&extensions);
            let next = u16::try_from(set_runs.len()).expect("fewer than 2^16 sets");
            let id = *sets.entry(run.clone()).or_insert_with(|| {
                set_runs.push(run);
                next
            });
            script_fields.push(ScriptField::Set(id));
        }
    }
    let set_count = u16::try_from(set_runs.len()).expect("fewer than 2^16 sets");
    assert!(
        u32::from(script_count) + u32::from(set_count) <= 1 << SCRIPT.bits,
        "{script_count} scripts and {set_count} sets do not fit in {} bits",
        SCRIPT.bits
    );

    let script_tags: Vec<[u8; 4]> = (0..script_count)
        .map(|value| {
            let script = script_from_value(value);
            match script_names.get(script) {
                Some(name) => name
                    .as_bytes()
                    .try_into()
                    .unwrap_or_else(|_| panic!("script {value} is named {name:?}, not a tag")),
                None => *b"Zzzz",
            }
        })
        .collect();

    // --- brackets --------------------------------------------------------
    // One id per canonical closing character, in code point order.
    let mut closings: BTreeMap<char, u32> = BTreeMap::new();
    for cp in 0..=0x10FFFF_u32 {
        if let Some((closing, _)) = paired_bracket(cp, mirroring.get32(cp)) {
            closings.insert(closing, 0);
        }
    }
    for (id, slot) in (0..).zip(closings.values_mut()) {
        *slot = id;
    }
    let pair_count = closings.len();

    // --- the words -------------------------------------------------------
    let mut core = Vec::with_capacity(0x110000);
    let mut rare = Vec::with_capacity(0x110000);
    for (cp, script_field) in (0..=0x10FFFF_u32).zip(&script_fields) {
        let lb = line_break.get32(cp);

        let mut word = 0;
        BIDI_CLASS.put(&mut word, bidi_class(bidi.get32(cp)));
        let script = match *script_field {
            ScriptField::Script(script) => u32::from(script),
            ScriptField::Set(set) => u32::from(script_count) + u32::from(set),
        };
        SCRIPT.put(&mut word, script);
        if let Some((closing, opens)) = paired_bracket(cp, mirroring.get32(cp)) {
            BRACKET_PAIR.put(&mut word, closings[&closing] + 1);
            OPENS_BRACKET.put(&mut word, u32::from(opens));
        }
        REGIONAL_INDICATOR.put(&mut word, u32::from(regional.contains32(cp)));
        EXTENDED_PICTOGRAPHIC.put(&mut word, u32::from(pictographic.contains32(cp)));
        EMOJI_PRESENTATION.put(&mut word, u32::from(presentation.contains32(cp)));
        PARAGRAPH_SEPARATOR.put(&mut word, u32::from(is_paragraph_separator(lb)));
        WHITE_SPACE.put(&mut word, u32::from(white_space.contains32(cp)));
        COMPLEX_CONTEXT.put(&mut word, u32::from(lb == LineBreak::ComplexContext));
        MARK.put(&mut word, u32::from(is_mark(categories.get32(cp))));
        EMOJI.put(&mut word, u32::from(emoji.contains32(cp)));
        core.push(i64::from(word));

        let mut word = 0;
        JOIN_CAUSING.put(&mut word, u32::from(is_join_causing(joining.get32(cp))));
        WIDE.put(&mut word, u32::from(is_wide(width.get32(cp))));
        UPRIGHT_IN_MIXED.put(
            &mut word,
            u32::from(is_mixed_upright(orientation.get32(cp))),
        );
        CONDITIONAL_JAPANESE_STARTER.put(
            &mut word,
            u32::from(lb == LineBreak::ConditionalJapaneseStarter),
        );
        GLUE.put(&mut word, u32::from(is_glue(lb)));
        EMOJI_MODIFIER_BASE.put(&mut word, u32::from(modifier_base.contains32(cp)));
        EMOJI_COMPONENT.put(&mut word, u32::from(component.contains32(cp)));
        DEFAULT_IGNORABLE.put(&mut word, u32::from(ignorable.contains32(cp)));
        GRAPHEME_CLUSTER_BREAK.put(&mut word, grapheme_cluster_break(grapheme.get32(cp)));
        INDIC_CONJUNCT_BREAK.put(&mut word, indic_conjunct_break(conjunct.get32(cp)));
        let gc = categories.get32(cp);
        let eaw = width.get32(cp);
        EAST_ASIAN_SPACING.put(
            &mut word,
            east_asian_spacing(cp, gc, eaw, script_map.get32(cp), |wanted| {
                scripts.has_script32(cp, wanted)
            }),
        );
        TEXT_SPACING.put(&mut word, text_spacing(cp, gc, eaw));
        EMPHASIS.put(&mut word, emphasis(cp, gc));
        FIRST_LETTER.put(&mut word, first_letter(cp, gc));
        CASED.put(&mut word, u32::from(cased.contains32(cp)));
        CASE_IGNORABLE.put(&mut word, u32::from(case_ignorable.contains32(cp)));
        rare.push(i64::from(word));
    }

    // --- case --------------------------------------------------------------
    // Every code point's full mappings in the root language, one character
    // alone. Uppercase leaves Georgian Mkhedruli as it is, as Chrome does
    // (`DisableNewGeorgianCapitalLetters`). A character whose mappings one
    // delta cannot say, as where one is more than one character or its
    // titlecase is not its uppercase, is an exception: its three mappings are
    // spelled in `CASE_STRINGS`. Neighbours that map alike are a run.
    let case = icu_casemap::CaseMapper::new();
    let root = icu_locale_core::LanguageIdentifier::UNKNOWN;
    let mut title_options = icu_casemap::options::TitlecaseOptions::default();
    title_options.leading_adjustment = Some(icu_casemap::options::LeadingAdjustment::None);
    title_options.trailing_case = Some(icu_casemap::options::TrailingCase::Unchanged);
    let mut exceptions: Vec<[u16; 3]> = Vec::new();
    let mut strings: Vec<u16> = Vec::new();
    // Appends `mapping` to the strings, or finds it there, and returns its
    // offset and length.
    let mut spell = |mapping: &str| -> u16 {
        let units: Vec<u16> = mapping
            .chars()
            .map(|ch| u16::try_from(u32::from(ch)).expect("an exception's mapping is in the BMP"))
            .collect();
        assert!(
            (1..=3).contains(&units.len()),
            "{mapping:?} is not one to three characters"
        );
        let at = strings
            .windows(units.len())
            .position(|window| window == units)
            .unwrap_or_else(|| {
                strings.extend(&units);
                strings.len() - units.len()
            });
        let at = u16::try_from(at).expect("the strings fit in u16 offsets");
        assert!(at < 1 << 14, "the strings outgrow 14 bits of offset");
        at << 2 | u16::try_from(units.len()).expect("three fit")
    };
    // Each mapped character's run kind and delta, or exception index.
    let mut mapped: Vec<(u32, u32, i64)> = Vec::new();
    for ch in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
        let cp = u32::from(ch);
        let one = ch.to_string();
        let lower = case.lowercase_to_string(&one, &root).into_owned();
        let mut upper = case.uppercase_to_string(&one, &root).into_owned();
        let title = case
            .titlecase_segment_with_only_case_data_to_string(&one, &root, title_options)
            .into_owned();
        if lower == one
            && upper
                .chars()
                .all(|ch| ('\u{1C90}'..='\u{1CBF}').contains(&ch))
        {
            upper.clone_from(&one);
        }
        if lower == one && upper == one && title == one {
            continue;
        }
        let single = |mapping: &str| {
            let mut chars = mapping.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Some(i64::from(u32::from(ch)) - i64::from(cp)),
                _ => None,
            }
        };
        let run = if lower == one && title == upper {
            single(&upper).map(|delta| (RUN_LOWER, delta))
        } else if upper == one && title == one {
            single(&lower).map(|delta| (RUN_UPPER, delta))
        } else {
            None
        };
        mapped.push(match run {
            Some((kind, delta)) => (cp, kind, delta),
            None => {
                assert!(cp < 0x10000, "U+{cp:04X} is an exception past the BMP");
                let index = exceptions.len();
                exceptions.push([spell(&lower), spell(&upper), spell(&title)]);
                (
                    cp,
                    RUN_EXCEPTIONS,
                    i64::try_from(index).expect("few exceptions"),
                )
            }
        });
    }
    // Runs as (start, length, kind, delta or first exception).
    let mut runs: Vec<(u32, u32, u32, i64)> = Vec::new();
    let mut at = 0;
    while at < mapped.len() {
        let (cp, kind, value) = mapped[at];
        // Whether a capital and its small letter start at `i`, in step with
        // the run from `at`.
        let pair = |i: usize| {
            let first = cp + u32::try_from(i - at).expect("short runs");
            mapped.get(i) == Some(&(first, RUN_UPPER, 1))
                && mapped.get(i + 1) == Some(&(first + 1, RUN_LOWER, -1))
        };
        if pair(at) {
            let mut end = at + 2;
            while end - at + 2 <= 1 << RUN_LENGTH_BITS && pair(end) {
                end += 2;
            }
            let length = u32::try_from(end - at).expect("short runs");
            runs.push((cp, length, RUN_PAIRS, 0));
            at = end;
            continue;
        }
        let mut end = at + 1;
        while end - at < 1 << RUN_LENGTH_BITS {
            let Some(&(next, next_kind, next_value)) = mapped.get(end) else {
                break;
            };
            let offset = u32::try_from(end - at).expect("short runs");
            let follows = next == cp + offset
                && next_kind == kind
                && if kind == RUN_EXCEPTIONS {
                    next_value == value + i64::from(offset)
                } else {
                    next_value == value
                };
            if !follows {
                break;
            }
            end += 1;
        }
        let length = u32::try_from(end - at).expect("short runs");
        runs.push((cp, length, kind, value));
        at = end;
    }
    let mut deltas: Vec<i32> = Vec::new();
    let mut run_words: Vec<u32> = Vec::new();
    let mut run_data: Vec<u8> = Vec::new();
    for &(start, length, kind, value) in &runs {
        run_words.push(
            start << (RUN_LENGTH_BITS + RUN_KIND_BITS) | (length - 1) << RUN_KIND_BITS | kind,
        );
        let data = match kind {
            RUN_LOWER | RUN_UPPER => {
                let delta = i32::try_from(value).expect("a delta fits in i32");
                let index = deltas.iter().position(|&d| d == delta).unwrap_or_else(|| {
                    deltas.push(delta);
                    deltas.len() - 1
                });
                u8::try_from(index).expect("fewer than 256 deltas")
            }
            RUN_EXCEPTIONS => u8::try_from(value).expect("fewer than 256 exceptions"),
            _ => 0,
        };
        run_data.push(data);
    }
    // Soft_Dotted, each range its start shifted left by eight over its
    // length less one.
    let soft_dotted_runs: Vec<u32> = soft_dotted
        .iter_ranges()
        .map(|range| {
            let (start, end) = (*range.start(), *range.end());
            assert!(end - start < 1 << 8, "a Soft_Dotted range is long");
            start << 8 | (end - start)
        })
        .collect();
    let nfd = icu_normalizer::DecomposingNormalizerBorrowed::new_nfd();
    let greek: Vec<u32> = (0x370..=0x3FF_u32)
        .chain(0x1F00..=0x1FFF)
        .map(|cp| greek_letter(cp, &nfd, &categories, &case))
        .collect();

    // --- emit ------------------------------------------------------------
    let mut code = String::from(HEADER);

    code.push_str("\n// The core word, read for every character.\n");
    for field in CORE {
        field.emit(&mut code);
    }
    code.push_str("\n// The rare word, read per cluster or behind a gate.\n");
    for field in RARE {
        field.emit(&mut code);
    }

    writeln!(
        code,
        "\n/// The number of script values. A script field below this is a script; at\n\
         /// or above it, it names a Script_Extensions set.\n\
         pub(super) const SCRIPT_COUNT: u16 = {script_count};"
    )
    .unwrap();
    for (name, script) in [
        ("SCRIPT_COMMON", Script::Common),
        ("SCRIPT_INHERITED", Script::Inherited),
        ("SCRIPT_UNKNOWN", Script::Unknown),
        ("SCRIPT_HAN", Script::Han),
        ("SCRIPT_HIRAGANA", Script::Hiragana),
        ("SCRIPT_KATAKANA", Script::Katakana),
    ] {
        writeln!(
            code,
            "pub(super) const {name}: u16 = {};",
            script_value(script)
        )
        .unwrap();
    }

    writeln!(
        code,
        "\n/// The ISO 15924 tag of each script, by value. A value no character has\n\
         /// keeps ICU's name for it, or `Zzzz` where ICU has none.\n\
         pub(super) static SCRIPT_TAGS: [[u8; 4]; {script_count}] = ["
    )
    .unwrap();
    for tag in &script_tags {
        let tag = std::str::from_utf8(tag).expect("tags are ASCII");
        write!(code, "*b\"{tag}\",").unwrap();
    }
    code.push_str("];\n");

    let mut starts = Vec::with_capacity(set_runs.len() + 1);
    let mut list: Vec<u16> = (0..script_count).collect();
    for run in &set_runs {
        starts.push(u16::try_from(list.len()).expect("runs fit in u16 offsets"));
        list.extend(run);
    }
    starts.push(u16::try_from(list.len()).expect("runs fit in u16 offsets"));
    writeln!(
        code,
        "\n/// Where each Script_Extensions set's run begins in [`SCRIPT_RUNS`], by set\n\
         /// id, and last, where the last run ends.\n\
         pub(super) static SCRIPT_SET_STARTS: [u16; {}] = [",
        starts.len()
    )
    .unwrap();
    for start in &starts {
        write!(code, "{start},").unwrap();
    }
    code.push_str("];\n");
    writeln!(
        code,
        "\n/// Every script alone, in value order, and then each set's run: the\n\
         /// character's Script, then its Script_Extensions in ICU's order.\n\
         pub(super) static SCRIPT_RUNS: [ScriptId; {}] = [",
        list.len()
    )
    .unwrap();
    for script in &list {
        write!(code, "ScriptId({script}),").unwrap();
    }
    code.push_str("];\n");

    writeln!(
        code,
        "\n/// The closing bracket of each bracket pair, by pair id. U+2329 and U+232A\n\
         /// share U+3009's pair, being canonically equivalent to U+3008 and U+3009.\n\
         pub(super) static BRACKET_CLOSINGS: [char; {pair_count}] = ["
    )
    .unwrap();
    for closing in closings.keys() {
        write!(code, "'\\u{{{:04X}}}',", u32::from(*closing)).unwrap();
    }
    code.push_str("];\n\n");

    writeln!(
        code,
        "// The case runs: each its first character shifted left by {}, over its\n\
         // length less one shifted left by {}, over its kind.\n\
         pub(super) const RUN_LENGTH_SHIFT: u32 = {RUN_KIND_BITS};\n\
         pub(super) const RUN_START_SHIFT: u32 = {};\n\
         pub(super) const RUN_KIND_MASK: u32 = {};\n\
         pub(super) const RUN_LENGTH_MASK: u32 = {};\n\
         pub(super) const RUN_LOWER: u32 = {RUN_LOWER};\n\
         pub(super) const RUN_UPPER: u32 = {RUN_UPPER};\n\
         pub(super) const RUN_PAIRS: u32 = {RUN_PAIRS};\n\
         pub(super) const RUN_EXCEPTIONS: u32 = {RUN_EXCEPTIONS};",
        RUN_LENGTH_BITS + RUN_KIND_BITS,
        RUN_KIND_BITS,
        RUN_LENGTH_BITS + RUN_KIND_BITS,
        (1 << RUN_KIND_BITS) - 1,
        (1 << RUN_LENGTH_BITS) - 1,
    )
    .unwrap();
    emit_array(
        &mut code,
        "Every run of characters that map alike, in code point order.",
        "CASE_RUNS",
        "u32",
        &run_words,
    );
    emit_array(
        &mut code,
        "Each run's delta, as an index into [`CASE_DELTAS`], or its first\n\
         /// exception, as an index into [`CASE_EXCEPTIONS`].",
        "CASE_RUN_DATA",
        "u8",
        &run_data,
    );
    emit_array(
        &mut code,
        "The deltas the runs add to a character to map it.",
        "CASE_DELTAS",
        "i32",
        &deltas,
    );
    let exception_words: Vec<u16> = exceptions.iter().flatten().copied().collect();
    emit_array(
        &mut code,
        "Each exception's full lowercase, uppercase and titlecase mappings, three\n\
         /// in a row: each its offset into [`CASE_STRINGS`] shifted left by two,\n\
         /// over its length in characters.",
        "CASE_EXCEPTIONS",
        "u16",
        &exception_words,
    );
    emit_array(
        &mut code,
        "The exceptions' mappings, spelled out, each character in the BMP.",
        "CASE_STRINGS",
        "u16",
        &strings,
    );
    emit_array(
        &mut code,
        "Soft_Dotted, each range its first character shifted left by eight,\n\
         /// over its length less one.",
        "SOFT_DOTTED",
        "u32",
        &soft_dotted_runs,
    );

    writeln!(
        code,
        "\n// The Greek letter flags, above the ten bits of the capital.\n\
         pub(super) const GREEK_UPPER_MASK: u32 = 0x{GREEK_UPPER_MASK:X};\n\
         pub(super) const GREEK_VOWEL: u32 = 1 << {};\n\
         pub(super) const GREEK_YPOGEGRAMMENI: u32 = 1 << {};\n\
         pub(super) const GREEK_ACCENT: u32 = 1 << {};\n\
         pub(super) const GREEK_DIALYTIKA: u32 = 1 << {};\n\n\
         /// What Greek uppercasing reads of each character of U+0370 to U+03FF and\n\
         /// then U+1F00 to U+1FFF: its base letter's capital and the flags its\n\
         /// diacritics set, or zero where it is no Greek letter.\n\
         pub(super) static GREEK_LETTERS: [u16; {}] = [",
        GREEK_VOWEL.trailing_zeros(),
        GREEK_YPOGEGRAMMENI.trailing_zeros(),
        GREEK_ACCENT.trailing_zeros(),
        GREEK_DIALYTIKA.trailing_zeros(),
        greek.len()
    )
    .unwrap();
    for entry in &greek {
        write!(code, "0x{entry:X},").unwrap();
    }
    code.push_str("];\n\n");

    let core = pack("core", &core);
    let rare = pack("rare", &rare);
    code.push_str(&core.code);
    code.push_str(&rare.code);
    code.push_str(AT);

    std::fs::write(OUT, code).expect("writing tables.rs");

    let tags = script_tags.len() * 4;
    let set_bytes = starts.len() * 2 + list.len() * 2;
    let bracket_bytes = pair_count * 4;
    println!("wrote {}", std::path::Path::new(OUT).display());
    println!("core:  {} bytes, packtab {}", core.bytes, core.shape);
    println!("rare: {} bytes, packtab {}", rare.bytes, rare.shape);
    println!(
        "case: {} runs {} B, {} deltas {} B, {} exceptions {} B, strings {} B, \
         Soft_Dotted {} B, Greek {} B",
        runs.len(),
        runs.len() * 5,
        deltas.len(),
        deltas.len() * 4,
        exceptions.len(),
        exceptions.len() * 6,
        strings.len() * 2,
        soft_dotted_runs.len() * 4,
        greek.len() * 2
    );
    println!(
        "side: {} bytes ({script_count} script tags {tags} B, {set_count} sets in {} entries {set_bytes} B, {pair_count} bracket pairs {bracket_bytes} B)",
        tags + set_bytes + bracket_bytes,
        list.len()
    );
}
