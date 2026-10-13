//! Coverage tests: charset pages, composed and decomposed forms, what
//! the shaper substitutes, the best partial font, and clusters no font
//! draws.

use super::*;

/// Characters on three pages of one charset are covered by it, a character
/// on a page it has none of is not, nor one it lacks on a page it has: the
/// page kept answers only for its own characters.
#[test]
fn coverage_holds_across_charset_pages() {
    let font = fontwich::Font::from_data(pages().build(), 0);
    // A cluster of each, as the run loop asks of a font, none of which has
    // another form to try.
    let covers = |text: &str| coverage::covers(&font, text, &mut None);
    assert!(covers("A\u{100}\u{4E00}B\u{17F}\u{4EFF}"));
    assert!(!covers("A\u{4F00}"));
    assert!(!covers("\u{4E00}a"));
    assert!(!covers("A\u{180}"));

    let mut fixture = fixture_with(&[pages()]);
    let mut layout = Layout::new();
    let listed = [FontFamilyName::named("Test Pages")];
    fixture.span(
        &mut layout,
        &families_style(&listed),
        "A\u{100}\u{4E00}B\u{101}C\u{4E01}",
    );
    assert_eq!(fixture.families(&layout), ["Test Pages"; 7]);
    assert_eq!(text_runs(&layout).len(), 1);
}

/// A cluster a font lacks as written is covered composed, where the font has
/// the precomposed letter and not the mark, or decomposed, where it has the
/// letter and the mark and not the precomposed one, as harfrust's
/// normalizer draws them.
#[test]
fn a_cluster_is_covered_composed_or_decomposed() {
    let mut fixture = fixture_with(&[marks()]);
    let mut layout = Layout::new();
    // Ahem has é and no combining acute.
    let ahem = [FontFamilyName::named("Ahem")];
    fixture.span(&mut layout, &families_style(&ahem), "e\u{301}");
    assert_eq!(fixture.families(&layout), ["Ahem"]);
    // Test Acute Marks has e and the acute, and no é.
    let marked = [
        FontFamilyName::named("Test Acute Marks"),
        FontFamilyName::named("Ahem"),
    ];
    fixture.span(&mut layout, &families_style(&marked), "\u{E9}");
    assert_eq!(fixture.families(&layout), ["Test Acute Marks"]);
}

/// The allowances mirror what the shaper draws: a space separator as the
/// plain space, U+2011 as U+2010, and a missing joiner as nothing. A tab
/// draws nothing and keeps the font before it. A visible character the font
/// lacks still falls back.
#[test]
fn what_the_shaper_substitutes_counts_as_covered() {
    let mut fixture = fixture_with(&[plain()]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Test Plain Hyphen"),
        FontFamilyName::named("Ahem"),
    ];
    let mut style = families_style(&listed);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    fixture.span(
        &mut layout,
        &style,
        "a\u{A0}b\u{2003}c\u{2011}d\te\u{200D}f",
    );
    let families = fixture.families(&layout);
    assert!(
        families.iter().all(|family| *family == "Test Plain Hyphen"),
        "{families:?}"
    );
    fixture.span(&mut layout, &style, "a\u{E9}b");
    assert_eq!(
        fixture.families(&layout),
        ["Test Plain Hyphen", "Ahem", "Test Plain Hyphen"]
    );
}

/// Where no font draws a whole cluster, the one drawing its base wins over
/// one drawing more of its marks, and default-ignorables count for nothing.
#[test]
fn the_best_partial_font_draws_the_base() {
    let mut fixture = fixture_with(&[marks(), only_marks()]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Test Only Marks"),
        FontFamilyName::named("Test Acute Marks"),
    ];
    fixture.span(&mut layout, &families_style(&listed), "a\u{301}\u{302}");
    assert_eq!(fixture.families(&layout), ["Test Acute Marks"]);
}

/// Only the listed families draw a private-use character or a noncharacter:
/// the platform's fallback is not asked for one, as Blink's
/// `FallbackFontForCharacter` asks for none
/// (`font-unicode-PUA-primary-font`). A fallback font that maps U+E000 and
/// U+FDD0 draws neither beside a primary of ASCII, which draws them as its
/// missing glyph. Listed, it draws both.
#[test]
fn a_private_use_character_falls_back_to_no_platform_font() {
    let ascii = TestFont::new("Test Bare Ascii", &[(0x20, 0x7E)]);
    let private = TestFont::new("Test Private Use", &[(0xE000, 0xE0FF), (0xFDD0, 0xFDD0)]);
    let fallback = ahem_fallback().family("Test Private Use");
    let mut fixture = Fixture::new(
        &[ascii, private],
        fallback,
        StageCheck::BuiltWithContext(check),
    );
    let mut layout = Layout::new();
    let ascii_only = [FontFamilyName::named("Test Bare Ascii")];
    fixture.span(
        &mut layout,
        &families_style(&ascii_only),
        "a\u{E000}\u{FDD0}b",
    );
    assert_eq!(fixture.families(&layout), ["Test Bare Ascii"; 4]);
    let both = [
        FontFamilyName::named("Test Bare Ascii"),
        FontFamilyName::named("Test Private Use"),
    ];
    fixture.span(&mut layout, &families_style(&both), "a\u{E000}\u{FDD0}b");
    assert_eq!(
        fixture.families(&layout),
        [
            "Test Bare Ascii",
            "Test Private Use",
            "Test Private Use",
            "Test Bare Ascii"
        ]
    );
}

/// A cluster no font draws any of is set in the style's primary font, which
/// draws `.notdef` for it, as Chrome draws its missing-glyph box.
#[test]
fn a_cluster_nothing_draws_is_set_in_the_primary_font() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    fixture.span(&mut layout, &families_style(&ahem), "a\u{E000}b");
    let primary = fixture.primary(&layout);
    assert_eq!(cluster_font(&layout, 1), Some(primary));
    assert_eq!(fixture.families(&layout), ["Ahem"; 3]);
}
