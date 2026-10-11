//! `text-fit` tests, each a WPT case in css-text/text-fit. They pin:
//! - per-line, per-line-all and consistent scales, growing and shrinking,
//!   with limits;
//! - fixed and percentage spacing;
//! - line heights from `normal`, numbers and lengths;
//! - `vertical-align`, `text-box-trim`, emphasis marks and box edges on a
//!   scaled line;
//! - alignment, vertical lines, carets and hit testing on the scaled text;
//! - the blocks text-fit leaves alone.
//!
//! Each case lays out the test and its reference, which sets the scaled
//! sizes as font sizes, and compares what a painter reads: every line box
//! and every glyph with its font size. Chrome draws the test as its
//! reference, so the values pinned beside are Chrome's.

use super::*;
use crate::config::PastLines;
use crate::style::{
    LengthPercentage, TextBoxEdge, TextBoxTrim, TextEdgeOver, TextEdgeUnder, TextFit, TextFitKind,
    TextFitTarget, TextGroup, TextWrapMode, VerticalAlign, WritingMode,
};
use crate::tests::ahem;

/// A line as a painter reads it: its left, top, ascent and descent, and
/// each glyph's x and y from the block and its font size.
#[derive(Clone, PartialEq, Debug)]
struct Drawn {
    line: (f32, f32, f32, f32),
    glyphs: Vec<(f32, f32, f32)>,
}

/// Every line of `layout` as a painter reads it.
fn drawn(layout: &Layout) -> Vec<Drawn> {
    layout
        .lines()
        .map(|line| {
            let metrics = line.metrics();
            let mut glyphs = Vec::new();
            for item in line.items() {
                if let Item::Text(run) = item {
                    let size = run.font().map_or(0.0, |font| font.size);
                    for glyph in run.glyphs() {
                        glyphs.push((metrics.left + glyph.x, metrics.top + glyph.y, size));
                    }
                }
            }
            Drawn {
                line: (metrics.left, metrics.top, metrics.ascent, metrics.descent),
                glyphs,
            }
        })
        .collect()
}

/// Whether `test` draws as `reference` does to within a quarter pixel.
///
/// A scale that is no whole number puts the scaled font's ascent between
/// pixels, where the reference's font rounds it: Chrome scales the float
/// metrics, so the two differ by under a pixel there too.
fn draws_near(test: &[Drawn], reference: &[Drawn]) -> bool {
    let near = |a: f32, b: f32| (a - b).abs() <= 0.25;
    test.len() == reference.len()
        && test.iter().zip(reference).all(|(a, b)| {
            let (l, r) = (a.line, b.line);
            near(l.0, r.0)
                && near(l.1, r.1)
                && near(l.2, r.2)
                && near(l.3, r.3)
                && a.glyphs.len() == b.glyphs.len()
                && (a.glyphs.iter().zip(&b.glyphs))
                    .all(|(g, h)| near(g.0, h.0) && near(g.1, h.1) && g.2 == h.2)
        })
}

/// Lays out `calls` in `block` at `width`.
fn laid_out(
    cx: &mut Context,
    width: f32,
    block: &ComputedBlockStyle<'_>,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) -> Layout {
    let mut layout = Layout::new();
    build(cx, &mut layout, block, calls);
    layout.break_lines(cx, Area::new(width), &mut NoExclusions);
    layout
}

/// `block` with `text-fit` set to `kind`, `target` and `limit`.
fn fitted<'a>(
    block: &ComputedBlockStyle<'a>,
    kind: TextFitKind,
    target: TextFitTarget,
    limit: Option<f32>,
) -> ComputedBlockStyle<'a> {
    ComputedBlockStyle {
        text_fit: TextFit {
            kind,
            target,
            limit,
        },
        ..*block
    }
}

/// Ahem at `size` under `white-space: pre`, with `line-height`.
fn pre(size: f32, height: LineHeight) -> ComputedStyle<'static> {
    let mut style = ahem(size);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    style.text.wrap_mode = TextWrapMode::NoWrap;
    style.line.height = height;
    style
}

/// `style` at `size`.
fn resized(style: &ComputedStyle<'static>, size: f32) -> ComputedStyle<'static> {
    let mut style = *style;
    style.font.size = size;
    style
}

/// Each line's text-fit scale, read as its last run's font size over
/// `size`.
fn scales(layout: &Layout, size: f32) -> Vec<f32> {
    (0..layout.lines().len())
        .map(|n| {
            runs(layout, n)
                .last()
                .and_then(TextRun::font)
                .map_or(1.0, |font| font.size / size)
        })
        .collect()
}

/// grow-per-line.html: the first line, 80 px of 120, grows by 1.5 to 30 px;
/// the paragraph's last line keeps its size. The reference sets the first
/// line's text at 30 px.
#[test]
fn per_line_grows_each_line_but_the_last() {
    let mut cx = context();
    let style = ahem(20.0);
    let block = ComputedBlockStyle::new(&style);
    let fit = fitted(&block, TextFitKind::Grow, TextFitTarget::PerLine, None);
    let test = laid_out(&mut cx, 120.0, &fit, |b| {
        b.text(NodeKey(1), "ABCD EFGHIJ");
    });
    let big = resized(&style, 30.0);
    let reference = laid_out(&mut cx, 120.0, &block, |b| {
        b.open_box(NodeKey(1), &big, None);
        b.text(NodeKey(2), "ABCD ");
        b.close_box();
        b.text(NodeKey(3), "EFGHIJ");
    });
    assert_eq!(drawn(&test), drawn(&reference));
    assert_eq!(scales(&test, 20.0), [1.5, 1.0]);
    let first = test.line(0).map(|line| line.metrics());
    assert_eq!(first.map(|m| (m.width, m.height())), Some((120.0, 30.0)));
    assert_eq!(test.metrics().block_end, 50.0);
}

/// grow-consistent.html: lines of 60 and 30 px in 120 grow by the least
/// scale, 2, as Chrome's `MeasurePerBlockScale` takes it.
#[test]
fn consistent_grows_every_line_by_the_least_scale() {
    let mut cx = context();
    let style = pre(10.0, LineHeight::Normal);
    let block = ComputedBlockStyle::new(&style);
    let fit = fitted(&block, TextFitKind::Grow, TextFitTarget::Consistent, None);
    let test = laid_out(&mut cx, 120.0, &fit, |b| {
        b.text(NodeKey(1), "ABCDEF\nGHI");
    });
    let big = resized(&style, 20.0);
    let reference = laid_out(&mut cx, 120.0, &ComputedBlockStyle::new(&big), |b| {
        b.text(NodeKey(1), "ABCDEF\nGHI");
    });
    assert_eq!(drawn(&test), drawn(&reference));
    assert_eq!(scales(&test, 10.0), [2.0, 2.0]);
    assert_eq!(test.metrics().block_end, 40.0);
}

/// grow-per-line-all.html: a box's border keeps its size while the text
/// around it doubles; each line of a `pre` block takes its own scale; a
/// line height given as a length keeps its size.
#[test]
fn per_line_all_grows_every_line_and_keeps_lengths() {
    let mut cx = context();
    let style = pre(10.0, LineHeight::Normal);
    let block = ComputedBlockStyle::new(&style);
    let fit = fitted(&block, TextFitKind::Grow, TextFitTarget::PerLineAll, None);
    let bordered = |style: &ComputedStyle<'static>| ComputedStyle {
        edges: EdgesGroup {
            border: Sides::all(1.0),
            ..EdgesGroup::INITIAL
        },
        ..*style
    };
    let boxed = |size: f32| {
        let inner = bordered(&resized(&style, size));
        move |b: &mut LayoutBuilder<'_>| {
            b.text(NodeKey(1), "AB");
            b.open_box(NodeKey(2), &inner, None);
            b.text(NodeKey(3), "CD");
            b.close_box();
            b.text(NodeKey(4), "EF");
        }
    };
    let test = laid_out(&mut cx, 122.0, &fit, boxed(10.0));
    let big = resized(&style, 20.0);
    let reference = laid_out(&mut cx, 122.0, &ComputedBlockStyle::new(&big), boxed(20.0));
    assert_eq!(drawn(&test), drawn(&reference));
    let xs: Vec<f32> = drawn(&test)[0].glyphs.iter().map(|g| g.0).collect();
    assert_eq!(xs, [0.0, 20.0, 41.0, 61.0, 82.0, 102.0]);
    let border = |layout: &Layout| {
        let part = layout.box_fragments(NodeKey(2)).next();
        part.map(|part| (part.inline().left, part.inline().right, part.block()))
    };
    assert_eq!(border(&test), border(&reference));

    let test = laid_out(&mut cx, 120.0, &fit, |b| {
        b.text(NodeKey(1), "GHIJ\nKLM");
    });
    assert_eq!(scales(&test, 10.0), [3.0, 4.0]);
    let heights: Vec<f32> = test.lines().map(|line| line.metrics().height()).collect();
    assert_eq!(heights, [30.0, 40.0]);

    let fixed = pre(10.0, LineHeight::Px(20.0));
    let fit = fitted(
        &ComputedBlockStyle::new(&fixed),
        TextFitKind::Grow,
        TextFitTarget::PerLineAll,
        None,
    );
    let test = laid_out(&mut cx, 120.0, &fit, |b| {
        b.text(NodeKey(1), "ABC\nDEFGHI");
    });
    assert_eq!(scales(&test, 10.0), [4.0, 2.0]);
    let heights: Vec<f32> = test.lines().map(|line| line.metrics().height()).collect();
    assert_eq!(heights, [20.0, 20.0]);
    let mut reference = |size: f32, text: &'static str| {
        let style = pre(size, LineHeight::Px(20.0));
        let layout = laid_out(&mut cx, 120.0, &ComputedBlockStyle::new(&style), |b| {
            b.text(NodeKey(1), text);
        });
        drawn(&layout).remove(0)
    };
    let mut lines = drawn(&test);
    let second = lines.remove(1);
    assert_eq!(lines.remove(0), reference(40.0, "ABC"));
    let mut expected = reference(20.0, "DEFGHI");
    // The reference's second block stands under its first.
    expected.line.1 += 20.0;
    for glyph in &mut expected.glyphs {
        glyph.1 += 20.0;
    }
    assert_eq!(second, expected);
}

/// grow-per-line-all-line-height.html: the strut of each line scales with
/// it, so a line of 50 px text in a 10 px block is 50 px tall, and its
/// last line too.
#[test]
fn per_line_all_scales_each_line_strut() {
    let mut cx = context();
    let style = pre(10.0, LineHeight::Normal);
    let fit = fitted(
        &ComputedBlockStyle::new(&style),
        TextFitKind::Grow,
        TextFitTarget::PerLineAll,
        None,
    );
    let test = laid_out(&mut cx, 100.0, &fit, |b| {
        b.text(NodeKey(1), "AB\nCDEFG");
    });
    let block = ComputedBlockStyle::new(&style);
    let reference = laid_out(&mut cx, 100.0, &block, |b| {
        b.open_box(NodeKey(1), &resized(&style, 50.0), None);
        b.text(NodeKey(2), "AB");
        b.close_box();
        b.text(NodeKey(3), "\n");
        b.open_box(NodeKey(4), &resized(&style, 20.0), None);
        b.text(NodeKey(5), "CDEFG");
        b.close_box();
    });
    assert_eq!(drawn(&test), drawn(&reference));
    assert_eq!(test.metrics().block_end, 70.0);
}

/// shrink-consistent.html: a line of 160 px in 80 shrinks by half, and the
/// full line beside it takes no part in the scale but shrinks with it.
#[test]
fn consistent_shrinks_every_line_and_skips_full_lines() {
    let mut cx = context();
    let style = pre(20.0, LineHeight::Normal);
    let fit = fitted(
        &ComputedBlockStyle::new(&style),
        TextFitKind::Shrink,
        TextFitTarget::Consistent,
        None,
    );
    let test = laid_out(&mut cx, 80.0, &fit, |b| {
        b.text(NodeKey(1), "ABCDEFGH\nABCD");
    });
    let small = resized(&style, 10.0);
    let reference = laid_out(&mut cx, 80.0, &ComputedBlockStyle::new(&small), |b| {
        b.text(NodeKey(1), "ABCDEFGH\nABCD");
    });
    assert_eq!(drawn(&test), drawn(&reference));
    assert_eq!(scales(&test, 20.0), [0.5, 0.5]);
}

/// shrink-per-line-all.html: 200 px in 100 shrinks by half, or by 75% at
/// most under a 75% limit, and then overflows.
#[test]
fn a_limit_holds_how_far_text_shrinks() {
    let mut cx = context();
    let style = pre(20.0, LineHeight::Normal);
    let block = ComputedBlockStyle::new(&style);
    for (limit, size) in [(None, 10.0), (Some(0.75), 15.0)] {
        let fit = fitted(
            &block,
            TextFitKind::Shrink,
            TextFitTarget::PerLineAll,
            limit,
        );
        let test = laid_out(&mut cx, 100.0, &fit, |b| {
            b.text(NodeKey(1), "ABCDEABCDE");
        });
        let reference = resized(&style, size);
        let reference = laid_out(&mut cx, 100.0, &ComputedBlockStyle::new(&reference), |b| {
            b.text(NodeKey(1), "ABCDEABCDE");
        });
        assert_eq!(drawn(&test), drawn(&reference), "{limit:?}");
        let width = test.line(0).map(|line| line.metrics().width);
        assert_eq!(width, Some(size * 10.0));
    }
}

/// spacing.html: a percentage of letter- or word-spacing scales with the
/// text; a length keeps its size, and the text takes the rest of the line.
#[test]
fn fixed_spacing_keeps_its_size() {
    let mut cx = context();
    let base = pre(20.0, LineHeight::Factor(1.0));
    let spaced = |letter: LengthPercentage, word: LengthPercentage| {
        let mut style = base;
        style.text.letter_spacing = letter;
        style.text.word_spacing = word;
        style
    };
    let none = LengthPercentage::ZERO;
    let percent = LengthPercentage {
        px: 0.0,
        fraction: 0.5,
    };
    let px = LengthPercentage::from_px(10.0);
    let unspaced = spaced(none, none);
    // Each case's style, width, text, and the glyphs' left edges and font
    // size Chrome draws, as its reference sets them.
    let cases = [
        (spaced(percent, none), 100.0, "A", [0.0, 60.0], 40.0),
        (spaced(px, none), 110.0, "A", [0.0, 60.0], 50.0),
        (spaced(none, percent), 140.0, "A A", [0.0, 100.0], 40.0),
        (spaced(none, px), 130.0, "A A", [0.0, 90.0], 40.0),
    ];
    for target in [TextFitTarget::Consistent, TextFitTarget::PerLineAll] {
        for (style, width, text, xs, size) in &cases {
            let block = ComputedBlockStyle::new(style);
            let fit = fitted(&block, TextFitKind::Grow, target, None);
            // A single "A" is followed by one with no spacing, as the
            // test's span sets it.
            let test = laid_out(&mut cx, *width, &fit, |b| {
                b.text(NodeKey(1), text);
                if text.len() == 1 {
                    b.open_box(NodeKey(2), &unspaced, None);
                    b.text(NodeKey(3), "A");
                    b.close_box();
                }
            });
            let glyphs = &drawn(&test)[0].glyphs;
            let lefts: Vec<f32> = glyphs.iter().map(|g| g.0).collect();
            let wanted: Vec<f32> = if text.len() == 1 {
                xs.to_vec()
            } else {
                // The space's glyph stands after the first "A".
                vec![xs[0], xs[0] + size, xs[1]]
            };
            assert_eq!(lefts, wanted, "{target:?} {width}");
            assert!(glyphs.iter().all(|g| g.2 == *size), "{target:?} {width}");
            assert_eq!(walk(&test), [*width], "{target:?}");
        }
    }
    // A 75% limit holds 10 px of letter-spacing at 15 px text, 90 px in 40.
    let style = spaced(px, none);
    let block = ComputedBlockStyle::new(&style);
    let fit = fitted(
        &block,
        TextFitKind::Shrink,
        TextFitTarget::PerLineAll,
        Some(0.75),
    );
    let test = laid_out(&mut cx, 40.0, &fit, |b| {
        b.text(NodeKey(1), "ApA");
        b.open_box(NodeKey(2), &unspaced, None);
        b.text(NodeKey(3), "p");
        b.close_box();
    });
    let lefts: Vec<f32> = drawn(&test)[0].glyphs.iter().map(|g| g.0).collect();
    assert_eq!(lefts, [0.0, 25.0, 50.0, 75.0]);
    assert_eq!(walk(&test), [90.0]);
}

/// text-align.html: alignment sets the scaled width. Lines of 100 and 80
/// px in 120 grow by 1.2; lines of 100 and 160 shrink by 0.75, the least.
#[test]
fn alignment_sees_the_scaled_width() {
    let mut cx = context();
    let style = ahem(20.0);
    let block = ComputedBlockStyle {
        text_align: TextAlign::Right,
        ..ComputedBlockStyle::new(&style)
    };
    for (kind, text, size) in [
        (TextFitKind::Grow, "\nABCDE FGHI\n", 24.0),
        (TextFitKind::Shrink, "\nABCDE FGHIJKLM\n", 15.0),
    ] {
        let fit = fitted(&block, kind, TextFitTarget::Consistent, None);
        let test = laid_out(&mut cx, 120.0, &fit, |b| {
            b.text(NodeKey(1), text);
        });
        let reference = resized(&style, size);
        let reference = ComputedBlockStyle {
            style: &reference,
            ..block
        };
        let reference = laid_out(&mut cx, 120.0, &reference, |b| {
            b.text(NodeKey(1), text);
        });
        assert!(draws_near(&drawn(&test), &drawn(&reference)), "{kind:?}");
        // The lines' left edges, as the scaled widths put them.
        let lefts: Vec<f32> = test.lines().map(|line| line.metrics().left).collect();
        let wanted = if size == 24.0 {
            [0.0, 24.0]
        } else {
            [45.0, 0.0]
        };
        assert_eq!(lefts, wanted, "{kind:?}");
    }
}

/// consistent-blank-line.html: a line an atomic inline fills, an empty
/// line and a full line take no part in the scale, and every line takes
/// it, its `line-height: 2` doubling with the text.
#[test]
fn consistent_skips_full_and_empty_lines() {
    let mut cx = context();
    let wrap = pre(20.0, LineHeight::Factor(1.0));
    let wrap = ComputedStyle {
        text: TextGroup {
            white_space_collapse: WhiteSpaceCollapse::Collapse,
            wrap_mode: TextWrapMode::Wrap,
            ..wrap.text
        },
        ..wrap
    };
    let with_block = |b: &mut LayoutBuilder<'_>| {
        let size = BoxSize {
            inline: 160.0,
            block: 10.0,
            baseline: None,
        };
        b.atomic(NodeKey(1), &ComputedStyle::initial(), None, size);
        b.text(NodeKey(2), "xxxx");
    };
    let block = ComputedBlockStyle::new(&wrap);
    let grow = fitted(&block, TextFitKind::Grow, TextFitTarget::Consistent, None);
    let test = laid_out(&mut cx, 160.0, &grow, with_block);
    let big = resized(&wrap, 40.0);
    let reference = laid_out(&mut cx, 160.0, &ComputedBlockStyle::new(&big), with_block);
    assert_eq!(drawn(&test), drawn(&reference));
    assert_eq!(scales(&test, 20.0)[1], 2.0);

    for (kind, width, text, size) in [
        (TextFitKind::Grow, 160.0, "xxxx\n\nxx", 40.0),
        (TextFitKind::Shrink, 80.0, "xxxxxxxx\n\nxxxx", 10.0),
    ] {
        let style = pre(20.0, LineHeight::Factor(2.0));
        let block = ComputedBlockStyle::new(&style);
        let fit = fitted(&block, kind, TextFitTarget::Consistent, None);
        let test = laid_out(&mut cx, width, &fit, |b| {
            b.text(NodeKey(1), text);
        });
        let reference = pre(size, LineHeight::Factor(2.0));
        let reference = laid_out(&mut cx, width, &ComputedBlockStyle::new(&reference), |b| {
            b.text(NodeKey(1), text);
        });
        assert_eq!(drawn(&test), drawn(&reference), "{kind:?}");
        assert_eq!(test.metrics().block_end, 3.0 * 2.0 * size);
    }
}

/// line-height.html: `normal` and a number scale with the text, so the
/// block grows; a percentage, which the host gives as a length, and a
/// length keep their size.
#[test]
fn only_normal_and_number_line_heights_scale() {
    let mut cx = context();
    for (height, grows) in [
        (LineHeight::Normal, true),
        (LineHeight::Factor(1.6), true),
        (LineHeight::Px(24.0), false),
        (LineHeight::Px(32.0), false),
    ] {
        let mut style = ahem(16.0);
        style.line.height = height;
        let block = ComputedBlockStyle::new(&style);
        let fit = fitted(&block, TextFitKind::Grow, TextFitTarget::PerLineAll, None);
        let mut height = |block: &ComputedBlockStyle<'_>| {
            let layout = laid_out(&mut cx, 128.0, block, |b| {
                b.text(NodeKey(1), "xxxx");
            });
            layout.metrics().block_end
        };
        let (test, reference) = (height(&fit), height(&block));
        assert_eq!(test > reference, grows, "{style:?}");
        assert!(test >= reference);
    }
}

/// vertical-align.html: on a line doubled from 10 px, `sub` and `super`
/// move by the parent's scaled size and a percentage by the box's scaled
/// line height; a length keeps its size.
#[test]
fn vertical_align_shifts_scale_with_the_text() {
    let mut cx = context();
    let style = pre(10.0, LineHeight::Factor(1.0));
    let block = ComputedBlockStyle::new(&style);
    let fit = fitted(&block, TextFitKind::Grow, TextFitTarget::Consistent, None);
    let big = pre(20.0, LineHeight::Factor(1.0));
    for (align, test_align) in [
        (VerticalAlign::Baseline, VerticalAlign::Baseline),
        (VerticalAlign::Sub, VerticalAlign::Sub),
        (VerticalAlign::Super, VerticalAlign::Super),
        (VerticalAlign::Px(2.0), VerticalAlign::Px(2.0)),
        (VerticalAlign::Px(1.2), VerticalAlign::Px(1.2)),
        (VerticalAlign::Fraction(0.2), VerticalAlign::Fraction(0.2)),
    ] {
        let calls = |size: f32, align: VerticalAlign| {
            move |b: &mut LayoutBuilder<'_>| {
                let mut small = pre(size * 0.6, LineHeight::Factor(1.0));
                small.line.vertical_align = align;
                b.text(NodeKey(1), "AAA");
                b.open_box(NodeKey(2), &small, None);
                b.text(NodeKey(3), "B");
                b.close_box();
                b.text(NodeKey(4), "C");
            }
        };
        let test = laid_out(&mut cx, 92.0, &fit, calls(10.0, test_align));
        let reference = laid_out(
            &mut cx,
            92.0,
            &ComputedBlockStyle::new(&big),
            calls(20.0, align),
        );
        assert_eq!(drawn(&test), drawn(&reference), "{align:?}");
    }
}

/// text-box.html: the trimmed edges are the scaled font's, so a block
/// doubled from 20 px trims as a 40 px block does, and one halved as a 10
/// px block does.
#[test]
fn text_box_trim_takes_the_scaled_edges() {
    let mut cx = context();
    for (kind, text, size) in [
        (TextFitKind::Grow, "a b\nbc\ndef\n", 40.0),
        (TextFitKind::Shrink, "abc abc abcd\nbc m\ndef\n", 10.0),
    ] {
        let block = |style| ComputedBlockStyle {
            text_box_trim: TextBoxTrim::TrimBoth,
            text_box_edge: TextBoxEdge {
                over: TextEdgeOver::Cap,
                under: TextEdgeUnder::Text,
            },
            ..ComputedBlockStyle::new(style)
        };
        let style = pre(20.0, LineHeight::Factor(2.0));
        let fit = fitted(&block(&style), kind, TextFitTarget::Consistent, None);
        let test = laid_out(&mut cx, 120.0, &fit, |b| {
            b.text(NodeKey(1), text);
        });
        let reference = pre(size, LineHeight::Factor(2.0));
        let reference = laid_out(&mut cx, 120.0, &block(&reference), |b| {
            b.text(NodeKey(1), text);
        });
        assert_eq!(drawn(&test), drawn(&reference), "{kind:?}");
        assert_eq!(test.metrics(), reference.metrics(), "{kind:?}");
    }
}

/// text-emphasis.html: marks are set from the scaled size, and the line
/// makes room for them as for marks on text of that size.
#[test]
fn emphasis_marks_scale_with_the_text() {
    let mut cx = context();
    let style = |size: f32| {
        let mut style = marked(size, EmphasisSkip::INITIAL);
        style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
        style.text.wrap_mode = TextWrapMode::NoWrap;
        style.line.height = LineHeight::Factor(1.0);
        style
    };
    let marks = |layout: &Layout| -> Vec<(f32, f32, f32)> {
        let mut found = Vec::new();
        for (n, line) in layout.lines().enumerate() {
            let metrics = line.metrics();
            for run in runs(layout, n) {
                for mark in run.emphasis_marks() {
                    found.push((
                        metrics.left + mark.x,
                        metrics.top + mark.baseline,
                        mark.size,
                    ));
                }
            }
        }
        found
    };
    let small = style(20.0);
    let block = ComputedBlockStyle::new(&small);
    for (kind, width, text, size) in [
        (
            TextFitKind::Grow,
            160.0,
            "\u{c9}p\u{c9}\np\u{c9}p\u{c9}",
            40.0,
        ),
        (
            TextFitKind::Shrink,
            80.0,
            "\u{c9}p\u{c9}\np\u{c9}p\u{c9}p\u{c9}p\u{c9}",
            10.0,
        ),
    ] {
        let fit = fitted(&block, kind, TextFitTarget::Consistent, None);
        let test = laid_out(&mut cx, width, &fit, |b| {
            b.text(NodeKey(1), text);
        });
        let reference = style(size);
        let reference = laid_out(&mut cx, width, &ComputedBlockStyle::new(&reference), |b| {
            b.text(NodeKey(1), text);
        });
        assert_eq!(drawn(&test), drawn(&reference), "{kind:?}");
        assert_eq!(marks(&test), marks(&reference), "{kind:?}");
    }
}

/// writing-mode.html: vertical and sideways lines grow as horizontal ones
/// do.
#[test]
fn vertical_lines_scale_as_horizontal_ones() {
    let mut cx = context();
    for mode in [
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::SidewaysRl,
        WritingMode::SidewaysLr,
    ] {
        let style = pre(10.0, LineHeight::Normal);
        let block = ComputedBlockStyle {
            writing_mode: mode,
            ..ComputedBlockStyle::new(&style)
        };
        let fit = fitted(&block, TextFitKind::Grow, TextFitTarget::Consistent, None);
        let test = laid_out(&mut cx, 120.0, &fit, |b| {
            b.text(NodeKey(1), "ABCDEF\nGHI");
        });
        let big = pre(20.0, LineHeight::Normal);
        let reference = ComputedBlockStyle {
            style: &big,
            ..block
        };
        let reference = laid_out(&mut cx, 120.0, &reference, |b| {
            b.text(NodeKey(1), "ABCDEF\nGHI");
        });
        assert_eq!(drawn(&test), drawn(&reference), "{mode:?}");
    }
}

/// selection-highlight-painting.html and hittest.html: clusters, carets and
/// hits follow the scaled text, which is set at 2 and 0.8 times.
#[test]
fn carets_and_hits_follow_the_scaled_text() {
    let mut cx = context();
    let clusters = |layout: &Layout| -> Vec<(f32, f32)> {
        let mut found = Vec::new();
        for (n, line) in layout.lines().enumerate() {
            let left = line.metrics().left;
            for run in runs(layout, n) {
                found.extend(
                    run.clusters()
                        .map(|c| (left + c.inline().left, c.advance())),
                );
            }
        }
        found
    };
    for (kind, width, text, size) in [
        (TextFitKind::Grow, 160.0, "ppp\npppp", 40.0),
        (TextFitKind::Shrink, 64.0, "ppp\npppppppp", 8.0),
    ] {
        let style = pre(20.0, LineHeight::Factor(2.0));
        let block = ComputedBlockStyle::new(&style);
        let fit = fitted(&block, kind, TextFitTarget::Consistent, None);
        let test = laid_out(&mut cx, width, &fit, |b| {
            b.text(NodeKey(1), text);
        });
        let reference = pre(size, LineHeight::Factor(2.0));
        let reference = laid_out(&mut cx, width, &ComputedBlockStyle::new(&reference), |b| {
            b.text(NodeKey(1), text);
        });
        assert_eq!(clusters(&test), clusters(&reference), "{kind:?}");
        for x in [1.0, size * 1.4, size * 2.6] {
            let hit = |layout: &Layout| {
                layout
                    .hit_test(x, size, PastLines::Column)
                    .map(|position| position.offset)
            };
            assert_eq!(hit(&test), hit(&reference), "{kind:?} at {x}");
        }
    }
}

/// background-image.html and text-decoration-thickness.html: a span's
/// border box and its text's extents scale with the text.
#[test]
fn box_and_text_extents_scale_with_the_text() {
    let mut cx = context();
    let style = pre(20.0, LineHeight::Factor(2.0));
    let painted = ComputedStyle {
        paints: true,
        ..style
    };
    let block = ComputedBlockStyle::new(&style);
    let fit = fitted(&block, TextFitKind::Grow, TextFitTarget::Consistent, None);
    let calls = |inner: ComputedStyle<'static>| {
        move |b: &mut LayoutBuilder<'_>| {
            b.open_box(NodeKey(1), &inner, None);
            b.text(NodeKey(2), "\u{c9}p\u{c9}\np\u{c9}p\u{c9}");
            b.close_box();
        }
    };
    let test = laid_out(&mut cx, 160.0, &fit, calls(painted));
    let big = pre(40.0, LineHeight::Factor(2.0));
    let reference = laid_out(
        &mut cx,
        160.0,
        &ComputedBlockStyle::new(&big),
        calls(ComputedStyle {
            paints: true,
            ..big
        }),
    );
    assert_eq!(drawn(&test), drawn(&reference));
    let parts = |layout: &Layout| -> Vec<(f32, f32, CrossExtents)> {
        (layout.box_fragments(NodeKey(1)))
            .map(|part| (part.inline().left, part.inline().right, part.block()))
            .collect()
    };
    assert_eq!(parts(&test), parts(&reference));
    let blocks = |layout: &Layout| -> Vec<CrossExtents> {
        (0..layout.lines().len())
            .flat_map(|n| runs(layout, n).into_iter().map(|run| run.block()))
            .collect()
    };
    assert_eq!(blocks(&test), blocks(&reference));
}

/// A block with a float or an initial letter keeps its text's size, as
/// Chrome's `ShouldApplyTextFit` declines it; so does `text-fit: none`, and
/// a line within 2 px of its room.
#[test]
fn text_fit_leaves_some_blocks_alone() {
    let mut cx = context();
    let style = pre(10.0, LineHeight::Normal);
    let block = ComputedBlockStyle::new(&style);
    let grow = fitted(&block, TextFitKind::Grow, TextFitTarget::PerLineAll, None);
    let plain = laid_out(&mut cx, 120.0, &block, |b| {
        b.text(NodeKey(1), "ABC");
    });
    let none = fitted(&block, TextFitKind::None, TextFitTarget::PerLineAll, None);
    let test = laid_out(&mut cx, 120.0, &none, |b| {
        b.text(NodeKey(1), "ABC");
    });
    assert_eq!(drawn(&test), drawn(&plain));
    let lettered = ComputedStyle {
        line: LineGroup {
            initial_letter: InitialLetter {
                size: 2.0,
                sink: 2,
                ..InitialLetter::NONE
            },
            ..style.line
        },
        ..style
    };
    let test = laid_out(&mut cx, 120.0, &grow, |b| {
        b.open_box(NodeKey(1), &lettered, None);
        b.text(NodeKey(2), "A");
        b.close_box();
        b.text(NodeKey(3), "BC");
    });
    assert_eq!(scales(&test, 10.0), [1.0]);
    // 120 px of 121 is within the epsilon.
    let test = laid_out(&mut cx, 121.0, &grow, |b| {
        b.text(NodeKey(1), "ABCDEFGHIJKL");
    });
    assert_eq!(scales(&test, 10.0), [1.0]);
}

/// The scale applies on relayout as on a fresh layout, at each width.
#[test]
fn a_relayout_with_text_fit_equals_a_fresh_layout() {
    let mut cx = context();
    let style = ahem(20.0);
    let block = ComputedBlockStyle::new(&style);
    for target in [
        TextFitTarget::Consistent,
        TextFitTarget::PerLine,
        TextFitTarget::PerLineAll,
    ] {
        let fit = fitted(&block, TextFitKind::Grow, target, None);
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &fit, |b| {
            b.text(NodeKey(1), "ABCD EFGHIJ KL MNOPQRS");
        });
        for width in [120.0, 200.0, 75.0, 120.0] {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            let fresh = laid_out(&mut cx, width, &fit, |b| {
                b.text(NodeKey(1), "ABCD EFGHIJ KL MNOPQRS");
            });
            assert_eq!(drawn(&layout), drawn(&fresh), "{target:?} {width}");
        }
    }
}
