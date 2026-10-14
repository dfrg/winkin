//! Fonts in full: variable fonts, synthesized small capitals, features and
//! `trak` allocate nothing warm.

use super::test_fonts::{self, TestAxis, TestFont, ahem_fallback};
use super::{count_allocations, text};
use fontwich::{Collection, FaceDescriptors, FontBytes, LayerBuilder, Role};
use winkin::config::{Config, PositionSynthesis};
use winkin::style::LengthPercentage;
use winkin::style::{
    AdjustMetric, ComputedStyle, FontFamilyName, FontFeature, FontGroup, FontSizeAdjust,
    FontVariantCaps, FontVariantPosition, FontVariants, FontVariation, FontWeight, Tag, TextGroup,
};
use winkin::{Area, BuildOptions, ComputedBlockStyle, Context, Layout, NoExclusions, NodeKey};

/// Ahem; a variable font whose weight and optical size move its
/// advances, with small capitals, figures and superscript figures of its
/// own; a font with none, whose capitals and positions are synthesized;
/// and, in a document layer, a face of the variable font with every
/// descriptor.
fn collection() -> Collection {
    let mut variable = TestFont::new("Test Variable", &[(0x20, 0x7E)]);
    variable.axes = vec![
        TestAxis {
            tag: *b"wght",
            min: 100.0,
            default: 400.0,
            max: 900.0,
            delta: 200,
        },
        TestAxis {
            tag: *b"opsz",
            min: 6.0,
            default: 12.0,
            max: 72.0,
            delta: 50,
        },
    ];
    variable.alternates = vec![
        (*b"smcp", ('a'..='z').map(|ch| (ch, 250)).collect()),
        (*b"onum", ('0'..='9').map(|ch| (ch, 300)).collect()),
        (*b"sups", ('0'..='9').map(|ch| (ch, 280)).collect()),
    ];
    variable.kerning = vec![('A', 'V', -100)];
    let plain = TestFont::new("Test Plain", &[(0x20, 0x7E), (0xDF, 0xDF)]);
    let mut document = LayerBuilder::new(Role::Document);
    let face = FaceDescriptors {
        variation_settings: vec![FontVariation::new(Tag::new(b"wght"), 600.0)],
        feature_settings: vec![FontFeature::new(Tag::new(b"onum"), 1)],
        size_adjust: Some(0.9),
        ascent_override: Some(0.8),
        descent_override: Some(0.2),
        line_gap_override: Some(0.1),
        ..FaceDescriptors::default()
    };
    let bytes = FontBytes::from(variable.build());
    assert!(document.add_face("Brand", face, Some((bytes, 0))).is_ok());
    test_fonts::collection(&[variable, plain], ahem_fallback()).with_layer(document.snapshot())
}

/// Builds `text` in paragraphs, each set eleven ways:
/// - the variable font at four weights, at a size its optical axis follows,
///   and in its own small capitals with figures and settings;
/// - the plain font in synthesized small capitals, all small capitals, and
///   a synthesized superscript;
/// - the variable font's superscript over text its `sups` does not cover,
///   synthesized, and over figures alone, which take it;
/// - the document's face;
/// - letter-spaced text under `font-size-adjust`.
fn prose(layout: &mut Layout, cx: &mut Context, text: &str) {
    let variable = [FontFamilyName::named("Test Variable")];
    let plain = [FontFamilyName::named("Test Plain")];
    let brand = [FontFamilyName::named("Brand")];
    let settings = [FontFeature::new(Tag::new(b"liga"), 0)];
    let variations = [FontVariation::new(Tag::new(b"opsz"), 30.0)];
    let root = ComputedStyle {
        font: FontGroup {
            families: &variable,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let weight = |value: f32| ComputedStyle {
        font: FontGroup {
            weight: FontWeight::new(value),
            size: 14.0 + value / 100.0,
            ..root.font
        },
        ..root
    };
    let capitals = ComputedStyle {
        font: FontGroup {
            features: &settings,
            variations: &variations,
            variant_caps: FontVariantCaps::SmallCaps,
            variants: FontVariants::OLDSTYLE_NUMS.union(FontVariants::TABULAR_NUMS),
            ..root.font
        },
        ..root
    };
    let synthesized = ComputedStyle {
        font: FontGroup {
            families: &plain,
            variant_caps: FontVariantCaps::SmallCaps,
            ..root.font
        },
        ..root
    };
    let all = ComputedStyle {
        font: FontGroup {
            variant_caps: FontVariantCaps::AllSmallCaps,
            ..synthesized.font
        },
        ..synthesized
    };
    let superscript = ComputedStyle {
        font: FontGroup {
            families: &plain,
            variant_position: FontVariantPosition::Super,
            ..root.font
        },
        ..root
    };
    let figures = ComputedStyle {
        font: FontGroup {
            variant_position: FontVariantPosition::Super,
            ..root.font
        },
        ..root
    };
    let mut face = root;
    face.font.families = &brand;
    let adjusted = ComputedStyle {
        font: FontGroup {
            size_adjust: FontSizeAdjust::Hold {
                metric: AdjustMetric::ExHeight,
                value: 0.45,
            },
            ..root.font
        },
        text: TextGroup {
            letter_spacing: LengthPercentage::from_px(1.5),
            ..root.text
        },
        ..root
    };
    let styles = [
        weight(300.0),
        weight(500.0),
        weight(700.0),
        weight(900.0),
        capitals,
        synthesized,
        all,
        superscript,
        figures,
        face,
        adjusted,
    ];
    let mut b = layout.builder(
        cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    for paragraph in text.split('\n') {
        for style in &styles {
            b.open_box(next(), style, None);
            b.text(next(), paragraph);
            b.close_box();
        }
        b.open_box(next(), &figures, None);
        b.text(next(), "2024");
        b.close_box();
        b.line_break(next());
    }
    assert!(b.finish().is_complete());
}

/// Building and breaking again allocate nothing with the full range of font
/// features.
///
/// Covers variable fonts at several weights and optical sizes, features,
/// real and synthesized small capitals (uppercased at shaping, `ß` as two
/// glyphs), synthesized and real superscripts, a face's descriptors and
/// `font-size-adjust`. The instances, their coordinates and features, each
/// font's coverage, the position runs and the used fonts' arenas keep
/// their capacity.
#[test]
fn fonts_in_full_allocate_nothing_warm() {
    let line = "Small Caps AVAIL office 1,234 straße ffi";
    let mut cx = Context::new(collection());
    let mut config = Config::chrome_windows();
    config.position_synthesis = PositionSynthesis::SynthesizeMissing;
    cx.set_config(config);
    let mut layout = Layout::new();
    let same = text(line, 6);
    let half: String = line.chars().take(line.chars().count() / 2).collect();
    let similar = text(&half, 5);
    let cold = count_allocations(|| prose(&mut layout, &mut cx, &same));
    assert!(cold > 0, "a cold context grows");
    let warm = count_allocations(|| prose(&mut layout, &mut cx, &same));
    assert_eq!(warm, 0, "rebuilding the same content allocated");
    let warm = count_allocations(|| prose(&mut layout, &mut cx, &similar));
    assert_eq!(warm, 0, "rebuilding similar content allocated");
    let mut other = Layout::new();
    prose(&mut other, &mut cx, &same);
    let warm = count_allocations(|| prose(&mut other, &mut cx, &same));
    assert_eq!(warm, 0, "a second layout allocated");
    let widths = [37.0, 81.25, 250.0, 600.0];
    let relayout = |layout: &mut Layout, cx: &mut Context| {
        for width in widths {
            layout.break_lines(cx, Area::new(width), &mut NoExclusions);
            for line in layout.lines() {
                let _ = line.metrics();
            }
        }
    };
    prose(&mut layout, &mut cx, &same);
    relayout(&mut layout, &mut cx);
    let warm = count_allocations(|| relayout(&mut layout, &mut cx));
    assert_eq!(warm, 0, "breaking again allocated");
}
