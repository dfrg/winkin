//! The sweep: every character's two words, and every side table, against
//! icu_properties 2.3.
//!
//! The expected values are mapped from ICU's here, by name, rather than by the
//! generator's code, so that the two have to agree rather than share a
//! mistake. Surrogates are not characters, so no lookup can be asked about
//! one, and the sweep skips them.

use icu_properties::props::{self as icu, BidiPairedBracketType};
use icu_properties::script::{ScriptWithExtensions, ScriptWithExtensionsBorrowed};
use icu_properties::{CodePointMapData, CodePointSetData, PropertyNamesShort};

use super::*;

/// Every character.
fn chars() -> impl Iterator<Item = char> {
    (0..=0x10FFFF_u32).filter_map(char::from_u32)
}

/// ICU4C's `UCharDirection` value for `class`, as the bidi resolver numbers it.
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

/// ICU's ISO 15924 tag for `script`.
fn tag(script: icu::Script) -> [u8; 4] {
    let name = PropertyNamesShort::<icu::Script>::new()
        .get(script)
        .expect("every script ICU hands out has a short name");
    name.as_bytes()
        .try_into()
        .expect("script short names are tags")
}

/// The member of a canonically equivalent pair of brackets the table keeps.
fn canonical_bracket(ch: char) -> char {
    match ch {
        '\u{2329}' => '\u{3008}',
        '\u{232A}' => '\u{3009}',
        other => other,
    }
}

#[test]
fn the_hot_word_agrees_with_icu() {
    let bidi = CodePointMapData::<icu::BidiClass>::new();
    let line_break = CodePointMapData::<icu::LineBreak>::new();
    let regional = CodePointSetData::new::<icu::RegionalIndicator>();
    let pictographic = CodePointSetData::new::<icu::ExtendedPictographic>();
    let presentation = CodePointSetData::new::<icu::EmojiPresentation>();
    let emoji = CodePointSetData::new::<icu::Emoji>();
    let white_space = CodePointSetData::new::<icu::WhiteSpace>();
    let categories = CodePointMapData::<icu::GeneralCategory>::new();
    for ch in chars() {
        let word = core_props(ch);
        let cp = u32::from(ch);
        let lb = line_break.get(ch);
        assert_eq!(word.0 >> 31, 0, "U+{cp:04X}: bits past the layout");
        assert_eq!(
            word.bidi_class(),
            BidiClass(bidi_class(bidi.get(ch))),
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
    let joining = CodePointMapData::<icu::JoiningType>::new();
    let width = CodePointMapData::<icu::EastAsianWidth>::new();
    let orientation = CodePointMapData::<icu::VerticalOrientation>::new();
    let line_break = CodePointMapData::<icu::LineBreak>::new();
    let grapheme = CodePointMapData::<icu::GraphemeClusterBreak>::new();
    let conjunct = CodePointMapData::<icu::IndicConjunctBreak>::new();
    let modifier_base = CodePointSetData::new::<icu::EmojiModifierBase>();
    let component = CodePointSetData::new::<icu::EmojiComponent>();
    let ignorable = CodePointSetData::new::<icu::DefaultIgnorableCodePoint>();
    let white_space = CodePointSetData::new::<icu::WhiteSpace>();
    for ch in chars() {
        let word = rare_props(ch);
        let cp = u32::from(ch);
        let lb = line_break.get(ch);
        // The first-letter class takes bits 24 and 25 (and its own sweep
        // checks it), and none past them is set.
        assert_eq!(word.0 >> 26, 0, "U+{cp:04X}: bits past the layout");
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
        // A space separator that glues is GL, a no-break space, which the
        // analysis's classes read the bit as.
        if white_space.contains(ch) && word.is_glue() {
            assert_eq!(lb, icu::LineBreak::Glue, "U+{cp:04X}");
        }
        // Latin-1 stands alone, and holds no default-ignorable character but
        // the soft hyphen, which the analysis reads no rare word of it for.
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
    }
}

/// UTR #59's East_Asian_Spacing for `ch`, derived by its own words (section 3.1):
/// the ideographic scripts less punctuation, symbols and halfwidth forms are
/// wide; narrow other punctuation, less eight, is conditional; narrow
/// letters, marks and decimal digits are narrow. The crate's two refinements
/// are then applied: marks apart, and digits apart.
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

/// CSS Text 4's text-spacing class of `ch`, in Blink's numbering.
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

/// The two fields `text-autospace` and `text-spacing-trim` read agree with
/// their derivations from icu_properties, for every character.
#[test]
fn the_spacing_classes_agree_with_icu() {
    let categories = CodePointMapData::<icu::GeneralCategory>::new();
    let width = CodePointMapData::<icu::EastAsianWidth>::new();
    let script = CodePointMapData::<icu::Script>::new();
    let scripts = ScriptWithExtensions::new();
    for ch in chars() {
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

/// The emphasis class of `ch`, derived from icu_properties as the generator
/// derives it, in Chrome's order (`Character::CanReceiveTextEmphasis`).
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

/// The first-letter class of `ch`, derived from icu_properties as the
/// generator derives it (CSS Pseudo-Elements 4, section 2.2.1).
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

/// The class `::first-letter` reads agrees with its derivation from
/// icu_properties, for every character.
#[test]
fn the_first_letter_classes_agree_with_icu() {
    let categories = CodePointMapData::<icu::GeneralCategory>::new();
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

/// The class `text-emphasis-skip` reads agrees with its derivation from
/// icu_properties, for every character.
#[test]
fn the_emphasis_classes_agree_with_icu() {
    let categories = CodePointMapData::<icu::GeneralCategory>::new();
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
    let scripts = ScriptWithExtensions::new();
    for ch in chars() {
        let word = core_props(ch);
        let cp = u32::from(ch);
        let expected = scripts.get_script_val(ch);
        let extensions = scripts.get_script_extensions_val(ch);

        assert_eq!(script_tag(word), tag(expected), "U+{cp:04X}");
        #[allow(deprecated, reason = "checking the numbering is ICU4C's")]
        let value = expected.to_icu4c_value();
        assert_eq!(script(word).get(), usize::from(value), "U+{cp:04X}");

        let ours = script_extensions(word);
        assert_eq!(ours.len(), extensions.iter().count(), "U+{cp:04X}");
        for (ours, theirs) in ours.iter().zip(extensions.iter()) {
            assert_eq!(ours.tag(), tag(theirs), "U+{cp:04X}");
        }

        // A set is spent only on a character whose extensions are more than
        // its script.
        let alone = extensions.iter().count() == 1 && extensions.iter().next() == Some(expected);
        assert_eq!(word.scripts().single().is_some(), alone, "U+{cp:04X}");
    }
}

#[test]
fn brackets_agree_with_icu() {
    let mirroring = CodePointMapData::<icu::BidiMirroringGlyph>::new();
    for ch in chars() {
        let word = core_props(ch);
        let cp = u32::from(ch);
        let bracket = mirroring.get(ch);
        let expected = match bracket.paired_bracket_type {
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
                // Analysis asks for a bracket only of an ON cluster.
                assert_eq!(word.bidi_class(), BidiClass::OTHER_NEUTRAL, "U+{cp:04X}");
                assert_eq!(
                    closing_bracket(word),
                    Some(canonical_bracket(closing)),
                    "U+{cp:04X}"
                );
            }
            None => {
                assert_eq!(word.bracket_pair(), None, "U+{cp:04X}");
                assert!(!word.opens_bracket(), "U+{cp:04X}");
                assert_eq!(closing_bracket(word), None, "U+{cp:04X}");
            }
        }
    }

    // One pair, one closing bracket, so comparing ids is comparing closings.
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
    assert_eq!(closing_bracket(angles[0]), Some('\u{3009}'));

    assert_eq!(
        core_props('(').bracket_pair(),
        core_props(')').bracket_pair()
    );
    assert_ne!(
        core_props('(').bracket_pair(),
        core_props('[').bracket_pair()
    );
    assert_eq!(closing_bracket(core_props('(')), Some(')'));
    assert_eq!(closing_bracket(core_props('a')), None);
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
    assert_eq!(script_tag(core_props('a')), *b"Latn");
    assert_eq!(script(core_props('.')), ScriptId::COMMON);
    assert_eq!(script(core_props('\u{10FFFF}')), ScriptId::UNKNOWN);

    // A plain script is its own extensions.
    assert_eq!(
        core_props('a').scripts().single(),
        Some(script(core_props('a')))
    );
    assert_eq!(
        script_extensions(core_props('a')),
        [script(core_props('a'))]
    );

    // U+0BEB TAMIL DIGIT FIVE is Tamil, and Grantha uses it too.
    let five = core_props('\u{BEB}');
    assert_eq!(five.scripts().single(), None);
    assert_eq!(script_tag(five), *b"Taml");
    let tags: [[u8; 4]; 2] = [*b"Taml", *b"Gran"];
    assert!(
        script_extensions(five)
            .iter()
            .all(|s| tags.contains(&s.tag()))
    );
    assert_eq!(script_extensions(five).len(), 2);

    // U+0951 DEVANAGARI STRESS SIGN UDATTA is Inherited, and its extensions
    // leave Inherited out.
    let udatta = core_props('\u{951}');
    assert_eq!(script(udatta), ScriptId::INHERITED);
    assert!(!script_extensions(udatta).contains(&ScriptId::INHERITED));
    assert!(script_extensions(udatta).len() > 1);
}

#[test]
fn the_script_tables_are_well_formed() {
    let count = usize::from(tables::SCRIPT_COUNT);
    let starts = &tables::SCRIPT_SET_STARTS;
    let runs = &tables::SCRIPT_RUNS;
    let sets = starts.len() - 1;

    assert_eq!(tables::SCRIPT_TAGS.len(), count);
    assert!(count + sets <= 1 << 10, "the script field is ten bits");
    assert!(runs[..count].iter().map(|id| id.get()).eq(0..count));
    assert_eq!(usize::from(starts[0]), count);
    assert_eq!(usize::from(starts[sets]), runs.len());
    for pair in starts.windows(2) {
        // A Script and at least one extension.
        assert!(pair[1] >= pair[0] + 2);
    }
    assert!(runs.iter().all(|id| id.get() < count));

    // Every tag is ICU's, including for values no character has.
    let names = PropertyNamesShort::<icu::Script>::new();
    for (value, ours) in (0..tables::SCRIPT_COUNT).zip(&tables::SCRIPT_TAGS) {
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

/// The characters word motion segments a run of whole are exactly those
/// ICU4X's word segmenter hands its dictionaries or its LSTM (the word
/// data's complex property): Line_Break SA always, and Script Han and
/// Hiragana with dictionaries, which without them are stood in for and never
/// reach it. For every character, against icu_properties and against the
/// word data itself, so that an ICU marking others fails here.
#[test]
fn a_complex_character_is_icus() {
    use icu_segmenter::provider::Baked;
    let line_break = CodePointMapData::<icu::LineBreak>::new();
    let scripts = CodePointMapData::<icu::Script>::new();
    let data = Baked::SINGLETON_SEGMENTER_BREAK_WORD_V1;
    for ch in chars() {
        let sa = line_break.get(ch) == icu::LineBreak::ComplexContext;
        let han_or_hiragana = matches!(scripts.get(ch), icu::Script::Han | icu::Script::Hiragana);
        let icus = data.property_table.get32(u32::from(ch)) == data.complex_property;
        assert_eq!(icus, sa || han_or_hiragana, "{ch:?}: ICU's word data");
        let expected = sa || (cfg!(feature = "dictionaries") && han_or_hiragana);
        assert_eq!(segmenters::is_complex(ch), expected, "{ch:?}");
        assert_eq!(
            segmenters::segmenter_char(ch).is_some(),
            !cfg!(feature = "dictionaries") && han_or_hiragana,
            "{ch:?}: stood in for"
        );
    }
}

/// Without dictionaries, what ICU's word segmenter is given in place of
/// each ideograph and hiragana has its Word_Break, so that UAX #29's rules
/// segment it as they would it: every Han, Hiragana and Katakana character,
/// as it is given, against icu_properties. It is no pictograph where the
/// character is none (WB3c); it is as long in UTF-8, so that `capitalize`
/// reads its words where the copy has them; it is a letter or a digit where
/// the character is one, for Chrome's full stops (`word_boundaries`), but
/// where it is Other, which UAX #29 parts from a full stop already; and it
/// never reaches ICU's dictionaries itself. With dictionaries nothing is
/// stood in for.
#[test]
fn a_stand_in_has_its_characters_word_break() {
    use icu_segmenter::provider::Baked;
    let word_break = CodePointMapData::<icu::WordBreak>::new();
    let scripts = CodePointMapData::<icu::Script>::new();
    let pictographic = CodePointSetData::new::<icu::ExtendedPictographic>();
    let data = Baked::SINGLETON_SEGMENTER_BREAK_WORD_V1;
    let mut asked = 0;
    for ch in chars() {
        if !matches!(
            scripts.get(ch),
            icu::Script::Han | icu::Script::Hiragana | icu::Script::Katakana
        ) {
            continue;
        }
        asked += 1;
        let given = segmenters::segmenter_char(ch).unwrap_or(ch);
        if cfg!(feature = "dictionaries") {
            assert_eq!(given, ch, "{ch:?}: stood in for with dictionaries");
            continue;
        }
        let wb = word_break.get(ch);
        assert_eq!(word_break.get(given), wb, "{ch:?} as {given:?}");
        assert_eq!(
            pictographic.contains(given),
            pictographic.contains(ch),
            "{ch:?} as {given:?}"
        );
        assert_eq!(given.len_utf8(), ch.len_utf8(), "{ch:?} as {given:?}");
        if wb != icu::WordBreak::Other {
            assert_eq!(
                given.is_alphanumeric(),
                ch.is_alphanumeric(),
                "{ch:?} as {given:?}"
            );
            assert_eq!(given.is_numeric(), ch.is_numeric(), "{ch:?} as {given:?}");
        }
        assert!(!segmenters::is_complex(given), "{ch:?} as {given:?}");
        assert_ne!(
            data.property_table.get32(u32::from(given)),
            data.complex_property,
            "{ch:?} as {given:?}: ICU's word data"
        );
    }
    assert!(asked > 100_000, "{asked} characters");
}

mod normalize;
