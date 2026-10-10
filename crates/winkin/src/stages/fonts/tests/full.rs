//! Tests of font selection with the font's full features:
//! - a variable font's axes from `font-weight`, the settings and `opsz`;
//! - a face's descriptors between matching and the element;
//! - a platform font's variations under each config;
//! - faux bold and oblique gated by `font-synthesis`;
//! - features merged in Chrome's order, `chws` among them;
//! - small capitals the font has, synthesized, and falling back as Blink's;
//! - `font-size-adjust`, and an initial letter sized to the lines it spans;
//! - a face's `size-adjust` and overrides;
//! - broken tables drawing the default;
//! - generated text and the fonts it falls back to.

use super::*;
use crate::style::FontGroup;
use crate::tests::TestFallback;

/// `font-weight` sets the weight axis of a variable font, as font matching
/// asks for it; `font-variation-settings` sets it over that, as it does in
/// Chrome and CSS; and a setting for an axis the font lacks, a value past an
/// axis's end or one that is not a number sets nothing beyond the axis.
#[test]
fn font_weight_sets_the_weight_axis_and_the_settings_win() {
    let mut fixture = fixture_with(&[variable()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Three Axes")];
    let mut style = families_style(&family);
    style.font.optical_sizing = FontOpticalSizing::None;
    fixture.span(&mut layout, &style, "ab");
    assert!(
        cluster_coords(&layout, 0).is_empty(),
        "the default instance"
    );

    let mut bold = style;
    bold.font.weight = FontWeight::new(700.0);
    fixture.span(&mut layout, &bold, "ab");
    let coords = cluster_coords(&layout, 0);
    assert_eq!(coords.len(), 3, "one coordinate per axis");
    near(coords[0], 0.6);
    assert_eq!(&coords[1..], [0, 0]);
    assert!(
        !cluster_used_font(&layout, 0)
            .instance
            .as_ref()
            .unwrap()
            .embolden,
        "reached"
    );

    let settings = [FontVariation::new(parlance::Tag::new(b"wght"), 400.0)];
    let mut set = bold;
    set.font.variations = &settings;
    fixture.span(&mut layout, &set, "ab");
    assert!(cluster_coords(&layout, 0).is_empty(), "the setting wins");

    let past = [
        FontVariation::new(parlance::Tag::new(b"wght"), 5000.0),
        FontVariation::new(parlance::Tag::new(b"XXXX"), 1.0),
        FontVariation::new(parlance::Tag::new(b"wdth"), f32::NAN),
    ];
    let past = ComputedStyle {
        font: FontGroup {
            variations: &past,
            ..style.font
        },
        ..style
    };
    fixture.span(&mut layout, &past, "ab");
    assert_eq!(
        cluster_coords(&layout, 0),
        [16384, 0, 0],
        "clamped to the axis"
    );
}

/// `opsz` follows the used size under `font-optical-sizing: auto`, and stays
/// at the font's default under `none`; a setting of `opsz` wins over the
/// size (Blink's `FontCustomPlatformData`).
#[test]
fn the_optical_size_follows_the_used_size() {
    let mut fixture = fixture_with(&[variable()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Three Axes")];
    let at_42 = sized(&family, 42.0);
    fixture.span(&mut layout, &at_42, "ab");
    near(cluster_coords(&layout, 0)[2], 0.5);

    let mut none = at_42;
    none.font.optical_sizing = FontOpticalSizing::None;
    fixture.span(&mut layout, &none, "ab");
    assert!(cluster_coords(&layout, 0).is_empty());

    let settings = [FontVariation::new(parlance::Tag::new(b"opsz"), 72.0)];
    let mut set = at_42;
    set.font.variations = &settings;
    fixture.span(&mut layout, &set, "ab");
    assert_eq!(cluster_coords(&layout, 0)[2], 16384);
}

/// A `@font-face` descriptor's variations come after the axes font matching
/// sets and before the element's own settings, which win where both name an
/// axis, as Chrome merges them (`MergeFontVariationSettingsWithDescriptor`),
/// measured with Chrome 153; its features come before the element's, which
/// win likewise.
#[test]
fn a_faces_descriptors_come_between_matching_and_the_element() {
    let mut document = LayerBuilder::new(Role::Document);
    let bytes = fontwich::FontBytes::from(capitals_variable().build());
    let face = FaceDescriptors {
        variation_settings: vec![FontVariation::new(parlance::Tag::new(b"wght"), 900.0)],
        feature_settings: vec![
            FontFeature::new(parlance::Tag::new(b"ss01"), 1),
            FontFeature::new(parlance::Tag::new(b"onum"), 1),
        ],
        ..FaceDescriptors::default()
    };
    assert!(document.add_face("Brand", face, Some((bytes, 0))).is_ok());
    let collection = collection(&[], ahem_fallback()).with_layer(document.snapshot());
    let mut fixture = fixture_over(collection, &[]);
    let mut layout = Layout::new();
    let brand = [FontFamilyName::named("Brand")];
    let mut style = families_style(&brand);
    style.font.optical_sizing = FontOpticalSizing::None;
    // The descriptor's weight over the one matching sets.
    style.font.weight = FontWeight::new(700.0);
    fixture.span(&mut layout, &style, "ab");
    assert_eq!(cluster_coords(&layout, 0)[0], 16384);
    assert_eq!(cluster_features(&layout, 0), [("onum", 1), ("ss01", 1)]);
    // The element's over the descriptor's.
    let variations = [FontVariation::new(parlance::Tag::new(b"wght"), 400.0)];
    let features = [FontFeature::new(parlance::Tag::new(b"onum"), 0)];
    let set = ComputedStyle {
        font: FontGroup {
            variations: &variations,
            features: &features,
            ..style.font
        },
        ..style
    };
    fixture.span(&mut layout, &set, "ab");
    assert!(cluster_coords(&layout, 0).is_empty());
    assert_eq!(cluster_features(&layout, 0), [("onum", 0), ("ss01", 1)]);
}

/// Every preset applies every variation CSS sets to a platform font too;
/// under [`PlatformFontVariations::MatchingOnly`], which reproduces Chrome on
/// Windows and Linux, a platform font takes the axes font matching sets and
/// nothing else, as Chrome 153 on Windows draws Bahnschrift at `'wdth' 75`
/// as wide as at 100.
#[test]
fn a_platform_font_takes_every_variation_unless_matched() {
    let mut system = LayerBuilder::new(Role::System);
    assert!(system.add_data(AHEM).is_ok());
    assert!(system.add_data(variable().build()).is_ok());
    system.set_fallback_override(TestFallback::new().family("Ahem"));
    let collection = Collection::new().with_layer(system.snapshot());
    let mut fixture = fixture_over(collection, &[]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Three Axes")];
    let settings = [FontVariation::new(parlance::Tag::new(b"wdth"), 50.0)];
    let mut style = sized(&family, 42.0);
    style.font.variations = &settings;
    style.font.weight = FontWeight::new(900.0);
    let mut every = None;
    for config in [
        Config::chrome_windows(),
        Config::chrome_mac(),
        Config::chrome_linux(),
    ] {
        fixture.cx.set_config(config);
        fixture.span(&mut layout, &style, "ab");
        let coords = cluster_coords(&layout, 0);
        assert_eq!(coords[0], 16384);
        assert_eq!(coords[1], -16384, "the width setting");
        near(coords[2], 0.5);
        assert_eq!(
            *every.get_or_insert(coords.clone()),
            coords,
            "every preset alike"
        );
    }
    let mut matched = Config::chrome_windows();
    matched.platform_font_variations = PlatformFontVariations::MatchingOnly;
    fixture.cx.set_config(matched);
    fixture.span(&mut layout, &style, "ab");
    assert_eq!(cluster_coords(&layout, 0), [16384, 0, 0], "weight alone");
}

/// Faux bold and a faux oblique are the instance's, where the font lacks the
/// weight or the slope, and `font-synthesis` turns each off; faux bold
/// widens no advance, as Chrome 153 sets Ahem at 700 as wide as at 400.
#[test]
fn faux_bold_and_oblique_are_gated_by_font_synthesis() {
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let mut style = families_style(&ahem);
    style.font.weight = FontWeight::BOLD;
    style.font.style = FontStyle::Italic;
    fixture.span(&mut layout, &style, "X");
    let drawn = cluster_used_font(&layout, 0).instance.as_ref().unwrap();
    assert!(drawn.embolden);
    assert_eq!(drawn.skew, Some(14.0));
    let mut none = style;
    none.font.synthesis.weight = false;
    none.font.synthesis.style = false;
    fixture.span(&mut layout, &none, "X");
    let drawn = cluster_used_font(&layout, 0).instance.as_ref().unwrap();
    assert!(!drawn.embolden);
    assert_eq!(drawn.skew, None);
    // Its size and metrics are the plain font's.
    assert_eq!(cluster_used_font(&layout, 0).size.to_px(), 16.0);
}

/// The features a style asks for are Chrome's, in Chrome's order, and each
/// tag is kept once, at the value set last: `font-kerning: none` turns off
/// `kern` and `vkrn`; the variants name their features;
/// `font-feature-settings` comes after them and wins; and
/// `font-variant-position`'s `sups` comes after the settings, so it wins
/// over them, as Chrome 153 draws superscripts under `'sups' 0`.
#[test]
fn features_are_merged_in_chromes_order() {
    let mut fixture = fixture_with(&[capitals()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Capitals")];
    let settings = [
        FontFeature::new(parlance::Tag::new(b"tnum"), 0),
        FontFeature::new(parlance::Tag::new(b"sups"), 0),
        FontFeature::new(parlance::Tag::new(b"dlig"), 1),
    ];
    let mut style = families_style(&family);
    style.font.features = &settings;
    style.font.kerning = FontKerning::None;
    style.font.variants = FontVariants::TABULAR_NUMS
        .union(FontVariants::OLDSTYLE_NUMS)
        .union(FontVariants::HISTORICAL_FORMS);
    style.font.variant_position = FontVariantPosition::Super;
    fixture.span(&mut layout, &style, "12");
    assert_eq!(
        cluster_features(&layout, 0),
        [
            ("dlig", 1),
            ("hist", 1),
            ("kern", 0),
            ("onum", 1),
            ("sups", 1),
            ("tnum", 0),
            ("vkrn", 0),
        ]
    );
    // The font's `sups` covers the figures, so they take it.
    assert_eq!(cluster_used_font(&layout, 0).synthesis, UsedSynthesis::None);
}

/// Every style shapes with `chws` under `text-spacing-trim: normal`, as
/// Blink's `default_enable_chws` has it, so a font with contextual
/// half-width spacing collapses adjacent punctuation itself; not under
/// `space-all`, and not where `font-feature-settings` names `chws` or turns
/// the font's own `halt` or `palt` on.
#[test]
fn chws_is_on_unless_trimming_is_off_or_the_settings_say() {
    let mut fixture = fixture_with(&[capitals()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Capitals")];
    let style = families_style(&family);
    fixture.span(&mut layout, &style, "ab");
    assert_eq!(cluster_all_features(&layout, 0), [("chws", 1)]);
    for trim in [TextSpacingTrim::SpaceFirst, TextSpacingTrim::TrimStart] {
        let mut trimmed = families_style(&family);
        trimmed.text.spacing_trim = trim;
        fixture.span(&mut layout, &trimmed, "ab");
        assert_eq!(cluster_all_features(&layout, 0), [("chws", 1)]);
    }
    let mut spaced = families_style(&family);
    spaced.text.spacing_trim = TextSpacingTrim::SpaceAll;
    fixture.span(&mut layout, &spaced, "ab");
    assert_eq!(cluster_all_features(&layout, 0), []);
    let tag = |tag: &[u8; 4], value: u16| FontFeature::new(parlance::Tag::new(tag), value);
    for (settings, expected) in [
        ([tag(b"halt", 1)], [("halt", 1)]),
        ([tag(b"palt", 1)], [("palt", 1)]),
        ([tag(b"chws", 0)], [("chws", 0)]),
        ([tag(b"halt", 0)], [("chws", 1)]),
    ] {
        let mut set = families_style(&family);
        set.font.features = &settings;
        fixture.span(&mut layout, &set, "ab");
        let mut found = cluster_all_features(&layout, 0);
        found.retain(|&(name, _)| name != "halt" || expected[0].0 == "halt");
        assert_eq!(found, expected, "{settings:?}");
    }
}

/// A font with `smcp` draws small capitals with it; the letters keep their
/// case and one run.
#[test]
fn small_caps_take_the_fonts_smcp() {
    let mut fixture = fixture_with(&[capitals()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Capitals")];
    let mut style = families_style(&family);
    style.font.variant_caps = FontVariantCaps::SmallCaps;
    fixture.span(&mut layout, &style, "Small Caps");
    assert_eq!(text_runs(&layout).len(), 1);
    assert_eq!(cluster_features(&layout, 0), [("smcp", 1)]);
    assert_eq!(cluster_used_font(&layout, 0).synthesis, UsedSynthesis::None);
    // A setting of `smcp` wins over the capitals, which Blink prepends.
    let off = [FontFeature::new(parlance::Tag::new(b"smcp"), 0)];
    let mut set = style;
    set.font.features = &off;
    fixture.span(&mut layout, &set, "Small Caps");
    assert_eq!(cluster_features(&layout, 0), [("smcp", 0)]);
}

/// A font without `smcp` has small capitals synthesized, as Blink's
/// `SmallCapsIterator` makes them: the text is divided where letters stop or
/// start changing when uppercased, and those that do are fed uppercased at
/// 0.7 of the computed size, rounded to a pixel. The rest are set as they
/// are, and a mark goes with its letter.
#[test]
fn small_caps_are_synthesized_where_the_font_has_none() {
    let mut fixture = fixture_with(&[plain()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Plain Hyphen")];
    let mut style = sized(&family, 15.0);
    style.font.variant_caps = FontVariantCaps::SmallCaps;
    fixture.span(&mut layout, &style, "Ab Cd 12");
    let synthesized = UsedSynthesis::Synthesized {
        case: CaseMap::Upper,
        glyph_size: TextUnit::from_px(11.0),
        raise: LayoutUnit::ZERO,
    };
    let kinds: Vec<bool> = (0..8)
        .map(|at| cluster_used_font(&layout, at).synthesis == synthesized)
        .collect();
    assert_eq!(
        kinds,
        [false, true, false, false, true, false, false, false],
        "A b ' ' C d ' ' 1 2"
    );
    assert_eq!(
        cluster_used_font(&layout, 1).size.to_px(),
        15.0,
        "metrics' size"
    );
    assert_eq!(text_runs(&layout).len(), 5);
    // Without synthesis, nothing is divided.
    style.font.synthesis.small_caps = false;
    fixture.span(&mut layout, &style, "Ab Cd 12");
    assert_eq!(text_runs(&layout).len(), 1);
    assert_eq!(cluster_used_font(&layout, 1).synthesis, UsedSynthesis::None);
}

/// `all-small-caps` synthesizes every letter, capitals, figures and
/// punctuation among them; `unicase` from a font with `smcp` feeds the
/// capitals lowercased to take it, and the lowercase as they are; and petite
/// capitals fall back to a font's small ones (Blink's `OpenTypeCapsSupport`).
#[test]
fn the_other_capitals_fall_back_as_blink_has_them() {
    let mut fixture = fixture_with(&[plain(), capitals()]);
    let mut layout = Layout::new();
    let plain_family = [FontFamilyName::named("Test Plain Hyphen")];
    let mut all = sized(&plain_family, 20.0);
    all.font.variant_caps = FontVariantCaps::AllSmallCaps;
    fixture.span(&mut layout, &all, "AB12,cd");
    assert_eq!(text_runs(&layout).len(), 1, "every letter alike");
    assert_eq!(
        cluster_used_font(&layout, 0).synthesis,
        UsedSynthesis::Synthesized {
            case: CaseMap::Upper,
            glyph_size: TextUnit::from_px(14.0),
            raise: LayoutUnit::ZERO,
        }
    );

    let capitals_family = [FontFamilyName::named("Test Capitals")];
    let mut unicase = sized(&capitals_family, 20.0);
    unicase.font.variant_caps = FontVariantCaps::Unicase;
    fixture.span(&mut layout, &unicase, "ABcd");
    assert_eq!(
        cluster_used_font(&layout, 0).synthesis.case(),
        CaseMap::Lower
    );
    assert_eq!(cluster_features(&layout, 0), [("smcp", 1)]);
    assert_eq!(cluster_used_font(&layout, 2).synthesis, UsedSynthesis::None);
    assert!(cluster_features(&layout, 2).is_empty());

    let mut petite = unicase;
    petite.font.variant_caps = FontVariantCaps::PetiteCaps;
    fixture.span(&mut layout, &petite, "ABcd");
    assert_eq!(text_runs(&layout).len(), 1);
    assert_eq!(cluster_features(&layout, 0), [("smcp", 1)]);
}

/// `font-size-adjust` holds the primary font's metric at the value asked
/// for, and every font of the style is used at that size, as Chrome sets
/// its fallback fonts at the size the primary's aspect makes (measured with
/// Chrome 153: a CJK fallback behind Courier New at `0.5` is as large as
/// Courier's adjustment makes it). A metric the font does not give counts
/// as one em, as Chrome takes it. A value of zero uses the fonts at zero,
/// as CSS Fonts 4 says and Chrome does; one that is not a finite number of
/// zero or more adjusts nothing.
#[test]
fn font_size_adjust_holds_the_primary_fonts_metric() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let mut style = sized(&ahem, 20.0);
    // Ahem's x-height is 0.8 of its em.
    style.font.size_adjust = FontSizeAdjust::Hold {
        metric: AdjustMetric::ExHeight,
        value: 0.4,
    };
    fixture.span(&mut layout, &style, "a漢");
    assert_eq!(cluster_used_font(&layout, 0).size.to_px(), 10.0);
    assert_eq!(
        cluster_used_font(&layout, 1).size.to_px(),
        10.0,
        "the fallback's"
    );
    assert_eq!(fixture.family(&layout, fixture.primary(&layout)), "Ahem");
    // Ahem has no 水: its ic-width counts as an em.
    style.font.size_adjust = FontSizeAdjust::Hold {
        metric: AdjustMetric::IcWidth,
        value: 0.5,
    };
    fixture.span(&mut layout, &style, "a");
    assert_eq!(cluster_used_font(&layout, 0).size.to_px(), 10.0);
    style.font.size_adjust = FontSizeAdjust::Hold {
        metric: AdjustMetric::ExHeight,
        value: 0.0,
    };
    fixture.span(&mut layout, &style, "a");
    assert_eq!(cluster_used_font(&layout, 0).size.to_px(), 0.0, "zero");
    for value in [f32::NAN, -1.0, f32::INFINITY] {
        style.font.size_adjust = FontSizeAdjust::Hold {
            metric: AdjustMetric::ExHeight,
            value,
        };
        fixture.span(&mut layout, &style, "a");
        assert_eq!(cluster_used_font(&layout, 0).size.to_px(), 20.0, "{value}");
    }
    style.font.size_adjust = FontSizeAdjust::Hold {
        metric: AdjustMetric::ExHeight,
        value: 1.0e30,
    };
    fixture.span(&mut layout, &style, "a");
    assert_eq!(
        cluster_used_font(&layout, 0).size.to_px(),
        10_000.0,
        "sanitized"
    );
}

/// An initial letter is set at the size that makes its over point reach
/// from the first line's own to the under point `size` lines down, as
/// Chrome's `ComputeInitialLetterFont` works it out, whatever its own
/// `font-size` says. Ahem's capitals are 0.8 of its em, so
/// on 10 px lines of 10 px text three lines are capitals of 28, at 35 px;
/// two lines of 30 at 20 px text are capitals of 46, at 57.5; two and a
/// half lines, 23, at 28.75. `hanging` reads the hanging baseline, which
/// Ahem puts at 0.8 of its ascent, 0.64 of its em: 26.4 at 41.25;
/// `ideographic` the em box, 30 at 30. Under `::first-line` the lines are
/// the first line's; and a box with `initial-letter` that is not at the
/// block's start is set as its own size says.
#[test]
fn an_initial_letter_is_sized_to_the_lines_it_spans() {
    use crate::style::{InitialLetter, InitialLetterAlign, LineHeight};
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let size_of = |fixture: &mut Fixture,
                   layout: &mut Layout,
                   root: &ComputedStyle<'_>,
                   first_line: Option<&ComputedStyle<'_>>,
                   letter: InitialLetter| {
        let mut style = *root;
        style.font.size = 13.0;
        style.line.initial_letter = letter;
        let mut b = layout.builder(
            NodeKey(0),
            &ComputedBlockStyle {
                first_line,
                ..ComputedBlockStyle::new(root)
            },
            BuildOptions::default(),
        );
        b.set_first_letter(NodeKey(9), &style, first_line.map(|_| &style));
        b.text(NodeKey(1), "Once upon a time");
        b.finish(&mut fixture.cx);
        if first_line.is_none() {
            check(layout);
        }
        cluster_used_font(layout, 0).size.to_px()
    };
    let letter = |size: f32, align| InitialLetter {
        size,
        sink: 2,
        align,
    };
    let alphabetic = InitialLetterAlign::Alphabetic;
    let ten = sized(&ahem, 10.0);
    let mut twenty = sized(&ahem, 20.0);
    twenty.line.height = LineHeight::Factor(1.5);
    let cases = [
        (&ten, letter(3.0, alphabetic), 35.0),
        (&twenty, letter(2.0, alphabetic), 57.5),
        (&ten, letter(2.5, alphabetic), 28.75),
        (&ten, letter(3.0, InitialLetterAlign::Hanging), 41.25),
        (&ten, letter(3.0, InitialLetterAlign::Ideographic), 30.0),
    ];
    for (root, letter, size) in cases {
        let used = size_of(&mut fixture, &mut layout, root, None, letter);
        assert_eq!(used, size, "{letter:?}");
    }
    // Sized by the first line's lines, on the first line and off it.
    let first = sized(&ahem, 20.0);
    let used = size_of(
        &mut fixture,
        &mut layout,
        &ten,
        Some(&first),
        letter(3.0, alphabetic),
    );
    assert_eq!(used, 70.0);
    // A box with `initial-letter` after the block's first text is no
    // initial letter.
    let mut span = ten;
    span.line.initial_letter = letter(3.0, alphabetic);
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&ten),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "Once ");
    b.open_box(NodeKey(2), &span, None);
    b.text(NodeKey(3), "upon");
    b.close_box();
    b.finish(&mut fixture.cx);
    assert_eq!(cluster_used_font(&layout, 6).size.to_px(), 10.0);
}

/// A face's `size-adjust` scales the size its font is used at, unless the
/// style has `font-size-adjust`, which sets it instead; its metric overrides
/// replace the line's metrics, at the size used, as Chrome 153 sets them.
#[test]
fn a_faces_size_adjust_and_overrides_apply_as_chrome_applies_them() {
    let mut document = LayerBuilder::new(Role::Document);
    let face = FaceDescriptors {
        size_adjust: Some(0.5),
        ascent_override: Some(0.5),
        descent_override: Some(0.25),
        line_gap_override: Some(0.1),
        ..FaceDescriptors::default()
    };
    let bytes = fontwich::FontBytes::from(AHEM);
    assert!(document.add_face("Half", face, Some((bytes, 0))).is_ok());
    let broken = FaceDescriptors {
        size_adjust: Some(f32::NAN),
        ascent_override: Some(-1.0),
        ..FaceDescriptors::default()
    };
    let bytes = fontwich::FontBytes::from(AHEM);
    assert!(
        document
            .add_face("Broken", broken, Some((bytes, 0)))
            .is_ok()
    );
    let collection = collection(&[], ahem_fallback()).with_layer(document.snapshot());
    let mut fixture = fixture_over(collection, &[]);
    let mut layout = Layout::new();
    let half = [FontFamilyName::named("Half")];
    let style = sized(&half, 40.0);
    fixture.span(&mut layout, &style, "X");
    let used = cluster_used_font(&layout, 0);
    assert_eq!(used.size.to_px(), 20.0);
    let metrics = used.metrics;
    assert_eq!(
        (
            metrics.ascent.to_px(),
            metrics.descent.to_px(),
            metrics.line_gap.to_px()
        ),
        (10.0, 5.0, 2.0)
    );
    let mut adjusted = style;
    adjusted.font.size_adjust = FontSizeAdjust::Hold {
        metric: AdjustMetric::ExHeight,
        value: 0.5,
    };
    fixture.span(&mut layout, &adjusted, "X");
    assert_eq!(cluster_used_font(&layout, 0).size.to_px(), 25.0);

    let broken = [FontFamilyName::named("Broken")];
    fixture.span(&mut layout, &sized(&broken, 40.0), "X");
    let used = cluster_used_font(&layout, 0);
    assert_eq!(used.size.to_px(), 40.0);
    assert_eq!(used.metrics.ascent.to_px(), 32.0, "Ahem's own");
}

/// A font whose `fvar` and `GSUB` are cut short, asked for variations,
/// capitals and positions, measures and draws as its default, and nothing
/// panics.
#[test]
fn broken_variation_and_layout_tables_do_not_panic() {
    let mut font = variable();
    font.family = "Test Broken Variable".into();
    font.broken_layout = true;
    let mut bytes = font.build();
    // Every `fvar` axis past the table's end.
    let at = bytes
        .windows(4)
        .position(|tag| tag == b"fvar")
        .expect("an fvar record");
    let offset = u32::from_be_bytes(bytes[at + 8..at + 12].try_into().unwrap()) as usize;
    bytes[offset + 8..offset + 10].copy_from_slice(&u16::MAX.to_be_bytes());
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(AHEM).is_ok());
    let _ = layer.add_data(bytes);
    layer.set_fallback_override(TestFallback::new().family("Ahem"));
    let collection = Collection::new().with_layer(layer.snapshot());
    let mut fixture = fixture_over(collection, &[]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Broken Variable")];
    let settings = [FontVariation::new(parlance::Tag::new(b"wght"), 700.0)];
    let mut style = families_style(&family);
    style.font.variations = &settings;
    style.font.variant_caps = FontVariantCaps::AllSmallCaps;
    style.font.variant_position = FontVariantPosition::Sub;
    let mut config = Config::chrome_windows();
    config.position_synthesis = PositionSynthesis::SynthesizeMissing;
    fixture.cx.set_config(config);
    let built = fixture.span(&mut layout, &style, "Abc 12");
    assert!(built.is_complete());
}

/// A generated text's characters that the style's primary font does not
/// map are set in the fonts the text's would be, a run a font, as Blink's
/// shaper sets a hyphen and an ellipsis in the style's `Font` with its
/// fallback: a `hyphenate-character` of `a`, an arrow and `b`
/// in a font of ASCII alone is three runs, the arrow's in the font the
/// fallback gives it, where the showcase of 2026-09 drew it as the
/// primary's missing glyph; in a style whose own families hold the arrow,
/// in that family; and a string the primary maps all of, one run in it,
/// exactly as before. A string nothing draws any of is set in the primary.
#[test]
fn generated_text_falls_back_as_the_text_does() {
    use crate::style::TextOverflow;
    let ascii = TestFont::new("Test Bare Ascii", &[(0x20, 0x7E)]);
    let arrows = TestFont::new("Test Arrow Block", &[(0x2190, 0x2193)]);
    let other = TestFont::new("Test Other Arrows", &[(0x2192, 0x2192)]);
    let fallback = ahem_fallback().family("Test Arrow Block");
    let mut fixture = Fixture::new(&[ascii, arrows, other], fallback, StageCheck::Built(check));
    let mut layout = Layout::new();
    let runs_of = |fixture: &Fixture, layout: &Layout, kind| -> Vec<(usize, String)> {
        let block = layout
            .content()
            .nodes
            .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
        let generated = layout.fonts().generated();
        let text = generated.text(block, kind).expect("chosen");
        generated
            .runs(text)
            .iter()
            .map(|run| (run.end(), String::from(fixture.family(layout, run.font))))
            .collect()
    };
    let named = |runs: &[(usize, &str)]| -> Vec<(usize, String)> {
        runs.iter()
            .map(|&(end, name)| (end, String::from(name)))
            .collect()
    };
    for (families, string, want) in [
        (
            &["Test Bare Ascii"][..],
            "a\u{2192}b",
            named(&[
                (1, "Test Bare Ascii"),
                (4, "Test Arrow Block"),
                (5, "Test Bare Ascii"),
            ]),
        ),
        (
            &["Test Bare Ascii", "Test Other Arrows"][..],
            "\u{2192}",
            named(&[(3, "Test Other Arrows")]),
        ),
        (
            &["Test Bare Ascii"][..],
            "=",
            named(&[(1, "Test Bare Ascii")]),
        ),
        (
            &["Test Bare Ascii"][..],
            "\u{E000}",
            named(&[(3, "Test Bare Ascii")]),
        ),
    ] {
        let families: Vec<_> = families
            .iter()
            .map(|&name| FontFamilyName::named(name))
            .collect();
        let mut style = families_style(&families);
        style.text.hyphenate_character = Some(string);
        let cut = ComputedBlockStyle {
            text_overflow: TextOverflow::Ellipsis,
            ..ComputedBlockStyle::new(&style)
        };
        let mut b = layout.builder(NodeKey(0), &cut, BuildOptions::default());
        b.text(NodeKey(1), "soft\u{AD}hyphen");
        assert!(b.finish(&mut fixture.cx).is_complete());
        assert_eq!(
            runs_of(&fixture, &layout, Generated::Hyphen),
            want,
            "{string:?}"
        );
        // Three full stops, which the primary maps, in it alone.
        assert_eq!(
            runs_of(&fixture, &layout, Generated::Ellipsis),
            named(&[(3, "Test Bare Ascii")]),
            "{string:?}"
        );
    }
    // A primary with neither a hyphen nor a full stop: `-` and three full
    // stops, as Blink chooses them by what the primary maps, set in the
    // fallback that has them, Ahem.
    let families = [FontFamilyName::named("Test Arrow Block")];
    let style = families_style(&families);
    let cut = ComputedBlockStyle {
        text_overflow: TextOverflow::Ellipsis,
        ..ComputedBlockStyle::new(&style)
    };
    let mut b = layout.builder(NodeKey(0), &cut, BuildOptions::default());
    b.text(NodeKey(1), "soft\u{AD}hyphen");
    assert!(b.finish(&mut fixture.cx).is_complete());
    assert_eq!(
        runs_of(&fixture, &layout, Generated::Hyphen),
        named(&[(1, "Ahem")])
    );
    assert_eq!(
        runs_of(&fixture, &layout, Generated::Ellipsis),
        named(&[(3, "Ahem")])
    );
}

/// The hyphen and the ellipsis say what their style's primary font maps, as
/// Blink's `HyphenString` and `ComputeEllipsisText` choose:
/// U+2010 and U+2026 in Ahem, which has both; U+2010 and three full stops in
/// a font with the hyphen alone; `-` and three full stops in one of ASCII
/// alone; a style's own `hyphenate-character` as written. Each is set in its
/// style's primary font, and only a style that can need one has it: the
/// hyphen where the style's text holds a soft hyphen a line may break
/// after, the ellipsis where a line may be cut, in the block's style.
#[test]
fn generated_text_says_what_the_primary_font_maps() {
    use crate::style::{Hyphens, LineClamp, TextOverflow};
    let ascii = TestFont::new("Test Bare Ascii", &[(0x20, 0x7E)]);
    let mut fixture = fixture_with(&[plain(), ascii]);
    let mut layout = Layout::new();
    let block_text = |layout: &Layout| {
        layout
            .content()
            .nodes
            .text_facts(NodeId::BLOCK, FirstLineVariant::Standard)
    };
    for (family, hyphen, ellipsis) in [
        ("Ahem", GeneratedString::Hyphen, GeneratedString::Ellipsis),
        (
            "Test Plain Hyphen",
            GeneratedString::Hyphen,
            GeneratedString::FullStops,
        ),
        (
            "Test Bare Ascii",
            GeneratedString::HyphenMinus,
            GeneratedString::FullStops,
        ),
    ] {
        let families = [FontFamilyName::named(family)];
        let style = families_style(&families);
        let cut = ComputedBlockStyle {
            text_overflow: TextOverflow::Ellipsis,
            ..ComputedBlockStyle::new(&style)
        };
        let mut b = layout.builder(NodeKey(0), &cut, BuildOptions::default());
        b.text(NodeKey(1), "soft\u{AD}hyphen");
        assert!(b.finish(&mut fixture.cx).is_complete());
        check(&layout);
        let generated = layout.fonts().generated();
        let text = block_text(&layout);
        let primary = primary_id(&layout, text);
        let found = |kind| generated.text(text, kind).expect("chosen");
        assert_eq!(found(Generated::Hyphen).string, hyphen, "{family}");
        assert_eq!(found(Generated::Ellipsis).string, ellipsis, "{family}");
        // What the primary maps, it sets alone, in one run.
        for kind in [Generated::Hyphen, Generated::Ellipsis] {
            let fonts: Vec<_> = generated
                .runs(found(kind))
                .iter()
                .map(|run| Some(run.font))
                .collect();
            assert_eq!(fonts, [primary], "{family} {kind:?}");
        }
    }
    // A style's own hyphen, and a clamp that may cut a line.
    let families = [FontFamilyName::named("Ahem")];
    let mut own = families_style(&families);
    own.text.hyphenate_character = Some("~");
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(3),
        ..ComputedBlockStyle::new(&own)
    };
    let mut b = layout.builder(NodeKey(0), &clamped, BuildOptions::default());
    b.text(NodeKey(1), "soft\u{AD}hyphen");
    assert!(b.finish(&mut fixture.cx).is_complete());
    let text = block_text(&layout);
    let generated = layout.fonts().generated();
    let hyphen = generated.text(text, Generated::Hyphen).expect("chosen");
    let strings = layout.content().lists.hyphen_strings();
    assert_eq!(hyphen.string.text(strings), "~");
    assert!(generated.text(text, Generated::Ellipsis).is_some());
    // Nothing where nothing can need it: no soft hyphen and no cut, and a
    // soft hyphen `hyphens: none` declines.
    let plain_style = families_style(&families);
    fixture.span(&mut layout, &plain_style, "plain text");
    assert_eq!(layout.fonts().generated().iter().count(), 0);
    let mut none = families_style(&families);
    none.text.hyphens = Hyphens::None;
    fixture.span(&mut layout, &none, "soft\u{AD}hyphen");
    assert_eq!(layout.fonts().generated().iter().count(), 0);
}
