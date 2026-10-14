//! Box tests. They pin:
//! - the room between items, the edges of the boxes between them;
//! - culled boxes answered from what they hold, split by reordering;
//! - atomic inlines as their margin boxes.

use super::*;
use crate::build::FloatSide;
use crate::style::{FirstLineVariant, LengthPercentage, VerticalAlign, WritingMode};

/// Returns a box's edge widths along a line, `left` and `right`.
fn edge_room(left: f32, right: f32) -> InlineEdges {
    InlineEdges { left, right }
}

/// The room between two items is exactly the edges of the boxes between
/// them, as each kept box part reports them: margin, border and padding on
/// its closed sides, and nothing on an open one.
#[test]
fn the_room_between_items_is_the_edges_of_the_boxes_between_them() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 10.0);
    let roomy = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(2.0),
            border: Sides::all(1.0),
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let odd = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: 0.3,
                right: -1.2,
                ..Sides::ZERO
            }
            .into(),
            padding: Sides::from_px(0.7),
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &roomy, None);
        b.text(NodeKey(2), "XX ");
        b.open_box(NodeKey(3), &odd, None);
        b.text(NodeKey(4), "YY ZZ");
        b.close_box();
        b.close_box();
        b.text(NodeKey(5), " WW");
        b.open_box(NodeKey(6), &roomy, None);
        b.close_box();
    });
    for width in [1000.0, 70.0, 45.0, 10.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        walk(&layout);
    }
    // All on one line: the walk ends past the empty box at the end.
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    let end = walk(&layout)[0];
    // The odd box's edges truncated onto the grid, as Chrome holds them:
    // 0.3 is 19/64, 0.7 is 44/64 and -1.2 is -76/64.
    let expected = 6.0 + 30.0 + (0.296875 + 0.6875) + 50.0 + (0.6875 - 1.1875) + 6.0 + 30.0 + 12.0;
    assert!((end - expected).abs() < 1e-3, "{end} against {expected}");
    // Broken inside the outer box, its first part is open to the right
    // and its second to the left, with no room there.
    layout.break_lines(&mut cx, Area::new(45.0), &mut NoExclusions);
    let first: Vec<_> = layout
        .box_fragments(NodeKey(1))
        .map(|piece| {
            (
                piece.line(),
                piece.edges(),
                piece.is_open_left(),
                piece.is_open_right(),
            )
        })
        .collect();
    assert_eq!(first[0], (0, edge_room(6.0, 0.0), false, true));
    assert_eq!(first[1].1.left, 0.0);
    assert!(first[1].2);
}

/// A box that keeps a fragment is answered with its items. A culled box is
/// answered from its descendants, one part per line, as wide as they are and
/// as tall as its text, open where it continues past a line.
#[test]
fn a_culled_box_is_answered_from_what_it_holds() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 10.0);
    let paints = ComputedStyle {
        paints: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "AA ");
        b.open_box(NodeKey(2), &root, None);
        b.text(NodeKey(3), "BB CC ");
        b.open_box(NodeKey(4), &paints, None);
        b.text(NodeKey(5), "DD");
        b.close_box();
        b.close_box();
        b.text(NodeKey(6), " EE");
    });
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    let culled: Vec<_> = layout.box_fragments(NodeKey(2)).collect();
    assert_eq!(culled.len(), 1);
    let piece = culled[0];
    assert!(piece.is_culled());
    assert_eq!((piece.line(), piece.key()), (0, NodeKey(2)));
    assert_eq!(
        piece.inline(),
        along(30.0, 110.0),
        "`BB CC ` and the kept box's `DD`"
    );
    assert_eq!(piece.block(), across(0.0, 10.0));
    assert_eq!(piece.baseline(), 8.0);
    assert!(!piece.is_open_left() && !piece.is_open_right());
    assert_eq!(piece.edges(), edge_room(0.0, 0.0));
    let kept: Vec<_> = layout.box_fragments(NodeKey(4)).collect();
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].is_culled());
    assert_eq!(kept[0].inline(), along(90.0, 110.0));
    assert_eq!(kept[0].descendants(), 1);
    // The same as the line's own box item.
    let boxes: Vec<_> = layout
        .line(0)
        .expect("a line")
        .items()
        .filter_map(|item| match item {
            Item::Box(piece) => Some(piece.inline()),
            _ => None,
        })
        .collect();
    assert_eq!(boxes, [kept[0].inline()]);
    // Across two lines: a part on each, open on the side it continues past,
    // and none for a key no box has.
    layout.break_lines(&mut cx, Area::new(60.0), &mut NoExclusions);
    let texts: Vec<&str> = layout
        .lines()
        .map(|line| &layout.text()[line.text_range()])
        .collect();
    assert_eq!(texts, ["AA BB ", "CC DD ", "EE"]);
    let culled: Vec<_> = layout
        .box_fragments(NodeKey(2))
        .map(|piece| {
            (
                piece.line(),
                piece.inline(),
                piece.is_open_left(),
                piece.is_open_right(),
            )
        })
        .collect();
    assert_eq!(
        culled,
        [
            (0, along(30.0, 50.0), false, true),
            (1, along(0.0, 50.0), true, false)
        ]
    );
    assert_eq!(layout.box_fragments(NodeKey(3)).count(), 0, "a text node");
    assert_eq!(layout.box_fragments(NodeKey(99)).count(), 0);
    // An outline keeps a fragment as a background does.
    let outlines = ComputedStyle {
        outlines: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "AA ");
        b.open_box(NodeKey(2), &outlines, None);
        b.text(NodeKey(3), "BB");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    let outlined: Vec<_> = layout.box_fragments(NodeKey(2)).collect();
    assert_eq!(outlined.len(), 1);
    assert!(!outlined[0].is_culled());
    assert_eq!(outlined[0].inline(), along(30.0, 50.0));
}

/// A culled box gives one part for each group of what it holds that
/// reordering leaves side by side, as Chrome answers a culled inline.
///
/// In `A<b>B<i>CD</i></b><i>EF</i>G` at 40 px, with both `i`s overriding
/// right to left, the outer `b` is `[40, 80]` and `[160, 240]`. Each `i` is
/// one part, `[160, 240]` and `[80, 160]`, with no empty part where an edge
/// lands. Its own edges bound its leftmost part's left and its rightmost
/// part's right. The start is on the left where it reads left to right.
#[test]
fn a_culled_box_split_by_reordering_has_a_part_each_side() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 40.0);
    let overriding = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::BidiOverride,
        },
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &root, None);
        b.text(NodeKey(3), "B");
        b.open_box(NodeKey(4), &overriding, None);
        b.text(NodeKey(5), "CD");
        b.close_box();
        b.close_box();
        b.open_box(NodeKey(6), &overriding, None);
        b.text(NodeKey(7), "EF");
        b.close_box();
        b.text(NodeKey(8), "G");
    });
    layout.break_lines(&mut cx, Area::new(800.0), &mut NoExclusions);
    let parts = |key: u64| -> Vec<(InlineExtents, bool, bool)> {
        layout
            .box_fragments(NodeKey(key))
            .map(|piece| {
                assert!(piece.is_culled());
                (piece.inline(), piece.is_open_left(), piece.is_open_right())
            })
            .collect()
    };
    assert_eq!(
        parts(2),
        [
            (along(40.0, 80.0), false, true),
            (along(160.0, 240.0), true, false)
        ]
    );
    // Right to left, the start of each `i` is on its right.
    assert_eq!(parts(4), [(along(160.0, 240.0), false, false)]);
    assert_eq!(parts(6), [(along(80.0, 160.0), false, false)]);
}

/// A line holding no cluster but an empty box with an inline margin,
/// border or padding is a line box of the strut's height, as CSS 2, section
/// 9.4.2, has it; a box with only block-axis edges makes none. Chrome 155,
/// 20px Ahem on lines of 20 in a block 200 wide, the box's fragment along
/// and across its line:
/// - 3px of left padding and a 2px border: a line 20 tall, the box from 0
///   to 7 and -2 to 22; right-aligned, the line starts at 193;
/// - 3px of top padding: no line;
/// - a 5px left margin on lines of 30: a line 30 tall, the box at 5, from
///   5 to 25;
/// - after `x<br>`: a second line, the box from 0 to 3 on it;
/// - on lines of 0: a line of no height, the box from -10 to 10.
#[test]
fn an_empty_box_with_inline_edges_makes_a_line() {
    use crate::config::PastLines;
    let mut cx = context();
    let edged = |margin: f32, padding: Sides<f32>, border: f32| ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: margin,
                ..Sides::ZERO
            }
            .into(),
            padding: padding.into(),
            border: Sides::all(border),
            ..EdgesGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, 20.0)
    };
    let left = Sides::<f32> {
        left: 3.0,
        ..Sides::ZERO
    };
    let top = Sides::<f32> {
        top: 3.0,
        ..Sides::ZERO
    };
    let right = TextAlign::Right;
    let start = TextAlign::Start;
    // Each case: the box, its line height, a line before it, the
    // alignment; then the lines, the block's end, the line's left, and the
    // box's line and extents.
    let cases = [
        (
            edged(0.0, left, 2.0),
            20.0,
            false,
            start,
            1,
            20.0,
            0.0,
            Some((0, (0.0, 7.0), (-2.0, 22.0))),
        ),
        (
            edged(0.0, left, 2.0),
            20.0,
            false,
            right,
            1,
            20.0,
            193.0,
            Some((0, (0.0, 7.0), (-2.0, 22.0))),
        ),
        (edged(0.0, top, 0.0), 20.0, false, start, 0, 0.0, 0.0, None),
        (
            edged(5.0, Sides::ZERO, 0.0),
            30.0,
            false,
            start,
            1,
            30.0,
            0.0,
            Some((0, (5.0, 5.0), (5.0, 25.0))),
        ),
        (
            edged(0.0, left, 0.0),
            20.0,
            true,
            start,
            2,
            40.0,
            0.0,
            Some((1, (0.0, 3.0), (0.0, 20.0))),
        ),
        (
            edged(0.0, left, 0.0),
            0.0,
            false,
            start,
            1,
            0.0,
            0.0,
            Some((0, (0.0, 3.0), (-10.0, 10.0))),
        ),
    ];
    for (n, (span, height, br, align, lines, end, line_left, expected)) in
        cases.into_iter().enumerate()
    {
        let mut layout = Layout::new();
        let mut root = sized(&AHEM_FAMILY, 20.0);
        root.line.height = LineHeight::Px(height);
        let mut span = span;
        span.line.height = root.line.height;
        let mut block = ComputedBlockStyle::new(&root);
        block.text_align = align;
        build(&mut cx, &mut layout, &block, |b| {
            if br {
                b.text(NodeKey(1), "x");
                b.line_break(NodeKey(2));
            }
            b.open_box(NodeKey(3), &span, None);
            b.close_box();
        });
        layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
        assert_eq!(layout.lines().len(), lines, "case {n}");
        assert_eq!(layout.metrics().block_end, end, "case {n}");
        let fragment = layout.box_fragments(NodeKey(3)).next().map(|fragment| {
            let block = fragment.block();
            (
                fragment.line(),
                ends(fragment.inline()),
                (block.over, block.under),
            )
        });
        assert_eq!(fragment, expected, "case {n}");
        if let Some((line, ..)) = expected {
            let left = layout.line(line).map(|line| line.metrics().left);
            assert_eq!(left, Some(line_left), "case {n}");
        }
        // The readers walk the empty line as any other.
        for past in [PastLines::Column, PastLines::LineEnds] {
            for (x, y) in [(0.0, 0.0), (5.0, 30.0), (300.0, 300.0), (-5.0, -5.0)] {
                if let Some(position) = layout.hit_test(x, y, past) {
                    let _ = (layout.caret(position), layout.carets(position));
                }
            }
        }
        let rects = layout.selection_rects(0..layout.text().len()).count();
        let paints: usize = layout
            .lines()
            .map(|line| line.paints(|_| Decorates::None).count() + line.items().count())
            .sum();
        let _ = (rects, paints);
    }
}

/// The intrinsic sizes count a line holding only an empty box with inline
/// edges. Chrome 155, 20px Ahem, `width: max-content` and `min-content`
/// alike: a box with 3px of left padding and a 2px border is 7 wide;
/// `x<br>` and a box with 30px of left padding, 30.
#[test]
fn an_empty_box_line_counts_in_the_intrinsic_sizes() {
    let mut cx = context();
    let root = sized(&AHEM_FAMILY, 20.0);
    for (padding, border, br, expected) in [(3.0, 2.0, false, 7.0), (30.0, 0.0, true, 30.0)] {
        let span = ComputedStyle {
            edges: EdgesGroup {
                padding: Sides::<f32> {
                    left: padding,
                    ..Sides::ZERO
                }
                .into(),
                border: Sides::all(border),
                ..EdgesGroup::INITIAL
            },
            ..root
        };
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            if br {
                b.text(NodeKey(1), "x");
                b.line_break(NodeKey(2));
            }
            b.open_box(NodeKey(3), &span, None);
            b.close_box();
        });
        let sizes = layout.intrinsic_sizes();
        assert_eq!((sizes.min_content, sizes.max_content), (expected, expected));
    }
}

/// Empty ruby containers on a line of their own keep their fragments, a
/// bordered one making the line. Chrome 155, 20px Ahem, three empty
/// `<ruby>`s, the middle one with a 1px border: at 0, from 0 to 2, and at
/// 2.
#[test]
fn empty_ruby_containers_keep_their_fragments_on_their_line() {
    let mut cx = context();
    let root = sized(&AHEM_FAMILY, 20.0);
    let bordered = ComputedStyle {
        edges: EdgesGroup {
            border: Sides::all(1.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &root, None);
        b.close_ruby();
        b.open_ruby(NodeKey(2), &bordered, None);
        b.close_ruby();
        b.open_ruby(NodeKey(3), &root, None);
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    assert_eq!(layout.lines().len(), 1);
    let found: Vec<Vec<(f32, f32)>> = (1..4)
        .map(|key| {
            layout
                .box_fragments(NodeKey(key))
                .map(|fragment| ends(fragment.inline()))
                .collect()
        })
        .collect();
    assert_eq!(found, [[(0.0, 0.0)], [(0.0, 2.0)], [(2.0, 2.0)]]);
}

/// An atomic inline is its margin box: as wide as its size and its margins,
/// its baseline where it said, its top and bottom around it.
#[test]
fn an_atomic_inline_is_its_margin_box() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let margined = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(1.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let size = BoxSize {
        inline: 24.0,
        block: 28.0,
        baseline: Some(14.0),
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX");
        b.atomic(NodeKey(2), &margined, None, size);
        b.text(NodeKey(3), "XX");
    });
    layout.break_lines(&mut cx, Area::new(500.0), &mut NoExclusions);
    let atomic = layout
        .line(0)
        .expect("a line")
        .items()
        .find_map(|item| match item {
            Item::Atomic(atomic) => Some(atomic),
            _ => None,
        })
        .expect("the atomic");
    assert_eq!(atomic.key(), NodeKey(2));
    assert_eq!(
        (atomic.inline(), atomic.advance()),
        (along(40.0, 66.0), 26.0)
    );
    assert_eq!(atomic.baseline(), 16.0);
    // Its margin box: 14 over the baseline and a margin, the rest under.
    assert_eq!(atomic.block(), across(1.0, 31.0));
    walk(&layout);
}

/// An empty box after the space a line ends at is on that line, before the
/// collapsed space, as Chrome's `RewindOverflow` keeps the boxes that open
/// and close at a break. Chrome 153, 10px Ahem in 50px, the web platform
/// test `trailing-space-position-001`: in `1234 <span> </span>567` the
/// span's space collapses and the span stands on the first line at 40; in
/// `1234567 <span> </span>567` at 70; an empty span with 3px of padding at
/// 40, 3 wide. A span holding text starts the next line, and so does an
/// empty span inside it: in `1234 <b><span></span>567</b>`, with 2px of
/// border on the left of the `b`, the empty span is on the second line at
/// 2.
#[test]
fn an_empty_box_after_a_lines_last_space_ends_the_line() {
    let root = sized(&AHEM_FAMILY, 10.0);
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
    let cases = [
        ("1234 ", " ", &root, (0, along(40.0, 40.0))),
        ("1234567 ", " ", &root, (0, along(70.0, 70.0))),
        ("1234 ", "", &root, (0, along(40.0, 40.0))),
        ("1234 ", "", &padded, (0, along(40.0, 43.0))),
        ("1234 ", "56", &root, (1, along(0.0, 20.0))),
    ];
    for (before, inside, style, expected) in cases {
        let mut cx = context();
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), before);
            b.open_box(NodeKey(2), style, None);
            b.text(NodeKey(3), inside);
            b.close_box();
            b.text(NodeKey(4), "567");
        });
        layout.break_lines(&mut cx, Area::new(50.0), &mut NoExclusions);
        let pieces: Vec<_> = layout
            .box_fragments(NodeKey(2))
            .map(|piece| (piece.line(), piece.inline()))
            .collect();
        assert_eq!(pieces, [expected], "{before:?} {inside:?}");
    }
    let bordered = ComputedStyle {
        edges: EdgesGroup {
            border: Sides {
                left: 2.0,
                ..Sides::ZERO
            },
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    let mut cx = context();
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "1234 ");
        b.open_box(NodeKey(2), &bordered, None);
        b.open_box(NodeKey(3), &root, None);
        b.close_box();
        b.text(NodeKey(4), "567");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(50.0), &mut NoExclusions);
    let pieces = |key| -> Vec<_> {
        layout
            .box_fragments(NodeKey(key))
            .map(|piece| (piece.line(), piece.inline()))
            .collect()
    };
    assert_eq!(pieces(2), [(1, along(0.0, 32.0))]);
    assert_eq!(pieces(3), [(1, along(2.0, 2.0))]);
}

/// Everything a host reads of laid-out lines: each line's metrics and
/// records, every item's key and place, and the block's metrics.
fn laid_out(layout: &Layout) -> String {
    let mut read = format!("{:?}\n", layout.metrics());
    read += &format!("{:?}\n", layout.line_records().lines.as_slice());
    for line in layout.lines() {
        read += &format!("{:?}\n", line.metrics());
        for item in line.all_items() {
            read += &match item {
                Item::Text(run) | Item::Generated(run) => format!(
                    "text {:?} {:?} {:?} {}\n",
                    run.key(),
                    run.inline(),
                    run.block(),
                    run.baseline()
                ),
                Item::Atomic(atomic) => format!(
                    "atomic {:?} {:?} {:?} {}\n",
                    atomic.key(),
                    atomic.inline(),
                    atomic.block(),
                    atomic.baseline()
                ),
                Item::Box(piece) => format!(
                    "box {:?} {:?} {:?} {}\n",
                    piece.key(),
                    piece.inline(),
                    piece.block(),
                    piece.baseline()
                ),
            };
        }
    }
    read
}

/// Builds text with atomic inlines aligned every way, two of them inside a
/// box that is aligned itself, the `n`th sized by `size(n)`, under a
/// `::first-line` that sets the first line's text larger.
fn aligned_atomics(
    cx: &mut Context,
    layout: &mut Layout,
    block: ComputedBlockStyle<'_>,
    size: impl Fn(u64) -> BoxSize,
) {
    let root = block.style;
    let aligns = [
        VerticalAlign::Baseline,
        VerticalAlign::Super,
        VerticalAlign::Middle,
        VerticalAlign::TextTop,
        VerticalAlign::TextBottom,
        VerticalAlign::Top,
        VerticalAlign::Bottom,
        VerticalAlign::Px(3.5),
    ];
    let aligned = |n: u64| ComputedStyle {
        line: LineGroup {
            vertical_align: aligns[n as usize % aligns.len()],
            ..root.line
        },
        edges: EdgesGroup {
            margin: Sides::<f32> {
                top: 1.0,
                bottom: 2.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..*root
    };
    build(cx, layout, &block, |b| {
        for n in 0..8 {
            b.text(NodeKey(100 + n), "XX X ");
            b.atomic(NodeKey(n), &aligned(n), None, size(n));
        }
        b.open_box(NodeKey(200), &aligned(1), None);
        b.text(NodeKey(201), "X ");
        b.atomic(NodeKey(8), &aligned(5), None, size(8));
        b.atomic(NodeKey(9), &aligned(2), None, size(9));
        b.close_box();
        b.text(NodeKey(202), " XX");
    });
}

/// The size [`aligned_atomics`] builds its `n`th atomic inline with: no
/// block size and no baseline.
fn built_size(n: u64) -> BoxSize {
    BoxSize {
        inline: 12.0 + n as f32,
        block: 0.0,
        baseline: None,
    }
}

/// The size set on [`aligned_atomics`]'s `n`th atomic inline after building:
/// the same inline size, and a block size and baseline of its own.
fn set_size(n: u64) -> BoxSize {
    BoxSize {
        inline: 12.0 + n as f32,
        block: 8.0 + (n % 4) as f32 * 7.5,
        baseline: (!n.is_multiple_of(3)).then_some(4.0 + n as f32),
    }
}

/// Measuring with new atomic inline block sizes and baselines, then
/// breaking again, equals a layout built with the new sizes, on the first
/// line and after it, in horizontal and vertical lines alike.
#[test]
fn a_relayout_after_measuring_new_sizes_equals_a_fresh_layout() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let first_line = sized(&AHEM_FAMILY, 16.0);
    for writing_mode in [WritingMode::HorizontalTb, WritingMode::VerticalRl] {
        let block = ComputedBlockStyle {
            first_line: Some(&first_line),
            writing_mode,
            ..ComputedBlockStyle::new(&root)
        };
        let mut cx = context();
        let mut layout = Layout::new();
        aligned_atomics(&mut cx, &mut layout, block, built_size);
        layout.break_lines(&mut cx, Area::new(120.0), &mut NoExclusions);
        assert!(
            layout.measured().first_line().is_some(),
            "a first line's measure"
        );
        let intrinsic = layout.intrinsic_sizes();
        assert!(layout.measure(&mut cx, 0.0, (0..10).map(|n| (NodeKey(n), set_size(n)))));
        assert_eq!(layout.lines().len(), 0, "setting a size clears the lines");
        assert_eq!(layout.intrinsic_sizes(), intrinsic);
        let mut fresh = Layout::new();
        aligned_atomics(&mut cx, &mut fresh, block, set_size);
        for width in [120.0, 45.0, 400.0] {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            fresh.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            assert!(layout.lines().len() > 1 || width == 400.0);
            assert_eq!(
                laid_out(&layout),
                laid_out(&fresh),
                "{writing_mode:?} at {width}"
            );
        }
        // One atomic inline at a time sets the same.
        let mut one_by_one = Layout::new();
        aligned_atomics(&mut cx, &mut one_by_one, block, built_size);
        for n in 0..10 {
            assert!(one_by_one.measure(&mut cx, 0.0, [(NodeKey(n), set_size(n))]));
        }
        one_by_one.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        assert_eq!(laid_out(&one_by_one), laid_out(&fresh), "{writing_mode:?}");
    }
}

/// Sizes in reverse document order, or shuffled, measure what sizes in
/// document order do.
#[test]
fn atomic_sizes_in_any_order_set_the_same() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let mut cx = context();
    let mut forward = Layout::new();
    let mut backward = Layout::new();
    let mut shuffled = Layout::new();
    for layout in [&mut forward, &mut backward, &mut shuffled] {
        aligned_atomics(&mut cx, layout, block, built_size);
    }
    let pair = |n: u64| (NodeKey(n), set_size(n));
    assert!(forward.measure(&mut cx, 0.0, (0..10).map(pair)));
    assert!(backward.measure(&mut cx, 0.0, (0..10).rev().map(pair)));
    assert!(shuffled.measure(&mut cx, 0.0, [7, 2, 9, 0, 5, 3, 8, 1, 6, 4].map(pair)));
    for width in [45.0, 120.0] {
        for layout in [&mut forward, &mut backward, &mut shuffled] {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        }
        assert_eq!(laid_out(&backward), laid_out(&forward), "at {width}");
        assert_eq!(laid_out(&shuffled), laid_out(&forward), "at {width}");
    }
}

/// A key two atomic inlines share sets both, and the atomic inline keyed
/// otherwise between them its own size.
#[test]
fn a_shared_key_sets_every_atomic_inline_keyed_so() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let size = |block: f32| BoxSize {
        inline: 20.0,
        block,
        baseline: Some(block / 2.0),
    };
    let atomics = |cx: &mut Context, layout: &mut Layout, shared: f32, own: f32| {
        build(cx, layout, &block, |b| {
            b.atomic(NodeKey(5), &root, None, size(shared));
            b.text(NodeKey(1), "XX ");
            b.atomic(NodeKey(6), &root, None, size(own));
            b.text(NodeKey(2), " XX ");
            b.atomic(NodeKey(5), &root, None, size(shared));
        });
    };
    let mut cx = context();
    let mut layout = Layout::new();
    atomics(&mut cx, &mut layout, 10.0, 10.0);
    assert!(layout.measure(
        &mut cx,
        0.0,
        [(NodeKey(6), size(14.0)), (NodeKey(5), size(32.0))]
    ));
    let mut fresh = Layout::new();
    atomics(&mut cx, &mut fresh, 32.0, 14.0);
    for width in [40.0, 400.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        fresh.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(laid_out(&layout), laid_out(&fresh), "at {width}");
    }
    let blocks: Vec<_> = layout
        .lines()
        .flat_map(|line| line.all_items())
        .filter_map(|item| match item {
            Item::Atomic(atomic) => Some((atomic.key(), atomic.block())),
            _ => None,
        })
        .collect();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0], blocks[2], "both keyed 5 are set");
    assert_ne!(blocks[0].1, blocks[1].1);
}

/// Measuring with box sizes that change nothing keeps the lines: a key no
/// box has, a key of a box that is no atomic inline, and the sizes the
/// boxes have.
#[test]
fn measuring_what_changes_nothing_keeps_the_lines() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 10.0);
    let size = BoxSize {
        inline: 20.0,
        block: 10.0,
        baseline: Some(8.0),
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &root, None);
        b.text(NodeKey(2), "XX ");
        b.close_box();
        b.atomic(NodeKey(3), &root, None, size);
        b.atomic(NodeKey(4), &root, None, size);
    });
    layout.break_lines(&mut cx, Area::new(100.0), &mut NoExclusions);
    let before = laid_out(&layout);
    let taller = BoxSize {
        block: 30.0,
        ..size
    };
    assert!(!layout.measure(&mut cx, 0.0, [(NodeKey(9), taller)]));
    assert!(!layout.measure(&mut cx, 0.0, [(NodeKey(1), taller)]));
    assert!(!layout.measure(&mut cx, 0.0, [(NodeKey(3), size), (NodeKey(4), size)]));
    assert!(
        !layout.measure(&mut cx, 25.0, []),
        "no percentage reads the basis"
    );
    assert_eq!(laid_out(&layout), before, "nothing changed keeps the lines");
    assert!(layout.measure(&mut cx, 0.0, [(NodeKey(3), taller), (NodeKey(4), taller)]));
    assert_eq!(layout.lines().len(), 0, "a new size clears the lines");
}

/// Measures `layout`, built by `calls` with `built`, again with `sizes` and
/// percentages of `basis`, and checks that it lays out as content built
/// with them at each of `widths`.
fn measures_as_built(
    root: &ComputedStyle<'_>,
    calls: impl Fn(&mut LayoutBuilder<'_>, &dyn Fn(u64) -> BoxSize),
    built: impl Fn(u64) -> BoxSize,
    sizes: impl Fn(u64) -> BoxSize,
    basis: f32,
    keys: &[u64],
    widths: &[f32],
) {
    let mut cx = context();
    let block = ComputedBlockStyle::new(root);
    let mut layout = Layout::new();
    let mut b = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
    calls(&mut b, &built);
    b.finish();
    layout.break_lines(&mut cx, Area::new(widths[0]), &mut NoExclusions);
    let pairs: Vec<_> = keys.iter().map(|&key| (NodeKey(key), sizes(key))).collect();
    layout.measure(&mut cx, basis, pairs.iter().copied());
    let mut fresh = Layout::new();
    let options = BuildOptions {
        percentage_basis: basis,
        ..BuildOptions::default()
    };
    let mut b = fresh.builder(&mut cx, NodeKey(0), &block, options);
    calls(&mut b, &sizes);
    b.finish();
    assert_eq!(layout.intrinsic_sizes(), fresh.intrinsic_sizes());
    for &width in widths {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        fresh.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(laid_out(&layout), laid_out(&fresh), "at {width}");
    }
}

/// A new size along the line measures as content built with it.
#[test]
fn measuring_new_inline_sizes_equals_a_fresh_layout() {
    let root = sized(&AHEM_FAMILY, 10.0);
    measures_as_built(
        &root,
        |b, size| {
            b.text(NodeKey(10), "XX ");
            b.atomic(NodeKey(1), &root, None, size(1));
            b.text(NodeKey(11), " XXX ");
            b.atomic(NodeKey(2), &root, None, size(2));
        },
        built_size,
        |n| BoxSize {
            inline: 31.0 + n as f32,
            ..set_size(n)
        },
        0.0,
        &[1, 2],
        &[40.0, 75.0, 400.0],
    );
}

/// A new size of an atomic inline that is a ruby base measures as content
/// built with it.
#[test]
fn measuring_an_atomic_inline_in_ruby_equals_a_fresh_layout() {
    let root = sized(&AHEM_FAMILY, 10.0);
    measures_as_built(
        &root,
        |b, size| {
            b.open_ruby(NodeKey(1), &root, None);
            b.atomic(NodeKey(3), &root, None, size(3));
            b.open_annotation(NodeKey(4), &root, None);
            b.text(NodeKey(5), "X");
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(6), " XX");
        },
        built_size,
        |n| BoxSize {
            block: 30.0,
            ..built_size(n)
        },
        0.0,
        &[3],
        &[40.0, 400.0],
    );
}

/// A float's new size measures as content built with it.
#[test]
fn measuring_a_new_float_size_equals_a_fresh_layout() {
    let root = sized(&AHEM_FAMILY, 10.0);
    measures_as_built(
        &root,
        |b, size| {
            b.text(NodeKey(10), "XX XX ");
            b.float(NodeKey(1), &root, FloatSide::Left, size(1));
            b.text(NodeKey(11), "XXX XX XXXX");
        },
        built_size,
        |n| BoxSize {
            inline: 50.0,
            block: 22.0,
            ..built_size(n)
        },
        0.0,
        &[1],
        &[60.0, 120.0, 400.0],
    );
}

/// Percentage margins and padding of inline boxes, atomic inlines and
/// floats measure against a new basis as content built with it.
#[test]
fn measuring_a_new_percentage_basis_equals_a_fresh_layout() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let percent = |fraction: f32| LengthPercentage { px: 1.0, fraction };
    let edged = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides {
                left: percent(0.05),
                right: percent(0.0),
                ..Sides::from_px(0.0)
            },
            padding: Sides::all(percent(0.1)),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    for basis in [0.0, 80.0, 333.0] {
        measures_as_built(
            &root,
            |b, size| {
                b.text(NodeKey(10), "XX ");
                b.open_box(NodeKey(1), &edged, None);
                b.text(NodeKey(11), "XXX XX");
                b.close_box();
                b.atomic(NodeKey(2), &edged, None, size(2));
                b.float(NodeKey(3), &edged, FloatSide::Right, size(3));
                b.text(NodeKey(12), " XXXX XX");
            },
            built_size,
            built_size,
            basis,
            &[],
            &[50.0, 140.0, 500.0],
        );
    }
}

/// Content with no atomic inline, no float and no percentage margin or
/// padding keeps no shaped advances, in either first-line variant.
/// Measuring it with a new basis and sizes changes nothing and keeps the
/// lines, which lay out as content built with that basis.
#[test]
fn measuring_content_with_nothing_to_size_equals_a_fresh_layout() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let first_line = sized(&AHEM_FAMILY, 16.0);
    let block = ComputedBlockStyle {
        first_line: Some(&first_line),
        ..ComputedBlockStyle::new(&root)
    };
    let edged = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(2.0),
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let calls = |b: &mut LayoutBuilder<'_>| {
        b.text(NodeKey(10), "XX XXX ");
        b.open_box(NodeKey(1), &edged, None);
        b.text(NodeKey(11), "XXXX XX");
        b.close_box();
        b.text(NodeKey(12), " XXX XX XXXXX");
    };
    let mut cx = context();
    let mut layout = Layout::new();
    let mut b = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
    calls(&mut b);
    b.finish();
    assert!(
        layout.measured().first_line().is_some(),
        "a first line's measure"
    );
    for variant in [FirstLineVariant::Standard, FirstLineVariant::FirstLine] {
        let advances = &layout.shaped().text(variant).advances;
        assert_eq!(advances.capacity(), 0, "{variant:?} keeps advances");
    }
    layout.break_lines(&mut cx, Area::new(60.0), &mut NoExclusions);
    let before = laid_out(&layout);
    let size = BoxSize {
        inline: 20.0,
        block: 10.0,
        baseline: Some(8.0),
    };
    assert!(!layout.measure(&mut cx, 80.0, [(NodeKey(1), size), (NodeKey(10), size)]));
    assert_eq!(laid_out(&layout), before, "nothing changed keeps the lines");
    let mut fresh = Layout::new();
    let options = BuildOptions {
        percentage_basis: 80.0,
        ..BuildOptions::default()
    };
    let mut b = fresh.builder(&mut cx, NodeKey(0), &block, options);
    calls(&mut b);
    b.finish();
    assert_eq!(layout.intrinsic_sizes(), fresh.intrinsic_sizes());
    for width in [40.0, 60.0, 400.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        fresh.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(laid_out(&layout), laid_out(&fresh), "at {width}");
    }
}

/// Content with an atomic inline, a float or a percentage margin keeps its
/// shaped advances, in both first-line variants, for measuring again.
#[test]
fn content_with_something_to_size_keeps_its_advances() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let first_line = sized(&AHEM_FAMILY, 16.0);
    let block = ComputedBlockStyle {
        first_line: Some(&first_line),
        ..ComputedBlockStyle::new(&root)
    };
    let edged = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::all(LengthPercentage {
                px: 0.0,
                fraction: 0.1,
            }),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let size = built_size(1);
    let boxes: [&dyn Fn(&mut LayoutBuilder<'_>); 3] = [
        &|b| b.atomic(NodeKey(1), &root, None, size),
        &|b| b.float(NodeKey(1), &root, FloatSide::Left, size),
        &|b| {
            b.open_box(NodeKey(1), &edged, None);
            b.text(NodeKey(2), "XX");
            b.close_box();
        },
    ];
    let mut cx = context();
    for (n, calls) in boxes.iter().enumerate() {
        let mut layout = Layout::new();
        let mut b = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
        b.text(NodeKey(10), "XX XXX ");
        calls(&mut b);
        b.text(NodeKey(11), " XXX XX");
        b.finish();
        assert!(
            layout.measured().first_line().is_some(),
            "{n}: a first line"
        );
        for variant in [FirstLineVariant::Standard, FirstLineVariant::FirstLine] {
            let advances = &layout.shaped().text(variant).advances;
            assert!(!advances.is_empty(), "{n}: {variant:?} keeps none");
            assert_eq!(advances.len(), advances.capacity(), "{n}: {variant:?}");
        }
    }
}

/// Builds text with `count` atomic inlines keyed `0..count`, the `n`th
/// built with [`built_size`] of `n` modulo 10.
#[cfg(debug_assertions)]
fn many_atomics(cx: &mut Context, layout: &mut Layout, block: &ComputedBlockStyle<'_>, count: u64) {
    let root = block.style;
    build(cx, layout, block, |b| {
        for n in 0..count {
            b.text(NodeKey(100_000 + n), "XX ");
            b.atomic(NodeKey(n), root, None, built_size(n % 10));
        }
    });
}

/// Measuring the sizes of atomic inlines in document order costs steps
/// linear in their number: each search starts where the one before ended.
///
/// Twice the atomic inlines take at most about twice the steps. Steps are
/// counted in debug builds only.
#[cfg(debug_assertions)]
#[test]
fn setting_atomic_sizes_in_order_costs_linear_steps() {
    use crate::work;
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle::new(&root);
    let mut cx = context();
    let mut steps = |count: u64| {
        let mut layout = Layout::new();
        many_atomics(&mut cx, &mut layout, &block, count);
        let _ = work::take();
        let pairs = (0..count).map(|n| (NodeKey(n), set_size(n % 10)));
        assert!(layout.measure(&mut cx, 0.0, pairs));
        work::take()
    };
    let (small, large) = (steps(300), steps(600));
    assert!(small > 0);
    assert!(large <= small * 5 / 2, "{small} steps, then {large}");
}

/// `text-box-trim` on an inline box brings its trimmed edges to
/// `text-box-edge`'s, as Chrome's `InlineBoxState::AdjustEdges` does and
/// `text-box-on-non-root-inline-box` pins. In Ahem at 100 px on 100 px
/// lines, with the x-height at 80 px, `ex alphabetic` and `trim-end text
/// alphabetic` each make the 100 px box 80 tall, ending on the baseline,
/// and `trim-start cap text` leaves it whole. The line box keeps its height.
#[test]
fn text_box_trim_on_an_inline_box_trims_its_box() {
    use crate::style::{TextBoxEdge, TextBoxTrim, TextEdgeOver, TextEdgeUnder};
    let mut cx = context();
    let mut layout = Layout::new();
    let root = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Factor(1.0),
            ..LineGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, 100.0)
    };
    for (trim, over, under, expected) in [
        (
            TextBoxTrim::None,
            TextEdgeOver::Text,
            TextEdgeUnder::Text,
            (0.0, 100.0),
        ),
        (
            TextBoxTrim::TrimBoth,
            TextEdgeOver::Ex,
            TextEdgeUnder::Alphabetic,
            (0.0, 80.0),
        ),
        (
            TextBoxTrim::TrimEnd,
            TextEdgeOver::Text,
            TextEdgeUnder::Alphabetic,
            (0.0, 80.0),
        ),
        (
            TextBoxTrim::TrimStart,
            TextEdgeOver::Cap,
            TextEdgeUnder::Text,
            (0.0, 100.0),
        ),
    ] {
        let mut span = root;
        span.paints = true;
        span.line.text_box_trim = trim;
        span.line.text_box_edge = TextBoxEdge { over, under };
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.open_box(NodeKey(1), &span, None);
            b.text(NodeKey(2), "XX");
            b.close_box();
        });
        layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
        let piece = layout
            .box_fragments(NodeKey(1))
            .next()
            .map(|piece| piece.block());
        assert_eq!(piece, Some(across(expected.0, expected.1)), "{trim:?}");
        let height = layout.line(0).map(|line| line.metrics().height());
        assert_eq!(height, Some(100.0), "{trim:?}");
    }
}
