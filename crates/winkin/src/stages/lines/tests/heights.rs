//! Line height tests:
//! - a line as tall as what is on it, refitted to the band it reaches, and
//!   widened only by the fonts on it;
//! - every `vertical-align` value, as Chrome's `ApplyBaselineShift` has it;
//! - `top` and `bottom` settling against the aligned subtree and each
//!   other, their subtrees moving with them;
//! - the strut of a box open across a line's start;
//! - the shifts a line settles, kept sorted;
//! - nestings and values of any size breaking into valid lines.

use alloc::vec::Vec;

use super::*;
use crate::paint::{Decorates, Paint};
use crate::stages::content::NodeId;
use crate::style::FirstLineVariant;
use crate::style::{LineGroup, LineHeight, VerticalAlign};
use crate::tests::vertically;
use crate::{CrossExtents, InlineExtents};

/// A line box is the strut united with everything on it.
///
/// The block ends after the last line.
#[test]
fn a_line_is_as_tall_as_what_is_on_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let large = ahem(40.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX XX ");
        b.open_box(NodeKey(2), &large, None);
        b.text(NodeKey(3), "YY");
        b.close_box();
        b.text(NodeKey(4), " XX");
    });
    fixture.lay_out(&mut layout, 160.0);
    assert_eq!(texts(&layout), ["XX XX ", "YY XX"]);
    let metrics: Vec<LineMetrics> = layout.lines().map(|line| line.metrics()).collect();
    assert_eq!((metrics[0].top, metrics[0].height()), (0.0, 20.0));
    assert_eq!((metrics[0].ascent, metrics[0].descent), (16.0, 4.0));
    assert_eq!((metrics[1].top, metrics[1].ascent), (20.0, 32.0));
    assert_eq!(metrics[1].height(), 40.0);
    assert_eq!(
        layout.line_records().block.block_end,
        LayoutUnit::from_px(60.0)
    );
    // Lines start at the area's block start.
    let area = Area {
        block_start: 7.0,
        ..Area::new(160.0)
    };
    fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
    assert_eq!(layout.line(1).expect("a line").metrics().top, 27.0);
}

/// A line grows only for the fonts its own text is shaped in, as Chrome's
/// `AccumulateUsedFonts` reads only the line's part of the shape result.
///
/// A `漢` in a font twice as tall makes its line 40px; Latin lines stay
/// 20px. A fixed line height ignores the fonts.
#[test]
fn a_line_takes_only_the_fonts_on_it() {
    let mut tall = TestFont::cjk("Test Tall Han", false);
    tall.win = (1600, 400);
    tall.hhea = (1600, -400, 0);
    tall.typo = (1600, -400, 0);
    let mut fixture = Fixture::new(
        &[latin(), tall],
        han_fallback("Test Tall Han"),
        StageCheck::Placed(check),
    );
    let mut layout = Layout::new();
    let style = sized(&LATIN, 20.0);
    let heights = |layout: &Layout| -> Vec<f32> {
        layout.lines().map(|line| line.metrics().height()).collect()
    };
    for (text, lines, expected) in [
        ("ab ab 漢", ["ab ab ", "漢"], [20.0, 40.0]),
        ("漢 ab ab", ["漢 ab ", "ab"], [40.0, 20.0]),
    ] {
        fixture.text(&mut layout, &style, text);
        fixture.lay_out(&mut layout, 50.0);
        assert_eq!(texts(&layout), lines, "{text}");
        assert_eq!(heights(&layout), expected, "{text}");
    }
    // Over many lines, and again at other widths, every line with a `漢` on
    // it is 40px and every other line 20px.
    let text = "ab 漢 ab ab ab ab 漢漢 ab ab ab ab ab 漢 ab";
    fixture.text(&mut layout, &style, text);
    for width in [50.0, 30.0, 80.0, 50.0] {
        fixture.lay_out(&mut layout, width);
        let lines = texts(&layout);
        assert!(lines.len() > 3, "{width}: {lines:?}");
        let expected: Vec<f32> = lines
            .iter()
            .map(|line| if line.contains('漢') { 40.0 } else { 20.0 })
            .collect();
        assert_eq!(heights(&layout), expected, "{width}: {lines:?}");
    }
    let fixed = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Factor(1.0),
            ..style.line
        },
        ..style
    };
    fixture.text(&mut layout, &fixed, "ab ab 漢");
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(heights(&layout), [20.0, 20.0]);
}

/// A line taller than the strut is fitted again in the narrower band it
/// reaches.
///
/// The host narrows the band from 30 px down.
#[test]
fn a_taller_line_is_refitted_to_the_band_it_reaches() {
    struct Shelf;
    impl Exclusions for Shelf {
        fn band(&self, _line: usize, block: BlockExtents) -> InlineExtents {
            let right = if block.end > 30.0 { 60.0 } else { 200.0 };
            InlineExtents { left: 0.0, right }
        }
        fn below(&self, _top: f32) -> Option<f32> {
            None
        }
        fn place(&mut self, float: FloatRequest) -> PlacedFloat {
            NoExclusions.place(float)
        }
        fn checkpoint(&self) -> ExclusionsCheckpoint {
            ExclusionsCheckpoint(0)
        }
        fn rewind(&mut self, _to: ExclusionsCheckpoint) {}
    }
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let large = ahem(40.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &large, None);
        b.text(NodeKey(2), "Y");
        b.close_box();
        b.text(NodeKey(3), " XX XX XX");
    });
    fixture.lay_out_with(&mut layout, Area::new(500.0), &mut Shelf);
    assert_eq!(texts(&layout)[0], "Y ");
    assert_eq!(layout.line(0).expect("a line").metrics().band.size(), 60.0);
    // A first line no taller than the strut keeps its band, and the lines
    // below the shelf take the narrower one.
    fixture.text(&mut layout, &root, "XX XX XX XX XX XX");
    fixture.lay_out_with(&mut layout, Area::new(500.0), &mut Shelf);
    assert_eq!(texts(&layout), ["XX XX XX ", "XX ", "XX ", "XX"]);
}

/// Returns `style` with a line height of `height`.
fn leaded<'a>(style: &ComputedStyle<'a>, height: LineHeight) -> ComputedStyle<'a> {
    ComputedStyle {
        line: LineGroup {
            height,
            ..style.line
        },
        ..*style
    }
}

/// Returns a box `block` pixels tall and 20 wide, with no baseline, so its
/// bottom edge sits on the baseline.
fn picture(block: f32) -> BoxSize {
    BoxSize {
        inline: 20.0,
        block,
        baseline: None,
    }
}

/// Returns each line's ascent and descent, in pixels.
fn extents(layout: &Layout) -> Vec<(f32, f32)> {
    layout
        .lines()
        .map(|line| {
            let metrics = line.metrics();
            (metrics.ascent, metrics.descent)
        })
        .collect()
}

/// Returns the shifts line `line` settles, as node and pixels.
fn settled(layout: &Layout, line: usize) -> Vec<(usize, f32)> {
    let lines = layout.line_records();
    let record = &records(layout)[line];
    lines
        .line_shifts(record)
        .iter()
        .map(|shift| (shift.node.get(), shift.shift.to_px()))
        .collect()
}

/// A picture on the baseline stands on it and grows the line above.
///
/// A picture pinned to the top or bottom aligns against the strut there.
/// Pictures pinned each way settle against each other: 60 and 40 make a
/// line of 60, not 100, as in Chrome.
#[test]
fn a_picture_pinned_to_the_top_or_bottom_is_the_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let cases = [
        (VerticalAlign::Baseline, (60.0, 4.0), Vec::new()),
        // Its top is on the strut's: 60 high, with the line's baseline 16
        // down it.
        (VerticalAlign::Top, (16.0, 44.0), alloc::vec![(2, -44.0)]),
        // Its bottom is on the strut's, 4 under the baseline.
        (VerticalAlign::Bottom, (56.0, 4.0), alloc::vec![(2, -4.0)]),
    ];
    for (align, extent, shifts) in cases {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "XX ");
            b.atomic(NodeKey(2), &vertically(&root, align), None, picture(60.0));
            b.text(NodeKey(3), " XX");
        });
        fixture.lay_out(&mut layout, 500.0);
        assert_eq!(extents(&layout), [extent], "{align:?}");
        assert_eq!(settled(&layout, 0), shifts, "{align:?}");
    }
    // One of each. The taller, pinned to the top, grows the line down. The
    // shorter, pinned to the bottom, stands on that bottom.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX ");
        b.atomic(
            NodeKey(2),
            &vertically(&root, VerticalAlign::Top),
            None,
            picture(60.0),
        );
        b.text(NodeKey(3), " XX ");
        b.atomic(
            NodeKey(4),
            &vertically(&root, VerticalAlign::Bottom),
            None,
            picture(40.0),
        );
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(extents(&layout), [(16.0, 44.0)]);
    assert_eq!(settled(&layout, 0), [(2, -44.0), (4, -44.0)]);
    // Line layout places each where the line settles it: the first's top on
    // the line's top, the second's bottom on the line's bottom.
    let line = layout.line(0).expect("a line");
    let blocks: Vec<(f32, f32)> = line
        .items()
        .filter_map(|item| match item {
            crate::Item::Atomic(atomic) => Some((atomic.block().over, atomic.block().under)),
            _ => None,
        })
        .collect();
    assert_eq!(blocks, [(0.0, 60.0), (20.0, 60.0)]);
}

/// Each `vertical-align` value shifts a box as Chrome's
/// `ApplyBaselineShift` does.
///
/// - `middle` centres a box half the parent's x-height above its baseline.
/// - `text-top` and `text-bottom` align the box's leading edge with the
///   parent's text edge.
/// - `super` and `sub` move by a third and a fifth of the parent's font
///   size, plus a pixel.
/// - A percentage is of the box's own line height.
#[test]
fn every_value_of_vertical_align_moves_a_box_as_chrome_moves_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = leaded(&ahem(20.0), LineHeight::Px(40.0));
    // A small box with a tall line: 10 px Ahem reaching 8 over its baseline
    // and 2 under, with 10 of leading on each side.
    let small = leaded(&ahem(10.0), LineHeight::Px(30.0));
    let x_height = {
        fixture.text(&mut layout, &root, "X");
        let nodes = &layout.content().nodes;
        let text = nodes.text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
        primary(&layout, text).map_or(0.0, |used| used.metrics.x_height.to_px())
    };
    let half_x = LayoutUnit::from_px(x_height / 2.0).to_px();
    let cases = [
        // Its extent is 18 over and 12 under, so its middle is 3 over.
        (VerticalAlign::Middle, half_x - 3.0, true),
        // Its top, 18 over, goes on Ahem's 16.
        (VerticalAlign::TextTop, -2.0, true),
        // Its bottom, 12 under, goes on Ahem's 4.
        (VerticalAlign::TextBottom, 8.0, true),
        // A third of 20, truncated to the grid, plus a pixel.
        (VerticalAlign::Super, 7.65625, false),
        // A fifth of 20 plus a pixel, down.
        (VerticalAlign::Sub, -5.0, false),
        (VerticalAlign::Px(6.5), 6.5, false),
        // Half its own line height of 30.
        (VerticalAlign::Fraction(0.5), 15.0, false),
    ];
    for (align, up, settled_here) in cases {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "XX ");
            b.open_box(NodeKey(2), &vertically(&small, align), None);
            b.text(NodeKey(3), "YY");
            b.close_box();
        });
        fixture.lay_out(&mut layout, 500.0);
        let (line, box_node) = (0, 2);
        let got = if settled_here {
            settled(&layout, line)
                .iter()
                .find(|&&(node, _)| node == box_node)
                .map_or(0.0, |&(_, shift)| shift)
        } else {
            assert!(settled(&layout, line).is_empty(), "{align:?}");
            layout
                .measured()
                .text(FirstLineVariant::Standard)
                .shift(NodeId::new(box_node))
                .to_px()
        };
        assert_eq!(got, up, "{align:?}");
        // The box and its text stand on the box's baseline, raised by the
        // same amount.
        let metrics = layout.line(0).expect("a line").metrics();
        let baselines: Vec<(u64, f32)> = layout
            .line(0)
            .expect("a line")
            .all_items()
            .filter_map(|item| match item {
                crate::Item::Text(run) => Some((run.key().0, run.baseline())),
                crate::Item::Box(piece) => Some((piece.key().0, piece.baseline())),
                _ => None,
            })
            .collect();
        let line_baseline = metrics.ascent;
        for (key, baseline) in baselines {
            let expected = if key == 1 {
                line_baseline
            } else {
                line_baseline - up
            };
            assert_eq!(baseline, expected, "{align:?}: {key}");
        }
        // The line contains the box.
        let reach = (18.0 + up).max(metrics.ascent);
        assert!(metrics.ascent >= reach, "{align:?}");
        assert!(metrics.descent >= 12.0 - up, "{align:?}");
    }
}

/// A box pinned to the top moves its content with it, even a larger box.
#[test]
fn a_pinned_box_takes_its_subtree_with_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let top = vertically(&root, VerticalAlign::Top);
    let large = ahem(40.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX ");
        b.open_box(NodeKey(2), &top, None);
        b.text(NodeKey(3), "YY ");
        b.open_box(NodeKey(4), &large, None);
        b.text(NodeKey(5), "ZZ");
        b.close_box();
        b.close_box();
    });
    fixture.lay_out(&mut layout, 500.0);
    // The 40 px pinned box's top is the strut's, so the line grows down.
    assert_eq!(extents(&layout), [(16.0, 24.0)]);
    assert_eq!(settled(&layout, 0), [(2, -16.0)]);
    let line = layout.line(0).expect("a line");
    let baselines: Vec<(u64, f32)> = line
        .items()
        .filter_map(|item| match item {
            crate::Item::Text(run) => Some((run.key().0, run.baseline())),
            _ => None,
        })
        .collect();
    assert_eq!(baselines, [(1, 16.0), (3, 32.0), (5, 32.0)]);
}

/// A box pinned to the top inside a raised box lands on the line's top, as
/// CSS 2.1 aligns its subtree.
///
/// Blink also moves it with the raised box, because Blink shifts fragments
/// by range. This layout does not.
#[test]
fn a_pinned_box_inside_a_raised_one_lands_on_the_lines_top() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let raised = vertically(&root, VerticalAlign::Px(5.0));
    let top = vertically(&ahem(40.0), VerticalAlign::Top);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX ");
        b.open_box(NodeKey(2), &raised, None);
        b.text(NodeKey(3), "YY ");
        b.open_box(NodeKey(4), &top, None);
        b.text(NodeKey(5), "ZZ");
        b.close_box();
        b.close_box();
    });
    fixture.lay_out(&mut layout, 500.0);
    let metrics = layout.line(0).expect("a line").metrics();
    let line = layout.line(0).expect("a line");
    let tops: Vec<(u64, f32)> = line
        .items()
        .filter_map(|item| match item {
            crate::Item::Text(run) => Some((run.key().0, run.block().over)),
            _ => None,
        })
        .collect();
    // The raised text reaches 21 over the line's baseline, and the pinned
    // box's 40 px run from the line's top.
    assert_eq!(metrics.ascent, 21.0);
    assert!(tops.contains(&(5, 0.0)), "{tops:?}");
    assert!(tops.contains(&(3, 0.0)), "{tops:?}");
    assert_eq!(metrics.height(), 40.0);
}

/// A box open across a line's start holds its strut on the line, even with
/// no text of its own there.
///
/// Blink carries an open box's text metrics to the next line the same way.
#[test]
fn a_box_open_across_a_break_holds_its_strut_there() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let tall = leaded(&root, LineHeight::Px(60.0));
    let solid = leaded(&root, LineHeight::Px(20.0));
    for shifted in [false, true] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "XX ");
            b.open_box(NodeKey(2), &tall, None);
            b.text(NodeKey(3), "YY ");
            b.open_box(NodeKey(4), &solid, None);
            b.text(NodeKey(5), "ZZ ZZ ZZ");
            b.close_box();
            b.close_box();
            if shifted {
                b.open_box(NodeKey(6), &vertically(&root, VerticalAlign::Sub), None);
                b.text(NodeKey(7), " WW");
                b.close_box();
            }
        });
        fixture.lay_out(&mut layout, 100.0);
        let heights: Vec<f32> = layout.lines().map(|line| line.metrics().height()).collect();
        // Every line holds the tall box's strut, the later ones because the
        // box is open across their starts.
        assert!(heights.len() >= 3, "{heights:?}");
        assert!(
            heights.iter().all(|&height| height == 60.0),
            "{shifted}: {heights:?}"
        );
    }
}

/// Boxes nested ten thousand deep, with every `vertical-align` and values
/// of every size, give valid lines.
///
/// Every step walks the nesting as a stack, and every sum saturates.
#[test]
fn deep_nestings_and_wild_values_break_into_valid_lines() {
    const DEPTH: u64 = 10_000;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let aligns = [
        VerticalAlign::Top,
        VerticalAlign::Bottom,
        VerticalAlign::Middle,
        VerticalAlign::TextTop,
        VerticalAlign::TextBottom,
        VerticalAlign::Px(f32::MAX),
        VerticalAlign::Px(f32::NAN),
        VerticalAlign::Px(-1e30),
        VerticalAlign::Fraction(f32::INFINITY),
        VerticalAlign::Fraction(-3.0),
        VerticalAlign::Super,
        VerticalAlign::Sub,
    ];
    let styles: Vec<ComputedStyle<'_>> = aligns
        .iter()
        .map(|&align| vertically(&root, align))
        .collect();
    let picture_at = |b: &mut LayoutBuilder<'_>, depth: u64| {
        b.atomic(
            NodeKey(200_000 + depth),
            &styles[(depth as usize + 3) % styles.len()],
            None,
            picture(1e30),
        );
    };
    // A word at every depth, and a picture at every seventh.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        for depth in 0..DEPTH {
            b.open_box(NodeKey(depth), &styles[depth as usize % styles.len()], None);
            b.text(NodeKey(100_000 + depth), "X ");
            if depth % 7 == 0 {
                picture_at(b, depth);
            }
        }
        for _ in 0..DEPTH {
            b.close_box();
        }
    });
    for width in [1e6, 2e5, 5e4] {
        fixture.lay_out(&mut layout, width);
        read_back_whole(&layout, DEPTH);
    }
    // Every box open before the first word.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        for depth in 0..DEPTH {
            b.open_box(NodeKey(depth), &styles[depth as usize % styles.len()], None);
        }
        b.text(NodeKey(100_000), "X X X X X X X X X X X X X X X X");
        picture_at(b, DEPTH);
        for _ in 0..DEPTH {
            b.close_box();
        }
    });
    for width in [0.0, 30.0, 200.0] {
        fixture.lay_out(&mut layout, width);
        read_back_whole(&layout, DEPTH);
    }
}

/// Reads all of `layout` as a host does, and checks every position is
/// finite.
///
/// The boxes are keyed from 0 to `depth`; it samples their fragments and
/// decorations.
fn read_back_whole(layout: &Layout, depth: u64) {
    let finite_along = |along: InlineExtents| along.left.is_finite() && along.right.is_finite();
    let finite_across = |across: CrossExtents| across.over.is_finite() && across.under.is_finite();
    assert!(layout.lines().count() > 0);
    let decorates = |key: NodeKey| {
        if key.0.is_multiple_of(1000) {
            Decorates::Both
        } else {
            Decorates::None
        }
    };
    for line in layout.lines() {
        let metrics = line.metrics();
        assert!(metrics.ascent.is_finite() && metrics.descent.is_finite());
        assert!(metrics.left.is_finite() && metrics.top.is_finite());
        for item in line.all_items() {
            match item {
                crate::Item::Text(run) | crate::Item::Generated(run) => {
                    assert!(run.baseline().is_finite() && finite_along(run.inline()));
                    for glyph in run.glyphs() {
                        assert!(glyph.x.is_finite() && glyph.y.is_finite());
                    }
                }
                crate::Item::Atomic(atomic) => {
                    assert!(finite_along(atomic.inline()) && finite_across(atomic.block()));
                }
                crate::Item::Box(part) => {
                    assert!(finite_along(part.inline()) && finite_across(part.block()));
                }
            }
        }
        for paint in line.paints(decorates) {
            if let Paint::DecorationBeforeText(bar) | Paint::DecorationAfterText(bar) = paint {
                assert!(bar.baseline().is_finite() && finite_along(bar.inline()));
            }
        }
    }
    for key in (0..depth).step_by(97) {
        for part in layout.box_fragments(NodeKey(key)) {
            assert!(finite_along(part.inline()) && finite_across(part.block()));
        }
    }
    assert_eq!(layout.floats().count(), 0);
    let block = layout.metrics();
    assert!(block.block_end.is_finite());
    let sizes = layout.intrinsic_sizes();
    assert!(sizes.min_content.is_finite() && sizes.max_content.is_finite());
}

/// A relayout settles the same shifts as a fresh layout, sorted by node.
#[test]
fn a_relayout_settles_what_a_fresh_layout_does() {
    let mut fixture = fixture();
    let root = ahem(20.0);
    let styles = [
        vertically(&root, VerticalAlign::Top),
        vertically(&ahem(30.0), VerticalAlign::Middle),
        vertically(&ahem(10.0), VerticalAlign::TextBottom),
        vertically(&root, VerticalAlign::Bottom),
    ];
    let build = |fixture: &mut Fixture, layout: &mut Layout| {
        fixture.build(layout, &ComputedBlockStyle::new(&root), |b| {
            let mut key = 0;
            for n in 0..24 {
                key += 1;
                b.text(NodeKey(key), "XX X ");
                key += 1;
                b.open_box(NodeKey(key), &styles[n % styles.len()], None);
                key += 1;
                b.text(NodeKey(key), "YY Y");
                key += 1;
                b.atomic(
                    NodeKey(key),
                    &styles[(n + 1) % styles.len()],
                    None,
                    picture(10.0 + n as f32),
                );
                b.close_box();
            }
        });
    };
    let mut warm = Layout::new();
    build(&mut fixture, &mut warm);
    for width in [55.0, 120.0, 330.0, 90.0] {
        fixture.lay_out(&mut warm, width);
        let mut fresh = Layout::new();
        build(&mut fixture, &mut fresh);
        fixture.lay_out(&mut fresh, width);
        assert_eq!(records(&warm), records(&fresh), "{width}");
        assert_eq!(warm.line_records().shifts(), fresh.line_records().shifts());
        assert!(!warm.line_records().shifts().is_empty());
        for line in 0..records(&warm).len() {
            let nodes: Vec<usize> = settled(&warm, line).iter().map(|&(node, _)| node).collect();
            assert!(nodes.windows(2).all(|w| w[0] < w[1]), "{nodes:?}");
        }
    }
}
