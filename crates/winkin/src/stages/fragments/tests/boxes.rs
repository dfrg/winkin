//! Box tests. They pin:
//! - which boxes keep an item and which are culled;
//! - atomic inlines on the baseline;
//! - a box covering the white space that hangs in it;
//! - box edges on the grid, and a cloned box's on every line.

use super::*;
use crate::style::FirstLineVariant;
use crate::style::LengthPercentage;

/// A box that paints or has room gets an item, and any other box is culled.
///
/// - Items come in visual order, pre-order.
/// - A kept box heads the items of its part of the line. Its border box
///   surrounds them, starting after its margin.
/// - A culled box's text belongs to the nearest painting box.
/// - A box broken across lines is open on the side where it continues.
#[test]
fn a_box_that_paints_gets_an_item_and_one_that_does_not_is_culled() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(10.0);
    let paints = ComputedStyle {
        paints: true,
        ..root
    };
    let roomy = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(2.0),
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX ");
        b.open_box(NodeKey(2), &roomy, None);
        b.text(NodeKey(3), "YY ");
        b.open_box(NodeKey(4), &root, None);
        b.text(NodeKey(5), "ZZ");
        b.close_box();
        b.close_box();
        b.text(NodeKey(6), " ");
        b.open_box(NodeKey(7), &paints, None);
        b.text(NodeKey(8), "WW");
        b.close_box();
    });
    fixture.lay_out(&mut layout, 500.0);
    let line = items(&layout, 0);
    let kinds: Vec<FragmentItemKind> = line.iter().map(|item| item.kind()).collect();
    use FragmentItemKind::{Box as B, Text as T};
    // XX_ [box: YY_ ZZ] _ [box: WW]
    assert_eq!(kinds, [T, B, T, T, T, B, T]);
    let nodes = &layout.content().nodes;
    let roomy_box = line[1];
    assert_eq!(roomy_box.descendants(), 2);
    assert_eq!(roomy_box.flags, FragmentItemFlags::NONE);
    // The box starts after `XX ` (30) and its margin (2). It holds 3 of
    // padding, `YY ZZ` (50) and 3 more.
    assert_eq!(roomy_box.inline, px(32.0));
    assert_eq!(roomy_box.size, LayoutUnit::from_px(56.0));
    assert_eq!(line[2].inline, px(35.0));
    assert_eq!(line[3].inline, px(65.0), "the culled box's text");
    let text = |node| nodes.text_facts(node, FirstLineVariant::Standard);
    assert_eq!(text(line[3].node), text(NodeId::BLOCK));
    // The space follows the box's closing padding and margin.
    assert_eq!(line[4].inline, px(90.0));
    let painted = line[5];
    assert_eq!(
        (painted.inline, painted.size),
        (px(100.0), LayoutUnit::from_px(20.0))
    );
    assert_eq!(painted.descendants(), 1);
    // Every box's block offset is its baseline, as its text's is.
    assert!(
        line.iter()
            .all(|item| item.block == LayoutUnit::from_px(8.0))
    );
    // Broken inside, the box with room is open to the right on the first
    // line and to the left on the second. It ends before the space that
    // hangs after `YY`.
    fixture.lay_out(&mut layout, 70.0);
    assert_eq!(texts(&layout)[0], "XX YY ");
    let first = items(&layout, 0);
    let part = first
        .iter()
        .find(|item| item.kind() == FragmentItemKind::Box)
        .expect("the box");
    assert_eq!(part.flags, FragmentItemFlags::OPEN_RIGHT);
    assert_eq!(
        (part.inline, part.size),
        (px(32.0), LayoutUnit::from_px(23.0))
    );
    let second = items(&layout, 1);
    let part = second
        .iter()
        .find(|item| item.kind() == FragmentItemKind::Box)
        .expect("the box");
    assert_eq!(part.flags, FragmentItemFlags::OPEN_LEFT);
    assert_eq!(
        (part.inline, part.size),
        (px(0.0), LayoutUnit::from_px(23.0))
    );
}

/// A box that paints nothing and takes no room keeps its fragment if shifted or in other metrics.
///
/// Blink keeps one for a box shifted or set in a font of other metrics than
/// its parent's. A box in the same font at the same size is culled.
#[test]
fn a_box_in_other_metrics_or_shifted_keeps_its_fragment() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(10.0);
    let larger = ahem(20.0);
    let raised = ComputedStyle {
        line: LineGroup {
            vertical_align: VerticalAlign::Super,
            ..root.line
        },
        ..root
    };
    for (style, kept) in [(root, false), (larger, true), (raised, true)] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "XX ");
            b.open_box(NodeKey(2), &style, None);
            b.text(NodeKey(3), "YY");
            b.close_box();
        });
        fixture.lay_out(&mut layout, 500.0);
        let boxes = items(&layout, 0)
            .iter()
            .filter(|item| item.kind() == FragmentItemKind::Box)
            .count();
        assert_eq!(boxes, usize::from(kept), "{style:?}");
        assert_eq!(
            layout
                .measured()
                .text(FirstLineVariant::Standard)
                .kept_boxes
                .get(NodeId::new(2))
                .is_some(),
            kept
        );
    }
}

/// An empty box keeps its fragment, painting or not, as Blink keeps one.
///
/// Its item heads nothing and is as wide as its own edges.
#[test]
fn an_empty_box_gets_an_item() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(10.0);
    let paints = ComputedStyle {
        paints: true,
        ..root
    };
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(4.0),
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX");
        b.open_box(NodeKey(2), &paints, None);
        b.close_box();
        b.text(NodeKey(3), "YY");
        b.open_box(NodeKey(4), &padded, None);
        b.close_box();
        b.text(NodeKey(5), "ZZ");
        b.open_box(NodeKey(6), &root, None);
        b.close_box();
    });
    fixture.lay_out(&mut layout, 500.0);
    let boxes: Vec<(InlineLayoutUnit, LayoutUnit, usize)> = items(&layout, 0)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Box)
        .map(|item| (item.inline, item.size, item.descendants()))
        .collect();
    assert_eq!(
        boxes,
        [
            (px(20.0), LayoutUnit::ZERO, 0),
            (px(40.0), LayoutUnit::from_px(8.0), 0),
            (px(68.0), LayoutUnit::ZERO, 0),
        ]
    );
}

/// An atomic inline stands on the line's baseline.
///
/// Its item's block offset is the line's baseline. The atomic's own baseline
/// sits there, or its margin box's bottom where it has none. Its size is its
/// margin box.
#[test]
fn an_atomic_inline_stands_on_its_baseline() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let margined = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(1.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    for (baseline, ascent) in [(Some(14.0), 16.0), (None, 30.0)] {
        let size = BoxSize {
            inline: 24.0,
            block: 28.0,
            baseline,
        };
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "XX");
            b.atomic(NodeKey(2), &margined, None, size);
            b.text(NodeKey(3), "XX");
        });
        fixture.lay_out(&mut layout, 500.0);
        let line = items(&layout, 0);
        let atomic = line
            .iter()
            .find(|item| item.kind() == FragmentItemKind::Atomic)
            .expect("the atomic");
        assert_eq!(atomic.block, LayoutUnit::from_px(ascent), "{baseline:?}");
        assert_eq!(
            (atomic.inline, atomic.size),
            (px(40.0), LayoutUnit::from_px(26.0))
        );
        assert!(line.iter().all(|item| item.block == atomic.block));
        assert_eq!(line.last().map(|item| item.inline), Some(px(66.0)));
    }
}

/// An inline box covers the white space hanging inside it, as Chrome's does.
///
/// The hanging piece belongs to its box, so the box's fragment spans it. The
/// line's width and alignment still leave it out. Right to left, it hangs
/// past the line's left, the box with it. The rectangles are Chrome 153's,
/// with painting boxes and 20 px Ahem in 60 px.
#[test]
fn a_box_covers_the_white_space_hanging_in_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let painted = |style: &ComputedStyle<'static>| ComputedStyle {
        paints: true,
        ..*style
    };
    let mut pre_wrap = root;
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let ltr = ComputedBlockStyle::new(&pre_wrap);
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::default()
    };
    let right = aligned(TextAlign::Right, TextAlignLast::Auto);
    let mut padded = painted(&pre_wrap);
    padded.edges.padding.right = LengthPercentage::from_px(5.0);
    // Each case is (block, style, text before, in the box, after, the box
    // on line 0).
    let cases = [
        (&ltr, &root, "X\u{3000}", "X\u{3000}", "", (40.0, 80.0)),
        (&ltr, &root, "X\u{3000}", "X\u{3000}", "XX", (40.0, 80.0)),
        (&ltr, &root, "XXX", "\u{3000}", "XX", (60.0, 80.0)),
        (&right, &root, "X", "X\u{3000}", "XX", (40.0, 80.0)),
        (&ltr, &pre_wrap, "X ", "X  ", "", (40.0, 100.0)),
        (&ltr, &pre_wrap, "X ", "X  ", "XX", (40.0, 100.0)),
        (&ltr, &pre_wrap, "XXX", "  ", "XX", (60.0, 100.0)),
        (&ltr, &pre_wrap, "XX", "X   ", "", (40.0, 120.0)),
        (&ltr, &pre_wrap, "X", "XX", " XX", (20.0, 60.0)),
        (&rtl, &pre_wrap, "XX", " ", "XX", (0.0, 20.0)),
        (&rtl, &pre_wrap, "XX", "  ", "XX", (-20.0, 20.0)),
        (&rtl, &root, "XX", "\u{3000}", "XX", (0.0, 20.0)),
        // A collapsible space is removed, and no box covers it.
        (&ltr, &root, "X ", "X  ", "XX", (40.0, 60.0)),
    ];
    for (block, style, before, inside, after, span) in cases {
        let boxed = painted(style);
        fixture.build(&mut layout, &ComputedBlockStyle { style, ..*block }, |b| {
            b.text(NodeKey(1), before);
            b.open_box(NodeKey(2), &boxed, None);
            b.text(NodeKey(3), inside);
            b.close_box();
            b.text(NodeKey(4), after);
        });
        fixture.lay_out(&mut layout, 60.0);
        assert_eq!(
            box_spans(&layout, 0),
            [span],
            "{before:?} [{inside:?}] {after:?}"
        );
    }
    // The same holds before a forced break as at the block's end.
    fixture.build(&mut layout, &ltr, |b| {
        b.text(NodeKey(1), "XX");
        b.open_box(NodeKey(2), &painted(&pre_wrap), None);
        b.text(NodeKey(3), "X   ");
        b.close_box();
        b.line_break(NodeKey(4));
        b.text(NodeKey(5), "XX");
    });
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(box_spans(&layout, 0), [(40.0, 120.0)]);
    // A box's end padding comes after what hangs in it, and the box spans
    // both.
    fixture.build(&mut layout, &ltr, |b| {
        b.text(NodeKey(1), "XX");
        b.open_box(NodeKey(2), &padded, None);
        b.text(NodeKey(3), "X ");
        b.close_box();
        b.text(NodeKey(4), "XX");
    });
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(box_spans(&layout, 0), [(40.0, 85.0)]);
    // Nested boxes each cover the hanging space.
    fixture.build(&mut layout, &ltr, |b| {
        b.text(NodeKey(1), "X");
        b.open_box(NodeKey(2), &painted(&pre_wrap), None);
        b.text(NodeKey(3), "X");
        b.open_box(NodeKey(4), &painted(&pre_wrap), None);
        b.text(NodeKey(5), "X ");
        b.close_box();
        b.close_box();
        b.text(NodeKey(6), "XX");
    });
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(box_spans(&layout, 0), [(20.0, 80.0), (40.0, 80.0)]);
}

/// A box's margins and padding are truncated onto the grid, as Chrome holds them.
///
/// - A left margin of 10.012px puts the border box 10px after `A`.
/// - A padding of 10.99px puts `B` 10.984375px inside it.
/// - A right margin of -10.012px pulls `C` back 10px.
#[test]
fn a_boxs_margins_and_padding_place_it_truncated_onto_the_grid() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let edged = ComputedStyle {
        paints: true,
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: 10.012,
                right: -10.012,
                ..Sides::ZERO
            }
            .into(),
            padding: Sides::<f32> {
                left: 10.99,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &edged, None);
        b.text(NodeKey(3), "B");
        b.close_box();
        b.text(NodeKey(4), "C");
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(box_parts(&layout, 0), [(2, 30.0, 60.984375, false, false)]);
    assert_eq!(
        places(&layout, 0),
        [
            ("A".into(), 0.0),
            ("B".into(), 40.984375),
            ("C".into(), 50.984375)
        ]
    );
}

/// A cloned box has both its edges on every line it reaches.
///
/// They take the room the breaker charged for them. A sliced box has an edge
/// only where it opens and where it closes.
#[test]
fn a_cloned_box_has_both_edges_on_every_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    for (decoration_break, first, second) in [
        (
            BoxDecorationBreak::Clone,
            (1, 0.0, 60.0, false, false),
            (1, 0.0, 60.0, false, false),
        ),
        (
            BoxDecorationBreak::Slice,
            (1, 0.0, 50.0, false, true),
            (1, 0.0, 50.0, true, false),
        ),
    ] {
        let padded = ComputedStyle {
            paints: true,
            edges: EdgesGroup {
                padding: Sides::<f32> {
                    left: 10.0,
                    right: 10.0,
                    ..Sides::ZERO
                }
                .into(),
                decoration_break,
                ..EdgesGroup::INITIAL
            },
            ..root
        };
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.open_box(NodeKey(1), &padded, None);
            b.text(NodeKey(2), "XX YY");
            b.close_box();
        });
        fixture.lay_out(&mut layout, 70.0);
        assert_eq!(texts(&layout), ["XX ", "YY"], "{decoration_break:?}");
        assert_eq!(box_parts(&layout, 0), [first], "{decoration_break:?}");
        assert_eq!(box_parts(&layout, 1), [second], "{decoration_break:?}");
        assert_eq!(places(&layout, 0)[0], ("X".into(), 10.0));
    }
}

/// A collapsible space among trailing ideographic spaces hangs with them;
/// only the spaces after the last of them are removed.
///
/// CSS Text 3, section 4.1.3, removes "a sequence of collapsible spaces at
/// the end of a line", and the rest hangs, as Chrome hangs it. In 20 px
/// Ahem in 50 px, the box holding the ideographic spaces with spaces among
/// them covers all six.
#[test]
fn a_space_among_hanging_ideographic_spaces_hangs_with_them() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let boxed = ComputedStyle {
        paints: true,
        ..root
    };
    for (inside, span) in [
        ("\u{3000}\u{3000} \u{3000} \u{3000}", (40.0, 160.0)),
        (" \u{3000}", (40.0, 80.0)),
        ("\u{3000} ", (40.0, 60.0)),
    ] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "XX");
            b.open_box(NodeKey(2), &boxed, None);
            b.text(NodeKey(3), inside);
            b.close_box();
            b.text(NodeKey(4), "XX");
        });
        fixture.lay_out(&mut layout, 50.0);
        assert_eq!(texts(&layout)[1], "XX", "{inside:?}");
        assert_eq!(box_spans(&layout, 0), [span], "{inside:?}");
    }
}
