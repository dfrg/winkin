//! Emphasis mark tests. They pin:
//! - a mark against its text's em box;
//! - what `text-emphasis-skip` leaves bare;
//! - a ligature's advance shared among its marks;
//! - the side each writing mode sets marks on;
//! - marks over an annotated base;
//! - a mark's middle whatever its font's metrics.

use super::*;

/// Chrome's numbers: marks over two Ahem ems at 40px on a line 80px tall. The text's em box is 20 down and 40 tall, so its baseline is 52,
/// and the marks' own 20px ems stand on it, their baselines 16 in, centred
/// on 20 and 60. They fit in the half-leading, so the line keeps its 80.
#[test]
fn an_emphasis_mark_sits_against_the_em_box_of_its_text() {
    let mut cx = context();
    let mut layout = Layout::new();
    let mut style = marked(40.0, EmphasisSkip::INITIAL);
    style.line.height = LineHeight::Px(80.0);
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&style),
        |b| b.text(NodeKey(1), "XX"),
    );
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let run = runs(&layout, 0)[0];
    assert_eq!(run.baseline(), 52.0);
    let marks: Vec<(f32, f32, f32, usize)> = run
        .emphasis_marks()
        .map(|mark| (mark.x, mark.baseline, mark.size, mark.text_offset))
        .collect();
    assert_eq!(marks, [(20.0, 16.0, 20.0, 0), (60.0, 16.0, 20.0, 1)]);
    assert!(run.emphasis_marks().all(|mark| mark.key == NodeKey(1)));
    assert_eq!(
        layout.line(0).map(|line| line.metrics().height()),
        Some(80.0)
    );
    // Under, the mark's em box hangs from the text's: 52 + 8, and its
    // ascent of 16 below that.
    style.text.emphasis.position.side = EmphasisSide::Under;
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&style),
        |b| b.text(NodeKey(1), "XX"),
    );
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let run = runs(&layout, 0)[0];
    let baselines: Vec<f32> = run.emphasis_marks().map(|mark| mark.baseline).collect();
    assert_eq!(baselines, [76.0, 76.0]);
}

/// `text-emphasis-skip` leaves bare what it names, by each cluster's first
/// character: the initial `spaces punctuation` marks the letters of `a b,
/// c#` and the `#` CSS marks anyway, `spaces` alone the comma too, and
/// `punctuation` alone the spaces; text with no marks set has none.
#[test]
fn emphasis_marks_skip_what_text_emphasis_skip_names() {
    use crate::style::EmphasisSkip;
    let mut cx = context();
    let mut layout = Layout::new();
    let offsets = |layout: &Layout| -> Vec<usize> {
        runs(layout, 0)
            .iter()
            .flat_map(|run| run.emphasis_marks().map(|mark| mark.text_offset))
            .collect()
    };
    for (skip, want) in [
        (EmphasisSkip::INITIAL, vec![0, 2, 5, 6]),
        (EmphasisSkip::SPACES, vec![0, 2, 3, 5, 6]),
        (EmphasisSkip::PUNCTUATION, vec![0, 1, 2, 4, 5, 6]),
    ] {
        let style = marked(20.0, skip);
        build(
            &mut cx,
            &mut layout,
            &ComputedBlockStyle::new(&style),
            |b| b.text(NodeKey(1), "a b, c#"),
        );
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        assert_eq!(offsets(&layout), want, "{skip:?}");
    }
    let plain = sized(&AHEM_FAMILY, 20.0);
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&plain),
        |b| b.text(NodeKey(1), "a b"),
    );
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert!(offsets(&layout).is_empty());
}

/// A ligature's advance is shared evenly among the clusters it draws, one
/// mark each, as Blink divides it among its graphemes: `fi` in the test
/// font is one glyph over two clusters, marked at a quarter and three
/// quarters of it.
#[test]
fn a_ligature_shares_its_advance_among_its_marks() {
    let mut cx = context();
    let mut layout = Layout::new();
    let mut style = sized(&LATIN, 20.0);
    style.text.emphasis.marks = true;
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&style),
        |b| b.text(NodeKey(1), "fi"),
    );
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let run = runs(&layout, 0)[0];
    assert_eq!(run.glyphs().count(), 1, "one ligature glyph");
    let InlineExtents { left, right } = run.inline();
    let xs: Vec<f32> = run.emphasis_marks().map(|mark| mark.x).collect();
    let quarter = (right - left) / 4.0;
    assert_eq!(xs.len(), 2);
    assert!((xs[0] - (left + quarter)).abs() < 0.01, "{xs:?}");
    assert!((xs[1] - (right - quarter)).abs() < 0.01, "{xs:?}");
}

/// Emphasis marks take the side `text-emphasis-position` gives in each
/// writing mode, as CSS Text Decoration 4, section 3.4, reads it: `right` and `left`
/// in `vertical-rl` and `vertical-lr`, where a line's over side is its
/// right, and `over` and `under` in the sideways modes, horizontal
/// typographic modes turned whole. Ahem at 20 px on lines of 20, 10 px
/// marks: the line grows by them on their side, and the text is 10 to 30
/// across from the line's over side with them over, 0 to 20 with them
/// under, their em boxes 0 to 10 and 20 to 30: their baselines 5 and 25 on
/// a vertical line's central baseline, 8 and 28 on a sideways line's
/// alphabetic one. Chrome 153, in both vertical modes: `over
/// right` and `under right` put the text at the block's left, 0 to 20 of
/// 30, the marks inked on its right; `over left` and `under left` at 10 to
/// 30, the marks on its left. In the sideways modes Chrome reads `right`
/// and `left` too, `left` as over in `sideways-lr`, which is not copied.
#[test]
fn emphasis_marks_take_the_side_each_writing_mode_reads() {
    use crate::style::{EmphasisVerticalSide, WritingMode};
    let mut cx = context();
    let mut layout = Layout::new();
    for mode in [
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::SidewaysRl,
        WritingMode::SidewaysLr,
    ] {
        let vertical = mode.is_vertical_typographic();
        // Where a mark's baseline is in its em box, 10 px of Ahem: its
        // middle, or 8 down it.
        let raised = if vertical { 5.0 } else { 8.0 };
        let (over, under) = (
            (across(10.0, 30.0), raised),
            (across(0.0, 20.0), 20.0 + raised),
        );
        for (side, vertical_side) in [
            (EmphasisSide::Over, EmphasisVerticalSide::Right),
            (EmphasisSide::Under, EmphasisVerticalSide::Right),
            (EmphasisSide::Over, EmphasisVerticalSide::Left),
            (EmphasisSide::Under, EmphasisVerticalSide::Left),
        ] {
            let mut style = marked(20.0, EmphasisSkip::INITIAL);
            style.line.height = LineHeight::Px(20.0);
            style.text.emphasis.position.side = side;
            style.text.emphasis.position.vertical_side = vertical_side;
            let block = ComputedBlockStyle {
                writing_mode: mode,
                ..ComputedBlockStyle::new(&style)
            };
            build(&mut cx, &mut layout, &block, |b| b.text(NodeKey(1), "XX"));
            layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
            let taken_over = if vertical {
                vertical_side == EmphasisVerticalSide::Right
            } else {
                side == EmphasisSide::Over
            };
            let (text, mark) = if taken_over { over } else { under };
            let case = (mode, side, vertical_side);
            let line = layout.line(0).expect("a line");
            assert_eq!(line.metrics().height(), 30.0, "{case:?}");
            let run = runs(&layout, 0)[0];
            assert_eq!(run.block(), text, "{case:?}");
            let baselines: Vec<f32> = run.emphasis_marks().map(|mark| mark.baseline).collect();
            assert_eq!(baselines, [mark, mark], "{case:?}");
        }
    }
}

/// An emphasis mark over a ruby base with an annotation over it goes past
/// the annotation, as Chrome sets it, and the line makes room for both: the
/// annotation's em box reaches 26 over the base's baseline, and the base's
/// 10px marks stand on it, so the line reaches 36 over its baseline, the
/// annotation's baseline is 18 down and the marks' 8, their em boxes from
/// the line's top to the annotation's.
#[test]
fn an_emphasis_mark_over_an_annotated_base_goes_past_the_annotation() {
    let mut cx = context();
    let mut layout = Layout::new();
    let plain = sized(&AHEM_FAMILY, 20.0);
    let base = marked(20.0, EmphasisSkip::INITIAL);
    let small = sized(&AHEM_FAMILY, 10.0);
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&plain),
        |b| {
            b.open_ruby(NodeKey(2), &plain, None);
            b.open_box(NodeKey(3), &base, None);
            b.text(NodeKey(4), "XX");
            b.close_box();
            b.open_annotation(NodeKey(5), &small, None);
            b.text(NodeKey(6), "XX");
            b.close_annotation();
            b.close_ruby();
        },
    );
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let line = layout.line(0).expect("a line");
    assert_eq!(line.metrics().ascent, 36.0);
    let annotation = line.annotations().next().expect("an annotation");
    assert_eq!(annotation.baseline(), 18.0);
    let run = runs(&layout, 0)[0];
    assert_eq!(run.baseline(), 36.0);
    let marks: Vec<(f32, f32)> = run.emphasis_marks().map(|m| (m.x, m.baseline)).collect();
    assert_eq!(marks, [(10.0, 8.0), (30.0, 8.0)]);
    // Painted right after their run.
    let order: Vec<&str> = line
        .paints(|_| Decorates::None)
        .map(|item| match item {
            Paint::Annotation(_) => "annotation",
            Paint::Text(_) => "text",
            Paint::Emphasis(_) => "mark",
            _ => "other",
        })
        .filter(|kind| *kind != "other")
        .collect();
    assert_eq!(order, ["annotation", "text", "mark", "mark"]);
}

/// A mark's middle along the line is its cluster's whatever the font's
/// ascent, descent and vertical metrics, in every writing mode. The font
/// has Meiryo's metrics: a 2048-unit em, ascent 2171 and descent 901, each
/// glyph 2048 down the line with a top side bearing of 450. Chrome 153
/// centres the ink of a U+25CF mark on its base's middle in Meiryo and in
/// Yu Gothic alike: at 40 px, 20 along the line within half a pixel, and at
/// 100 and 160 px, 50.0 and 80.0.
#[test]
fn an_emphasis_mark_is_centred_on_its_cluster_whatever_the_font_metrics() {
    use crate::style::WritingMode;
    use crate::tests::TestVertical;
    let mut font = TestFont::new("Test Tall Ascent", &[(0x8A66, 0x8A66)]);
    font.upem = 2048;
    font.win = (2171, 901);
    font.hhea = (2171, -901, 0);
    font.typo = (1798, -250, 0);
    font.advances = vec![('\u{8A66}', 2048)];
    font.vertical = Some(TestVertical {
        advance: 2048,
        top_side_bearing: 450,
        advances: Vec::new(),
        origins: None,
    });
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(font.build()).is_ok());
    layer.set_fallback_override(TestFallback::new().family("Test Tall Ascent"));
    let mut cx = Context::new(Collection::new().with_layer(layer.snapshot()));
    const TALL: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Tall Ascent"))];
    let mut layout = Layout::new();
    for mode in [
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::HorizontalTb,
    ] {
        for size in [40.0, 24.0] {
            let mut style = sized(&TALL, size);
            style.line.height = LineHeight::Px(size);
            style.text.emphasis.marks = true;
            let block = ComputedBlockStyle {
                writing_mode: mode,
                ..ComputedBlockStyle::new(&style)
            };
            build(&mut cx, &mut layout, &block, |b| {
                b.text(NodeKey(1), "\u{8A66}\u{8A66}");
            });
            layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
            let middles: Vec<f32> = runs(&layout, 0)
                .iter()
                .flat_map(|run| run.emphasis_marks().map(|mark| mark.x))
                .collect();
            assert_eq!(middles, [size / 2.0, size * 1.5], "{mode:?} at {size}");
        }
    }
}

/// Content with emphasis marks or ruby annotations has annotations, and
/// plain content has none and never lends a block after it negative room.
#[test]
fn a_layout_says_whether_it_has_annotations() {
    let mut cx = context();
    let plain_style = sized(&AHEM_FAMILY, 20.0);
    let mut plain = Layout::new();
    build(
        &mut cx,
        &mut plain,
        &ComputedBlockStyle::new(&plain_style),
        |b| b.text(NodeKey(1), "XX XX"),
    );
    plain.break_lines(&mut cx, Area::new(30.0), &mut NoExclusions);
    assert!(!plain.has_annotations());
    assert!(plain.room_below() >= 0.0);
    let marked_style = marked(20.0, EmphasisSkip::INITIAL);
    let mut marked_layout = Layout::new();
    build(
        &mut cx,
        &mut marked_layout,
        &ComputedBlockStyle::new(&marked_style),
        |b| b.text(NodeKey(1), "XX"),
    );
    assert!(marked_layout.has_annotations());
    let mut ruby = Layout::new();
    build(
        &mut cx,
        &mut ruby,
        &ComputedBlockStyle::new(&plain_style),
        |b| {
            b.open_ruby(NodeKey(1), &plain_style, None);
            b.text(NodeKey(2), "XX");
            b.open_annotation(NodeKey(3), &plain_style, None);
            b.text(NodeKey(4), "X");
            b.close_annotation();
            b.close_ruby();
        },
    );
    assert!(ruby.has_annotations());
}
