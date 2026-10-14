//! Shift tests. They pin:
//! - `vertical-align` shifts that don't depend on the line;
//! - `super` and `sub` from the font's offsets, and `dominant-baseline`,
//!   under the config options that enable them;
//! - saturation for shifts of any size.

use super::*;
use crate::style::FirstLineVariant;

/// Returns `style` with `dominant-baseline` set to `baseline`.
fn on_baseline<'a>(style: &ComputedStyle<'a>, baseline: DominantBaseline) -> ComputedStyle<'a> {
    ComputedStyle {
        line: LineGroup {
            dominant_baseline: baseline,
            ..style.line
        },
        ..*style
    }
}

/// Returns each stored shift as `(node, px)`.
fn node_shifts(layout: &Layout) -> Vec<(usize, f32)> {
    layout
        .measured()
        .text(FirstLineVariant::Standard)
        .shifts()
        .iter()
        .map(|(node, shift)| (node.get(), shift.to_px()))
        .collect()
}

/// Fixed shifts follow Chrome's `ApplyBaselineShift`: `super` is a third of
/// the parent's size plus a pixel up, `sub` a fifth plus a pixel down, both
/// truncated; a length truncates; a percentage is of the box's own line
/// height. Edge alignments and the block's own `vertical-align` store none.
#[test]
fn a_boxs_shift_is_measured_where_no_line_decides_it() {
    use crate::style::VerticalAlign;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = vertically(&sized(&AHEM_FAMILY, 20.0), VerticalAlign::Super);
    let small = sized(&AHEM_FAMILY, 10.0);
    let tall = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(30.0),
            ..small.line
        },
        ..small
    };
    let cases = [
        (vertically(&small, VerticalAlign::Super), Some(7.65625)),
        (vertically(&small, VerticalAlign::Sub), Some(-5.0)),
        (vertically(&small, VerticalAlign::Px(3.3)), Some(3.296875)),
        (vertically(&small, VerticalAlign::Px(-2.0)), Some(-2.0)),
        (vertically(&tall, VerticalAlign::Fraction(0.5)), Some(15.0)),
        (vertically(&tall, VerticalAlign::Fraction(-0.1)), Some(-3.0)),
        (vertically(&small, VerticalAlign::Middle), None),
        (vertically(&small, VerticalAlign::TextTop), None),
        (vertically(&small, VerticalAlign::TextBottom), None),
        (vertically(&small, VerticalAlign::Top), None),
        (vertically(&small, VerticalAlign::Bottom), None),
        (vertically(&small, VerticalAlign::Baseline), None),
    ];
    for (style, shift) in cases {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "X");
            b.open_box(NodeKey(2), &style, None);
            b.text(NodeKey(3), "Y");
            b.close_box();
            b.atomic(NodeKey(4), &style, None, BoxSize::default());
        });
        let expected: Vec<(usize, f32)> = shift
            .map(|shift| vec![(2, shift), (4, shift)])
            .unwrap_or_default();
        assert_eq!(
            node_shifts(&layout),
            expected,
            "{:?}",
            style.line.vertical_align
        );
    }
    // Nested, each uses its parent's size: a third of 10 px, truncated, plus
    // a pixel.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(2), &vertically(&small, VerticalAlign::Super), None);
        b.open_box(NodeKey(3), &vertically(&small, VerticalAlign::Super), None);
        b.text(NodeKey(4), "Y");
        b.close_box();
        b.close_box();
    });
    assert_eq!(node_shifts(&layout), [(1, 7.65625), (2, 4.328125)]);
}

/// Under `SuperSubPosition::FontMetrics`, `super` and `sub` use the parent
/// font's `OS/2` offsets: 480 units up and 140 down, so 9.6 and 2.8 px at
/// 20 px, rounded to the grid.
#[test]
fn super_and_sub_can_follow_the_fonts_offsets() {
    use crate::style::VerticalAlign;
    let mut fixture = fixture();
    let mut config = Config::chrome_windows();
    config.super_sub = SuperSubPosition::FontMetrics;
    fixture.cx.set_config(config);
    let mut layout = Layout::new();
    let root = sized(&METRICS, 20.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &vertically(&root, VerticalAlign::Super), None);
        b.text(NodeKey(2), "X");
        b.close_box();
        b.open_box(NodeKey(3), &vertically(&root, VerticalAlign::Sub), None);
        b.text(NodeKey(4), "X");
        b.close_box();
    });
    assert_eq!(node_shifts(&layout), [(1, 9.59375), (3, -2.796875)]);
}

/// `dominant-baseline` moves nothing by default, as in Chrome outside SVG.
/// Under `DominantBaselines::Applied` a box in another font moves by the
/// difference between its baseline and its parent's, and keeps its
/// fragment. A box in its parent's font and an atomic inline don't move.
#[test]
fn dominant_baseline_moves_a_box_only_where_the_config_applies_it() {
    use crate::style::DominantBaseline;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = on_baseline(&sized(&AHEM_FAMILY, 20.0), DominantBaseline::Central);
    let same = root;
    let other = sized(&METRICS, 40.0);
    let other = on_baseline(&other, DominantBaseline::Central);
    let build = |fixture: &mut Fixture, layout: &mut Layout| {
        fixture.build(layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "X");
            b.open_box(NodeKey(2), &same, None);
            b.text(NodeKey(3), "Y");
            b.close_box();
            b.open_box(NodeKey(4), &other, None);
            b.text(NodeKey(5), "Z");
            b.close_box();
            b.atomic(NodeKey(6), &other, None, BoxSize::default());
        });
    };
    build(&mut fixture, &mut layout);
    assert!(node_shifts(&layout).is_empty());
    assert!(
        layout
            .measured()
            .text(FirstLineVariant::Standard)
            .kept_boxes()
            .get(NodeId::new(2))
            .is_none()
    );
    let mut config = Config::chrome_windows();
    config.dominant_baseline = DominantBaselines::Applied;
    fixture.cx.set_config(config);
    build(&mut fixture, &mut layout);
    let central = |node: usize, layout: &Layout| {
        let nodes = &layout.content().nodes;
        let id = nodes.text_facts(NodeId::new(node), FirstLineVariant::Standard);
        primary(layout, id).map_or(0.0, |used| used.metrics.central.to_px())
    };
    let raise = central(4, &layout) - central(0, &layout);
    assert_ne!(raise, 0.0);
    assert_eq!(node_shifts(&layout), [(4, raise)]);
    assert!(
        layout
            .measured()
            .text(FirstLineVariant::Standard)
            .kept_boxes()
            .get(NodeId::new(4))
            .is_some()
    );
}

/// Lengths and factors of any size, or NaN, measure without panicking.
#[test]
fn any_shift_gives_valid_measurements() {
    use crate::style::VerticalAlign;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let aligns = [
        VerticalAlign::Px(f32::MAX),
        VerticalAlign::Px(f32::MIN),
        VerticalAlign::Px(f32::NAN),
        VerticalAlign::Px(f32::INFINITY),
        VerticalAlign::Fraction(f32::NAN),
        VerticalAlign::Fraction(1e30),
        VerticalAlign::Fraction(f32::NEG_INFINITY),
    ];
    for align in aligns {
        let huge = sized(&AHEM_FAMILY, f32::MAX);
        fixture.build(&mut layout, &ComputedBlockStyle::new(&huge), |b| {
            b.open_box(NodeKey(1), &vertically(&root, align), None);
            b.text(NodeKey(2), "X");
            b.open_box(NodeKey(3), &vertically(&huge, VerticalAlign::Super), None);
            b.text(NodeKey(4), "X");
            b.close_box();
            b.close_box();
        });
    }
}
