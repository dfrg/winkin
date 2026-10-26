//! The sweep: every character's two words, and every side table, checked
//! against icu_properties 2.3. The CSS classes are checked with the `css`
//! feature.
//!
//! The expected values are mapped from ICU's here by name, not by the
//! generator's code, so the two cannot share a mistake. Surrogates are not
//! characters, so the sweep skips them.
//!
//! ICU's data is Unicode 17.0 and the tables' 18.0. Where the two versions'
//! pinned UCD files differ, a property's expected value is 18.0's, read from
//! its file, and ICU must hold 17.0's: `Map` and `Set` do this. Each test
//! pins how many code points differ, and the list below says what changed.

use icu_properties::props::{self as icu, BidiPairedBracketType};
use icu_properties::props::{
    EnumeratedProperty, NamedEnumeratedProperty, ParseableEnumeratedProperty,
};
use icu_properties::script::ScriptWithExtensions;
#[cfg(feature = "css")]
use icu_properties::script::ScriptWithExtensionsBorrowed;
use icu_properties::{
    CodePointMapData, CodePointMapDataBorrowed, CodePointSetData, CodePointSetDataBorrowed,
    PropertyNamesShort, PropertyParser,
};
use std::collections::BTreeMap;
use std::eprintln;
use std::string::String;
use std::vec::Vec;

use super::*;
use crate::grapheme_props;
use crate::tests::{ICU_VERSION, TABLES_VERSION, Versions, changes};

/// An enumerated property: ICU's values, and the code points whose value
/// differs between ICU's version and the tables'.
struct Map<T: EnumeratedProperty + 'static> {
    icu: CodePointMapDataBorrowed<'static, T>,
    changes: BTreeMap<u32, (String, String)>,
    name: &'static str,
}

/// What a sweep reads of the two versions: the file, the property, its
/// default, and the field naming it where a file holds several.
type Source = (
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
);

impl<T> Map<T>
where
    T: EnumeratedProperty + NamedEnumeratedProperty + ParseableEnumeratedProperty + 'static,
{
    /// Reads `source` in both versions, and checks that `changed` code
    /// points differ.
    fn new(versions: &Versions, source: Source, changed: usize) -> Self {
        let (file, property, default, named) = source;
        let changes = versions.map_changes(file, property, default, named);
        assert_eq!(
            changes.len(),
            changed,
            "{property}: code points that differ between {ICU_VERSION} and {TABLES_VERSION}"
        );
        Self {
            icu: CodePointMapData::<T>::new(),
            changes,
            name: property,
        }
    }

    /// Returns the tables' version's value of `ch`: ICU's, unless the
    /// versions differ, where ICU must hold its version's.
    fn get(&self, ch: char) -> T {
        let icu = self.icu.get(ch);
        let Some((old, new)) = self.changes.get(&u32::from(ch)) else {
            return icu;
        };
        assert_eq!(
            PropertyNamesShort::<T>::new().get(icu),
            Some(old.as_str()),
            "{} of U+{:04X}: ICU holds {ICU_VERSION}'s value",
            self.name,
            u32::from(ch)
        );
        PropertyParser::<T>::new()
            .get_strict(new)
            .unwrap_or_else(|| panic!("{}: ICU has no value {new}", self.name))
    }
}

/// A binary property: ICU's set, and the code points whose membership
/// differs between ICU's version and the tables'.
struct Set {
    icu: CodePointSetDataBorrowed<'static>,
    changes: BTreeMap<u32, (bool, bool)>,
    name: &'static str,
}

impl Set {
    /// Reads `name` from `file` in both versions, and checks that `changed`
    /// code points differ.
    fn new(
        versions: &Versions,
        icu: CodePointSetDataBorrowed<'static>,
        (file, name): (&str, &'static str),
        changed: usize,
    ) -> Self {
        let changes = versions.set_changes(file, name);
        assert_eq!(
            changes.len(),
            changed,
            "{name}: code points that differ between {ICU_VERSION} and {TABLES_VERSION}"
        );
        Self { icu, changes, name }
    }

    /// Returns whether `ch` has the property in the tables' version.
    fn contains(&self, ch: char) -> bool {
        let icu = self.icu.contains(ch);
        match self.changes.get(&u32::from(ch)) {
            Some(&(old, new)) => {
                assert_eq!(
                    icu, old,
                    "{} of {ch:?}: ICU holds {ICU_VERSION}'s",
                    self.name
                );
                new
            }
            None => icu,
        }
    }
}

/// Returns both versions' files, or `None`, saying so, where they are
/// absent and a sweep cannot run.
fn versions() -> Option<Versions> {
    let versions = Versions::new();
    if versions.is_none() {
        eprintln!("the pinned UCD files are absent: the sweep is skipped");
    }
    versions
}

/// Returns the code points whose Script or Script_Extensions differ between
/// the versions, with each version's Script_Extensions (the Script alone
/// where none are listed), each sorted.
fn script_changes(versions: &Versions) -> BTreeMap<u32, (Vec<String>, Vec<String>)> {
    let sorted = |mut lists: Vec<Vec<String>>| {
        for list in &mut lists {
            list.sort();
        }
        lists
    };
    let old = versions.icu.map("Scripts.txt", "sc", "Unknown", None);
    let new = versions.tables.map("Scripts.txt", "sc", "Unknown", None);
    let script_changes = changes(old, new);
    let mut both = changes(
        sorted(versions.icu.script_extensions()),
        sorted(versions.tables.script_extensions()),
    );
    for cp in script_changes.keys() {
        let at = *cp as usize;
        both.entry(*cp).or_insert_with(|| {
            let mut old = versions.icu.script_extensions();
            let mut new = versions.tables.script_extensions();
            old[at].sort();
            new[at].sort();
            (old.swap_remove(at), new.swap_remove(at))
        });
    }
    both
}

// The 18.0 differences the sweeps meet, counted from the pinned files:
// - General_Category and Script: 13,007 code points newly assigned, 11,328
//   of them the Seal script's ideographs and 164 Proto_Cuneiform's signs;
//   and the Script_Extensions of U+0B83, U+1CF5 and U+1CF6;
// - Bidi_Class: 112, 78 newly assigned symbols L to ON, 34 marks to or
//   from NSM;
// - Line_Break: 1,672, all but 19 newly assigned; 16 ID to AL, 2 ID to EB,
//   and UAX #14's U+2012 and U+2013 HH to BA and U+00AD BA to HH;
// - Joining_Type 53, East_Asian_Width 990 (N to W), Vertical_Orientation
//   1,059, Grapheme_Cluster_Break 37 (to Extend), Indic_Conjunct_Break 42:
//   newly assigned characters, and the ranges reserved for them;
// - Extended_Pictographic 18 (reserved pictographs assigned as other
//   symbols), Emoji and Emoji_Presentation 9, Emoji_Modifier_Base 2,
//   Cased 165, Case_Ignorable 96: new characters;
// - White_Space, Regional_Indicator, Emoji_Component and
//   Default_Ignorable_Code_Point: none.
const BIDI: Source = ("DerivedBidiClass.txt", "bc", "L", None);
const CATEGORY: Source = ("DerivedGeneralCategory.txt", "gc", "Cn", None);
const LINE_BREAK: Source = ("LineBreak.txt", "lb", "XX", None);
const JOINING: Source = ("DerivedJoiningType.txt", "jt", "U", None);
const WIDTH: Source = ("EastAsianWidth.txt", "ea", "N", None);
const ORIENTATION: Source = ("VerticalOrientation.txt", "vo", "R", None);
const GRAPHEME: Source = ("GraphemeBreakProperty.txt", "GCB", "XX", None);
const CONJUNCT: Source = ("DerivedCoreProperties.txt", "InCB", "None", Some("InCB"));

/// Every character.
fn chars() -> impl Iterator<Item = char> {
    (0..=0x10FFFF_u32).filter_map(char::from_u32)
}

/// The short names of the Line_Break values, in ICU4C's `ULineBreak`
/// numbering.
const LINE_BREAK_NAMES: [&str; 49] = [
    "XX", "AI", "AL", "B2", "BA", "BB", "BK", "CB", "CL", "CM", "CR", "EX", "GL", "HY", "ID", "IN",
    "IS", "LF", "NS", "NU", "OP", "PO", "PR", "QU", "SA", "SG", "SP", "SY", "ZW", "NL", "WJ", "H2",
    "H3", "JL", "JT", "JV", "CP", "CJ", "HL", "RI", "EB", "EM", "ZWJ", "AK", "AP", "AS", "VF",
    "VI", "HH",
];

/// Returns ICU4C's `UCharDirection` value for `class`.
fn bidi_class(class: icu::BidiClass) -> u8 {
    match class {
        icu::BidiClass::LeftToRight => 0,
        icu::BidiClass::RightToLeft => 1,
        icu::BidiClass::EuropeanNumber => 2,
        icu::BidiClass::EuropeanSeparator => 3,
        icu::BidiClass::EuropeanTerminator => 4,
        icu::BidiClass::ArabicNumber => 5,
        icu::BidiClass::CommonSeparator => 6,
        icu::BidiClass::ParagraphSeparator => 7,
        icu::BidiClass::SegmentSeparator => 8,
        icu::BidiClass::WhiteSpace => 9,
        icu::BidiClass::OtherNeutral => 10,
        icu::BidiClass::LeftToRightEmbedding => 11,
        icu::BidiClass::LeftToRightOverride => 12,
        icu::BidiClass::ArabicLetter => 13,
        icu::BidiClass::RightToLeftEmbedding => 14,
        icu::BidiClass::RightToLeftOverride => 15,
        icu::BidiClass::PopDirectionalFormat => 16,
        icu::BidiClass::NonspacingMark => 17,
        icu::BidiClass::BoundaryNeutral => 18,
        icu::BidiClass::FirstStrongIsolate => 19,
        icu::BidiClass::LeftToRightIsolate => 20,
        icu::BidiClass::RightToLeftIsolate => 21,
        icu::BidiClass::PopDirectionalIsolate => 22,
        other => panic!("unexpected bidi class {other:?}"),
    }
}

fn grapheme_cluster_break(value: icu::GraphemeClusterBreak) -> GraphemeClusterBreak {
    match value {
        icu::GraphemeClusterBreak::Other => GraphemeClusterBreak::Other,
        icu::GraphemeClusterBreak::Control => GraphemeClusterBreak::Control,
        icu::GraphemeClusterBreak::CR => GraphemeClusterBreak::Cr,
        icu::GraphemeClusterBreak::Extend => GraphemeClusterBreak::Extend,
        icu::GraphemeClusterBreak::L => GraphemeClusterBreak::L,
        icu::GraphemeClusterBreak::LF => GraphemeClusterBreak::Lf,
        icu::GraphemeClusterBreak::LV => GraphemeClusterBreak::Lv,
        icu::GraphemeClusterBreak::LVT => GraphemeClusterBreak::Lvt,
        icu::GraphemeClusterBreak::T => GraphemeClusterBreak::T,
        icu::GraphemeClusterBreak::V => GraphemeClusterBreak::V,
        icu::GraphemeClusterBreak::SpacingMark => GraphemeClusterBreak::SpacingMark,
        icu::GraphemeClusterBreak::Prepend => GraphemeClusterBreak::Prepend,
        icu::GraphemeClusterBreak::RegionalIndicator => GraphemeClusterBreak::RegionalIndicator,
        icu::GraphemeClusterBreak::ZWJ => GraphemeClusterBreak::Zwj,
        other => panic!("unexpected Grapheme_Cluster_Break {other:?}"),
    }
}

fn indic_conjunct_break(value: icu::IndicConjunctBreak) -> IndicConjunctBreak {
    match value {
        icu::IndicConjunctBreak::None => IndicConjunctBreak::None,
        icu::IndicConjunctBreak::Consonant => IndicConjunctBreak::Consonant,
        icu::IndicConjunctBreak::Extend => IndicConjunctBreak::Extend,
        icu::IndicConjunctBreak::Linker => IndicConjunctBreak::Linker,
        other => panic!("unexpected Indic_Conjunct_Break {other:?}"),
    }
}

/// Returns ICU's ISO 15924 tag for `script`.
fn tag(script: icu::Script) -> [u8; 4] {
    let name = PropertyNamesShort::<icu::Script>::new()
        .get(script)
        .expect("every script ICU hands out has a short name");
    name.as_bytes()
        .try_into()
        .expect("script short names are tags")
}

/// Returns the bracket the table keeps for a canonically equivalent pair.
fn canonical_bracket(ch: char) -> char {
    match ch {
        '\u{2329}' => '\u{3008}',
        '\u{232A}' => '\u{3009}',
        other => other,
    }
}

#[test]
fn the_hot_word_agrees_with_icu() {
    let Some(versions) = versions() else {
        return;
    };
    let bidi = Map::<icu::BidiClass>::new(&versions, BIDI, 112);
    let line_break = Map::<icu::LineBreak>::new(&versions, LINE_BREAK, 1672);
    let regional = Set::new(
        &versions,
        CodePointSetData::new::<icu::RegionalIndicator>(),
        ("PropList.txt", "Regional_Indicator"),
        0,
    );
    let pictographic = Set::new(
        &versions,
        CodePointSetData::new::<icu::ExtendedPictographic>(),
        ("emoji-data.txt", "Extended_Pictographic"),
        18,
    );
    let presentation = Set::new(
        &versions,
        CodePointSetData::new::<icu::EmojiPresentation>(),
        ("emoji-data.txt", "Emoji_Presentation"),
        9,
    );
    let emoji = Set::new(
        &versions,
        CodePointSetData::new::<icu::Emoji>(),
        ("emoji-data.txt", "Emoji"),
        9,
    );
    let white_space = Set::new(
        &versions,
        CodePointSetData::new::<icu::WhiteSpace>(),
        ("PropList.txt", "White_Space"),
        0,
    );
    let categories = Map::<icu::GeneralCategory>::new(&versions, CATEGORY, 13007);
    for ch in chars() {
        let word = core_props(ch);
        let cp = u32::from(ch);
        let lb = line_break.get(ch);
        assert_eq!(word.0 >> 31, 0, "U+{cp:04X}: bits past the layout");
        assert_eq!(
            word.bidi_class(),
            BidiClass::new(bidi_class(bidi.get(ch))),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_regional_indicator(),
            regional.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_extended_pictographic(),
            pictographic.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_emoji_presentation(),
            presentation.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_paragraph_separator(),
            matches!(
                lb,
                icu::LineBreak::MandatoryBreak
                    | icu::LineBreak::CarriageReturn
                    | icu::LineBreak::LineFeed
                    | icu::LineBreak::NextLine
            ),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_white_space(),
            white_space.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_complex_context(),
            lb == icu::LineBreak::ComplexContext,
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_mark(),
            matches!(
                categories.get(ch),
                icu::GeneralCategory::NonspacingMark
                    | icu::GeneralCategory::SpacingMark
                    | icu::GeneralCategory::EnclosingMark
            ),
            "U+{cp:04X}"
        );
        assert_eq!(word.is_emoji(), emoji.contains(ch), "U+{cp:04X}");
    }
}

#[test]
fn the_cold_word_agrees_with_icu() {
    let Some(versions) = versions() else {
        return;
    };
    let joining = Map::<icu::JoiningType>::new(&versions, JOINING, 53);
    let width = Map::<icu::EastAsianWidth>::new(&versions, WIDTH, 990);
    let orientation = Map::<icu::VerticalOrientation>::new(&versions, ORIENTATION, 1059);
    let line_break = Map::<icu::LineBreak>::new(&versions, LINE_BREAK, 1672);
    let line_break_names = PropertyNamesShort::<icu::LineBreak>::new();
    #[cfg(feature = "css")]
    let categories = Map::<icu::GeneralCategory>::new(&versions, CATEGORY, 13007);
    let grapheme = Map::<icu::GraphemeClusterBreak>::new(&versions, GRAPHEME, 37);
    let conjunct = Map::<icu::IndicConjunctBreak>::new(&versions, CONJUNCT, 42);
    let modifier_base = Set::new(
        &versions,
        CodePointSetData::new::<icu::EmojiModifierBase>(),
        ("emoji-data.txt", "Emoji_Modifier_Base"),
        2,
    );
    let component = Set::new(
        &versions,
        CodePointSetData::new::<icu::EmojiComponent>(),
        ("emoji-data.txt", "Emoji_Component"),
        0,
    );
    let ignorable = Set::new(
        &versions,
        CodePointSetData::new::<icu::DefaultIgnorableCodePoint>(),
        ("DerivedCoreProperties.txt", "Default_Ignorable_Code_Point"),
        0,
    );
    let white_space = Set::new(
        &versions,
        CodePointSetData::new::<icu::WhiteSpace>(),
        ("PropList.txt", "White_Space"),
        0,
    );
    let cased = Set::new(
        &versions,
        CodePointSetData::new::<icu::Cased>(),
        ("DerivedCoreProperties.txt", "Cased"),
        165,
    );
    let case_ignorable = Set::new(
        &versions,
        CodePointSetData::new::<icu::CaseIgnorable>(),
        ("DerivedCoreProperties.txt", "Case_Ignorable"),
        96,
    );
    for ch in chars() {
        let word = rare_props(ch);
        let cp = u32::from(ch);
        let lb = line_break.get(ch);
        // Bits 27 and 28 hold Cased and Case_Ignorable, and bit 29 letters
        // and numbers; no higher bit is set.
        assert_eq!(word.0 >> 30, 0, "U+{cp:04X}: bits past the layout");
        #[cfg(feature = "css")]
        assert_eq!(
            word.is_letter_or_number(),
            matches!(
                categories.get(ch),
                icu::GeneralCategory::UppercaseLetter
                    | icu::GeneralCategory::LowercaseLetter
                    | icu::GeneralCategory::TitlecaseLetter
                    | icu::GeneralCategory::ModifierLetter
                    | icu::GeneralCategory::OtherLetter
                    | icu::GeneralCategory::DecimalNumber
                    | icu::GeneralCategory::LetterNumber
                    | icu::GeneralCategory::OtherNumber
            ),
            "U+{cp:04X}"
        );
        #[cfg(not(feature = "css"))]
        assert_eq!(word.0 >> 29, 0, "U+{cp:04X}: a CSS bit without `css`");
        // The line byte's class maps back to Line_Break.
        assert_eq!(
            LINE_BREAK_NAMES
                .get(usize::from(super::line_break(ch).get()))
                .copied(),
            line_break_names.get(lb),
            "U+{cp:04X}"
        );
        assert_eq!(word.is_cased(), cased.contains(ch), "U+{cp:04X}");
        assert_eq!(
            word.is_case_ignorable(),
            case_ignorable.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_join_causing(),
            joining.get(ch) == icu::JoiningType::JoinCausing,
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_wide(),
            matches!(
                width.get(ch),
                icu::EastAsianWidth::Wide | icu::EastAsianWidth::Fullwidth
            ),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_mixed_upright(),
            orientation.get(ch) != icu::VerticalOrientation::Rotated,
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_conditional_japanese_starter(),
            lb == icu::LineBreak::ConditionalJapaneseStarter,
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_glue(),
            matches!(
                lb,
                icu::LineBreak::Glue | icu::LineBreak::WordJoiner | icu::LineBreak::ZWJ
            ),
            "U+{cp:04X}"
        );
        // A space separator that glues is GL, a no-break space, as the
        // analysis's classes assume.
        if white_space.contains(ch) && word.is_glue() {
            assert_eq!(lb, icu::LineBreak::Glue, "U+{cp:04X}");
        }
        // Latin-1 stands alone, and its only default-ignorable is the soft
        // hyphen, which analysis classifies without the rare word.
        if cp < 0x100 {
            assert!(word.grapheme_cluster_break().stands_alone(), "U+{cp:04X}");
            assert_eq!(word.is_default_ignorable(), cp == 0xAD, "U+{cp:04X}");
        }
        assert_eq!(
            word.is_emoji_modifier_base(),
            modifier_base.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_emoji_component(),
            component.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.is_default_ignorable(),
            ignorable.contains(ch),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.grapheme_cluster_break(),
            grapheme_cluster_break(grapheme.get(ch)),
            "U+{cp:04X}"
        );
        assert_eq!(
            word.indic_conjunct_break(),
            indic_conjunct_break(conjunct.get(ch)),
            "U+{cp:04X}"
        );
        // The grapheme byte is the two classes and Extended_Pictographic,
        // which the hot word's sweep holds to ICU; the Latin-1 table agrees.
        let expected = GraphemeProps::from_bits(
            word.grapheme_cluster_break() as u8
                | (word.indic_conjunct_break() as u8) << 4
                | u8::from(core_props(ch).is_extended_pictographic()) << 6,
        );
        assert_eq!(word.grapheme_props(), expected, "U+{cp:04X}");
        assert_eq!(grapheme_props(ch), expected, "U+{cp:04X}");
    }
}

#[cfg(feature = "css")]
/// Returns UTR #59's East_Asian_Spacing for `ch`, derived from its text
/// (section 3.1), with the crate's separate classes for marks and digits.
///
/// The ideographic scripts less punctuation, symbols and halfwidth forms are
/// wide; narrow other punctuation, less eight characters, is conditional;
/// narrow letters, marks and decimal digits are narrow.
fn east_asian_spacing(
    ch: char,
    gc: icu::GeneralCategory,
    width: icu::EastAsianWidth,
    script: icu::Script,
    scripts: &ScriptWithExtensionsBorrowed<'_>,
) -> EastAsianSpacing {
    use icu::{EastAsianWidth as W, GeneralCategory as G};
    let group = |gc: G, members: &[G]| members.contains(&gc);
    if group(gc, &[G::NonspacingMark, G::SpacingMark, G::EnclosingMark]) {
        return EastAsianSpacing::Mark;
    }
    if gc == G::Unassigned {
        return EastAsianSpacing::Other;
    }
    let ideographic = [
        icu::Script::Bopomofo,
        icu::Script::Han,
        icu::Script::Hangul,
        icu::Script::Hiragana,
        icu::Script::Katakana,
        icu::Script::KhitanSmallScript,
        icu::Script::Nushu,
        icu::Script::Tangut,
        icu::Script::Yi,
    ];
    let named = ideographic.contains(&script);
    let extended = ideographic.iter().any(|&s| scripts.has_script(ch, s))
        && !matches!(width, W::Neutral | W::Narrow);
    let punctuation = [
        G::ConnectorPunctuation,
        G::DashPunctuation,
        G::OpenPunctuation,
        G::ClosePunctuation,
        G::InitialPunctuation,
        G::FinalPunctuation,
        G::OtherPunctuation,
    ];
    let excluded = width == W::Halfwidth
        || group(gc, &punctuation)
        || gc == G::OtherNumber
        || group(gc, &[G::MathSymbol, G::CurrencySymbol, G::OtherSymbol]);
    if ((named || extended) && !excluded) || ch == '\u{3013}' {
        return EastAsianSpacing::Wide;
    }
    let wide_width = matches!(width, W::Fullwidth | W::Halfwidth | W::Wide);
    if gc == G::OtherPunctuation
        && !wide_width
        && !"\"'*/\u{B7}\u{2020}\u{2021}\u{2026}".contains(ch)
    {
        return EastAsianSpacing::Conditional;
    }
    let letters = [
        G::UppercaseLetter,
        G::LowercaseLetter,
        G::TitlecaseLetter,
        G::ModifierLetter,
        G::OtherLetter,
    ];
    match gc {
        G::DecimalNumber if !wide_width => EastAsianSpacing::NarrowDigit,
        _ if !wide_width && group(gc, &letters) => EastAsianSpacing::Narrow,
        _ => EastAsianSpacing::Other,
    }
}

#[cfg(feature = "css")]
/// Returns CSS Text 4's text-spacing class of `ch`, in Blink's numbering.
fn text_spacing(
    ch: char,
    gc: icu::GeneralCategory,
    width: icu::EastAsianWidth,
) -> TextSpacingClass {
    match ch {
        '\u{2018}' | '\u{201C}' => return TextSpacingClass::OpenQuote,
        '\u{2019}' | '\u{201D}' => return TextSpacingClass::CloseQuote,
        '\u{3000}' | '\u{B7}' | '\u{2027}' | '\u{30FB}' => return TextSpacingClass::Middle,
        '\u{3001}' | '\u{3002}' | '\u{FF0C}' | '\u{FF0E}' => return TextSpacingClass::Dot,
        '\u{FF1A}' => return TextSpacingClass::Colon,
        '\u{FF1B}' => return TextSpacingClass::Semicolon,
        _ => {}
    }
    let full = ('\u{3000}'..='\u{303F}').contains(&ch) || width == icu::EastAsianWidth::Fullwidth;
    match gc {
        icu::GeneralCategory::OpenPunctuation if full => TextSpacingClass::Open,
        icu::GeneralCategory::ClosePunctuation if full => TextSpacingClass::Close,
        icu::GeneralCategory::OpenPunctuation => TextSpacingClass::OpenNarrow,
        icu::GeneralCategory::ClosePunctuation => TextSpacingClass::CloseNarrow,
        _ => TextSpacingClass::Other,
    }
}

#[cfg(feature = "css")]
/// The two fields `text-autospace` and `text-spacing-trim` read agree with
/// their derivations from icu_properties, for every character.
#[test]
fn the_spacing_classes_agree_with_icu() {
    let Some(versions) = versions() else {
        return;
    };
    let categories = Map::<icu::GeneralCategory>::new(&versions, CATEGORY, 13007);
    let width = Map::<icu::EastAsianWidth>::new(&versions, WIDTH, 990);
    let script = CodePointMapData::<icu::Script>::new();
    let scripts = ScriptWithExtensions::new();
    // ICU knows no 18.0 script, so the code points whose Script or
    // Script_Extensions changed are left out: the newly assigned ones, and
    // the three whose Script_Extensions changed (`scripts_agree_with_icu`).
    let script_changes = script_changes(&versions);
    assert_eq!(script_changes.len(), 13010);
    for ch in chars() {
        if script_changes.contains_key(&u32::from(ch)) {
            continue;
        }
        let word = rare_props(ch);
        let (gc, eaw) = (categories.get(ch), width.get(ch));
        assert_eq!(
            word.east_asian_spacing(),
            east_asian_spacing(ch, gc, eaw, script.get(ch), &scripts),
            "U+{:04X}",
            u32::from(ch)
        );
        assert_eq!(
            word.text_spacing(),
            text_spacing(ch, gc, eaw),
            "U+{:04X}",
            u32::from(ch)
        );
    }
    // UTR #59's own examples: an ideograph and kana are wide, Latin letters
    // narrow, U+3001 other, and `!` conditional.
    let spacing = |ch: char| rare_props(ch).east_asian_spacing();
    assert_eq!(spacing('\u{6F22}'), EastAsianSpacing::Wide);
    assert_eq!(spacing('\u{3042}'), EastAsianSpacing::Wide);
    assert_eq!(spacing('\u{D55C}'), EastAsianSpacing::Wide);
    assert_eq!(spacing('A'), EastAsianSpacing::Narrow);
    assert_eq!(spacing('1'), EastAsianSpacing::NarrowDigit);
    assert_eq!(spacing('\u{3001}'), EastAsianSpacing::Other);
    assert_eq!(spacing('\u{FF21}'), EastAsianSpacing::Other);
    assert_eq!(spacing('!'), EastAsianSpacing::Conditional);
    assert_eq!(spacing('\u{301}'), EastAsianSpacing::Mark);
    assert_eq!(spacing('\u{2C7}'), EastAsianSpacing::Wide);
}

#[cfg(feature = "css")]
/// Returns the emphasis class of `ch`, derived from icu_properties as the
/// generator does, in Chrome's order (`Character::CanReceiveTextEmphasis`).
fn emphasis(ch: char, gc: icu::GeneralCategory) -> EmphasisClass {
    use icu::GeneralCategory as G;
    match gc {
        G::Control | G::Format | G::Unassigned | G::LineSeparator | G::ParagraphSeparator => {
            return EmphasisClass::Never;
        }
        G::SpaceSeparator => return EmphasisClass::Space,
        _ => {}
    }
    match ch {
        '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{F0B}' | '\u{F0C}' => {
            return EmphasisClass::Space;
        }
        '#' | '%' | '&' | '@' | '\u{A7}' | '\u{B6}' | '\u{609}' | '\u{60A}' | '\u{66A}'
        | '\u{2030}' | '\u{2031}' | '\u{204A}' | '\u{204B}' | '\u{2053}' | '\u{303D}'
        | '\u{FE5F}' | '\u{FE60}' | '\u{FE6A}' | '\u{FE6B}' | '\u{FF03}' | '\u{FF05}'
        | '\u{FF06}' | '\u{FF20}' => return EmphasisClass::Marked,
        _ => {}
    }
    match gc {
        G::ConnectorPunctuation
        | G::DashPunctuation
        | G::OpenPunctuation
        | G::ClosePunctuation
        | G::InitialPunctuation
        | G::FinalPunctuation
        | G::OtherPunctuation => EmphasisClass::Punctuation,
        G::MathSymbol | G::CurrencySymbol | G::ModifierSymbol | G::OtherSymbol => {
            EmphasisClass::Symbol
        }
        _ => EmphasisClass::Marked,
    }
}

#[cfg(feature = "css")]
/// Returns the first-letter class of `ch`, derived from icu_properties as
/// the generator does (CSS Pseudo-Elements 4, section 2.2.1).
fn first_letter(ch: char, gc: icu::GeneralCategory) -> FirstLetterClass {
    use icu::GeneralCategory as G;
    match gc {
        G::SpaceSeparator if ch != '\u{3000}' => FirstLetterClass::Space,
        G::OpenPunctuation | G::DashPunctuation => FirstLetterClass::Opening,
        G::ConnectorPunctuation
        | G::ClosePunctuation
        | G::InitialPunctuation
        | G::FinalPunctuation
        | G::OtherPunctuation => FirstLetterClass::Punctuation,
        _ => FirstLetterClass::Other,
    }
}

#[cfg(feature = "css")]
/// The class `::first-letter` reads agrees with its derivation from
/// icu_properties, for every character.
#[test]
fn the_first_letter_classes_agree_with_icu() {
    let Some(versions) = versions() else {
        return;
    };
    let categories = Map::<icu::GeneralCategory>::new(&versions, CATEGORY, 13007);
    for ch in chars() {
        assert_eq!(
            rare_props(ch).first_letter(),
            first_letter(ch, categories.get(ch)),
            "U+{:04X}",
            u32::from(ch)
        );
    }
    let class = |ch: char| rare_props(ch).first_letter();
    assert_eq!(class('A'), FirstLetterClass::Other);
    assert_eq!(class('6'), FirstLetterClass::Other);
    assert_eq!(class(' '), FirstLetterClass::Space);
    assert_eq!(class('\u{2009}'), FirstLetterClass::Space);
    assert_eq!(class('\u{3000}'), FirstLetterClass::Other);
    assert_eq!(class('('), FirstLetterClass::Opening);
    assert_eq!(class('-'), FirstLetterClass::Opening);
    assert_eq!(class(')'), FirstLetterClass::Punctuation);
    assert_eq!(class('\u{201C}'), FirstLetterClass::Punctuation);
    assert_eq!(class('_'), FirstLetterClass::Punctuation);
}

#[cfg(feature = "css")]
/// The class `text-emphasis-skip` reads agrees with its derivation from
/// icu_properties, for every character.
#[test]
fn the_emphasis_classes_agree_with_icu() {
    let Some(versions) = versions() else {
        return;
    };
    let categories = Map::<icu::GeneralCategory>::new(&versions, CATEGORY, 13007);
    for ch in chars() {
        assert_eq!(
            rare_props(ch).emphasis(),
            emphasis(ch, categories.get(ch)),
            "U+{:04X}",
            u32::from(ch)
        );
    }
    let class = |ch: char| rare_props(ch).emphasis();
    assert_eq!(class('\u{6F22}'), EmphasisClass::Marked);
    assert_eq!(class('a'), EmphasisClass::Marked);
    assert_eq!(class('%'), EmphasisClass::Marked);
    assert_eq!(class(' '), EmphasisClass::Space);
    assert_eq!(class('\u{3000}'), EmphasisClass::Space);
    assert_eq!(class('\u{3002}'), EmphasisClass::Punctuation);
    assert_eq!(class('\u{300C}'), EmphasisClass::Punctuation);
    assert_eq!(class('+'), EmphasisClass::Symbol);
    assert_eq!(class('\u{200B}'), EmphasisClass::Never);
}

#[test]
fn scripts_agree_with_icu() {
    let Some(versions) = versions() else {
        return;
    };
    let scripts = ScriptWithExtensions::new();
    // 13,007 changed code points are newly assigned: 11,328 Seal, 965
    // Jurchen, 164 Proto_Cuneiform, 161 Latin, 159 Cuneiform, 153 Common and
    // the rest. Three change their Script_Extensions: U+0B83 TAMIL SIGN
    // VISARGA gains Kannada, Malayalam and Telugu, and U+1CF5 and U+1CF6
    // lose Devanagari. The tables hold 18.0's values there.
    let changes = script_changes(&versions);
    assert_eq!(changes.len(), 13010);
    let as_tag = |name: &String| -> [u8; 4] {
        name.as_bytes()
            .try_into()
            .expect("script short names are tags")
    };
    for ch in chars() {
        let word = core_props(ch);
        let cp = u32::from(ch);
        if let Some((old, new)) = changes.get(&cp) {
            let mut theirs: Vec<[u8; 4]> = scripts
                .get_script_extensions_val(ch)
                .iter()
                .map(tag)
                .collect();
            theirs.sort_unstable();
            let old: Vec<[u8; 4]> = old.iter().map(as_tag).collect();
            assert_eq!(theirs, old, "U+{cp:04X}: ICU holds {ICU_VERSION}'s");
            let mut ours: Vec<[u8; 4]> = word.script_extensions().iter().map(|s| s.tag()).collect();
            ours.sort_unstable();
            let new: Vec<[u8; 4]> = new.iter().map(as_tag).collect();
            assert_eq!(ours, new, "U+{cp:04X}");
            continue;
        }
        let expected = scripts.get_script_val(ch);
        let extensions = scripts.get_script_extensions_val(ch);

        assert_eq!(word.script().tag(), tag(expected), "U+{cp:04X}");
        #[allow(deprecated, reason = "checking the numbering is ICU4C's")]
        let value = expected.to_icu4c_value();
        assert_eq!(word.script().get(), value, "U+{cp:04X}");

        let ours = word.script_extensions();
        assert_eq!(ours.len(), extensions.iter().count(), "U+{cp:04X}");
        for (ours, theirs) in ours.iter().zip(extensions.iter()) {
            assert_eq!(ours.tag(), tag(theirs), "U+{cp:04X}");
        }

        // Only a character whose extensions go beyond its script uses a set.
        let alone = extensions.iter().count() == 1 && extensions.iter().next() == Some(expected);
        assert_eq!(word.scripts().single().is_some(), alone, "U+{cp:04X}");
    }
}

#[test]
fn brackets_agree_with_icu() {
    let Some(versions) = versions() else {
        return;
    };
    // 18.0 adds one pair, U+2E62 and U+2E63, newly assigned.
    let old = versions.icu.brackets();
    let new = versions.tables.brackets();
    let added: Vec<&usize> = new.keys().filter(|cp| !old.contains_key(cp)).collect();
    assert_eq!(added, [&0x2E62, &0x2E63]);
    assert!(old.iter().all(|(cp, bracket)| new.get(cp) == Some(bracket)));
    let mirroring = CodePointMapData::<icu::BidiMirroringGlyph>::new();
    for ch in chars() {
        let word = core_props(ch);
        let cp = u32::from(ch);
        let bracket = mirroring.get(ch);
        let expected = match bracket.paired_bracket_type {
            _ if added.contains(&&(cp as usize)) => {
                assert_eq!(bracket.paired_bracket_type, BidiPairedBracketType::None);
                let (pair, kind) = &new[&(cp as usize)];
                let pair = char::from_u32(*pair as u32).expect("a bracket");
                Some(if kind == "o" {
                    (pair, true)
                } else {
                    (ch, false)
                })
            }
            BidiPairedBracketType::Open => Some((
                bracket
                    .mirroring_glyph
                    .expect("an opening bracket names its closing one"),
                true,
            )),
            BidiPairedBracketType::Close => Some((ch, false)),
            _ => None,
        };
        match expected {
            Some((closing, opens)) => {
                assert!(word.bracket_pair().is_some(), "U+{cp:04X}");
                assert_eq!(word.opens_bracket(), opens, "U+{cp:04X}");
                // Analysis reads brackets only for ON clusters.
                assert_eq!(word.bidi_class(), BidiClass::OTHER_NEUTRAL, "U+{cp:04X}");
                assert_eq!(
                    word.closing_bracket(),
                    Some(canonical_bracket(closing)),
                    "U+{cp:04X}"
                );
            }
            None => {
                assert_eq!(word.bracket_pair(), None, "U+{cp:04X}");
                assert!(!word.opens_bracket(), "U+{cp:04X}");
                assert_eq!(word.closing_bracket(), None, "U+{cp:04X}");
            }
        }
    }

    // Each pair has one closing bracket, so comparing ids compares closings.
    for (a, first) in tables::BRACKET_CLOSINGS.iter().enumerate() {
        for second in &tables::BRACKET_CLOSINGS[a + 1..] {
            assert_ne!(first, second, "two pairs close with the same bracket");
        }
    }
}

#[test]
fn canonically_equivalent_brackets_share_a_pair() {
    let angles = ['\u{2329}', '\u{232A}', '\u{3008}', '\u{3009}'].map(core_props);
    let pair = angles[0].bracket_pair();
    assert!(pair.is_some());
    assert!(angles.iter().all(|word| word.bracket_pair() == pair));
    assert_eq!(
        angles.map(CoreProps::opens_bracket),
        [true, false, true, false]
    );
    assert_eq!(angles[0].closing_bracket(), Some('\u{3009}'));

    assert_eq!(
        core_props('(').bracket_pair(),
        core_props(')').bracket_pair()
    );
    assert_ne!(
        core_props('(').bracket_pair(),
        core_props('[').bracket_pair()
    );
    assert_eq!(core_props('(').closing_bracket(), Some(')'));
    assert_eq!(core_props('a').closing_bracket(), None);
}

#[test]
fn the_paragraph_separators_are_the_mandatory_breaks() {
    let separators = [
        '\n', '\u{B}', '\u{C}', '\r', '\u{85}', '\u{2028}', '\u{2029}',
    ];
    for ch in chars() {
        assert_eq!(
            core_props(ch).is_paragraph_separator(),
            separators.contains(&ch),
            "U+{:04X}",
            u32::from(ch)
        );
    }
    // Bidi class B, but they break no line.
    for ch in ['\u{1C}', '\u{1D}', '\u{1E}'] {
        assert_eq!(core_props(ch).bidi_class(), BidiClass::PARAGRAPH_SEPARATOR);
        assert!(!core_props(ch).is_paragraph_separator());
    }
}

#[test]
fn script_ids_name_their_scripts() {
    assert_eq!(ScriptId::COMMON.tag(), *b"Zyyy");
    assert_eq!(ScriptId::INHERITED.tag(), *b"Zinh");
    assert_eq!(ScriptId::UNKNOWN.tag(), *b"Zzzz");
    assert_eq!(ScriptId::HAN.tag(), *b"Hani");
    assert_eq!(ScriptId::HIRAGANA.tag(), *b"Hira");
    assert_eq!(core_props('a').script().tag(), *b"Latn");
    assert_eq!(core_props('.').script(), ScriptId::COMMON);
    assert_eq!(core_props('\u{10FFFF}').script(), ScriptId::UNKNOWN);

    // A plain script is its own extensions.
    assert_eq!(
        core_props('a').scripts().single(),
        Some(core_props('a').script())
    );
    assert_eq!(
        core_props('a').script_extensions(),
        [core_props('a').script()]
    );

    // U+0BEB TAMIL DIGIT FIVE is Tamil, and Grantha uses it too.
    let five = core_props('\u{BEB}');
    assert_eq!(five.scripts().single(), None);
    assert_eq!(five.script().tag(), *b"Taml");
    let tags: [[u8; 4]; 2] = [*b"Taml", *b"Gran"];
    assert!(
        five.script_extensions()
            .iter()
            .all(|s| tags.contains(&s.tag()))
    );
    assert_eq!(five.script_extensions().len(), 2);

    // U+0951 DEVANAGARI STRESS SIGN UDATTA is Inherited, and its extensions
    // leave Inherited out.
    let udatta = core_props('\u{951}');
    assert_eq!(udatta.script(), ScriptId::INHERITED);
    assert!(!udatta.script_extensions().contains(&ScriptId::INHERITED));
    assert!(udatta.script_extensions().len() > 1);
}

#[test]
fn the_script_tables_are_well_formed() {
    let count = usize::from(tables::SCRIPT_COUNT);
    let starts = &tables::SCRIPT_SET_STARTS;
    let runs = &tables::SCRIPT_RUNS;
    let sets = starts.len() - 1;

    assert_eq!(tables::SCRIPT_TAGS.len(), count);
    assert!(count + sets <= 1 << 10, "the script field is ten bits");
    assert!(
        runs[..count]
            .iter()
            .map(|id| usize::from(id.get()))
            .eq(0..count)
    );
    assert_eq!(usize::from(starts[0]), count);
    assert_eq!(usize::from(starts[sets]), runs.len());
    for pair in starts.windows(2) {
        // A Script and at least one extension.
        assert!(pair[1] >= pair[0] + 2);
    }
    assert!(runs.iter().all(|id| usize::from(id.get()) < count));

    // Every tag is ICU's, including for values no character has, but for
    // the scripts 18.0 adds, which ICU4C 78 has not numbered: they take the
    // next values.
    let names = PropertyNamesShort::<icu::Script>::new();
    let added = [*b"Pcun", *b"Seal"];
    let (numbered, new) = tables::SCRIPT_TAGS.split_at(tables::SCRIPT_TAGS.len() - added.len());
    assert_eq!(new, added);
    for (value, ours) in (0..tables::SCRIPT_COUNT).zip(numbered) {
        #[allow(deprecated, reason = "checking the numbering is ICU4C's")]
        let script = icu::Script::from_icu4c_value(value);
        let theirs = names.get(script).map_or(*b"Zzzz", |name| {
            name.as_bytes()
                .try_into()
                .expect("script short names are tags")
        });
        assert_eq!(*ours, theirs, "script {value}");
    }
}
