//! Line metrics tests:
//! - the table each metrics source reads;
//! - a zeroed win pair;
//! - the em box and the ideographic advance;
//! - the edge and middle baselines `dominant-baseline` names.

use super::*;

/// Returns a font whose three sets of vertical metrics all differ.
///
/// Where `use_typo` is true, the font sets `USE_TYPO_METRICS` and its family
/// is Test Line Metrics Typo rather than Test Line Metrics.
fn metrics_font(use_typo: bool) -> TestFont {
    let family = if use_typo {
        "Test Line Metrics Typo"
    } else {
        "Test Line Metrics"
    };
    let mut font = TestFont::new(family, &[(0x20, 0x7E)]);
    font.win = (800, 200);
    font.hhea = (850, -250, 200);
    font.typo = (700, -300, 50);
    font.use_typo = use_typo;
    font
}

/// Returns the ascent, descent and line gap of `font` at `size` under
/// `source`, in pixels.
fn line_metrics(font: TestFont, source: LineMetricsSource, size: f32) -> (f32, f32, f32) {
    let family = font.family.clone();
    let mut fixture = fixture_with(&[font]);
    let mut config = Config::chrome_windows();
    config.line_metrics = source;
    fixture.cx.set_config(config);
    let mut layout = Layout::new();
    let listed = [FontFamilyName::named(&family)];
    fixture.span(&mut layout, &sized(&listed, size), "x");
    let metrics = used_font(&layout, fixture.primary(&layout)).metrics;
    (
        metrics.ascent.to_px(),
        metrics.descent.to_px(),
        metrics.line_gap.to_px(),
    )
}

/// Each source reads its own table:
/// - `Win` reads the win pair, with the gap making up the `hhea` line, as
///   DirectWrite reports it;
/// - `Hhea` reads `hhea`;
/// - `TypoOrHhea` reads the typographic metrics only where the font asks
///   for them.
///
/// Each value is rounded to whole pixels, halves up, as Skia rounds them.
#[test]
fn each_metrics_source_reads_its_table() {
    use LineMetricsSource::{Hhea, TypoOrHhea, Win};
    // `hhea`'s 1300 units less the win pair's 1000 leave a gap of 300.
    assert_eq!(
        line_metrics(metrics_font(false), Win, 10.0),
        (8.0, 2.0, 3.0)
    );
    // 8.5, 2.5 and 2 at 10 px.
    assert_eq!(
        line_metrics(metrics_font(false), Hhea, 10.0),
        (9.0, 3.0, 2.0)
    );
    assert_eq!(
        line_metrics(metrics_font(false), TypoOrHhea, 10.0),
        (9.0, 3.0, 2.0)
    );
    // 7, 3 and 0.5 at 10 px.
    assert_eq!(
        line_metrics(metrics_font(true), TypoOrHhea, 10.0),
        (7.0, 3.0, 1.0)
    );
    assert_eq!(
        line_metrics(metrics_font(true), Hhea, 10.0),
        (9.0, 3.0, 2.0)
    );
    assert_eq!(
        line_metrics(metrics_font(true), Win, 30.0),
        (24.0, 6.0, 9.0)
    );
}

/// A zeroed win pair says nothing, so the Windows source reads `hhea`, as
/// DirectWrite reports such a font.
#[test]
fn a_zeroed_win_pair_falls_back_to_hhea() {
    let mut font = metrics_font(false);
    font.win = (0, 0);
    // 8.5, 2.5 and 2 at 10 px.
    assert_eq!(
        line_metrics(font, LineMetricsSource::Win, 10.0),
        (9.0, 3.0, 2.0)
    );
}

/// Ahem measures the same under every source: 0.8 above and 0.2 below with
/// no gap, an x-height and cap height of 0.8, and an em box of the same
/// split.
#[test]
fn agreeing_metrics_measure_the_same_under_every_source() {
    for source in [
        LineMetricsSource::Win,
        LineMetricsSource::Hhea,
        LineMetricsSource::TypoOrHhea,
    ] {
        let mut fixture = fixture_with(&[]);
        let mut config = Config::chrome_linux();
        config.line_metrics = source;
        fixture.cx.set_config(config);
        let mut layout = Layout::new();
        let ahem = [FontFamilyName::named("Ahem")];
        fixture.span(&mut layout, &sized(&ahem, 20.0), "X");
        let metrics = used_font(&layout, fixture.primary(&layout)).metrics;
        assert_eq!(metrics.ascent.to_px(), 16.0);
        assert_eq!(metrics.descent.to_px(), 4.0);
        assert_eq!(metrics.line_gap.to_px(), 0.0);
        assert_eq!(metrics.x_height.to_px(), 16.0);
        assert_eq!(metrics.cap_height.to_px(), 16.0);
        assert_eq!(metrics.em_over.to_px(), 16.0);
        assert_eq!(metrics.em_under.to_px(), 4.0);
        // Ahem has no `BASE`, so the ideographic baseline is at the bottom
        // of the descent and the central one in the middle of the line.
        assert_eq!(metrics.ideographic.to_px(), 4.0);
        assert_eq!(metrics.central.to_px(), -6.0);
        assert!(metrics.underline_offset.to_px() > 0.0);
        assert!(metrics.strikeout_offset.to_px() < 0.0);
        assert!(metrics.superscript.to_px() < 0.0);
        assert!(metrics.subscript.to_px() > 0.0);
    }
}

/// The em box is the typographic pair scaled to one em, its ascent rounded
/// onto layout's 1/64 grid and its descent the rest.
///
/// - Blink's `ComputeNormalizedTypoAscentAndDescent` stores it so, and
///   Chrome 153's ruby ceils it: 27 px up for Yu Gothic at 30 px, where a
///   whole pixel would be 26.
/// - Where the typographic ascender is not above zero, the line's ascent
///   and descent, in whole pixels, are scaled the same way.
/// - The ideographic advance is `水`'s, or one em where the font has none.
#[test]
fn the_em_box_is_the_typographic_pair_on_the_sixty_fourth() {
    let mut typo = TestFont::new("Test Typo", &[(0x20, 0x7E), (0x6C34, 0x6C34)]);
    typo.typo = (700, -300, 0);
    typo.advances = vec![('\u{6C34}', 900)];
    let mut none = TestFont::new("Test No Typo", &[(0x20, 0x7E)]);
    none.typo = (0, -300, 0);
    none.win = (750, 250);
    none.hhea = (750, -250, 0);
    let mut fixture = fixture_with(&[typo, none]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Typo")];
    fixture.span(&mut layout, &sized(&family, 17.0), "X");
    let metrics = used_font(&layout, fixture.primary(&layout)).metrics;
    // 0.7 of 17 is 11.9: 11.90625 on the grid, where a whole pixel is 12.
    assert_eq!(metrics.em_over.to_px(), 11.90625);
    assert_eq!(metrics.em_under.to_px(), 17.0 - 11.90625);
    assert!((metrics.ideographic_advance.to_px() - 0.9 * 17.0).abs() < 1e-4);
    let family = [FontFamilyName::named("Test No Typo")];
    fixture.span(&mut layout, &sized(&family, 17.0), "X");
    let metrics = used_font(&layout, fixture.primary(&layout)).metrics;
    // The line's whole-pixel 13 and 4, scaled to 17: 13 up.
    assert_eq!(metrics.em_over.to_px(), 13.0);
    assert_eq!(metrics.em_under.to_px(), 4.0);
    assert_eq!(metrics.ideographic_advance.to_px(), 17.0);
}

/// `text-top` and `text-bottom` are the ascent's and descent's edges, and
/// `middle` is half the x-height over the alphabetic baseline, each against
/// the dominant baseline, positive down.
#[test]
fn edge_and_middle_baselines_sit_on_the_line_metrics() {
    use crate::style::DominantBaseline;
    let font = metrics_font(false);
    let family = font.family.clone();
    let mut fixture = fixture_with(&[font]);
    let mut layout = Layout::new();
    let listed = [FontFamilyName::named(&family)];
    fixture.span(&mut layout, &sized(&listed, 10.0), "x");
    let metrics = used_font(&layout, fixture.primary(&layout)).metrics;
    let at = |baseline| metrics.baseline(baseline).to_px();
    assert_eq!(at(DominantBaseline::TextTop), -8.0);
    assert_eq!(at(DominantBaseline::TextBottom), 2.0);
    let half_x = metrics.x_height.to_px() / 2.0;
    assert!(half_x > 0.0);
    assert_eq!(at(DominantBaseline::Middle), -half_x);
}
