//! Mixed directions and boxes: isolates, overrides, nested and cloned boxes,
//! and nestings hundreds deep allocate nothing warm.

use super::test_fonts::{self, TestFont, ahem_fallback};
use super::{arabic, count_allocations};
use fontwich::Collection;
use winkin::paint::{Decorates, Paint};
use winkin::style::{
    BaseDirection, BoxDecorationBreak, ComputedStyle, Direction, EdgesGroup, FontFamilyName,
    FontGroup, Sides, UnicodeBidi,
};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

/// Ahem, Latin and Hebrew with ligatures, and Arabic with joining forms.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E), (0x5D0, 0x5EA)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i']];
    latin.kerning = vec![('A', 'V', -100)];
    test_fonts::collection(&[latin, arabic()], ahem_fallback())
}

/// Mixed-direction paragraphs in a block reading `direction`: Latin,
/// Hebrew and Arabic with digits and brackets, in boxes with every
/// `unicode-bidi` that nest three deep and have edges, some of them
/// cloning their decoration, across forced breaks, `repeat` times over;
/// so the resolver runs on synthesized controls, runs split at levels,
/// shaping goes both ways, the breaker pays cloned edges, and line
/// layout reorders pieces and splits boxes into parts.
fn document(layout: &mut Layout, cx: &mut Context, direction: BaseDirection, repeat: usize) {
    let families = [
        FontFamilyName::named("Test Latin"),
        FontFamilyName::named("Test Arabic"),
    ];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let with = |unicode_bidi, rtl: bool, clone: bool| ComputedStyle {
        bidi: winkin::style::BidiGroup {
            direction: if rtl { Direction::Rtl } else { Direction::Ltr },
            unicode_bidi,
        },
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 2.0,
                right: 3.5,
                ..Sides::ZERO
            }
            .into(),
            decoration_break: if clone {
                BoxDecorationBreak::Clone
            } else {
                BoxDecorationBreak::Slice
            },
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    // Two of them neither paint nor take room, so are culled, and read
    // back as parts of their descendants.
    let culled = |unicode_bidi, rtl: bool| ComputedStyle {
        edges: EdgesGroup::INITIAL,
        paints: false,
        ..with(unicode_bidi, rtl, false)
    };
    let styles = [
        with(UnicodeBidi::Isolate, true, false),
        with(UnicodeBidi::BidiOverride, true, true),
        culled(UnicodeBidi::Embed, false),
        with(UnicodeBidi::IsolateOverride, true, false),
        with(UnicodeBidi::Plaintext, false, true),
        culled(UnicodeBidi::Normal, true),
    ];
    let block = ComputedBlockStyle {
        direction,
        ..ComputedBlockStyle::new(&root)
    };
    let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    for n in 0..repeat {
        let outer = &styles[n % styles.len()];
        let inner = &styles[(n + 2) % styles.len()];
        let deepest = &styles[(n + 4) % styles.len()];
        b.text(next(), "office AVAIL \u{5E9}\u{5DC}\u{5D5}\u{5DD} (12) ");
        b.open_box(next(), outer, None);
        b.text(next(), "\u{628}\u{62A}\u{633} \u{661}\u{662} fit ");
        b.open_box(next(), inner, None);
        b.text(next(), "[\u{5D0}\u{5D1}] waffle ");
        b.open_box(next(), deepest, None);
        b.text(next(), "\u{645}\u{646}\u{64A} 3.5 ");
        b.close_box();
        if n % 3 == 0 {
            b.line_break(next());
        }
        b.text(next(), "flat ");
        b.close_box();
        b.text(next(), "\u{5D2}\u{5D3} ");
        b.close_box();
        b.text(next(), "end. ");
    }
    assert!(b.finish(cx).is_complete());
}

/// Reads every line back as a host does: items, glyphs, clusters, the
/// paint in Chrome's order with every box decorating, and every box's
/// parts by key, culled or kept. Returns a sum of what was read.
fn read(layout: &Layout, keys: u64) -> f32 {
    let mut sum = 0.0;
    for line in layout.lines() {
        for item in line.all_items() {
            match item {
                Item::Text(run) | Item::Generated(run) => {
                    sum += run.inline().left + run.advance();
                    for glyph in run.glyphs() {
                        sum += glyph.x + glyph.id as f32;
                    }
                    for cluster in run.clusters() {
                        sum += cluster.inline().left;
                    }
                }
                Item::Atomic(atomic) => sum += atomic.inline().right,
                Item::Box(piece) => sum += piece.inline().right + piece.edges().left,
            }
        }
        for item in line.paints(|_| Decorates::Both) {
            sum += match item {
                Paint::Background(background) => background.inline().right,
                Paint::Box(piece) => piece.inline().left,
                Paint::DecorationBeforeText(bar) | Paint::DecorationAfterText(bar) => {
                    bar.inline().right + bar.underline_thickness()
                }
                Paint::Text(run) | Paint::Generated(run) => run.inline().left,
                Paint::Atomic(atomic) => atomic.inline().left,
                _ => 0.0,
            };
        }
    }
    for key in 0..keys {
        for piece in layout.box_fragments(NodeKey(key)) {
            sum += piece.inline().right + piece.line() as f32;
        }
    }
    sum
}

/// A warm rebuild of mixed-direction text with isolates, overrides,
/// nested boxes and cloned ones allocates nothing, and nor does laying
/// it out again at any width laid out at before, nor reading it back,
/// paint and box queries included, in a block reading either way or
/// taking each paragraph's direction from its text.
#[test]
fn mixed_directions_nested_boxes_and_clones_allocate_nothing_warm() {
    let widths = [37.0, 81.25, 150.0, 600.0, 3.0];
    for direction in [BaseDirection::Ltr, BaseDirection::Rtl, BaseDirection::Auto] {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        let cold = count_allocations(|| document(&mut layout, &mut cx, direction, 12));
        assert!(cold > 0, "a cold layout grows");
        let warm = count_allocations(|| document(&mut layout, &mut cx, direction, 12));
        assert_eq!(warm, 0, "{direction:?}: rebuilding allocated");
        let warm = count_allocations(|| document(&mut layout, &mut cx, direction, 7));
        assert_eq!(
            warm, 0,
            "{direction:?}: rebuilding something smaller allocated"
        );
        document(&mut layout, &mut cx, direction, 12);
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            read(&layout, 200);
        }
        let warm = count_allocations(|| {
            for &width in &widths {
                layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            }
        });
        assert_eq!(warm, 0, "{direction:?}: laying out again allocated");
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            let mut sum = 0.0;
            let warm = count_allocations(|| sum = read(&layout, 200));
            assert!(sum != 0.0);
            assert_eq!(warm, 0, "{direction:?} at {width}: reading back allocated");
        }
    }
}

/// Boxes nested `depth` deep, each inside the last, of every kind
/// `document` sets and pinned to the line's top or its middle besides,
/// each holding a word, and a forced break every seventeenth: so every
/// line and paragraph starts inside hundreds of boxes, and line layout's
/// parts, the breaker's record of the boxes open and the controls a
/// paragraph re-opens all grow with the depth.
fn nested(layout: &mut Layout, cx: &mut Context, depth: u64) {
    use winkin::style::{LineGroup, VerticalAlign};
    let families = [
        FontFamilyName::named("Test Latin"),
        FontFamilyName::named("Test Arabic"),
    ];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let with = |unicode_bidi, rtl: bool, clone: bool| ComputedStyle {
        bidi: winkin::style::BidiGroup {
            direction: if rtl { Direction::Rtl } else { Direction::Ltr },
            unicode_bidi,
        },
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 1.0,
                right: 1.5,
                ..Sides::ZERO
            }
            .into(),
            decoration_break: if clone {
                BoxDecorationBreak::Clone
            } else {
                BoxDecorationBreak::Slice
            },
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    let aligned = |align| ComputedStyle {
        line: LineGroup {
            vertical_align: align,
            ..root.line
        },
        ..root
    };
    let styles = [
        with(UnicodeBidi::Isolate, true, false),
        with(UnicodeBidi::BidiOverride, true, true),
        ComputedStyle {
            edges: EdgesGroup::INITIAL,
            paints: false,
            ..with(UnicodeBidi::Embed, false, false)
        },
        with(UnicodeBidi::IsolateOverride, false, false),
        aligned(VerticalAlign::Top),
        with(UnicodeBidi::Plaintext, false, true),
        aligned(VerticalAlign::Middle),
        root,
    ];
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    for n in 0..depth {
        b.open_box(NodeKey(2 * n + 1), &styles[n as usize % styles.len()], None);
        b.text(NodeKey(2 * n + 2), "fit \u{5D0}\u{5D1} ");
        if n % 17 == 16 {
            b.line_break(NodeKey(2 * n + 2));
        }
    }
    for _ in 0..depth {
        b.close_box();
    }
    assert!(b.finish(cx).is_complete());
}

/// A warm rebuild of boxes nested hundreds deep allocates nothing, nor
/// laying them out again at any width laid out at before, nor reading
/// them back, paint and box queries included: every walk the depth
/// lengthens keeps its scratch in the context, cleared and never
/// dropped.
#[test]
fn a_deep_nesting_allocates_nothing_warm() {
    let widths = [3.0, 120.0, 800.0, 1e6];
    let mut cx = Context::new(collection());
    let mut layout = Layout::new();
    let cold = count_allocations(|| nested(&mut layout, &mut cx, 300));
    assert!(cold > 0, "a cold layout grows");
    let warm = count_allocations(|| nested(&mut layout, &mut cx, 300));
    assert_eq!(warm, 0, "rebuilding allocated");
    for &width in &widths {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        read(&layout, 601);
    }
    let warm = count_allocations(|| {
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        }
    });
    assert_eq!(warm, 0, "laying out again allocated");
    for &width in &widths {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        let mut sum = 0.0;
        let warm = count_allocations(|| sum = read(&layout, 601));
        assert!(sum != 0.0);
        assert_eq!(warm, 0, "at {width}: reading back allocated");
    }
}
