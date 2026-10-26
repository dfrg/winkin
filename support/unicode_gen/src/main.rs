//! Generates `cleft`'s property and case tables from the Unicode
//! Character Database.
//!
//! Run it from anywhere in the workspace, then format what it wrote:
//!
//! ```text
//! cargo run --release -p unicode_gen
//! cargo fmt
//! ```
//!
//! It reads the UCD files of one version, pinned in `data/<version>/` and
//! checked against the SHA-256 each has in `manifest.txt`, and writes
//! `crates/cleft/src/props/tables.rs` and
//! `crates/cleft/src/case/tables.rs`. Both are committed. Running it
//! again reproduces them byte for byte. `cleft`'s sweep tests check
//! them against icu_properties and icu_casemap for every character, and say
//! where the two versions' data differ.
//!
//! A version other than the default is named as the one argument:
//! `cargo run --release -p unicode_gen -- 17.0.0`.
//!
//! `cargo run --release -p unicode_gen -- dictionaries` writes the word
//! dictionaries instead, from ICU's word lists (`dictionary`).
//!
//! The input files are not in the public repository.
//! `cargo run -p unicode_gen -- fetch` downloads every file the manifest
//! lists into `data/` and checks it (`pinned`). Without them the generator
//! stops with an error that says so.
//!
//! # Why a packed table
//!
//! Analysis asks for a handful of properties of every character. Asking
//! `icu_properties` means one trie or inversion list per property, and above
//! U+0FFF an out-of-line call and four to six dependent loads for each trie:
//! about ten times what one packed lookup costs. So everything is packed into
//! two `u32` words per code point, each a three-level packtab lookup with no
//! slow path: three dependent loads in every plane. The line segmenter reads
//! a byte of its own, the third lookup.
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
//! | 14 | Extended_Pictographic |
//! | 15..18 | `css`: East Asian spacing: 0 other, 1 narrow, 2 narrow digit, 3 conditional, 4 wide, 5 a mark |
//! | 18..22 | `css`: the text-spacing class: Blink's `HanKerningCharType`, 0 to 10 |
//! | 22..25 | `css`: the emphasis class: 0 marked, 1 a space, 2 punctuation, 3 a symbol, 4 never marked |
//! | 25..27 | `css`: the first-letter class: 0 other, 1 a space, 2 opening punctuation or a dash, 3 other punctuation |
//! | 27 | Cased |
//! | 28 | Case_Ignorable |
//! | 29 | `css`: General_Category L or N: a letter or a number, which `word-break: keep-all` holds together |
//! | 30..32 | zero |
//!
//! # The line byte
//!
//! What the line segmenter reads of every character, one byte from a lookup
//! of its own, with a 256-entry table in front for Latin-1.
//!
//! | Bits | Field |
//! |---:|---|
//! | 0..6 | the line class: Line_Break, split where a rule or option reads more |
//! | 6 | East_Asian_Width = Fullwidth, Wide or Halfwidth: UAX #14's `$EastAsian` |
//! | 7 | Extended_Pictographic and unassigned, which LB30b holds before EM |
//!
//! The line class numbers Line_Break's values as `cleft`'s `line`
//! module does, and splits nine of them by another property:
//! - QU by General_Category: Pi and Pf have classes of their own, which LB15a,
//!   LB15b and LB19 read;
//! - OP by East_Asian_Width, since LB30 holds only an OP that is not East
//!   Asian;
//! - ID by being Extended_Pictographic and unassigned, which `keep-all`
//!   leaves out of the ideographs;
//! - SA by General_Category: Mn and Mc resolve to CM under LB1, the rest to
//!   AL;
//! - AL by being U+25CC DOTTED CIRCLE, which LB28a names;
//! - PO and PR by East_Asian_Width Ambiguous, Fullwidth or Wide, which the
//!   Japanese and Chinese `loose` tailoring breaks before and after. The
//!   sets are those ICU4C's `line_loose_cj.txt` lists as `$POX` and `$PRX`;
//!   the generator checks that;
//! - NS by the strictness that lets a line break before it, as ICU4C's rules
//!   list them: U+301C and U+30A0 under Japanese and Chinese `normal`, the
//!   iteration marks under `loose`, and the centred punctuation under
//!   Japanese and Chinese `loose`;
//! - EX by being U+FF01 or U+FF1F, which Japanese and Chinese `loose` lets a
//!   line break before.
//!
//! AI, XX and CJ keep classes of their own, since the `keep-all` and
//! `break-all` options read them apart from AL and NS. SG has no scalar
//! value, so no character has it. No CP is East Asian, so LB30's
//! `[CP-$EastAsian]` needs no split; the generator checks that.
//!
//! Joining_Type, East_Asian_Width, Vertical_Orientation and the glue classes
//! are each stored as the one question every reader asks of them: a
//! join-causing character that letter spacing sees through, a wide character
//! `text-emphasis-skip: narrow` marks, an upright one under
//! `text-orientation: mixed`, and a character that glues an atomic inline to
//! its neighbour.
//!
//! Bits 8 to 15 are what the grapheme segmenter reads, one byte taken with
//! one shift: Grapheme_Cluster_Break, Indic_Conjunct_Break and
//! Extended_Pictographic, which the core word holds too. Emoji_Modifier_Base
//! and Emoji_Component are reserved: only the sweep test reads them, so their
//! constants are compiled for the tests alone. Line_Break = SA is read by word motion, which segments a run
//! of it whole, as ICU's word segmenter hands such a run to its dictionary or
//! model. The mark bit is read by font coverage, which replays harfrust's
//! normalizer against a font: a mark is what that normalizer clusters with
//! the character before it, and what it composes onto a starter. The Emoji
//! bit is read by font selection, which applies `font-variant-emoji` to an
//! Emoji character as Chrome does (`Character::IsEmoji`), and to no other.
//!
//! # The CSS classes
//!
//! Bits 15 to 27 hold classes CSS defines and Unicode does not, derived
//! below, and bit 29 the letters and numbers `keep-all` reads. They are
//! compiled only with `cleft`'s `css` feature. The
//! rare word is packed twice: once with them, and once with them zero, which
//! packs smaller. Each build reads one of the two, so it makes one lookup per
//! character either way.
//!
//! # East Asian spacing
//!
//! What `text-autospace` reads: UTR #59's East_Asian_Spacing,
//! derived here as Chrome 153 derives it for its own table
//! (`character_property_data_generator.cc`, `SetEastAsianSpacing`), since
//! the UCD carries no such property. Wide: Script Bopomofo, Han, Hangul,
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
//! What `::first-letter` reads to find where its text ends, the categories
//! of CSS Pseudo-Elements 4, section 2.2.1, as Blink's
//! `FirstLetterPseudoElement` tests them: 1, a space separator (Zs) other
//! than U+3000 IDEOGRAPHIC SPACE, which may come between the punctuation and
//! the letter; 2, opening punctuation (Ps) or a dash (Pd), which may come
//! before the letter only; 3, the rest of the punctuation (Pc, Pe, Pi, Pf,
//! Po), which may come on either side of it. Everything else is 0.
//!
//! # Numbering
//!
//! The numbers are ICU4C's, baked into the generated file, except where a
//! field is too narrow for them. Each property is mapped here by a `match`
//! on its short value aliases. The match is also the check that the crate's
//! enums agree: a value added to Unicode fails here, loudly, rather than
//! being packed as something else.
//!
//! Grapheme_Cluster_Break is the one field that departs. ICU4C numbers ZWJ 17,
//! after four values Unicode has assigned to no character since 11.0. Four
//! bits hold 0 to 15, so ZWJ is 13 here and the four are refused.
//!
//! Scripts are numbered as ICU4C's `UScriptCode` numbers them, which the UCD
//! does not give: [`ICU_SCRIPTS`] lists ICU4C's tags in value order. A script
//! Unicode adds before ICU4C numbers it takes the next value, in the order of
//! the list's tail.
//!
//! # Case mapping
//!
//! What `text-transform` maps case by (`cleft`'s `case` module),
//! read from UnicodeData.txt and SpecialCasing.txt: each character's full
//! lowercase, uppercase and titlecase in the root language, alone. The
//! simple mappings come from UnicodeData.txt, where an empty titlecase is the
//! uppercase, and SpecialCasing.txt's unconditional entries replace them.
//! Its conditional entries and the language tailorings are code in the case
//! module, as ICU4C writes them.
//!
//! - `CASE_RUNS` holds every run of neighbours that map alike, binary
//!   searched: each the run's first character, its length less one and its
//!   kind. A run of lowercase letters maps up by one delta, a run of capitals
//!   down by one, and a run of pairs alternates capital and small letter.
//! - `CASE_RUN_DATA` names each run's delta in `CASE_DELTAS`, or its first
//!   exception.
//! - An exception is a character one delta cannot map: a mapping longer than
//!   one character (`ß`, `ﬁ`, `ŉ`), or a titlecase that is not the uppercase
//!   (`ǆ`, `ǅ`). `CASE_EXCEPTIONS` spells its three mappings in
//!   `CASE_STRINGS`, which holds BMP characters only. A mapping spelled
//!   empty is the character itself, which is how an exception past the BMP
//!   (U+1DF95, from 18.0) keeps its own lowercase.
//! - Uppercase leaves Georgian Mkhedruli as it is, as Chrome's
//!   `DisableNewGeorgianCapitalLetters` does, so it needs no exception.
//! - `SOFT_DOTTED` is the Soft_Dotted ranges, which Lithuanian reads.
//! - `ACCENTS` is the ranges of nonzero Canonical_Combining_Class, each
//!   marked where the class is 230 (Above). Lithuanian reads them as ICU4C's
//!   dot types: an accent above, or another accent.
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
//!   `SCRIPT_RUNS` is the character's Script and then its Script_Extensions,
//!   in ICU4C's value order. ICU stores the script first in the same way for
//!   its "other" case; here every set does, Common and Inherited included.
//!
//! One set exists per distinct pair of Script and Script_Extensions, numbered
//! in the order the sweep first meets it. `SCRIPT_RUNS` begins with every
//! script alone, in value order, so that a plain script's extensions are a
//! one-element slice of it. A code point ScriptExtensions.txt does not list
//! has its Script alone.
//!
//! # Brackets
//!
//! A pair id and an "opens" bit rather than the closing character: Unicode 18
//! has 65 bracket pairs, 64 once the equivalent ones below are merged, so
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
//! packtab reads its arrays with `[]`, and the crate allows no panic that a
//! caller's input could reach. Every read goes through `at`, which answers
//! zero past the end of its array. No index the lookups compute is past the
//! end; the sweep test visits every character to show it.

mod dictionary;
mod pinned;
mod sha256;
mod ucd;

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;
use std::process::ExitCode;

use ucd::{CODE_POINTS, Ucd, UnicodeData};

/// The version generated when no argument names one.
const DEFAULT_VERSION: &str = "18.0.0";

/// The pinned data, from this crate's manifest.
const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/data");

/// Where the property tables go, from this crate's manifest.
const PROPS_OUT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/cleft/src/props/tables.rs"
);

/// Where the case tables go, from this crate's manifest.
const CASE_OUT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/cleft/src/case/tables.rs"
);

/// ICU4C's `UScriptCode` values, as ISO 15924 tags in value order, and then
/// the scripts Unicode added that ICU4C has not numbered yet.
///
/// A value no character has keeps ICU4C's tag for it.
#[rustfmt::skip]
const ICU_SCRIPTS: [&str; 215] = [
    "Zyyy", "Zinh", "Arab", "Armn", "Beng", "Bopo", "Cher", "Copt", "Cyrl", "Dsrt", "Deva", "Ethi",
    "Geor", "Goth", "Grek", "Gujr", "Guru", "Hani", "Hang", "Hebr", "Hira", "Knda", "Kana", "Khmr",
    "Laoo", "Latn", "Mlym", "Mong", "Mymr", "Ogam", "Ital", "Orya", "Runr", "Sinh", "Syrc", "Taml",
    "Telu", "Thaa", "Thai", "Tibt", "Cans", "Yiii", "Tglg", "Hano", "Buhd", "Tagb", "Brai", "Cprt",
    "Limb", "Linb", "Osma", "Shaw", "Tale", "Ugar", "Hrkt", "Bugi", "Glag", "Khar", "Sylo", "Talu",
    "Tfng", "Xpeo", "Bali", "Batk", "Blis", "Brah", "Cham", "Cirt", "Cyrs", "Egyd", "Egyh", "Egyp",
    "Geok", "Hans", "Hant", "Hmng", "Hung", "Inds", "Java", "Kali", "Latf", "Latg", "Lepc", "Lina",
    "Mand", "Maya", "Mero", "Nkoo", "Orkh", "Perm", "Phag", "Phnx", "Plrd", "Roro", "Sara", "Syre",
    "Syrj", "Syrn", "Teng", "Vaii", "Visp", "Xsux", "Zxxx", "Zzzz", "Cari", "Jpan", "Lana", "Lyci",
    "Lydi", "Olck", "Rjng", "Saur", "Sgnw", "Sund", "Moon", "Mtei", "Armi", "Avst", "Cakm", "Kore",
    "Kthi", "Mani", "Phli", "Phlp", "Phlv", "Prti", "Samr", "Tavt", "Zmth", "Zsym", "Bamu", "Lisu",
    "Nkgb", "Sarb", "Bass", "Dupl", "Elba", "Gran", "Kpel", "Loma", "Mend", "Merc", "Narb", "Nbat",
    "Palm", "Sind", "Wara", "Afak", "Jurc", "Mroo", "Nshu", "Shrd", "Sora", "Takr", "Tang", "Wole",
    "Hluw", "Khoj", "Tirh", "Aghb", "Mahj", "Ahom", "Hatr", "Modi", "Mult", "Pauc", "Sidd", "Adlm",
    "Bhks", "Marc", "Newa", "Osge", "Hanb", "Jamo", "Zsye", "Gonm", "Soyo", "Zanb", "Dogr", "Gong",
    "Maka", "Medf", "Rohg", "Sogd", "Sogo", "Elym", "Hmnp", "Nand", "Wcho", "Chrs", "Diak", "Kits",
    "Yezi", "Cpmn", "Ougr", "Tnsa", "Toto", "Vith", "Kawi", "Nagm", "Aran", "Gara", "Gukh", "Krai",
    "Onao", "Sunu", "Todr", "Tutg", "Berf", "Sidt", "Tayo", "Tols", "Hntl",
    // Unicode 18.0's, which ICU4C 78 has not numbered.
    "Pcun", "Seal",
];

/// A field of one of the two words.
struct Field {
    /// The constant's name in the generated file, and in errors.
    name: &'static str,
    shift: u32,
    bits: u32,
    /// The `cfg` the field's constants are compiled under, or `None` for
    /// every build.
    gate: Option<&'static str>,
}

impl Field {
    const fn new(name: &'static str, shift: u32, bits: u32) -> Self {
        Self {
            name,
            shift,
            bits,
            gate: None,
        }
    }

    /// A field no reader asks for yet: its constants are for the sweep test.
    const fn reserved(name: &'static str, shift: u32, bits: u32) -> Self {
        Self {
            gate: Some("test"),
            ..Self::new(name, shift, bits)
        }
    }

    /// A class CSS defines, compiled with the `css` feature.
    const fn css(name: &'static str, shift: u32, bits: u32) -> Self {
        Self {
            gate: Some("feature = \"css\""),
            ..Self::new(name, shift, bits)
        }
    }

    /// The field's bits in place.
    const fn mask(&self) -> u32 {
        ((1 << self.bits) - 1) << self.shift
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
    /// a mask for anything wider; under the field's gate.
    fn emit(&self, code: &mut String) {
        let gate = match self.gate {
            Some(gate) => format!("#[cfg({gate})]\n"),
            None => String::new(),
        };
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
const INDIC_CONJUNCT_BREAK: Field = Field::new("INDIC_CONJUNCT_BREAK", 12, 2);
const GRAPHEME_PICTOGRAPHIC: Field = Field::new("GRAPHEME_PICTOGRAPHIC", 14, 1);
const EAST_ASIAN_SPACING: Field = Field::css("EAST_ASIAN_SPACING", 15, 3);
const TEXT_SPACING: Field = Field::css("TEXT_SPACING", 18, 4);
const EMPHASIS: Field = Field::css("EMPHASIS", 22, 3);
const FIRST_LETTER: Field = Field::css("FIRST_LETTER", 25, 2);
const CASED: Field = Field::new("CASED", 27, 1);
const CASE_IGNORABLE: Field = Field::new("CASE_IGNORABLE", 28, 1);
const LETTER_OR_NUMBER: Field = Field::css("LETTER_OR_NUMBER", 29, 1);

const RARE: [&Field; 18] = [
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
    &GRAPHEME_PICTOGRAPHIC,
    &EAST_ASIAN_SPACING,
    &TEXT_SPACING,
    &EMPHASIS,
    &FIRST_LETTER,
    &CASED,
    &CASE_IGNORABLE,
    &LETTER_OR_NUMBER,
];

// The line byte.
const LINE_CLASS: Field = Field::new("LINE_CLASS", 0, 6);
const LINE_EAST_ASIAN: Field = Field::new("LINE_EAST_ASIAN", 6, 1);
const LINE_UNASSIGNED_PICTOGRAPHIC: Field = Field::new("LINE_UNASSIGNED_PICTOGRAPHIC", 7, 1);

// The word byte.
const WORD_CLASS: Field = Field::new("WORD_CLASS", 0, 5);
const WORD_PICTOGRAPHIC: Field = Field::new("WORD_PICTOGRAPHIC", 5, 1);
const WORD_RUN: Field = Field::new("WORD_RUN", 6, 2);

/// The word run of a character in no dictionary run.
const WORD_RUN_NONE: u32 = 0;
/// The word run of a Line_Break SA character, whose script's dictionary
/// finds its words.
const WORD_RUN_SOUTHEAST_ASIAN: u32 = 1;
/// The word run of a Han, Hiragana or Katakana character, whose words the
/// Chinese and Japanese dictionary finds where there is one.
const WORD_RUN_CHINESE_JAPANESE: u32 = 2;

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
fn greek_diacritic(cp: u32) -> Option<u32> {
    match cp {
        0x300 | 0x301 | 0x342 | 0x302 | 0x303 | 0x311 => Some(GREEK_ACCENT),
        0x308 => Some(GREEK_DIALYTIKA),
        0x344 => Some(GREEK_DIALYTIKA | GREEK_ACCENT),
        0x345 => Some(GREEK_YPOGEGRAMMENI),
        0x304 | 0x306 | 0x313 | 0x314 | 0x343 => Some(0),
        _ => None,
    }
}

/// Appends the full canonical decomposition of `cp` to `out`.
fn decompose(cp: u32, data: &[UnicodeData], out: &mut Vec<u32>) {
    let parts = &data[cp as usize].decomposition;
    if parts.is_empty() {
        out.push(cp);
    } else {
        for &part in parts {
            decompose(part, data, out);
        }
    }
}

/// The Greek uppercasing entry of `cp`: its base letter's capital and the
/// flags its diacritics set, or zero for a character that is not a Greek
/// letter, possibly with Greek diacritics.
fn greek_letter(cp: u32, data: &[UnicodeData]) -> u32 {
    let mut decomposed = Vec::new();
    decompose(cp, data, &mut decomposed);
    let (&base, marks) = decomposed
        .split_first()
        .expect("a decomposition is never empty");
    if !(0x370..=0x3FF).contains(&base) || !is_letter(&data[base as usize].general_category) {
        return 0;
    }
    let mut flags = 0;
    for &mark in marks {
        match greek_diacritic(mark) {
            Some(flag) => flags |= flag,
            None => return 0,
        }
    }
    let upper = data[base as usize].upper.unwrap_or(base);
    assert!(
        upper <= GREEK_UPPER_MASK,
        "U+{cp:04X}'s capital is past U+03FF"
    );
    if [0x391, 0x395, 0x397, 0x399, 0x39F, 0x3A5, 0x3A9].contains(&upper) {
        flags |= GREEK_VOWEL;
    }
    upper | flags
}

/// ICU4C's `UCharDirection` numbering, which the bidi resolver's is.
fn bidi_class(class: &str) -> u32 {
    match class {
        "L" => 0,
        "R" => 1,
        "EN" => 2,
        "ES" => 3,
        "ET" => 4,
        "AN" => 5,
        "CS" => 6,
        "B" => 7,
        "S" => 8,
        "WS" => 9,
        "ON" => 10,
        "LRE" => 11,
        "LRO" => 12,
        "AL" => 13,
        "RLE" => 14,
        "RLO" => 15,
        "PDF" => 16,
        "NSM" => 17,
        "BN" => 18,
        "FSI" => 19,
        "LRI" => 20,
        "RLI" => 21,
        "PDI" => 22,
        other => panic!("bidi class {other} is new; the bidi resolver needs a value for it"),
    }
}

/// Whether a character causes a join on both sides with no forms of its
/// own: Joining_Type Join_Causing, the tatweel and U+200D.
fn is_join_causing(joining: &str) -> bool {
    match joining {
        "C" => true,
        "U" | "D" | "L" | "R" | "T" => false,
        other => panic!("joining type {other} is new; decide whether it causes a join"),
    }
}

/// Whether a character is wide: East_Asian_Width Wide or Fullwidth.
fn is_wide(width: &str) -> bool {
    match width {
        "W" | "F" => true,
        "N" | "A" | "H" | "Na" => false,
        other => panic!("East_Asian_Width {other} is new; decide whether it is wide"),
    }
}

/// Whether a character stands upright in a vertical line under
/// `text-orientation: mixed`: Vertical_Orientation U, Tu or Tr, as Blink's
/// `Character::IsUprightInMixedVertical` takes them.
fn is_mixed_upright(orientation: &str) -> bool {
    match orientation {
        "R" => false,
        "Tr" | "Tu" | "U" => true,
        other => panic!("Vertical_Orientation {other} is new; decide whether it stands up"),
    }
}

/// Whether a character glues its neighbours together, which the object-edge
/// rule reads: Line_Break GL, WJ or ZWJ.
fn is_glue(line_break: &str) -> bool {
    matches!(line_break, "GL" | "WJ" | "ZWJ")
}

/// The line class of `cp`: its Line_Break, numbered as `cleft`'s
/// `line` module numbers it, with the splits the module documentation lists.
/// An OP is split by East_Asian_Width afterwards.
fn line_class(cp: u32, line_break: &str, gc: &str, pictographic: bool) -> u32 {
    match line_break {
        "XX" | "SG" => 0,
        "AL" if cp == 0x25CC => 3,
        "AL" => 1,
        "AI" => 2,
        "SA" if matches!(gc, "Mn" | "Mc") => 5,
        "SA" => 4,
        "BK" => 6,
        "CR" => 7,
        "LF" => 8,
        "NL" => 9,
        "SP" => 10,
        "ZW" => 11,
        "WJ" => 12,
        "GL" => 13,
        "CM" => 14,
        "ZWJ" => 15,
        "CL" => 16,
        "CP" => 17,
        "EX" => 18,
        "IS" => 19,
        "SY" => 20,
        "OP" => 21,
        "QU" if gc == "Pi" => 24,
        "QU" if gc == "Pf" => 25,
        "QU" => 23,
        "NS" => 26,
        "CJ" => 27,
        "B2" => 28,
        "BA" => 29,
        "BB" => 30,
        "HY" => 31,
        "HH" => 32,
        "CB" => 33,
        "IN" => 34,
        "NU" => 35,
        "PO" => 36,
        "PR" => 37,
        "HL" => 38,
        "ID" if pictographic && gc == "Cn" => 40,
        "ID" => 39,
        "EB" => 41,
        "EM" => 42,
        "JL" => 43,
        "JV" => 44,
        "JT" => 45,
        "H2" => 46,
        "H3" => 47,
        "RI" => 48,
        "AK" => 49,
        "AP" => 50,
        "AS" => 51,
        "VF" => 52,
        "VI" => 53,
        other => panic!("Line_Break {other} is new; give it a line class"),
    }
}

/// The word class of `cp`: its Word_Break, numbered as `cleft`'s
/// `word` module numbers it, with U+002E FULL STOP split from MidNumLet,
/// since the POSIX tailoring reads it as MidNum, and the ALetter of Han and
/// Hiragana split from ALetter, since a Chinese and Japanese dictionary
/// reads them as no letter. `chinese_japanese` says whether `cp` is in a
/// Chinese and Japanese run: Han, Hiragana or Word_Break Katakana.
fn word_class(cp: u32, word_break: &str, chinese_japanese: bool) -> u32 {
    match word_break {
        "MB" if cp == 0x2E => 19,
        "LE" if chinese_japanese => 20,
        "XX" => 0,
        "CR" => 1,
        "LF" => 2,
        "NL" => 3,
        "Extend" => 4,
        "ZWJ" => 5,
        "RI" => 6,
        "FO" => 7,
        "KA" => 8,
        "HL" => 9,
        "LE" => 10,
        "SQ" => 11,
        "DQ" => 12,
        "MB" => 13,
        "ML" => 14,
        "MN" => 15,
        "NU" => 16,
        "EX" => 17,
        "WSegSpace" => 18,
        other => panic!("Word_Break {other} is new; give it a word class"),
    }
}

/// The line class of an OP that is East Asian.
const LINE_OP_EAST_ASIAN: u32 = 22;
/// The line class of a PO that is East_Asian_Width A, F or W.
const LINE_PO_WIDE: u32 = 54;
/// The line class of a PR that is East_Asian_Width A, F or W.
const LINE_PR_WIDE: u32 = 55;

/// The line class of U+301C and U+30A0, NS that Japanese and Chinese
/// `normal` and `loose` let a line break before.
const LINE_NS_DASH: u32 = 56;
/// The line class of the iteration marks, NS that `loose` lets a line break
/// before.
const LINE_NS_ITERATION: u32 = 57;
/// The line class of the centred punctuation, NS that Japanese and Chinese
/// `loose` lets a line break before.
const LINE_NS_CENTRED: u32 = 58;
/// The line class of U+FF01 and U+FF1F, EX that Japanese and Chinese `loose`
/// lets a line break before.
const LINE_EX_WIDE: u32 = 59;

/// The PO and PR that ICU4C's `line_loose_cj.txt` lists as `$POX` and
/// `$PRX`, which the East_Asian_Width split must give.
const LOOSE_AFFIXES: [u32; 19] = [
    0x00B0, 0x2030, 0x2032, 0x2033, 0x2035, 0x2103, 0x2109, 0xFE6A, 0xFF05, 0xFFE0, 0x00A4, 0x00B1,
    0x20AC, 0x2116, 0xFE69, 0xFF04, 0xFFE1, 0xFFE5, 0xFFE6,
];

/// Whether `width` is Ambiguous, Fullwidth or Wide, the PO and PR that the
/// Japanese and Chinese `loose` tailoring reads.
fn is_cjk_wide(width: &str) -> bool {
    matches!(width, "A" | "F" | "W")
}

/// Whether `width` is UAX #14's `$EastAsian`: East_Asian_Width Fullwidth, Wide
/// or Halfwidth.
fn is_line_east_asian(width: &str) -> bool {
    match width {
        "F" | "W" | "H" => true,
        "N" | "A" | "Na" => false,
        other => panic!("East_Asian_Width {other} is new; decide whether UAX #14 reads it"),
    }
}

/// ICU4C's numbering up to 12, and ZWJ at 13 rather than 17.
fn grapheme_cluster_break(class: &str) -> u32 {
    match class {
        "XX" => 0,
        "CN" => 1,
        "CR" => 2,
        "EX" => 3,
        "L" => 4,
        "LF" => 5,
        "LV" => 6,
        "LVT" => 7,
        "T" => 8,
        "V" => 9,
        "SM" => 10,
        "PP" => 11,
        "RI" => 12,
        "ZWJ" => 13,
        other => panic!(
            "Grapheme_Cluster_Break {other} is assigned; four bits only hold it if an \
             unused value makes room"
        ),
    }
}

/// ICU4C's numbering, which is `cleft`'s `IndicConjunctBreak`'s.
fn indic_conjunct_break(class: &str) -> u32 {
    match class {
        "None" => 0,
        "Consonant" => 1,
        "Extend" => 2,
        "Linker" => 3,
        other => panic!("Indic_Conjunct_Break {other} is new"),
    }
}

/// The scripts UTR #59 calls ideographic for East_Asian_Spacing.
const WIDE_SCRIPTS: [&str; 9] = [
    "Bopo", "Hani", "Hang", "Hira", "Kana", "Kits", "Nshu", "Tang", "Yiii",
];

/// Whether `gc` is a mark: General_Category Mn, Mc or Me.
fn is_mark(gc: &str) -> bool {
    matches!(gc, "Mn" | "Mc" | "Me")
}

/// Whether `gc` is a letter.
fn is_letter(gc: &str) -> bool {
    matches!(gc, "Lu" | "Ll" | "Lt" | "Lm" | "Lo")
}

/// Whether `gc` is punctuation.
fn is_punctuation(gc: &str) -> bool {
    matches!(gc, "Pc" | "Pd" | "Ps" | "Pe" | "Pi" | "Pf" | "Po")
}

/// The East Asian spacing value of `cp` (see the module documentation): 0
/// other, 1 narrow, 2 a narrow decimal digit, 3 conditional, 4 wide, 5 a
/// mark. `extensions` is its Script_Extensions.
fn east_asian_spacing(cp: u32, gc: &str, width: &str, script: &str, extensions: &[&str]) -> u32 {
    if is_mark(gc) {
        return 5;
    }
    if gc == "Cn" {
        return 0;
    }
    let wide_width = matches!(width, "F" | "H" | "W");
    let mut wide = WIDE_SCRIPTS.contains(&script)
        || (WIDE_SCRIPTS.iter().any(|wide| extensions.contains(wide))
            && !matches!(width, "N" | "Na"));
    wide &= width != "H" && !is_punctuation(gc) && gc != "No" && !matches!(gc, "Sm" | "Sc" | "So");
    if wide || cp == 0x3013 {
        return 4;
    }
    let listed = [0x22, 0x27, 0x2A, 0x2F, 0xB7, 0x2020, 0x2021, 0x2026];
    if gc == "Po" && !wide_width && !listed.contains(&cp) {
        return 3;
    }
    if !wide_width && gc == "Nd" {
        return 2;
    }
    if !wide_width && is_letter(gc) {
        return 1;
    }
    0
}

/// The text-spacing class of `cp` (see the module documentation), Blink's
/// `HanKerningCharType` numbering.
fn text_spacing(cp: u32, gc: &str, width: &str) -> u32 {
    match cp {
        0x2018 | 0x201C => return 9,
        0x2019 | 0x201D => return 10,
        0x3000 | 0x00B7 | 0x2027 | 0x30FB => return 3,
        0x3001 | 0x3002 | 0xFF0C | 0xFF0E => return 6,
        0xFF1A => return 7,
        0xFF1B => return 8,
        _ => {}
    }
    let full = (0x3000..=0x303F).contains(&cp) || width == "F";
    match gc {
        "Ps" if full => 1,
        "Pe" if full => 2,
        "Ps" => 4,
        "Pe" => 5,
        _ => 0,
    }
}

/// The emphasis class of `cp` (see the module documentation): 0 marked, 1 a
/// space, 2 punctuation, 3 a symbol, 4 never marked. In Chrome's order
/// (`Character::CanReceiveTextEmphasis`): the categories never marked, the
/// word separators, the punctuation CSS marks anyway, and then the rest of
/// the punctuation.
fn emphasis(cp: u32, gc: &str) -> u32 {
    match gc {
        "Cc" | "Cf" | "Cn" | "Zl" | "Zp" => return 4,
        "Zs" => return 1,
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
    if matches!(gc, "Sm" | "Sc" | "Sk" | "So") {
        return 3;
    }
    0
}

/// The first-letter class of `cp` (see the module documentation): 0 other,
/// 1 a space separator other than U+3000, 2 opening punctuation or a dash, 3
/// other punctuation.
fn first_letter(cp: u32, gc: &str) -> u32 {
    match gc {
        "Zs" if cp != 0x3000 => 1,
        "Ps" | "Pd" => 2,
        "Pc" | "Pe" | "Pi" | "Pf" | "Po" => 3,
        _ => 0,
    }
}

/// Whether a character forces a line break, and so ends a bidi paragraph.
fn is_paragraph_separator(line_break: &str) -> bool {
    matches!(line_break, "BK" | "CR" | "LF" | "NL")
}

/// ICU4C's value of the script tagged `tag`.
fn script_value(tag: &str) -> u16 {
    let value = ICU_SCRIPTS
        .iter()
        .position(|known| *known == tag)
        .unwrap_or_else(|| panic!("script {tag} is new; give it the next value in ICU_SCRIPTS"));
    u16::try_from(value).expect("fewer than 2^16 scripts")
}

/// The one member of a canonical equivalence class of brackets that the table
/// keeps (UAX #9 BD16).
fn canonical_bracket(cp: u32) -> u32 {
    match cp {
        0x2329 => 0x3008,
        0x232A => 0x3009,
        other => other,
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
/// `fn {name}_word(ch: char) -> {ty}`, reading every array through `at`, each
/// item under `gate` where one is given. `ty` is the type of packtab's
/// widest array: `u32` for a word, `u8` for a byte.
fn pack(name: &str, values: &[i64], gate: Option<&str>, ty: &str) -> Lookup {
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

    let gate = match gate {
        Some(gate) => format!("#[cfg({gate})]\n"),
        None => String::new(),
    };
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
            "{gate}static {}: [{ty}; {}] = [",
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
         {gate}\
         #[allow(\n\
             clippy::unnecessary_cast,\n\
             reason = \"packtab casts every index to usize, whatever its type\"\n\
         )]\n\
         #[inline]\n\
         pub(super) fn {name}_word(ch: char) -> {ty} {{\n\
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

/// Writes the first lines of a generated file: what it holds and where from.
fn header(code: &mut String, holds: &str, version: &str) {
    writeln!(
        code,
        "//! {holds}\n\
         //!\n\
         //! Generated from the Unicode {version} character database files in\n\
         //! `support/unicode_gen/data/{version}` by `cargo run --release -p unicode_gen`,\n\
         //! then `cargo fmt`. Do not edit: change the generator and run it again. The\n\
         //! generator's documentation (`support/unicode_gen/src/main.rs`) says what\n\
         //! each field holds and why, and `super` reads them."
    )
    .unwrap();
}

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

/// Every code point.
fn code_points() -> std::ops::Range<u32> {
    0..CODE_POINTS as u32
}

fn main() -> ExitCode {
    let argument = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_VERSION.to_owned());
    let data = Path::new(DATA);
    let done = match argument.as_str() {
        "fetch" => pinned::fetch(data),
        "dictionaries" => dictionary::generate(data),
        version => generate(data, version),
    };
    match done {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("unicode_gen: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Writes the property and case tables of `version` from the files in
/// `data`.
fn generate(data: &Path, version: &str) -> Result<(), String> {
    let parts: Option<Vec<u8>> = version.split('.').map(|part| part.parse().ok()).collect();
    let Some([major, minor, update]) = parts
        .as_deref()
        .and_then(|parts| <[u8; 3]>::try_from(parts).ok())
    else {
        return Err(format!(
            "{version:?} is neither a version of three numbers, `dictionaries` nor `fetch`"
        ));
    };
    let ucd = Ucd::new(data, version)?;

    let bidi = ucd.map("DerivedBidiClass.txt", "bc", "L");
    let categories = ucd.map("DerivedGeneralCategory.txt", "gc", "Cn");
    let line_break = ucd.map("LineBreak.txt", "lb", "XX");
    let joining = ucd.map("DerivedJoiningType.txt", "jt", "U");
    let width = ucd.map("EastAsianWidth.txt", "ea", "N");
    let orientation = ucd.map("VerticalOrientation.txt", "vo", "R");
    let grapheme = ucd.map("GraphemeBreakProperty.txt", "GCB", "XX");
    let word_break = ucd.map("WordBreakProperty.txt", "WB", "XX");
    let conjunct = ucd.named_map("DerivedCoreProperties.txt", "InCB", "None");
    let regional = ucd.set("PropList.txt", "Regional_Indicator");
    let white_space = ucd.set("PropList.txt", "White_Space");
    let soft_dotted = ucd.set("PropList.txt", "Soft_Dotted");
    let pictographic = ucd.set("emoji-data.txt", "Extended_Pictographic");
    let presentation = ucd.set("emoji-data.txt", "Emoji_Presentation");
    let emoji = ucd.set("emoji-data.txt", "Emoji");
    let modifier_base = ucd.set("emoji-data.txt", "Emoji_Modifier_Base");
    let component = ucd.set("emoji-data.txt", "Emoji_Component");
    let ignorable = ucd.set("DerivedCoreProperties.txt", "Default_Ignorable_Code_Point");
    let cased = ucd.set("DerivedCoreProperties.txt", "Cased");
    let case_ignorable = ucd.set("DerivedCoreProperties.txt", "Case_Ignorable");
    let script_map = ucd.scripts();
    let listed_extensions = ucd.script_extensions();
    let brackets = ucd.brackets();
    let unicode_data = ucd.unicode_data();
    let special_casing = ucd.special_casing();

    // --- scripts ---------------------------------------------------------
    // Every code point's Script, and its Script_Extensions where they are more
    // than the script alone, in ICU4C's value order. A set is named by the
    // pair, in the order the sweep first meets it.
    let extension_tags = |cp: u32| -> Vec<&str> {
        match listed_extensions.get(&cp) {
            Some(tags) => tags.iter().map(String::as_str).collect(),
            None => vec![script_map.get(cp)],
        }
    };
    let mut script_count = 0;
    let mut sets: BTreeMap<Vec<u16>, u16> = BTreeMap::new();
    let mut set_runs: Vec<Vec<u16>> = Vec::new();
    let mut script_fields = Vec::with_capacity(CODE_POINTS);
    for cp in code_points() {
        let script = script_value(script_map.get(cp));
        let mut extensions: Vec<u16> = extension_tags(cp).into_iter().map(script_value).collect();
        extensions.sort_unstable();
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
    let script_tags = &ICU_SCRIPTS[..usize::from(script_count)];

    // --- brackets --------------------------------------------------------
    // One id per canonical closing character, in code point order.
    let paired_bracket = |cp: u32| -> Option<(u32, bool)> {
        let &(paired, opens) = brackets.get(&cp)?;
        let closing = if opens { paired } else { cp };
        Some((canonical_bracket(closing), opens))
    };
    let mut closings: BTreeMap<u32, u32> = BTreeMap::new();
    for cp in code_points() {
        if let Some((closing, _)) = paired_bracket(cp) {
            closings.insert(closing, 0);
        }
    }
    for (id, slot) in (0..).zip(closings.values_mut()) {
        *slot = id;
    }
    let pair_count = closings.len();

    // --- the words -------------------------------------------------------
    let css_mask = EAST_ASIAN_SPACING.mask()
        | TEXT_SPACING.mask()
        | EMPHASIS.mask()
        | FIRST_LETTER.mask()
        | LETTER_OR_NUMBER.mask();
    let mut core = Vec::with_capacity(CODE_POINTS);
    let mut rare = Vec::with_capacity(CODE_POINTS);
    let mut rare_plain = Vec::with_capacity(CODE_POINTS);
    let mut line = Vec::with_capacity(CODE_POINTS);
    let mut words = Vec::with_capacity(CODE_POINTS);
    for (cp, script_field) in code_points().zip(&script_fields) {
        let lb = line_break.get(cp);
        let gc = categories.get(cp);

        let mut word = 0;
        BIDI_CLASS.put(&mut word, bidi_class(bidi.get(cp)));
        let script = match *script_field {
            ScriptField::Script(script) => u32::from(script),
            ScriptField::Set(set) => u32::from(script_count) + u32::from(set),
        };
        SCRIPT.put(&mut word, script);
        if let Some((closing, opens)) = paired_bracket(cp) {
            BRACKET_PAIR.put(&mut word, closings[&closing] + 1);
            OPENS_BRACKET.put(&mut word, u32::from(opens));
        }
        REGIONAL_INDICATOR.put(&mut word, u32::from(regional.contains(cp)));
        EXTENDED_PICTOGRAPHIC.put(&mut word, u32::from(pictographic.contains(cp)));
        EMOJI_PRESENTATION.put(&mut word, u32::from(presentation.contains(cp)));
        PARAGRAPH_SEPARATOR.put(&mut word, u32::from(is_paragraph_separator(lb)));
        WHITE_SPACE.put(&mut word, u32::from(white_space.contains(cp)));
        COMPLEX_CONTEXT.put(&mut word, u32::from(lb == "SA"));
        MARK.put(&mut word, u32::from(is_mark(gc)));
        EMOJI.put(&mut word, u32::from(emoji.contains(cp)));
        core.push(i64::from(word));

        let mut word = 0;
        JOIN_CAUSING.put(&mut word, u32::from(is_join_causing(joining.get(cp))));
        WIDE.put(&mut word, u32::from(is_wide(width.get(cp))));
        UPRIGHT_IN_MIXED.put(&mut word, u32::from(is_mixed_upright(orientation.get(cp))));
        CONDITIONAL_JAPANESE_STARTER.put(&mut word, u32::from(lb == "CJ"));
        GLUE.put(&mut word, u32::from(is_glue(lb)));
        EMOJI_MODIFIER_BASE.put(&mut word, u32::from(modifier_base.contains(cp)));
        EMOJI_COMPONENT.put(&mut word, u32::from(component.contains(cp)));
        DEFAULT_IGNORABLE.put(&mut word, u32::from(ignorable.contains(cp)));
        GRAPHEME_CLUSTER_BREAK.put(&mut word, grapheme_cluster_break(grapheme.get(cp)));
        INDIC_CONJUNCT_BREAK.put(&mut word, indic_conjunct_break(conjunct.get(cp)));
        GRAPHEME_PICTOGRAPHIC.put(&mut word, u32::from(pictographic.contains(cp)));
        let eaw = width.get(cp);
        EAST_ASIAN_SPACING.put(
            &mut word,
            east_asian_spacing(cp, gc, eaw, script_map.get(cp), &extension_tags(cp)),
        );
        TEXT_SPACING.put(&mut word, text_spacing(cp, gc, eaw));
        EMPHASIS.put(&mut word, emphasis(cp, gc));
        FIRST_LETTER.put(&mut word, first_letter(cp, gc));
        CASED.put(&mut word, u32::from(cased.contains(cp)));
        CASE_IGNORABLE.put(&mut word, u32::from(case_ignorable.contains(cp)));
        LETTER_OR_NUMBER.put(
            &mut word,
            u32::from(is_letter(gc) || matches!(gc, "Nd" | "Nl" | "No")),
        );
        rare.push(i64::from(word));
        rare_plain.push(i64::from(word & !css_mask));

        let mut word = 0;
        let east_asian = is_line_east_asian(width.get(cp));
        let class = match line_class(cp, lb, gc, pictographic.contains(cp)) {
            21 if east_asian => LINE_OP_EAST_ASIAN,
            36 if is_cjk_wide(width.get(cp)) => LINE_PO_WIDE,
            37 if is_cjk_wide(width.get(cp)) => LINE_PR_WIDE,
            26 => match cp {
                0x301C | 0x30A0 => LINE_NS_DASH,
                0x3005 | 0x303B | 0x309D | 0x309E | 0x30FD | 0x30FE => LINE_NS_ITERATION,
                0x30FB | 0xFF1A | 0xFF1B | 0xFF65 | 0x203C | 0x2047..=0x2049 => LINE_NS_CENTRED,
                _ => 26,
            },
            18 if matches!(cp, 0xFF01 | 0xFF1F) => LINE_EX_WIDE,
            17 => {
                assert!(
                    !east_asian,
                    "U+{cp:04X} is an East Asian CP; LB30 needs a class for it"
                );
                17
            }
            class => class,
        };
        assert_eq!(
            matches!(class, LINE_PO_WIDE | LINE_PR_WIDE),
            LOOSE_AFFIXES.contains(&cp),
            "U+{cp:04X}: the East Asian PO and PR are not ICU4C's lists"
        );
        LINE_CLASS.put(&mut word, class);
        LINE_EAST_ASIAN.put(&mut word, u32::from(east_asian));
        LINE_UNASSIGNED_PICTOGRAPHIC.put(
            &mut word,
            u32::from(pictographic.contains(cp) && gc == "Cn"),
        );
        line.push(i64::from(word));

        let mut word = 0;
        // ICU4C's word rules hand Script Han and Hiragana and Word_Break
        // Katakana to the dictionary (`$KanaKanji`), the prolonged sound mark
        // included. The halfwidth katakana stay out: ICU4C finds their words
        // through NFKC, which the matcher does not apply, so WB13 keeps a run
        // of them whole.
        let chinese_japanese = matches!(script_map.get(cp), "Hani" | "Hira")
            || (word_break.get(cp) == "KA" && !(0xFF66..=0xFF9F).contains(&cp));
        WORD_CLASS.put(
            &mut word,
            word_class(cp, word_break.get(cp), chinese_japanese),
        );
        WORD_PICTOGRAPHIC.put(&mut word, u32::from(pictographic.contains(cp)));
        let run = if lb == "SA" {
            WORD_RUN_SOUTHEAST_ASIAN
        } else if chinese_japanese {
            WORD_RUN_CHINESE_JAPANESE
        } else {
            WORD_RUN_NONE
        };
        WORD_RUN.put(&mut word, run);
        words.push(i64::from(word));
    }

    // --- case --------------------------------------------------------------
    // Every code point's full mappings in the root language, one character
    // alone. Uppercase leaves Georgian Mkhedruli as it is, as Chrome does
    // (`DisableNewGeorgianCapitalLetters`). A character whose mappings one
    // delta cannot say, as where one is more than one character or its
    // titlecase is not its uppercase, is an exception: its three mappings are
    // spelled in `CASE_STRINGS`. Neighbours that map alike are a run.
    let mut exceptions: Vec<[u16; 3]> = Vec::new();
    let mut strings: Vec<u16> = Vec::new();
    // Appends `mapping` to the strings, or finds it there, and returns its
    // offset and length.
    let mut spell = |mapping: &[u32]| -> u16 {
        let units: Vec<u16> = mapping
            .iter()
            .map(|&cp| u16::try_from(cp).expect("an exception's mapping is in the BMP"))
            .collect();
        assert!(
            (1..=3).contains(&units.len()),
            "{mapping:X?} is not one to three characters"
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
    for cp in code_points().filter(|&cp| char::from_u32(cp).is_some()) {
        let data = &unicode_data[cp as usize];
        let one = vec![cp];
        let simple_upper = data.upper.unwrap_or(cp);
        let (lower, mut upper, title) = match special_casing.get(&cp) {
            Some(special) => (
                special.lower.clone(),
                special.upper.clone(),
                special.title.clone(),
            ),
            None => (
                vec![data.lower.unwrap_or(cp)],
                vec![simple_upper],
                vec![data.title.unwrap_or(simple_upper)],
            ),
        };
        if lower == one && upper.iter().all(|cp| (0x1C90..=0x1CBF).contains(cp)) {
            upper.clone_from(&one);
        }
        if lower == one && upper == one && title == one {
            continue;
        }
        let single = |mapping: &[u32]| match mapping {
            [to] => Some(i64::from(*to) - i64::from(cp)),
            _ => None,
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
                // A mapping to the character itself is spelled empty where
                // the character is past the BMP, as U+1DF95's lowercase is;
                // the strings hold BMP characters only.
                let mut spell_mapping = |mapping: &[u32]| match mapping {
                    [to] if *to == cp && cp > 0xFFFF => 0,
                    _ => spell(mapping),
                };
                let index = exceptions.len();
                exceptions.push([
                    spell_mapping(&lower),
                    spell_mapping(&upper),
                    spell_mapping(&title),
                ]);
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
        .ranges()
        .into_iter()
        .map(|(start, end)| {
            assert!(end - start < 1 << 8, "a Soft_Dotted range is long");
            start << 8 | (end - start)
        })
        .collect();
    // The accents: each range of nonzero combining class, its start shifted
    // left by eight over its length less one shifted left by one, over
    // whether the class is 230.
    let mut accents: Vec<(u32, u32, bool)> = Vec::new();
    for cp in code_points() {
        let class = unicode_data[cp as usize].combining_class;
        if class == 0 {
            continue;
        }
        let above = class == 230;
        match accents.last_mut() {
            Some(last) if last.1 + 1 == cp && last.2 == above && cp - last.0 < 1 << 7 => {
                last.1 = cp;
            }
            _ => accents.push((cp, cp, above)),
        }
    }
    let accent_runs: Vec<u32> = accents
        .iter()
        .map(|&(start, end, above)| start << 8 | (end - start) << 1 | u32::from(above))
        .collect();
    let greek: Vec<u32> = (0x370..=0x3FF_u32)
        .chain(0x1F00..=0x1FFF)
        .map(|cp| greek_letter(cp, &unicode_data))
        .collect();

    // --- emit the properties -------------------------------------------
    let mut code = String::new();
    header(
        &mut code,
        "The packed Unicode character properties: two words per code point\n\
         //! and the side tables they name.",
        version,
    );
    code.push_str("\nuse super::ScriptId;\n");
    writeln!(
        code,
        "\n/// The version of Unicode the tables are generated from.\n\
         pub(super) const UNICODE_VERSION: (u8, u8, u8) = ({major}, {minor}, {update});"
    )
    .unwrap();

    code.push_str("\n// The core word, read for every character.\n");
    for field in CORE {
        field.emit(&mut code);
    }
    code.push_str("\n// The rare word, read per cluster or behind a gate.\n");
    for field in RARE {
        field.emit(&mut code);
    }

    code.push_str("\n// The line byte, read by the line segmenter for every character.\n");
    for field in [&LINE_CLASS, &LINE_EAST_ASIAN, &LINE_UNASSIGNED_PICTOGRAPHIC] {
        field.emit(&mut code);
    }

    code.push_str("\n// The word byte, read by the word segmenter for every character.\n");
    for field in [&WORD_CLASS, &WORD_PICTOGRAPHIC, &WORD_RUN] {
        field.emit(&mut code);
    }
    writeln!(
        code,
        "pub(super) const WORD_RUN_SOUTHEAST_ASIAN: u32 = {WORD_RUN_SOUTHEAST_ASIAN};\n\
         pub(super) const WORD_RUN_CHINESE_JAPANESE: u32 = {WORD_RUN_CHINESE_JAPANESE};"
    )
    .unwrap();

    writeln!(
        code,
        "\n/// The number of script values. A script field below this is a script; at\n\
         /// or above it, it names a Script_Extensions set.\n\
         pub(super) const SCRIPT_COUNT: u16 = {script_count};"
    )
    .unwrap();
    for (name, tag) in [
        ("SCRIPT_COMMON", "Zyyy"),
        ("SCRIPT_INHERITED", "Zinh"),
        ("SCRIPT_UNKNOWN", "Zzzz"),
        ("SCRIPT_HAN", "Hani"),
        ("SCRIPT_HIRAGANA", "Hira"),
        ("SCRIPT_KATAKANA", "Kana"),
        ("SCRIPT_KHMER", "Khmr"),
        ("SCRIPT_LAO", "Laoo"),
        ("SCRIPT_MYANMAR", "Mymr"),
        ("SCRIPT_THAI", "Thai"),
    ] {
        writeln!(
            code,
            "pub(super) const {name}: u16 = {};",
            script_value(tag)
        )
        .unwrap();
    }

    writeln!(
        code,
        "\n/// The ISO 15924 tag of each script, by value. A value no character has\n\
         /// keeps ICU4C's tag for it.\n\
         pub(super) static SCRIPT_TAGS: [[u8; 4]; {script_count}] = ["
    )
    .unwrap();
    for tag in script_tags {
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
         /// character's Script, then its Script_Extensions in ICU4C's value order.\n\
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
        write!(code, "'\\u{{{closing:04X}}}',").unwrap();
    }
    code.push_str("];\n\n");

    let core = pack("core", &core, None, "u32");
    let rare_css = pack("rare", &rare, Some("feature = \"css\""), "u32");
    let rare_plain = pack("rare", &rare_plain, Some("not(feature = \"css\")"), "u32");
    code.push_str(&core.code);
    code.push_str(&rare_css.code);
    code.push_str(&rare_plain.code);
    emit_array(
        &mut code,
        "The line byte of U+0000 to U+00FF, read before the lookup.",
        "LINE_LATIN_1",
        "u8",
        &line[..256],
    );
    let line = pack("line", &line, None, "u8");
    code.push_str(&line.code);
    emit_array(
        &mut code,
        "The word byte of U+0000 to U+00FF, read before the lookup.",
        "WORD_LATIN_1",
        "u8",
        &words[..256],
    );
    let words = pack("word_break", &words, None, "u8");
    code.push_str(&words.code);
    code.push_str(AT);
    std::fs::write(PROPS_OUT, code).expect("writing the property tables");

    // --- emit the case tables ------------------------------------------
    let mut code = String::new();
    header(
        &mut code,
        "The case mapping tables: runs of characters that map alike, the\n\
         //! exceptions, and what the conditional mappings read.",
        version,
    );
    writeln!(
        code,
        "\n// The case runs: each its first character shifted left by {}, over its\n\
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
    emit_array(
        &mut code,
        "The characters of nonzero Canonical_Combining_Class, each range its\n\
         /// first character shifted left by eight, over its length less one\n\
         /// shifted left by one, over whether the class is 230 (Above).",
        "ACCENTS",
        "u32",
        &accent_runs,
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
    code.push_str("];\n");
    std::fs::write(CASE_OUT, code).expect("writing the case tables");

    let tags = script_tags.len() * 4;
    let set_bytes = starts.len() * 2 + list.len() * 2;
    let bracket_bytes = pair_count * 4;
    println!("Unicode {version}");
    println!("wrote {}", Path::new(PROPS_OUT).display());
    println!("wrote {}", Path::new(CASE_OUT).display());
    println!("core: {} bytes, packtab {}", core.bytes, core.shape);
    println!(
        "rare: {} bytes with css, packtab {}; {} bytes without, packtab {}",
        rare_css.bytes, rare_css.shape, rare_plain.bytes, rare_plain.shape
    );
    println!(
        "line: {} bytes, packtab {}, and 256 for Latin-1",
        line.bytes, line.shape
    );
    println!(
        "word: {} bytes, packtab {}, and 256 for Latin-1",
        words.bytes, words.shape
    );
    println!(
        "case: {} runs {} B, {} deltas {} B, {} exceptions {} B, strings {} B, \
         Soft_Dotted {} B, accents {} B, Greek {} B",
        runs.len(),
        runs.len() * 5,
        deltas.len(),
        deltas.len() * 4,
        exceptions.len(),
        exceptions.len() * 6,
        strings.len() * 2,
        soft_dotted_runs.len() * 4,
        accent_runs.len() * 4,
        greek.len() * 2
    );
    println!(
        "side: {} bytes ({script_count} script tags {tags} B, {set_count} sets in {} entries {set_bytes} B, {pair_count} bracket pairs {bracket_bytes} B)",
        tags + set_bytes + bracket_bytes,
        list.len()
    );
    Ok(())
}
