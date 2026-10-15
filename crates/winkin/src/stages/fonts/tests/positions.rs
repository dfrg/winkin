//! Tests of `font-variant-position`, by CSS Fonts 4's rule:
//! - a run the feature covers whole takes it, and one it doesn't is
//!   synthesized whole, across boxes of its style;
//! - what counts toward coverage and what doesn't;
//! - a font without the feature, and `FeaturesOnly`;
//! - the synthesized size and raise;
//! - what a feature covers is read once, and a contextual lookup alone
//!   covers nothing.

use super::*;

/// Returns a superscript style at 30 px in `families`.
fn style_super<'a>(families: &'a [FontFamilyName<'a>]) -> ComputedStyle<'a> {
    let mut style = sized(families, 30.0);
    style.font.variant_position = FontVariantPosition::Super;
    style
}

/// Returns how a test font draws a superscript synthesized at 30 px: by its
/// `OS/2`, 650 units big and 480 up, with the text's case kept.
fn super_30px() -> UsedSynthesis {
    UsedSynthesis::Synthesized {
        case: CaseMap::Keep,
        glyph_size: TextUnit::from_px_truncated(19.5),
        raise: LayoutUnit::from_px(14.4),
    }
}

/// Returns whether every cluster of `layout` is set in one used font, drawn
/// as `synthesis` says, shaping with `features`.
fn all_set(layout: &Layout, synthesis: UsedSynthesis, features: &[(&str, u16)]) -> bool {
    let clusters = layout.analysis().clusters.len();
    text_runs(layout).len() == 1
        && (0..clusters).all(|at| {
            cluster_used_font(layout, at).synthesis == synthesis
                && cluster_features(layout, at) == features
        })
}

/// A run whose every character the font's `sups` or `subs` covers takes the
/// feature, drawn as the font draws it, at the run's size, on the baseline.
#[test]
fn a_run_the_feature_covers_whole_takes_it() {
    let mut fixture = fixture_with(&[superscripts()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Superscripts")];
    let mut style = style_super(&family);
    fixture.span(&mut layout, &style, "23");
    assert!(all_set(&layout, UsedSynthesis::None, &[("sups", 1)]));
    style.font.variant_position = FontVariantPosition::Sub;
    fixture.span(&mut layout, &style, "2023");
    assert!(all_set(&layout, UsedSynthesis::None, &[("subs", 1)]));
}

/// By CSS Fonts 4, a run with one character the feature does not cover is
/// synthesized whole, covered characters included. The synthesis uses the
/// glyphs the feature would have replaced, so no feature. The run continues
/// across inline boxes in its style, and ends at text set otherwise.
#[test]
fn a_run_the_feature_does_not_cover_whole_is_synthesized_whole() {
    let mut fixture = fixture_with(&[superscripts()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Superscripts")];
    let style = style_super(&family);
    for text in ["x²", "x2", "1a2", "²3"] {
        fixture.span(&mut layout, &style, text);
        assert!(all_set(&layout, super_30px(), &[]), "{text}");
    }
    // Two boxes of one style are one run: the letter in the second denies
    // the feature to the figures in the first.
    fixture.spans(&mut layout, &style, &[(&style, "12"), (&style, "a")]);
    assert!(all_set(&layout, super_30px(), &[]));
    // Text set otherwise between them makes them two, each decided alone.
    let plain = sized(&family, 30.0);
    fixture.spans(
        &mut layout,
        &plain,
        &[(&style, "12"), (&plain, "a"), (&style, "34")],
    );
    assert_eq!(cluster_used_font(&layout, 0).synthesis, UsedSynthesis::None);
    assert_eq!(cluster_features(&layout, 0), [("sups", 1)]);
    assert!(cluster_features(&layout, 2).is_empty());
    assert_eq!(cluster_features(&layout, 3), [("sups", 1)]);
    assert_eq!(text_runs(&layout).len(), 3);
}

/// A space has a glyph a variant could replace, so one the feature does not
/// cover makes the run synthesized, as Firefox counts it
/// (`gfxFont::SupportsSubSuperscript` asks every character of the run, and
/// WPT's `font-variant-position-05` names the space). What has no glyph to
/// vary is passed over: a default-ignorable, which the shaper hides, and a
/// cluster that draws nothing.
#[test]
fn a_space_counts_and_what_draws_nothing_does_not() {
    let mut fixture = fixture_with(&[superscripts()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Superscripts")];
    let style = style_super(&family);
    fixture.span(&mut layout, &style, "2 3");
    assert!(all_set(&layout, super_30px(), &[]));
    for text in ["2\u{200D}3", "2\u{AD}3", "2\u{200B}3", "\u{2060}23"] {
        fixture.span(&mut layout, &style, text);
        assert!(
            all_set(&layout, UsedSynthesis::None, &[("sups", 1)]),
            "{text:?}"
        );
    }
}

/// A font without the feature under the run's script has the run
/// synthesized, and the used font keeps the line metrics of the plain font.
#[test]
fn a_font_without_the_feature_synthesizes_the_run() {
    let mut fixture = fixture_with(&[plain()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Plain Hyphen")];
    let mut style = style_super(&family);
    fixture.span(&mut layout, &style, "x2");
    assert!(all_set(&layout, super_30px(), &[]));
    let line = cluster_used_font(&layout, 0).metrics.ascent;
    style.font.variant_position = FontVariantPosition::Sub;
    fixture.span(&mut layout, &style, "x2");
    // 140 units down.
    let sub = UsedSynthesis::Synthesized {
        case: CaseMap::Keep,
        glyph_size: TextUnit::from_px_truncated(19.5),
        raise: LayoutUnit::from_px(-4.2),
    };
    assert!(all_set(&layout, sub, &[]));
    style.font.variant_position = FontVariantPosition::Normal;
    fixture.span(&mut layout, &style, "x2");
    assert_eq!(cluster_used_font(&layout, 0).metrics.ascent, line);
}

/// `FeaturesOnly`, which is Chrome's behaviour, and
/// `font-synthesis-position: none` never synthesize. The feature is asked
/// for, and whatever it does not cover is drawn as it is.
#[test]
fn features_only_never_synthesizes() {
    let mut fixture = fixture_with(&[plain(), superscripts()]);
    let mut layout = Layout::new();
    let mut config = Config::chrome_windows();
    config.position_synthesis = PositionSynthesis::FeaturesOnly;
    fixture.cx.set_config(config);
    for name in ["Test Plain Hyphen", "Test Superscripts"] {
        let family = [FontFamilyName::named(name)];
        fixture.span(&mut layout, &style_super(&family), "x2");
        assert!(
            all_set(&layout, UsedSynthesis::None, &[("sups", 1)]),
            "{name}"
        );
    }
    fixture.cx.set_config(Config::chrome_windows());
    let family = [FontFamilyName::named("Test Plain Hyphen")];
    let mut none = style_super(&family);
    none.font.synthesis.position = false;
    fixture.span(&mut layout, &none, "x2");
    assert!(all_set(&layout, UsedSynthesis::None, &[("sups", 1)]));
}

/// A synthesized position is sized and raised by the font's own `OS/2`
/// alone. The `super_sub` config, which is `vertical-align`'s, changes
/// nothing. A size outside a third of an em to one em becomes two thirds.
#[test]
fn a_synthesized_position_reads_the_fonts_metrics_and_not_super_sub() {
    let mut odd = plain();
    odd.family = "Test Odd Scripts".into();
    odd.script_sizes = (100, 1200);
    let mut fixture = fixture_with(&[plain(), odd]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Plain Hyphen")];
    for super_sub in [SuperSubPosition::SizeRatio, SuperSubPosition::FontMetrics] {
        let mut config = Config::chrome_windows();
        config.super_sub = super_sub;
        fixture.cx.set_config(config);
        fixture.span(&mut layout, &style_super(&family), "x2");
        assert!(all_set(&layout, super_30px(), &[]), "{super_sub:?}");
    }
    let odd = [FontFamilyName::named("Test Odd Scripts")];
    fixture.span(&mut layout, &style_super(&odd), "x2");
    assert_eq!(
        cluster_used_font(&layout, 0).synthesis,
        UsedSynthesis::Synthesized {
            case: CaseMap::Keep,
            glyph_size: TextUnit::from_px_truncated(20.0),
            raise: LayoutUnit::from_px(14.4),
        }
    );
}

/// A feature's coverage is read once per font, script and feature, and kept
/// across builds. A new context over the same fonts starts with none and
/// reads the same coverage again.
#[test]
fn a_features_coverage_is_read_once_and_again_alike() {
    let mut fixture = fixture_with(&[superscripts()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Superscripts")];
    let mut style = style_super(&family);
    fixture.span(&mut layout, &style, "23");
    fixture.span(&mut layout, &style, "4567");
    let coverages = &fixture.cx.font_context().coverages();
    assert_eq!(coverages.len(), 1);
    let read = coverages.first().expect("one coverage").clone();
    let digits: Vec<u32> = ('0'..='9').map(u32::from).collect();
    assert_eq!(read.chars().collect::<Vec<_>>(), digits);
    // Latin text asks under `latn`, figures alone under `DFLT`, and `sub`
    // asks for `subs`.
    fixture.span(&mut layout, &style, "2x");
    assert_eq!(fixture.cx.font_context().coverages().len(), 2);
    style.font.variant_position = FontVariantPosition::Sub;
    fixture.span(&mut layout, &style, "23");
    assert_eq!(fixture.cx.font_context().coverages().len(), 3);

    fixture.cx = Context::new(fixture.cx.collection().clone());
    assert_eq!(fixture.cx.font_context().coverages().len(), 0);
    let style = style_super(&family);
    fixture.span(&mut layout, &style, "23");
    let coverages = &fixture.cx.font_context().coverages();
    assert_eq!(coverages.len(), 1);
    assert_eq!(coverages.first(), Some(&read));
    assert!(all_set(&layout, UsedSynthesis::None, &[("sups", 1)]));
}

/// A `sups` reached only through a contextual lookup covers nothing, since no
/// set of characters can say what a context allows. The run is synthesized,
/// though the font draws the figures with the feature.
#[test]
fn a_feature_only_a_context_reaches_covers_nothing() {
    let mut fixture = fixture_with(&[contextual_superscripts()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Contextual Superscripts")];
    fixture.span(&mut layout, &style_super(&family), "23");
    assert!(all_set(&layout, super_30px(), &[]));
    let coverages = &fixture.cx.font_context().coverages();
    assert_eq!(coverages.len(), 1);
    assert!(coverages.first().is_some_and(Charset::is_empty));
}

/// A font over ASCII and the characters of `block` whose features are
/// listed only under the script tags `tags`, its `sups` covering `chars`.
fn superscripts_under(
    family: &str,
    block: (u32, u32),
    tags: &[[u8; 4]],
    chars: &[char],
) -> TestFont {
    let mut font = TestFont::new(family, &[(0x20, 0x7E), block]);
    font.alternates = vec![(*b"sups", chars.iter().map(|&ch| (ch, 300)).collect())];
    font.scripts = tags.to_vec();
    font
}

/// A font that lists its features only under a newer Indic or Myanmar
/// script tag (`dev2`, `dev3`, `mym2`) has them for that script's text, as
/// Chrome's HarfBuzz tries those tags before the first-generation one
/// (`hb_ot_tags_from_script`). One listing them under another script's tag
/// has none for it, and the run is synthesized.
#[test]
fn a_font_with_only_a_newer_indic_script_tag_has_its_features() {
    let devanagari = ['\u{915}', '\u{916}'];
    let myanmar = ['\u{1000}', '\u{1001}'];
    let fonts = [
        superscripts_under("Test Dev2", (0x900, 0x97F), &[*b"dev2"], &devanagari),
        superscripts_under("Test Dev3", (0x900, 0x97F), &[*b"dev3"], &devanagari),
        superscripts_under("Test Mym2", (0x1000, 0x109F), &[*b"mym2"], &myanmar),
        superscripts_under("Test Tml2", (0x900, 0x97F), &[*b"tml2"], &devanagari),
    ];
    let mut fixture = fixture_with(&fonts);
    let mut layout = Layout::new();
    for (name, text, offered) in [
        ("Test Dev2", "\u{915}\u{916}", true),
        ("Test Dev3", "\u{915}\u{916}", true),
        ("Test Mym2", "\u{1000}\u{1001}", true),
        ("Test Tml2", "\u{915}\u{916}", false),
    ] {
        let family = [FontFamilyName::named(name)];
        fixture.span(&mut layout, &style_super(&family), text);
        if offered {
            assert!(
                all_set(&layout, UsedSynthesis::None, &[("sups", 1)]),
                "{name}"
            );
        } else {
            assert!(all_set(&layout, super_30px(), &[]), "{name}");
        }
    }
}
