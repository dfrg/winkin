//! Tests of font selection in vertical lines: a line's metrics are centred
//! as Chrome 153 sets Ahem, with the odd pixel over.

use super::*;

/// Ahem's line metrics at `size` in a block whose lines run as
/// `writing_mode` says, its text standing as `orientation` says: ascent,
/// descent, where the alphabetic baseline is under the line's own, and the
/// em box, in pixels.
fn ahem_lines(writing_mode: WritingMode, orientation: TextOrientation, size: f32) -> [f32; 5] {
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let mut style = sized(&ahem, size);
    style.orientation.text_orientation = orientation;
    let block = ComputedBlockStyle {
        writing_mode,
        ..ComputedBlockStyle::new(&style)
    };
    let mut b = layout.builder(&mut fixture.cx, NodeKey(0), &block, BuildOptions::default());
    b.text(NodeKey(1), "X");
    b.finish();
    check(&layout, &fixture.cx);
    let metrics = used_font(&layout, fixture.primary(&layout)).metrics;
    [
        metrics.ascent.to_px(),
        metrics.descent.to_px(),
        metrics.alphabetic.to_px(),
        metrics.em_over.to_px(),
        metrics.em_under.to_px(),
    ]
}

/// A vertical line whose text is upright or mixed is centred on the central
/// baseline: its height, whole pixels, split with the odd pixel over, as
/// Blink's `FontMetrics::IntAscentInternal(kCentralBaseline)` splits it and
/// as Chrome 153 sets Ahem on the `centering.html` probe page: 15px, 12 over
/// the alphabetic baseline and 3 under it, is 8 over and 7 under; 17px 9
/// and 8; 13px 7 and 6; 21px 11 and 10. Its em box is the size halved on
/// the grid. Sideways text, and the sideways modes, keep the alphabetic
/// metrics.
#[test]
fn a_vertical_line_is_centred_with_its_odd_pixel_over() {
    use TextOrientation::{Mixed, Sideways, Upright};
    use WritingMode::{SidewaysRl, VerticalLr, VerticalRl};
    for (size, over, under) in [
        (15.0, 8.0, 7.0),
        (17.0, 9.0, 8.0),
        (13.0, 7.0, 6.0),
        (21.0, 11.0, 10.0),
    ] {
        for mode in [VerticalRl, VerticalLr] {
            for orientation in [Mixed, Upright] {
                let [ascent, descent, alphabetic, em_over, em_under] =
                    ahem_lines(mode, orientation, size);
                assert_eq!(
                    (ascent, descent),
                    (over, under),
                    "{size}px {mode:?} {orientation:?}"
                );
                // Ahem's ascent is 0.8 of the size, rounded.
                let alphabetic_ascent = (size * 0.8 + 0.5).floor();
                assert_eq!(alphabetic, alphabetic_ascent - over);
                assert_eq!(em_over + em_under, size);
                assert_eq!(em_under, (size * 64.0 / 2.0).floor() / 64.0);
            }
        }
    }
    let alphabetic = [12.0, 3.0, 0.0, 12.0, 3.0];
    assert_eq!(ahem_lines(VerticalRl, Sideways, 15.0), alphabetic);
    assert_eq!(ahem_lines(SidewaysRl, Upright, 15.0), alphabetic);
    assert_eq!(
        ahem_lines(WritingMode::HorizontalTb, Upright, 15.0),
        alphabetic
    );
}
