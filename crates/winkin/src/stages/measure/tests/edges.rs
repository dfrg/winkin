//! Edge and pen tests. They pin:
//! - the pen that draws a cluster after the edges at its boundary;
//! - each box's start and end costs, and a cloned box's at every line;
//! - atomic inlines as their margin boxes;
//! - edges and atomic sizes truncated onto the grid.

use super::*;
use crate::style::FirstLineVariant;

/// The prefix at a boundary is taken after its edges up to the last closing
/// one and before the rest, and the pen that draws the cluster after it
/// stands past all of them: `A<b>B</b>C` with the box's edges on either side.
///
/// Left to right, the box opens on its left, 10 px of margin, and closes on
/// its right, 5 of padding. In a right-to-left paragraph, whose box reads
/// right to left too, the sides swap: a box opens with its own start's edge,
/// which reading right to left is its right. An empty box goes
/// wholly with what comes before, and a box closing where the next opens
/// puts its close before the break and the open after.
#[test]
fn the_pen_stands_after_the_opening_edges() {
    /// `A<b>B</b>C`, the box styled `root` with its edges.
    fn content(b: &mut LayoutBuilder<'_>, root: &ComputedStyle<'_>) {
        let boxed = edged(root, [10.0, 0.0, 0.0], [0.0, 0.0, 5.0]);
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &boxed, None);
        b.text(NodeKey(3), "B");
        b.close_box();
        b.text(NodeKey(4), "C");
    }
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        content(b, &ahem);
    });
    // After `A`, 16, before the box's 10 of margin. After `B`,
    // 16 + 10 + 16 + 5 = 47, past its 5 of padding. After `C`, 63.
    assert_eq!(positions(&layout), [0.0, 16.0, 47.0, 63.0]);
    let pens = |layout: &Layout| -> Vec<f32> {
        let stages = layout.stages().variant(FirstLineVariant::Standard);
        (0..layout.analysis().clusters.len())
            .map(|c| pen(&stages, at(c)).to_px())
            .collect()
    };
    assert_eq!(pens(&layout), [0.0, 26.0, 47.0]);
    let backward = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            ..BidiGroup::INITIAL
        },
        ..ahem
    };
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&backward)
    };
    fixture.build(&mut layout, &rtl, |b| content(b, &backward));
    assert_eq!(positions(&layout), [0.0, 16.0, 47.0, 63.0]);
    assert_eq!(pens(&layout), [0.0, 21.0, 47.0], "opens on its right");

    // An empty box at a boundary: all of it before the break, as Chrome's
    // breaker moves a break before a close tag to after it.
    let empty = edged(&ahem, [0.0, 0.0, 3.0], [0.0, 0.0, 3.0]);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &empty, None);
        b.close_box();
        b.text(NodeKey(3), "B");
    });
    assert_eq!(positions(&layout), [0.0, 22.0, 38.0]);
    assert_eq!(pens(&layout)[1], 22.0);

    // A close and an open at one boundary: the close before, the open after.
    let closing = edged(&ahem, [0.0; 3], [0.0, 1.0, 3.0]);
    let opening = edged(&ahem, [2.0, 4.0, 0.0], [0.0; 3]);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.open_box(NodeKey(1), &closing, None);
        b.text(NodeKey(2), "A");
        b.close_box();
        b.open_box(NodeKey(3), &opening, None);
        b.text(NodeKey(4), "B");
        b.close_box();
    });
    assert_eq!(positions(&layout), [0.0, 20.0, 42.0]);
    assert_eq!(pens(&layout), [0.0, 26.0]);
    // Fitting a line of the first cluster alone takes its box's close; a
    // line of the second takes the other's open.
    let text = layout.measured().text(FirstLineVariant::Standard);
    assert_eq!(text.prefix.fit_width(at(0), at(0), at(1)), lu(20.0));
    assert_eq!(text.prefix.fit_width(at(0), at(1), at(2)), lu(22.0));
}

/// A box's opening edge takes the margin, border and padding of its own
/// inline start, and its closing edge those of its end, in its own
/// `direction`, as Chrome's `ComputeLineMarginsForSelf` charges them; the
/// paragraph's direction, and the levels its text resolves to, move
/// neither. `A<b>B</b>C` with 10 px of margin on
/// the box's left and 3 on its right: reading left to right it opens with
/// 10 and closes with 3, and reading right to left, in a paragraph of
/// either direction, it opens with 3 and closes with 10.
#[test]
fn a_box_charges_its_own_start_and_its_own_end() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let mut layout = Layout::new();
    let rtl_block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&ahem)
    };
    for (direction, block) in [
        (Direction::Ltr, ComputedBlockStyle::new(&ahem)),
        (Direction::Ltr, rtl_block),
        (Direction::Rtl, ComputedBlockStyle::new(&ahem)),
        (Direction::Rtl, rtl_block),
    ] {
        let boxed = ComputedStyle {
            bidi: BidiGroup {
                direction,
                ..BidiGroup::INITIAL
            },
            ..edged(&ahem, [10.0, 0.0, 0.0], [3.0, 0.0, 0.0])
        };
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem,
                ..block
            },
            |b| {
                b.text(NodeKey(1), "A");
                b.open_box(NodeKey(2), &boxed, None);
                b.text(NodeKey(3), "B");
                b.close_box();
                b.text(NodeKey(4), "C");
            },
        );
        let (opens, closes) = match direction {
            Direction::Ltr => (10.0, 3.0),
            Direction::Rtl => (3.0, 10.0),
        };
        let stages = layout.stages().variant(FirstLineVariant::Standard);
        assert_eq!(
            pen(&stages, at(1)).to_px(),
            16.0 + opens,
            "{direction:?} in {:?}",
            block.direction
        );
        assert_eq!(
            positions(&layout),
            [0.0, 16.0, 32.0 + opens + closes, 48.0 + opens + closes]
        );
    }
}

/// A cloned box draws both its edges on every line it reaches, so every
/// boundary inside it where a line may start or end has a line-edge cost:
/// its opening edge for a line starting there and its closing edge for one
/// ending there, as Chrome's breaker charges `cloned_box_decorations_*`
/// Nested cloned boxes add up; a box that slices, and a
/// boundary no line can start or end at, have none; and the intrinsic sizes
/// pay them as their lines would.
#[test]
fn a_cloned_box_costs_its_edges_where_a_line_starts_or_ends_inside_it() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let cloned = |left: f32, right: f32| ComputedStyle {
        edges: EdgesGroup {
            decoration_break: BoxDecorationBreak::Clone,
            ..edged(&ahem, [0.0, 0.0, left], [0.0, 0.0, right]).edges
        },
        ..ahem
    };
    let (outer, inner) = (cloned(1.0, 2.0), cloned(4.0, 8.0));
    let mut layout = Layout::new();
    // `A <b>B C<i>D E</i>F</b> G`: boundaries after each space.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A ");
        b.open_box(NodeKey(2), &outer, None);
        b.text(NodeKey(3), "B C");
        b.open_box(NodeKey(4), &inner, None);
        b.text(NodeKey(5), "D E");
        b.close_box();
        b.text(NodeKey(6), "F");
        b.close_box();
        b.text(NodeKey(7), " G");
    });
    let costs: Vec<(usize, f32, f32)> = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .edge_costs()
        .iter()
        .map(|edge| (edge.key().get(), edge.start.to_px(), edge.end.to_px()))
        .collect();
    // After `B ` only the outer is open across; after `D ` both. After `A `
    // the outer opens past the break, and after `F ` it has closed before.
    assert_eq!(costs, [(4, 1.0, 2.0), (7, 5.0, 10.0)]);
    let text = layout.measured().text(FirstLineVariant::Standard);
    let flags = text.paragraph(ParagraphId::new(0));
    assert!(flags.contains(MeasureFlags::HAS_EDGE_COSTS));
    // Min-content is the widest min-content line with its costs: `CD`. It
    // starts inside the outer box, paying its opening edge, opens the inner,
    // and ends inside both, paying both closing edges.
    assert_eq!(layout.measured().intrinsic.min, lu(32.0 + 1.0 + 4.0 + 10.0));

    // Sliced, the same boxes cost nothing at any boundary.
    let sliced = |left: f32, right: f32| edged(&ahem, [0.0, 0.0, left], [0.0, 0.0, right]);
    let (outer, inner) = (sliced(1.0, 2.0), sliced(4.0, 8.0));
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A ");
        b.open_box(NodeKey(2), &outer, None);
        b.text(NodeKey(3), "B C");
        b.open_box(NodeKey(4), &inner, None);
        b.text(NodeKey(5), "D E");
        b.close_box();
        b.text(NodeKey(6), "F");
        b.close_box();
        b.text(NodeKey(7), " G");
    });
    assert!(
        layout
            .measured()
            .text(FirstLineVariant::Standard)
            .edge_costs()
            .is_empty()
    );

    // A forced break inside a cloned box: the paragraph before pays its
    // closing edge and the one after its opening edge.
    let box_style = cloned(3.0, 5.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.open_box(NodeKey(1), &box_style, None);
        b.text(NodeKey(2), "AB");
        b.line_break(NodeKey(3));
        b.text(NodeKey(4), "C");
        b.close_box();
    });
    let text = layout.measured().text(FirstLineVariant::Standard);
    let costs: Vec<(usize, f32, f32)> = text
        .edge_costs()
        .iter()
        .map(|edge| (edge.key().get(), edge.start.to_px(), edge.end.to_px()))
        .collect();
    assert_eq!(costs, [(3, 3.0, 5.0)]);
    // `AB`, its opening edge and the closing one it pays at the break.
    assert_eq!(layout.measured().intrinsic.max, lu(32.0 + 3.0 + 5.0));
}

/// No line starts at the text's end, so an empty box there, or a box left
/// open for finishing to close, is on the last line. Where the text ends in
/// a forced break, what follows the break is the empty last paragraph's, not
/// the line the break ends.
#[test]
fn what_ends_the_text_is_on_the_last_line() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let empty = edged(&ahem, [0.0, 0.0, 3.0], [0.0, 0.0, 3.0]);
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &empty, None);
        b.close_box();
    });
    assert_eq!(positions(&layout), [0.0, 22.0]);
    assert_eq!(layout.intrinsic_sizes().max_content, 22.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &empty, None);
    });
    assert_eq!(positions(&layout), [0.0, 22.0]);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A");
        b.line_break(NodeKey(2));
        b.open_box(NodeKey(3), &empty, None);
        b.close_box();
    });
    assert_eq!(positions(&layout), [0.0, 16.0, 16.0]);
    assert_eq!(layout.intrinsic_sizes().max_content, 16.0);
}

/// An atomic inline's U+FFFC is as wide as its margin box, and reaches from
/// its baseline as its margin box does: to its own baseline where it has
/// one, and from its margin box's under edge where it has none, as a
/// replaced element sits.
#[test]
fn an_atomic_inline_is_its_margin_box() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let margined = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                top: 2.0,
                right: 3.0,
                bottom: 4.0,
                left: 5.0,
            }
            .into(),
            padding: Sides::from_px(100.0),
            ..EdgesGroup::INITIAL
        },
        ..ahem
    };
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A");
        let size = BoxSize {
            inline: 30.0,
            block: 20.0,
            baseline: Some(15.0),
        };
        b.atomic(NodeKey(2), &margined, None, size);
        let replaced = BoxSize {
            baseline: None,
            ..size
        };
        b.atomic(NodeKey(3), &margined, None, replaced);
        b.text(NodeKey(4), "B");
    });
    // Its own padding is inside the border box the host measured: only its
    // margins are added.
    assert_eq!(positions(&layout), [0.0, 16.0, 54.0, 92.0, 108.0]);
    let items = &layout.content().items;
    let extents: Vec<Extent> = items
        .iter()
        .filter(|(_, item)| item.kind == ItemKind::Atomic)
        .map(|(id, _)| {
            layout
                .measured()
                .text(FirstLineVariant::Standard)
                .extents
                .get(id)
        })
        .collect();
    assert_eq!(
        extents,
        [
            Extent::new(lu(17.0), lu(9.0)),
            Extent::new(lu(26.0), LayoutUnit::ZERO)
        ]
    );
}

/// In a vertical line, set on its central baseline, an atomic inline with a
/// baseline of its own keeps it, and one with none is centred on the line
/// as Blink synthesizes its baseline there (`SynthesizeMetrics` with a
/// central baseline): half its margin box over and half under, the odd
/// 1/64 over. Its margins across the line are its right and left. Its own
/// baseline is down from its block-start edge, the left in `vertical-lr`,
/// whose lines are over on the right, so that there it is that far over
/// its under edge; and the left in `sideways-lr` too, which is its over
/// side there, as in every mode but `vertical-lr`.
#[test]
fn an_atomic_inline_with_no_baseline_is_centred_in_a_vertical_line() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let margined = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                top: 2.0,
                right: 3.0,
                bottom: 4.0,
                left: 5.0,
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..ahem
    };
    let extents = |fixture: &mut Fixture,
                   writing_mode: WritingMode,
                   style: &ComputedStyle<'static>,
                   block_size: f32| {
        let block = ComputedBlockStyle {
            writing_mode,
            ..ComputedBlockStyle::new(style)
        };
        let mut layout = Layout::new();
        fixture.build(&mut layout, &block, |b| {
            b.text(NodeKey(1), "A");
            let size = BoxSize {
                inline: 30.0,
                block: block_size,
                baseline: Some(15.0),
            };
            b.atomic(NodeKey(2), &margined, None, size);
            let replaced = BoxSize {
                baseline: None,
                ..size
            };
            b.atomic(NodeKey(3), &margined, None, replaced);
        });
        let items = &layout.content().items;
        items
            .iter()
            .filter(|(_, item)| item.kind == ItemKind::Atomic)
            .map(|(id, _)| {
                layout
                    .measured()
                    .text(FirstLineVariant::Standard)
                    .extents
                    .get(id)
            })
            .collect::<Vec<Extent>>()
    };
    let rl = WritingMode::VerticalRl;
    // 3 + 20 + 5 is 28: 14 each side.
    assert_eq!(
        extents(&mut fixture, rl, &ahem, 20.0),
        [
            Extent::new(lu(18.0), lu(10.0)),
            Extent::new(lu(14.0), lu(14.0))
        ]
    );
    // 28 and 1/64: the odd 1/64 over.
    assert_eq!(
        extents(&mut fixture, rl, &ahem, 20.015_625),
        [
            Extent::new(lu(18.0), lu(10.015_625)),
            Extent::new(lu(14.015_625), lu(14.0))
        ]
    );
    // In `vertical-lr` the baseline 15 from the left is 5 from the right.
    assert_eq!(
        extents(&mut fixture, WritingMode::VerticalLr, &ahem, 20.0),
        [
            Extent::new(lu(8.0), lu(20.0)),
            Extent::new(lu(14.0), lu(14.0))
        ]
    );
    // In `sideways-lr` the left, where the lines stack from, is their over
    // side, as Chrome 153 sets an inline-block there: the baseline 15 from
    // it is 15 under the box's over edge, past the left margin of 5, and the
    // right margin of 3 is under. Its lines are alphabetic, and a box with
    // no baseline stands on its margin box's under edge.
    assert_eq!(
        extents(&mut fixture, WritingMode::SidewaysLr, &ahem, 20.0),
        [
            Extent::new(lu(20.0), lu(8.0)),
            Extent::new(lu(28.0), LayoutUnit::ZERO)
        ]
    );
    // Text on its side sets the line on its alphabetic baseline, and the
    // box sits on its margin box's under edge.
    let sideways = {
        let mut style = ahem;
        style.orientation.text_orientation = TextOrientation::Sideways;
        style
    };
    assert_eq!(
        extents(&mut fixture, rl, &sideways, 20.0)[1],
        Extent::new(lu(28.0), LayoutUnit::ZERO)
    );
}

/// A box's margin, border and padding, and an atomic inline's size and
/// margins, go onto the grid truncated toward zero, as Chrome holds each as
/// a `LayoutUnit` made from a float, and not to the nearest 1/64: measured
/// with Chrome 153, padding 10.012px is
/// 10, 10.99px 10.984375, 10.5078125px 10.5 and 0.01px nothing; a margin of
/// 10.012px is 10 and -10.012px -10, and ±10.5078125px ±10.5; an inline
/// block's margins of 10.012px and width of 20.012px come to 40.
#[test]
fn edges_and_atomic_sizes_are_truncated_onto_the_grid() {
    let horizontal = WritingMode::HorizontalTb;
    for (margin, padding, expected) in [
        (0.0, 10.012, 10.0),
        (0.0, 10.99, 10.984375),
        (0.0, 10.507_812_5, 10.5),
        (0.0, 0.01, 0.0),
        (10.012, 0.0, 10.0),
        (-10.012, 0.0, -10.0),
        (10.507_812_5, 0.0, 10.5),
        (-10.507_812_5, 0.0, -10.5),
    ] {
        let edges = EdgesGroup {
            margin: Sides::<f32> {
                left: margin,
                right: margin,
                ..Sides::ZERO
            }
            .into(),
            padding: Sides::<f32> {
                left: padding,
                right: padding,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        };
        assert_eq!(
            edges.used(0.0).inline(horizontal),
            (lu(expected), lu(expected)),
            "margin {margin}, padding {padding}"
        );
    }

    // The prefix charges it, and so does an atomic inline.
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let padded = edged(&ahem, [10.012, 0.0, 10.99], [-10.012, 0.0, 0.01]);
    let margined = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(10.012),
            ..EdgesGroup::INITIAL
        },
        ..ahem
    };
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.open_box(NodeKey(1), &padded, None);
        b.text(NodeKey(2), "A");
        b.close_box();
        let size = BoxSize {
            inline: 20.012,
            block: 20.012,
            baseline: None,
        };
        b.atomic(NodeKey(3), &margined, None, size);
    });
    // The box's opening edge is 10 + 10.984375, `A` 16, its closing edge
    // -10 + 0, and the atomic's margin box 10 + 20 + 10.
    let boxed = 20.984375 + 16.0 - 10.0;
    assert_eq!(positions(&layout), [0.0, boxed, boxed + 40.0]);
    let atomic = layout
        .content()
        .items
        .iter()
        .find(|(_, item)| item.kind == ItemKind::Atomic)
        .map(|(id, _)| {
            layout
                .measured()
                .text(FirstLineVariant::Standard)
                .extents
                .get(id)
        });
    assert_eq!(atomic, Some(Extent::new(lu(40.0), LayoutUnit::ZERO)));
}
