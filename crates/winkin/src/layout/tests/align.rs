//! Alignment set on a built layout. It pins that:
//! - lines broken after a change equal a fresh layout's, in either direction
//!   and under greedy, balanced and pretty wrapping;
//! - the change runs no preparation and does not allocate;
//! - an unchanged alignment keeps the lines.

use super::*;
use crate::config::PastLines;
use crate::selection::{Affinity, Position};
use crate::style::TextWrapStyle;
use crate::tests::{BEH, MEEM, SEEN, allocator::count_allocations};
#[cfg(debug_assertions)]
use crate::work;

/// Builds words in Ahem with a padded inline box, an atomic inline, an
/// Arabic span and a forced break, in `block`.
fn words(cx: &mut Context, layout: &mut Layout, block: &ComputedBlockStyle<'_>) {
    let root = block.style;
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..*root
    };
    let arabic = sized(&ARABIC, 10.0);
    let size = BoxSize {
        inline: 24.0,
        block: 12.0,
        baseline: Some(10.0),
    };
    let word: String = [BEH, MEEM, SEEN, BEH].iter().collect();
    build(cx, layout, block, |b| {
        b.text(NodeKey(1), "XX XXX ");
        b.open_box(NodeKey(2), &padded, None);
        b.text(NodeKey(3), "XXXX XX");
        b.close_box();
        b.text(NodeKey(4), " X ");
        b.atomic(NodeKey(5), root, None, size);
        b.text(NodeKey(6), " XXX XX ");
        b.open_box(NodeKey(7), &arabic, None);
        b.text(NodeKey(8), &format!("{word} {word}"));
        b.close_box();
        b.text(NodeKey(9), " XXXXX X");
        b.line_break(NodeKey(10));
        b.text(NodeKey(11), "XXX XX XXXXX XX X XXXX");
    });
}

/// Returns everything the views read of `layout`: each line's metrics, each
/// text run's place and glyphs, each atomic inline's place, a caret at
/// every offset and the hit on a grid of points.
fn views(layout: &Layout) -> Vec<String> {
    let mut out = Vec::new();
    for line in layout.lines() {
        out.push(format!("line {:?} {:?}", line.text_range(), line.metrics()));
        for item in line.all_items() {
            match item {
                Item::Text(run) | Item::Generated(run) => {
                    let glyphs: Vec<_> = run.glyphs().collect();
                    out.push(format!("run {:?} {glyphs:?}", ends(run.inline())));
                }
                Item::Atomic(atomic) => out.push(format!("atomic {:?}", ends(atomic.inline()))),
                Item::Box(piece) => out.push(format!("box {:?}", piece.edges())),
            }
        }
    }
    for offset in 0..=layout.text().len() {
        for affinity in [Affinity::Downstream, Affinity::Upstream] {
            let caret = layout.caret(Position::new(offset, affinity));
            out.push(format!("caret {offset} {affinity:?} {caret:?}"));
        }
    }
    for y in (0..8).map(|n| n as f32 * 9.0) {
        for x in (0..30).map(|n| n as f32 * 6.5 - 10.0) {
            let hit = layout.hit_test(x, y, PastLines::Column);
            out.push(format!("hit {x} {y} {hit:?}"));
        }
    }
    out
}

/// A layout built with `start` alignment and then set to another breaks as
/// a layout built with that alignment: the same lines, items, glyph
/// positions, carets and hits, also right to left, balanced and pretty.
#[test]
fn a_relayout_with_another_alignment_equals_a_fresh_layout() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let mut cx = context();
    let lasts = [
        TextAlignLast::Auto,
        TextAlignLast::Start,
        TextAlignLast::End,
        TextAlignLast::Center,
        TextAlignLast::Justify,
    ];
    for direction in [BaseDirection::Ltr, BaseDirection::Rtl] {
        for wrap in [
            TextWrapStyle::Auto,
            TextWrapStyle::Balance,
            TextWrapStyle::Pretty,
        ] {
            let block = |text_align, text_align_last| ComputedBlockStyle {
                direction,
                text_align,
                text_align_last,
                text_wrap_style: wrap,
                ..ComputedBlockStyle::new(&root)
            };
            let mut layout = Layout::new();
            words(
                &mut cx,
                &mut layout,
                &block(TextAlign::Start, TextAlignLast::Auto),
            );
            layout.break_lines(&mut cx, Area::new(130.0), &mut NoExclusions);
            let started = views(&layout);
            for align in [TextAlign::End, TextAlign::Center, TextAlign::Justify] {
                for last in lasts {
                    let mut fresh = Layout::new();
                    words(&mut cx, &mut fresh, &block(align, last));
                    assert!(layout.set_text_align(align, last));
                    assert_eq!(layout.lines().len(), 0, "a change clears the lines");
                    for width in [55.0, 130.0, 400.0] {
                        let area = Area::new(width);
                        layout.break_lines(&mut cx, area, &mut NoExclusions);
                        fresh.break_lines(&mut cx, area, &mut NoExclusions);
                        let at = format!("{direction:?} {wrap:?} {align:?} {last:?} at {width}");
                        assert_eq!(
                            layout.line_records().lines.as_slice(),
                            fresh.line_records().lines.as_slice(),
                            "{at}"
                        );
                        assert_eq!(
                            layout.fragments().items.as_slice(),
                            fresh.fragments().items.as_slice(),
                            "{at}"
                        );
                        let seen = views(&layout);
                        assert_eq!(seen, views(&fresh), "{at}");
                        if width == 130.0 {
                            assert_ne!(seen, started, "{at} moves some line");
                        }
                        assert_eq!(layout.metrics(), fresh.metrics(), "{at}");
                        assert_eq!(layout.intrinsic_sizes(), fresh.intrinsic_sizes());
                    }
                }
            }
        }
    }
}

/// Setting the alignment runs no preparation and allocates nothing, and
/// setting the same alignment again keeps the lines.
#[test]
fn setting_the_alignment_prepares_nothing() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let mut cx = context();
    let mut layout = Layout::new();
    words(&mut cx, &mut layout, &ComputedBlockStyle::new(&root));
    let heap = layout.heap_bytes();
    #[cfg(debug_assertions)]
    work::take();
    let allocations = count_allocations(|| {
        assert!(layout.set_text_align(TextAlign::Justify, TextAlignLast::Center));
    });
    assert_eq!(allocations, 0);
    #[cfg(debug_assertions)]
    assert_eq!(work::take(), 0, "no stage takes a step");
    assert_eq!(layout.heap_bytes(), heap);
    layout.break_lines(&mut cx, Area::new(130.0), &mut NoExclusions);
    let lines = layout.lines().len();
    assert!(lines > 1);
    assert!(!layout.set_text_align(TextAlign::Justify, TextAlignLast::Center));
    assert_eq!(
        layout.lines().len(),
        lines,
        "the same alignment keeps the lines"
    );
}
