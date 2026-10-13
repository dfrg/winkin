//! Block tests. They pin:
//! - the block's end and baselines;
//! - every item on its box's baseline, shifted with it;
//! - `text-box-trim`, either way and on flipped lines;
//! - which writing modes flip their lines.

use super::*;

/// The block ends where its last line box does, and its baselines are its first and last lines'.
///
/// The case is three lines of Ahem at 20 px, from wherever the area starts.
/// A block with no lines has no baselines.
#[test]
fn the_block_ends_after_its_last_line_and_has_its_baselines() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle::new(&ahem(20.0)),
        "XX XX XX",
    );
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(layout.lines().len(), 3);
    let block = layout.line_records().block;
    assert_eq!(block.block_end, LayoutUnit::from_px(60.0));
    assert_eq!(block.first_baseline, Some(LayoutUnit::from_px(16.0)));
    assert_eq!(block.last_baseline, Some(LayoutUnit::from_px(56.0)));
    let area = Area {
        block_start: 7.5,
        ..Area::new(50.0)
    };
    fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
    let block = layout.line_records().block;
    assert_eq!(block.block_end, LayoutUnit::from_px(67.5));
    assert_eq!(block.first_baseline, Some(LayoutUnit::from_px(23.5)));
    assert_eq!(lefts(&layout), [0.0; 3]);
    let tops: Vec<LayoutUnit> = layout
        .fragments()
        .line_heads
        .as_slice()
        .iter()
        .map(|&head| layout.fragments().items[head].block)
        .collect();
    assert_eq!(tops, [7.5, 27.5, 47.5].map(LayoutUnit::from_px));
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&ahem(20.0)), "");
    fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
    let block = layout.line_records().block;
    assert_eq!(block.block_end, LayoutUnit::from_px(7.5));
    assert_eq!((block.first_baseline, block.last_baseline), (None, None));
}

/// Returns `style` with a line height of `height` pixels.
fn leaded_px<'a>(style: &ComputedStyle<'a>, height: f32) -> ComputedStyle<'a> {
    ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(height),
            ..style.line
        },
        ..*style
    }
}

/// Returns each item's kind and baseline from its line box's top, in pixels.
fn baselines(layout: &Layout, n: usize) -> Vec<(FragmentItemKind, f32)> {
    items(layout, n)
        .iter()
        .map(|item| (item.kind(), item.block.to_px()))
        .collect()
}

/// Every item stands on its box's baseline, raised as its box is.
///
/// That covers a raised box's text, its own item and an atomic inline inside
/// it, and an atomic shifted on its own beside it. A culled box inside a
/// raised one is read back on the raised one's baseline.
#[test]
fn every_item_stands_on_its_boxs_baseline() {
    use crate::style::VerticalAlign;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let raised = vertically(&root, VerticalAlign::Px(5.0));
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "X");
        b.open_box(NodeKey(2), &raised, None);
        b.text(NodeKey(3), "Y");
        b.open_box(NodeKey(4), &root, None);
        b.text(NodeKey(5), "Z");
        b.close_box();
        b.atomic(
            NodeKey(6),
            &root,
            None,
            BoxSize {
                inline: 10.0,
                block: 10.0,
                baseline: None,
            },
        );
        b.close_box();
        b.atomic(
            NodeKey(7),
            &vertically(&root, VerticalAlign::Px(-3.0)),
            None,
            BoxSize {
                inline: 10.0,
                block: 10.0,
                baseline: None,
            },
        );
    });
    fixture.lay_out(&mut layout, 500.0);
    // The line reaches 21 over its baseline, at the raised text's top. It
    // reaches 4 under, the strut's descent, since the lowered picture's
    // bottom is only 3 under.
    let line = layout.line(0).expect("a line").metrics();
    assert_eq!((line.ascent, line.descent), (21.0, 4.0));
    assert_eq!(
        baselines(&layout, 0),
        [
            (FragmentItemKind::Text, 21.0),
            (FragmentItemKind::Box, 16.0),
            (FragmentItemKind::Text, 16.0),
            (FragmentItemKind::Text, 16.0),
            (FragmentItemKind::Atomic, 16.0),
            (FragmentItemKind::Atomic, 24.0),
        ]
    );
    // The culled box, node 4, stands where the raised one does.
    let culled: Vec<f32> = layout
        .box_fragments(NodeKey(4))
        .map(|piece| piece.baseline())
        .collect();
    assert_eq!(culled, [16.0]);
}

/// `text-box-trim` takes the leading off the block's first and last lines, as Chrome's `ApplyTextBoxTrim` does.
///
/// - The first line loses what its line box reaches past the over edge, and
///   the last what it reaches past the under edge.
/// - Every line moves up by the first trim, and the block's end by both. The
///   lines keep their spacing.
/// - `text-box-edge` places each edge in the block's primary font: ascent and
///   descent for `text`, cap height, x-height or the alphabetic baseline.
///   Beyond Chrome, it also takes the ideographic em box.
#[test]
fn text_box_trim_takes_the_leading_off_the_ends() {
    use crate::style::{TextBoxEdge, TextBoxTrim, TextEdgeOver, TextEdgeUnder};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    // The font at 20 px is 16 over and 4 under, with a cap height of 14 and
    // an x-height of 10. With lines of 40, each line box is 26 over its
    // baseline and 14 under.
    let root = leaded_px(&sized(&NARROW, 20.0), 40.0);
    let edge = |over, under| TextBoxEdge { over, under };
    let cases = [
        (TextBoxTrim::None, TextBoxEdge::AUTO, (0.0, 0.0)),
        (TextBoxTrim::TrimStart, TextBoxEdge::AUTO, (10.0, 0.0)),
        (TextBoxTrim::TrimEnd, TextBoxEdge::AUTO, (0.0, 10.0)),
        (TextBoxTrim::TrimBoth, TextBoxEdge::AUTO, (10.0, 10.0)),
        (
            TextBoxTrim::TrimBoth,
            edge(TextEdgeOver::Cap, TextEdgeUnder::Alphabetic),
            (12.0, 14.0),
        ),
        (
            TextBoxTrim::TrimBoth,
            edge(TextEdgeOver::Ex, TextEdgeUnder::Text),
            (16.0, 10.0),
        ),
        // The em box is 20 with its bottom at the descent, so 16 over and 4
        // under.
        (
            TextBoxTrim::TrimBoth,
            edge(TextEdgeOver::Ideographic, TextEdgeUnder::Ideographic),
            (10.0, 10.0),
        ),
    ];
    for (trim, edges, (start, end)) in cases {
        let block = ComputedBlockStyle {
            text_box_trim: trim,
            text_box_edge: edges,
            ..ComputedBlockStyle::new(&root)
        };
        fixture.build(&mut layout, &block, |b| {
            b.text(NodeKey(1), "aaaa aaaa");
        });
        fixture.lay_out(&mut layout, 50.0);
        let metrics = layout.metrics();
        assert_eq!(
            (metrics.trim_start, metrics.trim_end),
            (start, end),
            "{trim:?} {edges:?}"
        );
        let tops: Vec<f32> = layout.lines().map(|line| line.metrics().top).collect();
        assert_eq!(tops, [-start, 40.0 - start], "{trim:?} {edges:?}");
        assert_eq!(metrics.block_end, 80.0 - start - end);
        assert_eq!(metrics.first_baseline, Some(26.0 - start));
        assert_eq!(metrics.last_baseline, Some(66.0 - start));
    }
}

/// `text-box-trim` can be negative, follows flipped lines, and trims nothing without a line.
///
/// - A line box shorter than its text, under a small `line-height`, trims by
///   a negative amount. The first line moves down to its edge and the
///   block's end moves out, as in Chrome.
/// - Where lines flip, as `vertical-lr` stacks them, the start is a line's
///   under side.
/// - A block with no line trims nothing.
#[test]
fn text_box_trim_goes_either_way_and_follows_the_lines() {
    use crate::style::{TextBoxTrim, WritingMode};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    // A line of 10 in Ahem at 20 is 11 over its baseline and one under,
    // negative.
    let tight = leaded_px(&ahem(20.0), 10.0);
    let block = ComputedBlockStyle {
        text_box_trim: TextBoxTrim::TrimBoth,
        ..ComputedBlockStyle::new(&tight)
    };
    fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), "XX XX"));
    fixture.lay_out(&mut layout, 50.0);
    let metrics = layout.metrics();
    assert_eq!((metrics.trim_start, metrics.trim_end), (-5.0, -5.0));
    assert_eq!(metrics.block_end, 30.0);
    assert_eq!(layout.line(0).expect("a line").metrics().top, 5.0);
    // Flipped, the start is the under side: the line box's 14 under the
    // baseline less the text's 4.
    let loose = leaded_px(&ahem(20.0), 40.0);
    let flipped = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalLr,
        text_box_trim: TextBoxTrim::TrimStart,
        ..ComputedBlockStyle::new(&loose)
    };
    fixture.build(&mut layout, &flipped, |b| b.text(NodeKey(1), "XX"));
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(layout.metrics().trim_start, 10.0);
    // A block with no line trims nothing.
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &loose,
            ..block
        },
        |_| {},
    );
    fixture.lay_out(&mut layout, 500.0);
    let metrics = layout.metrics();
    assert_eq!((metrics.trim_start, metrics.trim_end), (0.0, 0.0));
    assert_eq!(metrics.first_baseline, None);
}

/// `text-box-trim` measures the first line's edges in its `::first-line`
/// font, as in `text-box-trim-first-line-pseudo-001` and `-002`: Chrome's
/// `ApplyTextBoxTrim` reads the line's style.
///
/// - Ahem at 20 px on lines twice its size, with a `::first-line` of 40 px,
///   trims the first line's 20 px of half-leading, not the 36 px from it
///   to a 20 px ascent.
/// - At 40 px with a `::first-line` of 20 px, it trims that line's 10.
#[test]
fn text_box_trim_measures_the_first_line_in_its_own_font() {
    use crate::style::TextBoxTrim;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let doubled = |size| ComputedStyle {
        line: LineGroup {
            height: LineHeight::Factor(2.0),
            ..LineGroup::INITIAL
        },
        ..ahem(size)
    };
    for (size, first_size, trimmed) in [(20.0, 40.0, 20.0), (40.0, 20.0, 10.0)] {
        let (root, first_line) = (doubled(size), doubled(first_size));
        let block = ComputedBlockStyle {
            text_box_trim: TextBoxTrim::TrimStart,
            first_line: Some(&first_line),
            ..ComputedBlockStyle::new(&root)
        };
        fixture.build(&mut layout, &block, |b| {
            b.text(NodeKey(1), "XX");
            b.line_break(NodeKey(2));
            b.text(NodeKey(3), "XX");
        });
        fixture.lay_out(&mut layout, 500.0);
        assert_eq!(layout.metrics().trim_start, trimmed, "{size} {first_size}");
    }
}

/// Only `vertical-lr` flips its lines.
///
/// `sideways-lr` stacks lines from the left, as `vertical-lr` does. But they
/// run up the page with their over side on the left, where they stack from,
/// as CSS Writing Modes 4 says. Blink's `IsFlippedLinesWritingMode` is true
/// for `vertical-lr` alone. So `sideways-lr`'s start trims a line's over
/// side, as a horizontal block's does.
///
/// The values are Chrome 153's, for Ahem at 20 px on lines of 40 under
/// `text-box-edge: cap alphabetic`. A line is 26 over the baseline and 14
/// under, with the cap edge 16 over. One line's block is 30 trimmed at its
/// start and 26 at its end in `sideways-lr`, `sideways-rl` and `vertical-rl`.
/// It is 26 and 30 in `vertical-lr`.
#[test]
fn only_vertical_lr_flips_its_lines() {
    use crate::style::{TextBoxEdge, TextBoxTrim, TextEdgeOver, TextEdgeUnder, WritingMode};
    use TextBoxTrim::{TrimBoth, TrimEnd, TrimStart};
    use WritingMode::{SidewaysLr, SidewaysRl, VerticalLr, VerticalRl};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let loose = leaded_px(&ahem(20.0), 40.0);
    let cap = TextBoxEdge {
        over: TextEdgeOver::Cap,
        under: TextEdgeUnder::Alphabetic,
    };
    let cases = [
        (SidewaysLr, TrimStart, 30.0),
        (SidewaysLr, TrimEnd, 26.0),
        (SidewaysLr, TrimBoth, 16.0),
        (SidewaysRl, TrimStart, 30.0),
        (SidewaysRl, TrimEnd, 26.0),
        (VerticalRl, TrimStart, 30.0),
        (VerticalRl, TrimEnd, 26.0),
        (VerticalLr, TrimStart, 26.0),
        (VerticalLr, TrimEnd, 30.0),
        (VerticalLr, TrimBoth, 16.0),
    ];
    let mut got = Vec::new();
    for (mode, trim, _) in cases {
        let block = ComputedBlockStyle {
            writing_mode: mode,
            text_box_trim: trim,
            text_box_edge: cap,
            ..ComputedBlockStyle::new(&loose)
        };
        fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), "XX"));
        fixture.lay_out(&mut layout, 500.0);
        got.push((mode, trim, layout.metrics().block_end));
    }
    assert_eq!(got, cases);
}
