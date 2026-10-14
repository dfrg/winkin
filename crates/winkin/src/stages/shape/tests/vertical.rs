//! Vertical shaping tests:
//! - the vertical advance and origin come from `VORG`, `vmtx` or the line,
//!   as Blink reads them, once a glyph at a size;
//! - text on its side is shaped across;
//! - a mark gives its blank back through the font's `vhal`.

use super::*;
use crate::style::FirstLineVariant;

/// Returns a font with vertical metrics in a 1024-unit em, so that at 32 px
/// a unit is exactly 1/32 px.
///
/// Every glyph is 1024 units down the line with a top side bearing of 128,
/// but 水 is 896. Where `origins`, a `VORG` gives a default origin of 880
/// and 水's of 704. Every glyph is 512 units across. The two variants have
/// families of their own, [`VERTICAL`] and [`VERTICAL_ORIGINS`].
fn vertical_font(origins: bool) -> TestFont {
    let family = if origins {
        "Test Vertical Origins"
    } else {
        "Test Vertical Metrics"
    };
    let mut font = TestFont::new(family, &[(0x20, 0x7E), (0x6C34, 0x6C34)]);
    font.upem = 1024;
    font.hhea = (820, -204, 0);
    font.win = (820, 204);
    font.vertical = Some(TestVertical {
        advance: 1024,
        top_side_bearing: 128,
        advances: vec![('\u{6C34}', 896)],
        origins: origins.then(|| (880, vec![('\u{6C34}', 704)])),
    });
    font
}

/// The family of [`vertical_font`] without a `VORG`.
const VERTICAL: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed(
    "Test Vertical Metrics",
))];

/// The family of [`vertical_font`] with a `VORG`.
const VERTICAL_ORIGINS: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed(
    "Test Vertical Origins",
))];

/// Builds `text` into `layout` in `families` at 32 px, set `orientation` in a
/// `vertical-rl` block.
fn vertical_line_layout(
    fixture: &mut Fixture,
    layout: &mut Layout,
    families: &'static [FontFamilyName<'static>],
    orientation: TextOrientation,
    text: &str,
) {
    let mut style = sized(families, 32.0);
    style.orientation.text_orientation = orientation;
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    fixture.build_spans(layout, &block, &[(&style, text)]);
}

/// Returns each cluster's glyphs as drawn, with the cluster's advance.
fn drawn_clusters(fixture: &mut Fixture, layout: &Layout) -> Vec<Vec<Drawn>> {
    let advances = fixture.advances();
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    shaped
        .glyphs
        .iter()
        .map(|(cluster, &word)| drawn(word, &shaped.glyphs.sidecar, advances[cluster.get()]))
        .collect()
}

/// Returns `px` in 16.16; each value here is a whole number of 1/65536 px.
fn units(px: f32) -> i32 {
    TextUnit::from_px_truncated(px).raw()
}

/// Upright text is shaped down the line with the font's vertical metrics as
/// Blink reads them (`OpenTypeVerticalData`):
/// - each glyph's advance is its `vmtx` advance;
/// - its origin is half its width across and, with no `VORG`, its top side
///   bearing above the top of its bounds, which this font's glyphs lack;
/// - its offsets are kept along the line and across it: drawn 4 px along
///   from its pen (128 units) and 8 px under the line's centre (half of 512
///   units across).
#[test]
fn upright_text_takes_the_fonts_vertical_advance_and_origin() {
    use TextOrientation::Upright;
    let mut fixture = fixture_with(&[vertical_font(false)]);
    let mut layout = Layout::new();
    vertical_line_layout(&mut fixture, &mut layout, &VERTICAL, Upright, "A\u{6C34}");
    let font = vertical_font(false);
    let (a, water) = (font.glyph('A'), font.glyph('\u{6C34}'));
    assert_eq!(
        drawn_clusters(&mut fixture, &layout),
        [
            vec![(a, units(4.0), units(-8.0), units(32.0))],
            vec![(water, units(4.0), units(-8.0), units(28.0))],
        ]
    );
}

/// Where the font has `VORG`, it gives the vertical origin: its default for
/// a glyph it does not list, and a listed glyph's own.
#[test]
fn a_vertical_origin_table_gives_the_origin() {
    use TextOrientation::Upright;
    let mut fixture = fixture_with(&[vertical_font(true)]);
    let mut layout = Layout::new();
    vertical_line_layout(
        &mut fixture,
        &mut layout,
        &VERTICAL_ORIGINS,
        Upright,
        "A\u{6C34}",
    );
    let font = vertical_font(true);
    let (a, water) = (font.glyph('A'), font.glyph('\u{6C34}'));
    assert_eq!(
        drawn_clusters(&mut fixture, &layout),
        [
            vec![(a, units(27.5), units(-8.0), units(32.0))],
            vec![(water, units(22.0), units(-8.0), units(28.0))],
        ]
    );
}

/// The context reads a font's vertical metrics once a glyph at a size.
///
/// A rebuild reads none, and a glyph drawn twice is read once. Another size
/// counts as a font of its own. A new context reads them again into the
/// same glyphs.
#[test]
fn vertical_metrics_are_read_once_a_glyph_at_a_size() {
    use TextOrientation::Upright;
    let mut fixture = fixture_with(&[vertical_font(true)]);
    let mut layout = Layout::new();
    vertical_line_layout(
        &mut fixture,
        &mut layout,
        &VERTICAL_ORIGINS,
        Upright,
        "A\u{6C34}A",
    );
    let drawn = drawn_clusters(&mut fixture, &layout);
    assert_eq!(fixture.cx.shaping().vertical_counts(), (1, 2));
    vertical_line_layout(
        &mut fixture,
        &mut layout,
        &VERTICAL_ORIGINS,
        Upright,
        "A\u{6C34}A",
    );
    assert_eq!(drawn_clusters(&mut fixture, &layout), drawn);
    assert_eq!(fixture.cx.shaping().vertical_counts(), (1, 2));

    // At 16 px, half of 32: every value halves, read for a font of its own.
    let mut small = Layout::new();
    let mut style = sized(&VERTICAL_ORIGINS, 16.0);
    style.orientation.text_orientation = Upright;
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    fixture.build_spans(&mut small, &block, &[(&style, "A\u{6C34}A")]);
    let halves: Vec<Vec<Drawn>> = drawn
        .iter()
        .map(|glyphs| {
            let half = |value: i32| value / 2;
            glyphs
                .iter()
                .map(|&(id, along, over, advance)| (id, half(along), half(over), half(advance)))
                .collect()
        })
        .collect();
    assert_eq!(drawn_clusters(&mut fixture, &small), halves);
    assert_eq!(fixture.cx.shaping().vertical_counts(), (2, 4));

    fixture.cx = Context::new(fixture.cx.collection().clone());
    assert_eq!(fixture.cx.shaping().vertical_counts(), (0, 0));
    let mut again = Layout::new();
    vertical_line_layout(
        &mut fixture,
        &mut again,
        &VERTICAL_ORIGINS,
        Upright,
        "A\u{6C34}A",
    );
    assert_eq!(drawn_clusters(&mut fixture, &again), drawn);
    assert_eq!(fixture.cx.shaping().vertical_counts(), (1, 2));
}

/// A font with no vertical metrics, such as Ahem with no `vhea`, is set
/// upright as Blink sets it. Each glyph is as tall as the font's line, its
/// ascent and descent each a whole pixel, and its origin is the ascent
/// above its own. At 30 px Ahem is 24 over and 6 under: 30 a glyph, drawn
/// 24 along.
#[test]
fn a_font_with_no_vertical_metrics_is_set_by_its_line() {
    use TextOrientation::Upright;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&AHEM_FAMILY, 30.0);
    style.orientation.text_orientation = Upright;
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalLr,
        ..ComputedBlockStyle::new(&style)
    };
    fixture.build_spans(&mut layout, &block, &[(&style, "Xp")]);
    let drawn = drawn_clusters(&mut fixture, &layout);
    for glyphs in &drawn {
        let [(_, along, over, advance)] = glyphs[..] else {
            panic!("one glyph a cluster: {drawn:?}");
        };
        assert_eq!(
            (along, over, advance),
            (units(24.0), units(-15.0), units(30.0))
        );
    }
}

/// Under `mixed`, Latin is on its side, shaped across as horizontal text
/// is, one compact glyph a cluster at its own advance, and an ideograph
/// beside it is shaped down the line. Under `sideways` nothing is shaped
/// down the line.
#[test]
fn text_on_its_side_is_shaped_across() {
    use TextOrientation::{Mixed, Sideways};
    let mut fixture = fixture_with(&[vertical_font(false)]);
    let mut layout = Layout::new();
    vertical_line_layout(&mut fixture, &mut layout, &VERTICAL, Mixed, "AB\u{6C34}");
    let font = vertical_font(false);
    let drawn = drawn_clusters(&mut fixture, &layout);
    assert_eq!(drawn[0], [(font.glyph('A'), 0, 0, units(16.0))]);
    assert_eq!(drawn[1], [(font.glyph('B'), 0, 0, units(16.0))]);
    assert_eq!(drawn[2][0].3, units(28.0));
    let mut layout = Layout::new();
    vertical_line_layout(&mut fixture, &mut layout, &VERTICAL, Sideways, "AB\u{6C34}");
    assert_eq!(
        drawn_clusters(&mut fixture, &layout)[2],
        [(font.glyph('\u{6C34}'), 0, 0, units(16.0))]
    );
}

/// Returns a Japanese font with vertical metrics, every glyph an em down the
/// line, with `vhal` where `vhal` and `halt` where `halt`.
///
/// `vhal` sets each full-width mark in half its em down a vertical line: an
/// opening mark moved up half and its advance halved, a closing one's
/// advance halved.
fn vertical_punct(family: &str, halt: bool, vhal: bool) -> TestFont {
    let mut font = TestFont::cjk(family, halt);
    font.vertical = Some(TestVertical {
        advance: 1000,
        top_side_bearing: 0,
        advances: Vec::new(),
        origins: Some((880, Vec::new())),
    });
    if vhal {
        font.vhal = vec![
            ('\u{300C}', 500, -500),
            ('\u{300D}', 0, -500),
            ('\u{3001}', 0, -500),
            ('\u{3002}', 0, -500),
        ];
    }
    font
}

/// Down a vertical line a mark gives its blank back in the font's `vhal`,
/// found by the same rule as across one, as Blink's `HanKerning` trims
/// `!is_horizontal`.
///
/// A font with only the horizontal `halt` trims nothing upright, since its
/// `halt` would move its marks across the line. Text set on its side is
/// shaped across, and trims with `halt`.
#[test]
fn a_mark_gives_its_blank_back_down_a_vertical_line_in_the_fonts_vhal() {
    use TextOrientation::{Sideways, Upright};
    let mut fixture = fixture_with(&[
        vertical_punct("Test Vertical Punct", false, true),
        vertical_punct("Test Across Punct", true, false),
    ]);
    const VHAL: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Vertical Punct"))];
    const HALT: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Across Punct"))];
    let set = |fixture: &mut Fixture,
               families: &'static [FontFamilyName<'static>],
               orientation: TextOrientation,
               text: &str| {
        let mut layout = Layout::new();
        let mut style = sized(families, 20.0);
        style.orientation.text_orientation = orientation;
        let block = ComputedBlockStyle {
            writing_mode: WritingMode::VerticalRl,
            ..ComputedBlockStyle::new(&style)
        };
        fixture.build_spans(&mut layout, &block, &[(&style, text)]);
        (fixture.advances(), layout)
    };
    for (text, halved) in [("漢「「漢", 2), ("漢」」漢", 1), ("漢」「漢", 2)] {
        let (advances, layout) = set(&mut fixture, &VHAL, Upright, text);
        let mut expected = vec![px16(20); 4];
        expected[halved] = px16(10);
        assert_eq!(advances, expected, "{text}");
        let shaped = layout.shaped().text(FirstLineVariant::Standard);
        assert!(
            shaped.glyphs.word(ClusterId::new(2)).is_unsafe_to_break(),
            "{text}"
        );
    }
    // The second opening mark is drawn half an em up the line, into the
    // half it keeps: its offset along the line is half an em less than an
    // unmoved glyph's.
    let (_, layout) = set(&mut fixture, &VHAL, Upright, "漢「「漢");
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let along = |cluster: usize| {
        drawn(
            shaped.glyphs.word(ClusterId::new(cluster)),
            &shaped.glyphs.sidecar,
            0,
        )[0]
        .1
    };
    assert_eq!(along(1) - along(2), px16(10));
    // `halt` is across the line: it trims nothing upright, and trims text on
    // its side.
    let (advances, _) = set(&mut fixture, &HALT, Upright, "漢「「漢");
    assert_eq!(advances, vec![px16(20); 4]);
    let (advances, _) = set(&mut fixture, &HALT, Sideways, "漢「「漢");
    assert_eq!(advances, [px16(20), px16(20), px16(10), px16(20)]);
}
