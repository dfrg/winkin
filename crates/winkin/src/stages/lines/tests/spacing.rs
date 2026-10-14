//! Indent, spacing and hanging punctuation tests. They pin:
//! - `text-indent` taking room from the lines it starts, under `each-line`
//!   and `hanging` too;
//! - spacing moving where lines break, and reshaped edges spaced as the
//!   paragraph is;
//! - a hanging mark given back where the line ends;
//! - negative spacing, and spacing, `line-padding` or indents of any size
//!   giving valid lines.

use super::*;
use crate::data::IdRange;
use crate::style::FirstLineVariant;
use crate::style::LengthPercentage;

/// Returns each line's indent, in pixels.
fn indents(layout: &Layout) -> Vec<f32> {
    records(layout)
        .iter()
        .map(|line| line.indent.to_px())
        .collect()
}

/// `text-indent` takes room from the lines it starts.
///
/// - It indents the block's first line, or under `hanging` every other line.
/// - Under `each-line` it also indents each line after a forced break.
/// - A percentage is of the area's width, not the band's.
/// - A negative indent gives room; one wider than the band overflows.
#[test]
fn an_indent_takes_room_from_the_lines_it_starts() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let text = "XXX XXX XXX XXX";
    for (block, lines, expected) in [
        (
            indented(20.0, 0.0, false, false),
            vec!["XXX ", "XXX XXX ", "XXX"],
            vec![20.0, 0.0, 0.0],
        ),
        (
            indented(0.0, 0.25, false, false),
            vec!["XXX ", "XXX XXX ", "XXX"],
            vec![20.0, 0.0, 0.0],
        ),
        (
            indented(20.0, 0.0, true, false),
            vec!["XXX XXX ", "XXX ", "XXX"],
            vec![0.0, 20.0, 20.0],
        ),
        (
            indented(-20.0, 0.0, false, false),
            vec!["XXX XXX ", "XXX XXX"],
            vec![-20.0, 0.0],
        ),
    ] {
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem(10.0),
                ..block
            },
            |b| {
                b.text(NodeKey(1), text);
            },
        );
        fixture.lay_out(&mut layout, 80.0);
        assert_eq!(texts(&layout), lines, "{:?}", block.text_indent);
        assert_eq!(indents(&layout), expected, "{:?}", block.text_indent);
    }
    // After a forced break, only under `each-line`.
    let two = |b: &mut LayoutBuilder<'_>| {
        b.text(NodeKey(1), "XXX XXX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "XXX XXX");
    };
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..indented(20.0, 0.0, false, true)
        },
        two,
    );
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(texts(&layout), ["XXX ", "XXX\n", "XXX ", "XXX"]);
    assert_eq!(indents(&layout), [20.0, 0.0, 20.0, 0.0]);
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..indented(20.0, 0.0, false, false)
        },
        two,
    );
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(texts(&layout), ["XXX ", "XXX\n", "XXX XXX"]);
    // A percentage is of the area's width. A float narrows the band to
    // 100 px of the area's 200, and 10% indents 20 px.
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..indented(0.0, 0.1, false, false)
        },
        |b| {
            b.text(NodeKey(1), text);
        },
    );
    let area = Area::new(200.0);
    fixture.lay_out_with(&mut layout, area, &mut Pushed(100.0, FloatSide::Left));
    assert_eq!(indents(&layout)[0], 20.0);
    // An indent wider than the band leaves an overflowing line.
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..indented(200.0, 0.0, false, false)
        },
        |b| {
            b.text(NodeKey(1), text);
        },
    );
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(texts(&layout)[0], "XXX ");
    assert!(records(&layout)[0].flags.contains(LineFlags::OVERFLOWS));
}

/// Letter- and word-spacing are in the prefix, so they move where lines
/// break.
///
/// `XX XX` is 50 px plain, 60 px with 2 px letter-spacing, and 56 px with
/// 6 px word-spacing.
#[test]
fn spacing_moves_where_lines_break() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = ahem(10.0);
    fixture.text(&mut layout, &style, "XX XX XX");
    fixture.lay_out(&mut layout, 55.0);
    assert_eq!(texts(&layout), ["XX XX ", "XX"]);
    style.text.letter_spacing = LengthPercentage::from_px(2.0);
    fixture.text(&mut layout, &style, "XX XX XX");
    fixture.lay_out(&mut layout, 55.0);
    assert_eq!(texts(&layout), ["XX ", "XX ", "XX"]);
    assert_eq!(widths(&layout), [24.0, 24.0, 24.0]);
    style.text.letter_spacing = LengthPercentage::from_px(0.0);
    style.text.word_spacing = LengthPercentage {
        px: 6.0,
        fraction: 0.0,
    };
    fixture.text(&mut layout, &style, "XX XX XX");
    fixture.lay_out(&mut layout, 55.0);
    assert_eq!(texts(&layout), ["XX ", "XX ", "XX"]);
    fixture.lay_out(&mut layout, 56.0);
    assert_eq!(texts(&layout), ["XX XX ", "XX"]);
}

/// A reshaped line edge is spaced as the paragraph is.
///
/// Under 5 px of letter-spacing, every line is as wide as its text shaped
/// whole and spaced.
#[test]
fn a_reshaped_edge_is_spaced_as_the_paragraph_is() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = break_all(&sized(&LATIN, 20.0));
    style.text.letter_spacing = LengthPercentage::from_px(5.0);
    fixture.text(&mut layout, &style, "AVAVAVAV");
    let mut reshaped = 0;
    for width in [28.0, 30.0, 40.0, 43.0, 45.0, 55.0, 58.0, 60.0, 73.0, 90.0] {
        fixture.lay_out(&mut layout, width);
        for line in records(&layout).to_vec() {
            assert_eq!(
                line.width,
                shaped_whole(&mut fixture, &layout, &line),
                "at {width}: {line:?}"
            );
            reshaped += line.shapes.end.get() - line.shapes.start.get();
        }
        let lines = layout.line_records();
        for shape in lines.edges().shapes.as_slice() {
            let clusters = shape.clusters();
            for (k, cluster) in clusters.ids().enumerate() {
                let advance = lines.edges().advances[EdgeClusterId::new(shape.first.get() + k)];
                assert!(
                    advance >= TextUnit::from_px(13.0),
                    "{cluster:?} spaced: {advance:?}"
                );
            }
        }
    }
    assert!(reshaped > 0, "some line is reshaped");
}

/// Hanging punctuation is given back where the line ends at the mark.
///
/// - `force-end` always hangs; `allow-end` only where the line needs it.
/// - `first` hangs on the block's first line only, not after a forced break.
/// - `last` hangs only at the text end.
#[test]
fn a_hanging_mark_is_given_back_where_the_line_ends() {
    use crate::style::{HangEnd, HangingPunctuation};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let hanging = |first: bool, last: bool, end: HangEnd| {
        let mut style = ahem(10.0);
        style.text.hanging_punctuation = HangingPunctuation { first, last, end };
        style
    };
    let hangs = |layout: &Layout| -> Vec<(f32, f32)> {
        records(layout)
            .iter()
            .map(|line| (line.hang.start.to_px(), line.hang.end.to_px()))
            .collect()
    };
    fixture.text(&mut layout, &ahem(10.0), "XX XX, XX");
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(texts(&layout), ["XX ", "XX, ", "XX"]);
    for end in [HangEnd::Force, HangEnd::Allow] {
        fixture.text(&mut layout, &hanging(false, false, end), "XX XX, XX");
        fixture.lay_out(&mut layout, 50.0);
        assert_eq!(texts(&layout), ["XX XX, ", "XX"], "{end:?}");
        assert_eq!(widths(&layout), [50.0, 20.0], "{end:?}");
        assert_eq!(hangs(&layout)[0], (0.0, 10.0), "{end:?}");
    }
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(hangs(&layout)[0], (0.0, 0.0), "allow-end, with room");
    fixture.text(
        &mut layout,
        &hanging(false, false, HangEnd::Force),
        "XX XX, XX",
    );
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(hangs(&layout)[0], (0.0, 10.0), "force-end, with room");
    // `first`, and not after a forced break.
    fixture.build(
        &mut layout,
        &ComputedBlockStyle::new(&hanging(true, false, HangEnd::None)),
        |b| {
            b.text(NodeKey(1), "(XX XX");
            b.line_break(NodeKey(2));
            b.text(NodeKey(3), "(XX XX");
        },
    );
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(texts(&layout), ["(XX XX\n", "(XX ", "XX"]);
    assert_eq!(hangs(&layout)[0], (10.0, 0.0));
    assert_eq!(widths(&layout)[0], 50.0);
    // `last`, only where the text ends.
    fixture.text(&mut layout, &hanging(false, true, HangEnd::None), "XX) XX)");
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout), ["XX) XX)"]);
    assert_eq!(hangs(&layout)[0], (0.0, 10.0));
    fixture.lay_out(&mut layout, 30.0);
    assert_eq!(texts(&layout), ["XX) ", "XX)"]);
    assert_eq!(hangs(&layout)[0], (0.0, 0.0));
}

/// Negative spacing that takes the prefix back gives valid lines at any
/// width, walked as a negative margin is.
#[test]
fn negative_spacing_breaks_as_a_negative_margin_does() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = ahem(10.0);
    style.text.letter_spacing = LengthPercentage::from_px(-12.0);
    fixture.text(&mut layout, &style, "XXXX XXXX XXXX");
    assert!(
        layout
            .measured()
            .text(FirstLineVariant::Standard)
            .paragraph(ParagraphId::new(0))
            .contains(MeasureFlags::NONMONOTONE)
    );
    for width in [0.0, 5.0, 20.0, 100.0] {
        fixture.lay_out(&mut layout, width);
        assert!(layout.lines().len() > 0);
    }
}

/// Spacing, `line-padding` and indents of any size, NaN included, give
/// valid lines at any width.
#[test]
fn any_spacing_or_indent_gives_valid_lines() {
    use crate::style::{HangEnd, HangingPunctuation};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, -1e30] {
        let mut style = break_all(&sized(&LATIN, 20.0));
        style.text.letter_spacing = LengthPercentage::from_px(bad);
        style.text.word_spacing = LengthPercentage {
            px: bad,
            fraction: bad,
        };
        style.line.padding = bad;
        style.text.hanging_punctuation = HangingPunctuation {
            first: true,
            last: true,
            end: HangEnd::Allow,
        };
        for hanging in [false, true] {
            fixture.build(
                &mut layout,
                &ComputedBlockStyle {
                    style: &style,
                    ..indented(bad, bad, hanging, true)
                },
                |b| {
                    b.text(NodeKey(1), "(office, AVAVA.");
                    b.line_break(NodeKey(2));
                    b.text(NodeKey(3), "waffle)");
                },
            );
            for width in [0.0, 30.0, 500.0, f32::NAN] {
                fixture.lay_out(&mut layout, width);
            }
        }
    }
}
