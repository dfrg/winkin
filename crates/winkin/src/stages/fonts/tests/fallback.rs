//! Primary font and fallback tests: the first installed family, the used
//! size, fallback per cluster and where a fallback run ends, the
//! default language, the Standard font, and divided graphemes.

use super::*;
use crate::style::LengthPercentage;

// The primary font -----------------------------------------------------------

/// The first available font is the first family listed that is installed,
/// and a style naming nothing installed has the user agent's default, which
/// fallback's policy names: Ahem here.
#[test]
fn the_primary_font_is_the_first_listed_family_installed() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Missing"),
        FontFamilyName::named("Test Han Acute"),
    ];
    fixture.span(&mut layout, &families_style(&listed), "abc");
    assert_eq!(
        fixture.family(&layout, fixture.primary(&layout)),
        "Test Han Acute"
    );

    let missing = [FontFamilyName::named("Missing")];
    let built = fixture.span(&mut layout, &families_style(&missing), "abc");
    assert!(built.is_complete());
    assert_eq!(fixture.family(&layout, fixture.primary(&layout)), "Ahem");
    assert_eq!(fixture.families(&layout), ["Ahem"; 3]);
}

/// A style's primary font is at its used size, which is the computed size,
/// sanitized: a size that is not a number or is negative is zero, and one
/// past Chrome's largest is that.
#[test]
fn the_used_size_is_the_computed_size_sanitized() {
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    for (size, used) in [
        (20.0, 20.0),
        (13.5, 13.5),
        (f32::NAN, 0.0),
        (-5.0, 0.0),
        (f32::INFINITY, 0.0),
        (1.0e9, 10_000.0),
    ] {
        fixture.span(&mut layout, &sized(&ahem, size), "X");
        let primary = fixture.primary(&layout);
        let font = used_font(&layout, primary);
        assert_eq!(font.size.to_px(), used, "{size}");
        assert_eq!(cluster_font(&layout, 0), Some(primary));
    }
}

// Fallback -----------------------------------------------------------------

/// Each cluster the style's font lacks falls back on its own, to the first
/// font of the chain that has it.
#[test]
fn each_cluster_falls_back_on_its_own() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    fixture.span(&mut layout, &families_style(&ahem), "A漢B");
    assert_eq!(
        fixture.families(&layout),
        ["Ahem", "Test Han Acute", "Ahem"]
    );
    assert_eq!(text_runs(&layout).len(), 3);
}

/// Every cluster is owed to the first font that has it: the Han fallback
/// has digits and spaces too, and takes none of them from Ahem, so its run
/// ends where Ahem covers again.
#[test]
fn a_fallback_run_ends_where_an_earlier_font_covers_again() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let mut style = families_style(&ahem);
    style.text.language = Language::parse("ja").ok();
    fixture.span(&mut layout, &style, "漢字 12漢字");
    assert_eq!(
        fixture.families(&layout),
        [
            "Test Han Acute",
            "Test Han Acute",
            "Ahem",
            "Ahem",
            "Ahem",
            "Test Han Acute",
            "Test Han Acute"
        ]
    );
}

/// A platform's tables as fontwich reads them, for the default language:
/// every generic is Test Plain Hyphen where the language is not a Han tradition's
/// and Test Han Acute where it is, and Test Han Acute is Han's script font.
struct Generics;

impl FallbackOverride for Generics {
    fn families(&self, key: &FallbackKey, collection: &Collection, out: &mut Vec<Family>) {
        let han = match key.generic() {
            Some((_, bucket)) => matches!(
                bucket,
                GenericBucket::Hans
                    | GenericBucket::Hant
                    | GenericBucket::Jpan
                    | GenericBucket::Kore
            ),
            None => key.script() == Some(Script::from_bytes(*b"Hani")),
        };
        if han {
            out.extend(collection.fallback_family("Test Han Acute"));
        } else if key.generic().is_some() {
            out.extend(collection.fallback_family("Test Plain Hyphen"));
        }
    }
}

/// Text whose style names no language chooses its fonts in the config's
/// default language, as Chrome chooses in its default locale: under
/// English, every preset's, a Han run's `serif` is the Latin one, which
/// takes the run's space, and the ideographs fall back from it; under `und`
/// the run chooses by its own script, whose `serif` sets all of it; and a
/// style's own language wins over the default. The primary font follows
/// the default too.
#[test]
fn a_style_naming_no_language_chooses_its_fonts_in_the_default() {
    let mut layer = LayerBuilder::new(Role::Application);
    for font in [plain(), han()] {
        assert!(layer.add_data(font.build()).is_ok());
    }
    layer.set_fallback_override(Generics);
    let families = [
        String::from("Test Plain Hyphen"),
        String::from("Test Han Acute"),
    ];
    let mut fixture = fixture_over(Collection::new().with_layer(layer.snapshot()), &families);
    let serif = [FontFamilyName::Generic(GenericFamily::Serif)];
    let english = Language::parse("en").expect("a tag");
    let japanese = Language::parse("ja").expect("a tag");
    let mut layout = Layout::new();
    for (default, own, expected, primary) in [
        (
            english,
            None,
            ["Test Han Acute", "Test Plain Hyphen", "Test Han Acute"],
            "Test Plain Hyphen",
        ),
        (
            Language::UND,
            None,
            ["Test Han Acute"; 3],
            "Test Plain Hyphen",
        ),
        (
            english,
            Some(japanese),
            ["Test Han Acute"; 3],
            "Test Han Acute",
        ),
        (japanese, None, ["Test Han Acute"; 3], "Test Han Acute"),
    ] {
        let mut config = Config::chrome_windows();
        config.default_language = default;
        fixture.cx.set_config(config);
        let mut style = families_style(&serif);
        style.text.language = own;
        fixture.span(&mut layout, &style, "漢 字");
        assert_eq!(fixture.families(&layout), expected, "{default:?} {own:?}");
        assert_eq!(
            fixture.family(&layout, fixture.primary(&layout)),
            primary,
            "{default:?} {own:?}"
        );
    }
}

/// Chrome's generic settings on Windows, in miniature, as measured in Chrome.
///
/// Simplified Chinese `serif` is SimSun and `sans-serif` Microsoft YaHei, its `cursive` KaiTi and
/// Traditional `monospace` MingLiU are not installed, and the Common
/// `serif` is Times New Roman and `monospace` Consolas. Every script's
/// fallback is Test Kana, then Test Courier, standing in for the system's.
struct StandardFonts;

impl FallbackOverride for StandardFonts {
    fn families(&self, key: &FallbackKey, collection: &Collection, out: &mut Vec<Family>) {
        let names: &[&str] = match key.generic() {
            Some((generic, bucket)) => match (generic, bucket) {
                (GenericFamily::Serif, GenericBucket::Hans) => &["Test SimSun"],
                (GenericFamily::SansSerif, GenericBucket::Hans) => &["Test YaHei"],
                (GenericFamily::Cursive, GenericBucket::Hans) => &["Test KaiTi"],
                (GenericFamily::Serif | GenericFamily::SansSerif, GenericBucket::Hant) => {
                    &["Test JhengHei"]
                }
                (GenericFamily::Monospace, GenericBucket::Hant) => &["Test MingLiU"],
                (
                    GenericFamily::Monospace,
                    GenericBucket::Hans | GenericBucket::Jpan | GenericBucket::Kore,
                ) => &["Test Times"],
                (GenericFamily::Monospace, _) => &["Test Consolas"],
                _ => &["Test Times"],
            },
            None => match key.standard() {
                Some(GenericBucket::Hans) => &["Test YaHei"],
                Some(GenericBucket::Hant) => &["Test JhengHei"],
                Some(_) => &["Test Times"],
                None if key.presentation().is_some() => &[],
                None => &["Test Kana", "Test Courier"],
            },
        };
        out.extend(
            names
                .iter()
                .filter_map(|name| collection.fallback_family(name)),
        );
    }
}

/// After every `font-family` list, before any system fallback, Chrome tries
/// the Standard font of the content language (`font_fallback_list.cc:177–187`),
/// which is not `serif`. As the probes measured on Windows: `・` under
/// `zh-CN` `serif` draws in Microsoft YaHei, the Simplified Standard, where
/// SimSun lacks it and the Katakana font has it; a generic whose font is
/// not installed (Simplified `cursive` KaiTi, Traditional `monospace`
/// MingLiU) shows the Standard font, as the style's primary font too; and
/// `monospace` Hebrew under English draws in Times New Roman, the Common
/// Standard, where Consolas lacks it, not in the Courier New fallback has.
/// The initial `font-family` is the Standard font alone: Microsoft YaHei
/// under `zh-CN`, where `serif` would be SimSun.
#[test]
fn the_standard_font_follows_every_list() {
    let fonts = [
        TestFont::new("Test SimSun", &[(0x20, 0x20), (0x4E00, 0x9FFF)]),
        TestFont::new(
            "Test YaHei",
            &[(0x20, 0x20), (0x30FB, 0x30FB), (0x4E00, 0x9FFF)],
        ),
        TestFont::new("Test JhengHei", &[(0x20, 0x20), (0x4E00, 0x9FFF)]),
        TestFont::new("Test Kana", &[(0x20, 0x20), (0x30A0, 0x30FF)]),
        TestFont::new("Test Consolas", &[(0x20, 0x7E)]),
        TestFont::new("Test Times", &[(0x20, 0x7E), (0x5D0, 0x5EA)]),
        TestFont::new("Test Courier", &[(0x20, 0x7E), (0x5D0, 0x5EA)]),
    ];
    let mut layer = LayerBuilder::new(Role::Application);
    for font in &fonts {
        assert!(layer.add_data(font.build()).is_ok());
    }
    layer.set_fallback_override(StandardFonts);
    let families: Vec<String> = fonts.iter().map(|font| font.family.clone()).collect();
    let mut fixture = fixture_over(Collection::new().with_layer(layer.snapshot()), &families);
    let hans = Language::parse("zh-CN").expect("a tag");
    let hant = Language::parse("zh-TW").expect("a tag");
    let english = Language::parse("en").expect("a tag");
    let serif = [FontFamilyName::Generic(GenericFamily::Serif)];
    let cursive = [FontFamilyName::Generic(GenericFamily::Cursive)];
    let monospace = [FontFamilyName::Generic(GenericFamily::Monospace)];
    let mut layout = Layout::new();
    let initial: &[FontFamilyName<'_>] = &[];
    for (list, language, text, expected, primary) in [
        (
            &serif[..],
            hans,
            "漢・",
            &["Test SimSun", "Test YaHei"][..],
            "Test SimSun",
        ),
        (&cursive, hans, "漢・", &["Test YaHei"; 2], "Test YaHei"),
        (&monospace, hant, "漢", &["Test JhengHei"], "Test JhengHei"),
        (initial, hans, "漢", &["Test YaHei"], "Test YaHei"),
        (initial, english, "a", &["Test Times"], "Test Times"),
        (
            &monospace,
            english,
            "a\u{5D0}",
            &["Test Consolas", "Test Times"],
            "Test Consolas",
        ),
    ] {
        let mut style = families_style(list);
        style.text.language = Some(language);
        fixture.span(&mut layout, &style, text);
        assert_eq!(fixture.families(&layout), expected, "{list:?} {language:?}");
        assert_eq!(
            fixture.family(&layout, fixture.primary(&layout)),
            primary,
            "{list:?} {language:?}"
        );
    }
}

/// The same font at two sizes is two used fonts and two runs; the same font
/// at one size in two boxes is one used font and one run, since a box that
/// only paints does not end shaping.
#[test]
fn the_same_font_at_two_sizes_is_two_runs() {
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let (small, large) = (sized(&ahem, 16.0), sized(&ahem, 20.0));
    fixture.spans(
        &mut layout,
        &small,
        &[(&small, "ab"), (&small, "cd"), (&large, "ef")],
    );
    let runs = ordered_runs(&layout);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[1].start().get(), 4);
    let sizes: Vec<f32> = runs
        .iter()
        .map(|run| used_font(&layout, run.font).size.to_px())
        .collect();
    assert_eq!(sizes, [16.0, 20.0]);
}

/// Text under a `letter-spacing` that is not nothing is set in an instance
/// with the common ligatures and the contextual alternates off, `liga`,
/// `clig` and `calt` at 0, as Blink's `FontFeatureRange` turns them off;
/// other text in the same font keeps the
/// shaper's defaults. So the two are two used fonts and two runs, its
/// primary font the spaced instance too, and a spacing too small to round
/// to anything turns nothing off.
#[test]
fn letter_spacing_turns_the_common_ligatures_off() {
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let plain = sized(&ahem, 16.0);
    let mut spaced = plain;
    spaced.text.letter_spacing = LengthPercentage::from_px(1.5);
    let mut tiny = plain;
    tiny.text.letter_spacing = LengthPercentage::from_px(1e-9);
    fixture.spans(
        &mut layout,
        &plain,
        &[(&plain, "ab"), (&spaced, "cd"), (&tiny, "ef")],
    );
    // Every one but the `chws` every style asks for under `text-spacing-trim:
    // normal`.
    let features = |font: UsedFontId| {
        let used = used_font(&layout, font);
        let drawn = used.instance.as_ref().expect("an instance");
        let chws = parlance::Tag::new(b"chws");
        let mut list = drawn.features.to_vec();
        list.retain(|feature| feature.tag != chws);
        list
    };
    let runs = ordered_runs(&layout);
    assert_eq!(runs.len(), 3, "{runs:?}");
    assert!(features(runs[0].font).is_empty());
    let off: Vec<(parlance::Tag, u16)> = features(runs[1].font)
        .iter()
        .map(|feature| (feature.tag, feature.value))
        .collect();
    // Blink's `FontFeatureRange::FromFontDescription`: the common
    // ligatures and the contextual alternates off, kept in tag order.
    assert_eq!(
        off,
        [
            (parlance::Tag::new(b"calt"), 0),
            (parlance::Tag::new(b"clig"), 0),
            (parlance::Tag::new(b"liga"), 0),
        ]
    );
    assert!(features(runs[2].font).is_empty());
    let facts = &layout.content().facts;
    let spaced_text = facts
        .text_ids()
        .find(|&id| facts.shaping(facts.text(id).shaping).letter == TextUnit::from_px(1.5))
        .expect("the spaced text");
    let primary = primary_id(&layout, spaced_text).expect("fonts");
    assert_eq!(features(primary).len(), 3);
}

/// A cluster that draws nothing -- an atomic inline, a `<br>`, a lone joiner
/// -- takes the font of the text before it, so it neither ends a fallback
/// run nor sends the walk to fallback for a character no font maps.
#[test]
fn a_cluster_that_draws_nothing_keeps_the_font_before_it() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let style = families_style(&ahem);
    let mut b = layout.builder(
        &mut fixture.cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&style),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "漢");
    b.atomic(NodeKey(2), &style, None, BoxSize::default());
    b.text(NodeKey(3), "\u{2060}字");
    b.line_break(NodeKey(4));
    b.text(NodeKey(5), "A");
    b.finish();
    check(&layout, &fixture.cx);
    assert_eq!(
        fixture.families(&layout),
        [
            "Test Han Acute",
            "Test Han Acute",
            "Test Han Acute",
            "Test Han Acute",
            "Test Han Acute",
            "Ahem"
        ]
    );
}

/// A grapheme a box boundary divides where nothing that shapes differs --
/// a bare span, a span that only paints -- is chosen for whole, as Chrome
/// chooses. The first font covering all of it
/// sets every part. Test Plain Hyphen has the `e` and not the acute, so the
/// grapheme goes to Test Acute Marks, which has both, and not the `e` alone to Test
/// Plain with the mark sent after it.
#[test]
fn a_grapheme_divided_where_nothing_shapes_differently_is_chosen_whole() {
    let mut fixture = fixture_with(&[plain(), marks()]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Test Plain Hyphen"),
        FontFamilyName::named("Test Acute Marks"),
    ];
    let style = families_style(&listed);
    fixture.spans(&mut layout, &style, &[(&style, "e"), (&style, "\u{301}")]);
    assert_eq!(layout.analysis().clusters.len(), 2, "divided");
    assert_eq!(
        fixture.families(&layout),
        ["Test Acute Marks", "Test Acute Marks"]
    );
    assert_eq!(text_runs(&layout).len(), 1, "one used font");

    let painted = ComputedStyle {
        paints: true,
        ..style
    };
    fixture.spans(&mut layout, &style, &[(&style, "e"), (&painted, "\u{301}")]);
    assert_eq!(
        fixture.families(&layout),
        ["Test Acute Marks", "Test Acute Marks"]
    );
    assert_eq!(text_runs(&layout).len(), 1, "one used font");
}

/// A grapheme divided into three parts by two box boundaries, where nothing
/// shapes differently, is chosen for whole too: the unit reaches past the
/// item after its first part's into the one after that. Test Acute Marks has the
/// `e` and the acute and not the circumflex, so the grapheme goes to the
/// second family, which has all three, and not its first two parts to Test
/// Marks.
#[test]
fn a_grapheme_divided_twice_is_chosen_whole() {
    let all = TestFont::new(
        "Test Both Marks",
        &[(0x20, 0x20), (0x65, 0x65), (0x301, 0x302)],
    );
    let mut fixture = fixture_with(&[marks(), all]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Test Acute Marks"),
        FontFamilyName::named("Test Both Marks"),
    ];
    let style = families_style(&listed);
    fixture.spans(
        &mut layout,
        &style,
        &[(&style, "e"), (&style, "\u{301}"), (&style, "\u{302}")],
    );
    assert_eq!(layout.analysis().clusters.len(), 3, "divided twice");
    assert_eq!(
        fixture.families(&layout),
        ["Test Both Marks", "Test Both Marks", "Test Both Marks"]
    );
}

/// A grapheme a box boundary divides where a shaping property changes is two
/// units, as in Chrome: the later part chooses by its own style, and a mark
/// may leave its base. Another family sets the acute in that family's font;
/// another size sets it in the same font at its own size.
#[test]
fn a_grapheme_divided_by_a_shaping_property_is_chosen_for_by_part() {
    let mut fixture = fixture_with(&[han(), marks()]);
    let mut layout = Layout::new();
    let marked = [FontFamilyName::named("Test Acute Marks")];
    let hani = [FontFamilyName::named("Test Han Acute")];
    let first = sized(&marked, 16.0);
    let other_family = sized(&hani, 16.0);
    fixture.spans(
        &mut layout,
        &first,
        &[(&first, "a"), (&other_family, "\u{301}")],
    );
    assert_eq!(layout.analysis().clusters.len(), 2, "divided");
    assert_eq!(
        fixture.families(&layout),
        ["Test Acute Marks", "Test Han Acute"]
    );

    let other_size = sized(&marked, 30.0);
    fixture.spans(
        &mut layout,
        &first,
        &[(&first, "a"), (&other_size, "\u{301}")],
    );
    assert_eq!(
        fixture.families(&layout),
        ["Test Acute Marks", "Test Acute Marks"]
    );
    assert_eq!(text_runs(&layout).len(), 2, "one font at two sizes");
}
