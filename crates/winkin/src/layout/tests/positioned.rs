//! Absolutely positioned boxes: their static positions, and what their
//! anchors leave of line breaking.
//!
//! Every expected value is Chrome 153's, from a headless Chrome over the
//! same 10px Ahem: each box 4px square, `position: absolute`, no insets.

use super::*;
use crate::style::{
    LengthPercentage, TextCase, TextIndent, TextTransform, TextWrapStyle, WritingMode,
};
use crate::{OriginalDisplay, StaticPosition};

/// One call of a test build: text, an anchor, or a box around more.
#[derive(Copy, Clone)]
enum Call<'a> {
    Text(&'a str),
    Anchor,
    Block,
    Break,
    Open(&'a ComputedStyle<'a>),
    Close,
}

use Call::{Anchor, Block, Break, Close, Open, Text};

/// Builds `calls` into `layout` in `block`, the anchors keyed from 100 up,
/// and breaks it `width` wide.
fn lay_out(
    cx: &mut Context,
    layout: &mut Layout,
    block: &ComputedBlockStyle<'_>,
    calls: &[Call<'_>],
    width: f32,
) {
    build(cx, layout, block, |b| {
        let mut anchor = 100;
        for (key, call) in (1..).zip(calls) {
            match *call {
                Text(text) => b.text(NodeKey(key), text),
                Anchor => {
                    b.absolute(NodeKey(anchor), OriginalDisplay::Inline);
                    anchor += 1;
                }
                Block => {
                    b.absolute(NodeKey(anchor), OriginalDisplay::Block);
                    anchor += 1;
                }
                Break => b.line_break(NodeKey(key)),
                Open(style) => b.open_box(NodeKey(key), style, None),
                Close => b.close_box(),
            }
        }
    });
    layout.break_lines(cx, Area::new(width), &mut NoExclusions);
}

/// Each static position as `(line, inline, block, rtl)`.
fn statics(layout: &Layout) -> Vec<(Option<usize>, f32, f32, bool)> {
    layout
        .static_positions()
        .map(|at: StaticPosition| (at.line, at.inline, at.block, at.direction == Direction::Rtl))
        .collect()
}

/// The static positions of `calls` laid out `width` wide in `block`.
fn positions(
    block: &ComputedBlockStyle<'_>,
    calls: &[Call<'_>],
    width: f32,
) -> Vec<(Option<usize>, f32, f32, bool)> {
    let mut cx = context();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, block, calls, width);
    statics(&layout)
}

/// An anchor inside a line stands where it would have been, at the line's
/// top: in `aa<a>bb`, at 20. Each key comes back once, in content order.
#[test]
fn an_anchor_inside_a_line_stands_where_it_would_have_been() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let mut cx = context();
    let mut layout = Layout::new();
    lay_out(
        &mut cx,
        &mut layout,
        &block,
        &[Text("aa"), Anchor, Text("bb")],
        100.0,
    );
    assert_eq!(statics(&layout), [(Some(0), 20.0, 0.0, false)]);
    let keys: Vec<_> = layout.static_positions().map(|at| at.key).collect();
    assert_eq!(keys, [NodeKey(100)]);
    // No box fragment, and nothing else on the line: the two text nodes'
    // runs, side by side.
    assert_eq!(layout.box_fragments(NodeKey(100)).count(), 0);
    let line: Vec<_> = runs(&layout, 0)
        .iter()
        .map(|run| ends(run.inline()))
        .collect();
    assert_eq!(line, [(0.0, 20.0), (20.0, 40.0)]);
    // A line twice as tall as its text keeps the anchor at its top:
    // `12<span 20px>3</span><a>4`, at 40.
    let tall = sized(&AHEM_FAMILY, 20.0);
    let calls = [Text("12"), Open(&tall), Text("3"), Close, Anchor, Text("4")];
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(0), 40.0, 0.0, false)]
    );
    // Centered, it moves with its line: `12<a>34` at 50.
    let centered = ComputedBlockStyle {
        text_align: TextAlign::Center,
        ..block
    };
    let calls = [Text("12"), Anchor, Text("34")];
    assert_eq!(
        positions(&centered, &calls, 100.0),
        [(Some(0), 50.0, 0.0, false)]
    );
}

/// An anchor after the space a line breaks at ends the line where that
/// space fits, after the space is taken off, as Chrome's breaker breaks
/// after it (`HandleOverflow`). Where the space overflows, the anchor
/// starts the next line, as Chrome's breaker trails the space and stops
/// before the anchor. An anchor inside a box that goes on, or after an
/// empty box at the break, starts the next line; one after a box that
/// closes there ends the line. In 50px: `1234 <a>567` at 40 on the first
/// line, `12345 <a>67` at 0 on the second, and so on.
#[test]
fn an_anchor_at_a_break_falls_on_the_line_chromes_breaker_gives_it() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let first = (Some(0), 40.0, 0.0, false);
    let second = (Some(1), 0.0, 10.0, false);
    let cases: [(&[Call<'_>], &[_]); 6] = [
        (&[Text("1234 "), Anchor, Text("567")], &[first]),
        (&[Text("12345 "), Anchor, Text("67")], &[second]),
        (
            &[Text("1234 "), Anchor, Anchor, Text("567")],
            &[first, first],
        ),
        (
            &[Text("1234 "), Open(&root), Anchor, Text("567"), Close],
            &[second],
        ),
        (
            &[Open(&root), Text("1234 "), Close, Anchor, Text("567")],
            &[first],
        ),
        (
            &[Text("1234 "), Open(&root), Close, Anchor, Text("567")],
            &[second],
        ),
    ];
    for (calls, expected) in cases {
        assert_eq!(positions(&block, calls, 50.0), expected);
    }
    // An empty box after the anchor stays on the line with it, at 40 and 3
    // wide.
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 3.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let mut cx = context();
    let mut layout = Layout::new();
    let calls = [Text("1234 "), Anchor, Open(&padded), Close, Text("567")];
    lay_out(&mut cx, &mut layout, &block, &calls, 50.0);
    assert_eq!(statics(&layout), [first]);
    let pieces: Vec<_> = layout
        .box_fragments(NodeKey(3))
        .map(|piece| (piece.line(), ends(piece.inline())))
        .collect();
    assert_eq!(pieces, [(0, (40.0, 43.0))]);
    // Preserved, the space stays and the anchor stands after it: at 50 on
    // the first line, and on the second where the space overflows.
    let mut preserved = root;
    preserved.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let block = ComputedBlockStyle::new(&preserved);
    let calls = [Text("1234 "), Anchor, Text("567")];
    assert_eq!(
        positions(&block, &calls, 50.0),
        [(Some(0), 50.0, 0.0, false)]
    );
    let calls = [Text("12345 "), Anchor, Text("67")];
    assert_eq!(positions(&block, &calls, 50.0), [second]);
}

/// A balanced line ends where the scorer put its end, before an anchor
/// there, as Chrome's `break_at_` does: in 90px, `aaa bbb <a>ccc ddd` has
/// its anchor at 70 on the first line greedily, and at 0 on the second
/// balanced.
#[test]
fn a_balanced_break_puts_the_anchor_on_the_next_line() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let calls = [Text("aaa bbb "), Anchor, Text("ccc ddd")];
    let greedy = ComputedBlockStyle::new(&root);
    assert_eq!(
        positions(&greedy, &calls, 90.0),
        [(Some(0), 70.0, 0.0, false)]
    );
    let balanced = ComputedBlockStyle {
        text_wrap_style: TextWrapStyle::Balance,
        ..greedy
    };
    assert_eq!(
        positions(&balanced, &calls, 90.0),
        [(Some(1), 0.0, 10.0, false)]
    );
}

/// Next to a forced break, an anchor before the break ends its line and
/// one after it starts the next: `12<a><br>34` at 20 on the first line,
/// `12<br><a>34` at 0 on the second. After a final break it stands on the
/// empty line after the last, at 10.
#[test]
fn an_anchor_beside_a_forced_break_takes_its_side() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let calls = [Text("12"), Anchor, Break, Text("34")];
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(0), 20.0, 0.0, false)]
    );
    let calls = [Text("12"), Break, Anchor, Text("34")];
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(1), 0.0, 10.0, false)]
    );
    let calls = [Text("12"), Break, Anchor];
    assert_eq!(positions(&block, &calls, 100.0), [(None, 0.0, 10.0, false)]);
}

/// Right to left, an anchor takes its level from the text around it, as a
/// U+FFFC does, and its static position faces its level's way. In a
/// right-to-left block 100 wide:
/// - `aa<a>bbb`, left-to-right text, puts it at 70 facing right;
/// - overridden right to left, at 80 facing left;
/// - in 50px, `1234 <a>567` at 10 on the first line, facing left, and
///   `12345 <a>67` at 50 on the second.
#[test]
fn an_anchor_in_right_to_left_text_stands_where_reordering_puts_it() {
    let root = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            ..BidiGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, 10.0)
    };
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&root)
    };
    let calls = [Text("aa"), Anchor, Text("bbb")];
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(0), 70.0, 0.0, false)]
    );
    let calls = [Text("1234 "), Anchor, Text("567")];
    assert_eq!(
        positions(&block, &calls, 50.0),
        [(Some(0), 10.0, 0.0, true)]
    );
    let calls = [Text("12345 "), Anchor, Text("67")];
    assert_eq!(
        positions(&block, &calls, 50.0),
        [(Some(1), 50.0, 10.0, true)]
    );
    let overridden = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::BidiOverride,
        },
        ..root
    };
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&overridden)
    };
    let calls = [Text("aa"), Anchor, Text("bbb")];
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(0), 80.0, 0.0, true)]
    );
}

/// In vertical text the position is along the line and across the block,
/// as in horizontal text: `aa<a>bb` 20 down the first line, at its
/// block-start, which `vertical-rl` puts on the right and `vertical-lr` on
/// the left; `1234 <a>567` 40 down the first line of a 50px column.
#[test]
fn an_anchor_in_vertical_text_is_placed_along_its_line() {
    let root = sized(&AHEM_FAMILY, 10.0);
    for mode in [WritingMode::VerticalRl, WritingMode::VerticalLr] {
        let block = ComputedBlockStyle {
            writing_mode: mode,
            ..ComputedBlockStyle::new(&root)
        };
        let calls = [Text("aa"), Anchor, Text("bb")];
        assert_eq!(
            positions(&block, &calls, 100.0),
            [(Some(0), 20.0, 0.0, false)]
        );
        let calls = [Text("1234 "), Anchor, Text("567")];
        assert_eq!(
            positions(&block, &calls, 50.0),
            [(Some(0), 40.0, 0.0, false)]
        );
    }
}

/// In content with no line, an anchor stands where an empty first line
/// would start: at 0, at 50 centered in 100, and at 7 after a 7px indent,
/// which the anchor before `12` takes too.
#[test]
fn an_anchor_in_an_empty_paragraph_stands_on_an_empty_line() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let mut cx = context();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &block, &[Anchor], 100.0);
    assert_eq!(layout.lines().len(), 0);
    assert_eq!(statics(&layout), [(None, 0.0, 0.0, false)]);
    let centered = ComputedBlockStyle {
        text_align: TextAlign::Center,
        ..block
    };
    assert_eq!(
        positions(&centered, &[Anchor], 100.0),
        [(None, 50.0, 0.0, false)]
    );
    let indented = ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage {
                px: 7.0,
                fraction: 0.0,
            },
            hanging: false,
            each_line: false,
        },
        ..block
    };
    assert_eq!(
        positions(&indented, &[Anchor], 100.0),
        [(None, 7.0, 0.0, false)]
    );
    let calls = [Anchor, Text("12")];
    assert_eq!(
        positions(&indented, &calls, 100.0),
        [(Some(0), 7.0, 0.0, false)]
    );
}

/// A block-level box stands at the block's start edge, whatever the indent
/// or alignment, and below its line where in-flow content comes before it:
/// `12<div>34` at 0 and 10, `<div>12` at 0 and 0, right to left at 100 and
/// 10, facing left.
#[test]
fn a_block_level_box_stands_where_the_next_line_would_start() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let calls = [Text("12"), Block, Text("34")];
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(0), 0.0, 10.0, false)]
    );
    let calls = [Block, Text("12")];
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(0), 0.0, 0.0, false)]
    );
    let centered = ComputedBlockStyle {
        text_align: TextAlign::Center,
        ..block
    };
    let calls = [Text("12"), Block, Text("34")];
    assert_eq!(
        positions(&centered, &calls, 100.0),
        [(Some(0), 0.0, 10.0, false)]
    );
    let rtl = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            ..BidiGroup::INITIAL
        },
        ..root
    };
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&rtl)
    };
    assert_eq!(
        positions(&block, &calls, 100.0),
        [(Some(0), 100.0, 10.0, true)]
    );
}

/// An anchor changes no line: at every width, text with anchors between
/// its words, inside them and around boxes breaks as the same text without
/// them.
#[test]
fn anchors_leave_line_breaking_as_it_is() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let with = [
        Text("aaa "),
        Anchor,
        Text("bbb"),
        Anchor,
        Text(" cc"),
        Open(&root),
        Anchor,
        Text("dd ee"),
        Close,
        Anchor,
        Text(" fff "),
        Block,
        Text("g"),
    ];
    let without: Vec<_> = with
        .iter()
        .copied()
        .filter(|call| !matches!(call, Anchor | Block))
        .collect();
    let lines = |calls: &[Call<'_>], width: f32| {
        let mut cx = context();
        let mut layout = Layout::new();
        lay_out(&mut cx, &mut layout, &block, calls, width);
        layout
            .lines()
            .map(|line| (line.text_range(), line.metrics()))
            .collect::<Vec<_>>()
    };
    for width in (10..=200).step_by(5) {
        let width = width as f32;
        assert_eq!(lines(&with, width), lines(&without, width), "{width}");
    }
    // Capitalize reads past an anchor, as Chrome does (WPT
    // `text-transform-capitalize-033`): `p<a>ass` reads `Pass`.
    let mut capitalized = root;
    capitalized.text.transform = TextTransform {
        case: TextCase::Capitalize,
        ..TextTransform::NONE
    };
    let mut cx = context();
    let mut layout = Layout::new();
    let calls = [Text("p"), Anchor, Text("ass")];
    lay_out(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&capitalized),
        &calls,
        100.0,
    );
    assert_eq!(layout.text(), "Pass");
    // White space collapses across an anchor, and capitalize reads the
    // space left (`text-transform-capitalize-034`).
    let caps = Open(&capitalized);
    let cases: [&[Call<'_>]; 3] = [
        &[
            caps,
            Text("abc"),
            Close,
            Anchor,
            Text("\n"),
            caps,
            Text("abc"),
            Close,
        ],
        &[
            caps,
            Text("abc"),
            Close,
            Text(" "),
            Anchor,
            Text("\n"),
            caps,
            Text("abc"),
            Close,
        ],
        &[
            caps,
            Text("abc "),
            Anchor,
            Close,
            Text("\n"),
            caps,
            Text("abc"),
            Close,
        ],
    ];
    for calls in cases {
        let mut cx = context();
        let mut layout = Layout::new();
        lay_out(&mut cx, &mut layout, &block, calls, 500.0);
        assert_eq!(layout.text(), "Abc Abc");
    }
    // `aaa <a>bbb` fits 70 exactly, on one line.
    let calls = [Text("aaa "), Anchor, Text("bbb")];
    let mut cx = context();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &block, &calls, 70.0);
    assert_eq!(layout.lines().len(), 1);
    assert_eq!(statics(&layout), [(Some(0), 40.0, 0.0, false)]);
}
