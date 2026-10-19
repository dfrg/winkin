//! `calc()` tests. They pin, against values measured in Chrome 155:
//! - `max()`, `min()` and `clamp()` text indents resolved in each area;
//! - the intrinsic sizes taking a `calc()` indent at a zero basis;
//! - `max()` padding resolved against each percentage basis;
//! - padding clamped at zero, linear or not;
//! - a `calc()` without a resolver resolving to zero.

use alloc::boxed::Box;

use super::*;
use crate::style::{
    CalcHandle, CalcResolver, LengthPercentage, LengthPercentageOrCalc, TextIndent,
};

/// `max(20px, 10%)`.
const MAX: CalcHandle = CalcHandle::new(1);
/// `min(20px, 10%)`.
const MIN: CalcHandle = CalcHandle::new(2);
/// `clamp(5px, 50%, 30px)`.
const CLAMP: CalcHandle = CalcHandle::new(3);
/// `calc(10px - 50%)`, unclamped.
const LESS_HALF: CalcHandle = CalcHandle::new(4);
/// `max(4px, 10%)`.
const MAX_SMALL: CalcHandle = CalcHandle::new(5);

/// Resolves the handles above as a host would.
struct Expressions;

impl CalcResolver for Expressions {
    fn resolve(&self, calc: CalcHandle, basis: f32) -> f32 {
        match calc {
            MAX => f32::max(20.0, 0.1 * basis),
            MIN => f32::min(20.0, 0.1 * basis),
            CLAMP => (0.5 * basis).clamp(5.0, 30.0),
            LESS_HALF => 10.0 - 0.5 * basis,
            MAX_SMALL => f32::max(4.0, 0.1 * basis),
            _ => f32::NAN,
        }
    }
}

/// Returns a context resolving the handles above.
fn resolving() -> Context {
    let mut cx = context();
    cx.set_calc_resolver(Some(Box::new(Expressions)));
    cx
}

/// Returns where line 0's first text run starts.
fn text_start(layout: &Layout) -> Option<f32> {
    runs(layout, 0).first().map(|run| ends(run.inline()).0)
}

/// Builds `x` in Ahem at 10 px in a block indented by `amount`.
fn indented(cx: &mut Context, layout: &mut Layout, amount: LengthPercentageOrCalc) {
    let root = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle {
        text_indent: TextIndent {
            amount,
            hanging: false,
            each_line: false,
        },
        ..ComputedBlockStyle::new(&root)
    };
    build(cx, layout, &block, |b| b.text(NodeKey(1), "x"));
}

/// Returns where `x` starts in an inline box with `padding` on its left,
/// built with `basis` and broken 400 px wide.
fn padded_start(cx: &mut Context, padding: LengthPercentageOrCalc, basis: f32) -> Option<f32> {
    let root = sized(&AHEM_FAMILY, 10.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides {
                left: padding,
                ..Sides::from_px(0.0)
            },
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let mut layout = Layout::new();
    let block = ComputedBlockStyle::new(&root);
    let options = BuildOptions {
        percentage_basis: basis,
        ..BuildOptions::default()
    };
    let mut b = layout.builder(cx, NodeKey(0), &block, options);
    b.open_box(NodeKey(1), &padded, None);
    b.text(NodeKey(2), "x");
    b.close_box();
    assert!(b.finish().is_complete());
    layout.break_lines(cx, Area::new(400.0), &mut NoExclusions);
    text_start(&layout)
}

/// `text-indent: max(20px, 10%)` indents 20 px in a 100 px block and 40 px
/// in a 400 px one, as Chrome resolves it against the block's width.
#[test]
fn a_max_text_indent_takes_the_larger_in_each_area() {
    let mut cx = resolving();
    let mut layout = Layout::new();
    indented(&mut cx, &mut layout, MAX.into());
    for (width, indent) in [(100.0, 20.0), (400.0, 40.0)] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(text_start(&layout), Some(indent), "{width} px wide");
    }
}

/// `min(20px, 10%)` indents 10 px in a 100 px block, and
/// `clamp(5px, 50%, 30px)` 30 px there and 10 px in a 20 px one, as in
/// Chrome.
#[test]
fn min_and_clamp_text_indents_resolve_in_each_area() {
    let mut cx = resolving();
    for (amount, width, indent) in [
        (MIN, 100.0, 10.0),
        (CLAMP, 100.0, 30.0),
        (CLAMP, 20.0, 10.0),
    ] {
        let mut layout = Layout::new();
        indented(&mut cx, &mut layout, amount.into());
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(
            text_start(&layout),
            Some(indent),
            "{amount:?} {width} px wide"
        );
    }
}

/// The intrinsic sizes take `max(20px, 10%)` at a basis of zero, 20 px:
/// Chrome sizes such a block's max-content `x` 20 px plus its glyph wide.
#[test]
fn the_intrinsic_sizes_take_a_calc_indent_at_a_zero_basis() {
    let mut cx = resolving();
    let mut layout = Layout::new();
    indented(&mut cx, &mut layout, MAX.into());
    assert_eq!(layout.intrinsic_sizes().max_content, 30.0);
}

/// `padding-inline-start: max(4px, 10%)` is 10 px against a 100 px basis
/// and 40 px against 400 px, as in Chrome, whether built with the basis or
/// measured with it.
#[test]
fn a_max_padding_resolves_against_each_basis() {
    let mut cx = resolving();
    assert_eq!(padded_start(&mut cx, MAX_SMALL.into(), 100.0), Some(10.0));
    assert_eq!(padded_start(&mut cx, MAX_SMALL.into(), 400.0), Some(40.0));
    let root = sized(&AHEM_FAMILY, 10.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides {
                left: MAX_SMALL.into(),
                ..Sides::from_px(0.0)
            },
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &padded, None);
        b.text(NodeKey(2), "x");
        b.close_box();
    });
    for (basis, padding) in [(100.0, 10.0), (400.0, 40.0)] {
        assert!(layout.measure(&mut cx, basis, []));
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        assert_eq!(text_start(&layout), Some(padding), "basis {basis}");
    }
}

/// `padding-inline-start: calc(10px - 50%)` is 2 px against a 16 px basis
/// and clamps at zero against 400 px, as in Chrome. Given linear or as a
/// `calc()` whose resolver leaves it negative, it clamps alike.
#[test]
fn a_padding_below_zero_clamps_at_zero() {
    let mut cx = resolving();
    let linear = LengthPercentage {
        px: 10.0,
        fraction: -0.5,
    };
    for padding in [linear.into(), LESS_HALF.into()] {
        assert_eq!(
            padded_start(&mut cx, padding, 16.0),
            Some(2.0),
            "{padding:?}"
        );
        assert_eq!(
            padded_start(&mut cx, padding, 400.0),
            Some(0.0),
            "{padding:?}"
        );
    }
}

/// Without a resolver, a `calc()` indent and padding are zero.
#[test]
fn a_calc_without_a_resolver_is_zero() {
    let mut cx = context();
    let mut layout = Layout::new();
    indented(&mut cx, &mut layout, MAX.into());
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(text_start(&layout), Some(0.0));
    assert_eq!(layout.intrinsic_sizes().max_content, 10.0);
    assert_eq!(padded_start(&mut cx, MAX_SMALL.into(), 400.0), Some(0.0));
}
