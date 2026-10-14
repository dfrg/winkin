//! Shaping tests with a font's full features:
//! - a variable font shaped at its coordinates, two instances of it apart in
//!   one paragraph;
//! - synthesized small capitals fed uppercased at their size, with Turkish
//!   dotting its capital i;
//! - `font-kerning: none`;
//! - the point size a `trak` table reads.

use super::*;
use crate::style::FirstLineVariant;

/// Returns a font of ASCII, the sharp s and the Turkish capital dotted I,
/// varying in weight from 100 to 900 about 400, 200 units wider at 900.
fn cased() -> TestFont {
    let mut font = TestFont::new("Test Cased", &[(0x20, 0x7E), (0xDF, 0xDF), (0x130, 0x131)]);
    font.axes = vec![TestAxis {
        tag: *b"wght",
        min: 100.0,
        default: 400.0,
        max: 900.0,
        delta: 200,
    }];
    font
}

const CASED: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed("Test Cased"))];

/// A variable font is shaped at the coordinates its used font holds, so its
/// advances move with its axes. At 700 the weight axis is 0.6 of the way to
/// its end, which adds 120 of the 200 units every advance gains there.
#[test]
fn a_variable_font_is_shaped_at_its_coordinates() {
    let mut fixture = fixture_with(&[cased()]);
    let mut layout = Layout::new();
    let mut style = sized(&CASED, 100.0);
    fixture.span(&mut layout, &style, "ab");
    let em = scaled(500, 1000, i64::from(TextUnit::from_px(100.0).raw()));
    assert_eq!(fixture.advances(), [em, em]);
    style.font.weight = FontWeight::new(700.0);
    fixture.span(&mut layout, &style, "ab");
    let bold = scaled(620, 1000, i64::from(TextUnit::from_px(100.0).raw()));
    for advance in fixture.advances() {
        assert!(
            (advance - bold).abs() <= 1 << 10,
            "{advance} against {bold}"
        );
    }
}

/// Two instances of one font in one paragraph each shape at their own
/// coordinates, however the runs alternate between them.
#[test]
fn instances_of_one_font_shape_apart_in_one_paragraph() {
    let mut fixture = fixture_with(&[cased()]);
    let mut layout = Layout::new();
    let regular = sized(&CASED, 100.0);
    let mut bold = regular;
    bold.font.weight = FontWeight::new(700.0);
    fixture.spans(
        &mut layout,
        &regular,
        &[(&regular, "ab"), (&bold, "ab"), (&regular, "ab")],
    );
    let size = i64::from(TextUnit::from_px(100.0).raw());
    let (em, wide) = (scaled(500, 1000, size), scaled(620, 1000, size));
    let advances = fixture.advances();
    let wanted = [em, em, wide, wide, em, em];
    assert_eq!(advances.len(), wanted.len());
    for (advance, wanted) in advances.into_iter().zip(wanted) {
        assert!(
            (advance - wanted).abs() <= 1 << 10,
            "{advance} against {wanted}"
        );
    }
}

/// Synthesized small capitals are fed to the shaper uppercased, each
/// character they make labelled with the one it came from, and set at their
/// own size.
///
/// At 14 px of a 20 px style, `x` is drawn as `X`, and `ß` as `SS`, two
/// glyphs in one cluster. The text keeps the author's case, and a range of
/// the run shaped again is its slice.
#[test]
fn synthesized_small_caps_are_fed_uppercased_at_their_size() {
    let font = cased();
    let mut fixture = fixture_with(slice::from_ref(&font));
    let mut layout = Layout::new();
    let mut style = sized(&CASED, 20.0);
    style.font.variant_caps = FontVariantCaps::SmallCaps;
    fixture.span(&mut layout, &style, "Axßy");
    assert_eq!(layout.content().text, "Axßy");
    let big = font.glyph('A');
    let x = font.glyph('X');
    let s = font.glyph('S');
    let y = font.glyph('Y');
    assert_eq!(ids(&layout), [vec![big], vec![x], vec![s, s], vec![y]]);
    let small = scaled(500, 1000, i64::from(TextUnit::from_px(14.0).raw()));
    let full = scaled(500, 1000, i64::from(TextUnit::from_px(20.0).raw()));
    assert_eq!(
        fixture.advances(),
        [full, small, 2 * small, small],
        "the capital at the size, the rest at 0.7 of it"
    );
    let runs = &layout.shaped().text(FirstLineVariant::Standard).runs;
    assert_eq!(runs.len(), 2, "divided where the case changes");
    let run = ShapedRunId::new(1);
    let piece = fixture.reshape(&layout, run, ClusterId::new(2)..ClusterId::new(4));
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    for (at, cluster) in (2..4).enumerate() {
        let word = piece.words[ClusterId::new(at)];
        assert_eq!(
            drawn(word, &piece.sidecar, 0)
                .iter()
                .map(|g| g.0)
                .collect::<Vec<_>>(),
            drawn(
                shaped.glyphs.word(ClusterId::new(cluster)),
                &shaped.glyphs.sidecar,
                0
            )
            .iter()
            .map(|g| g.0)
            .collect::<Vec<_>>()
        );
    }
}

/// In Turkish text, synthesized small capitals take the dotted capital I
/// for `i`, as ICU uppercases it for Blink.
#[test]
fn turkish_small_caps_dot_the_capital_i() {
    let font = cased();
    let mut fixture = fixture_with(slice::from_ref(&font));
    let mut layout = Layout::new();
    let mut style = sized(&CASED, 20.0);
    style.font.variant_caps = FontVariantCaps::SmallCaps;
    style.text.language = Language::parse("tr").ok();
    fixture.span(&mut layout, &style, "i");
    assert_eq!(ids(&layout), [vec![font.glyph('\u{130}')]]);
    style.text.language = Language::parse("en").ok();
    fixture.span(&mut layout, &style, "i");
    assert_eq!(ids(&layout), [vec![font.glyph('I')]]);
}

/// `font-kerning: none` turns the font's kerning off, with `kern` off among
/// the features the run is shaped with.
#[test]
fn font_kerning_none_leaves_the_pairs_apart() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&LATIN_HEBREW, 100.0);
    fixture.span(&mut layout, &style, "AV");
    let kerned = fixture.advances()[0];
    style.font.kerning = FontKerning::None;
    fixture.span(&mut layout, &style, "AV");
    let apart = fixture.advances()[0];
    assert!(apart > kerned, "{apart} against {kerned}");
}

/// Returns a font that tracks by its point size, as San Francisco's `trak`
/// does: nothing at 10 points and 900 units at 40, read between them.
fn tracked() -> TestFont {
    let mut font = TestFont::new("Test Tracked", &[(0x20, 0x7E)]);
    font.tracking = vec![(10.0, 0), (40.0, 900)];
    font
}

const TRACKED: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Tracked"))];

/// The shaper is told the style's computed size as its point size, not the
/// size the glyphs are set at, and a `trak` table tracks by it. Chrome hands
/// HarfBuzz the specified size the same way (`HarfBuzzShaper::ShapeSegment`
/// passes `SpecifiedSize` to `hb_font_set_ptem`).
/// - At 20 px a grapheme gains 300 units, a third of the way from 10 points
///   to 40: 6 px on its 10.
/// - Where `font-size-adjust` sets the glyphs at 10 px, it gains the same
///   300 units of their em, 3 px; their own size would give nothing.
/// - Synthesized small capitals at 14 px gain 300 units of theirs, 4.2 px;
///   their own size would give 120.
/// - A line edge shaped again is tracked as its run was.
#[test]
fn the_point_size_is_the_computed_size() {
    let font = tracked();
    let mut fixture = fixture_with(slice::from_ref(&font));
    let mut layout = Layout::new();
    let in_px = |advances: Vec<i32>| -> Vec<f32> {
        advances
            .iter()
            .map(|&advance| (advance as f32 / 65536.0 * 1000.0).round() / 1000.0)
            .collect()
    };
    let mut style = sized(&TRACKED, 20.0);
    fixture.span(&mut layout, &style, "xx");
    assert_eq!(in_px(fixture.advances()), [16.0, 16.0]);

    // The x-height is half the em, so 0.25 sets the glyphs at 10 px.
    style.font.size_adjust = FontSizeAdjust::Hold {
        metric: AdjustMetric::ExHeight,
        value: 0.25,
    };
    fixture.span(&mut layout, &style, "xx");
    assert_eq!(in_px(fixture.advances()), [8.0, 8.0]);
    let piece = fixture.reshape(
        &layout,
        ShapedRunId::new(0),
        ClusterId::new(1)..ClusterId::new(2),
    );
    let reshaped: Vec<i32> = piece.advances.iter().map(|advance| advance.raw()).collect();
    assert_eq!(in_px(reshaped), [8.0]);

    style.font.size_adjust = FontSizeAdjust::None;
    style.font.variant_caps = FontVariantCaps::SmallCaps;
    fixture.span(&mut layout, &style, "xx");
    assert_eq!(in_px(fixture.advances()), [11.2, 11.2]);
}
