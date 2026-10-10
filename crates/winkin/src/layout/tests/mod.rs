//! Public view tests.
//!
//! This file holds the context, the builder and the shared views. It pins
//! that every degenerate area reads without panicking. The children pin:
//! - `text_runs`: lines and text runs, their fonts, clusters and glyphs;
//! - `boxes`: box edges between items, culled boxes and atomic inlines;
//! - `paint`: paint order and decorations;
//! - `generated`: hyphens and ellipses;
//! - `emphasis`: emphasis marks;
//! - `ruby`: annotations, nesting and splitting;
//! - `initial`: initial and first letters;
//! - `vertical`: vertical lines and combined text;
//! - `positioned`: absolutely positioned boxes' static positions;
//! - `seeks`: the searches painting and reading make.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use std::thread;

use fontwich::{Collection, LayerBuilder, Role};

use crate::tests::TestFallback;

use crate::build::BoxSize;
use crate::config::Config;
use crate::data::Id;
use crate::paint::{Decorates, Paint};
use crate::stages::fonts::UsedFontId;
use crate::stages::fragments::{FragmentItemKind, GlyphWalk};
use crate::stages::lines::{Area, InlineExtents, LineId, NoExclusions};
use crate::style::{
    BaseDirection, BidiGroup, ComputedStyle, Direction, EdgesGroup, EmphasisSide, EmphasisSkip,
    FontFamilyName, InitialLetter, LineClamp, LineGroup, LineHeight, RubyAlign, RubyPosition,
    Sides, TextAlign, TextAlignLast, UnicodeBidi, WhiteSpaceCollapse, WordBreak,
};
use crate::tests::{
    AHEM_FAMILY, ARABIC, LATIN, NARROW, TestFont, across, ahem_fallback, along, arabic, collection,
    latin, sized,
};
use crate::{
    BuildOptions, ComputedBlockStyle, Context, CrossExtents, InlineEdges, Item, Layout,
    LayoutBuilder, NodeKey, TextRun,
};

/// `extents`' two ends, to print as a pair.
fn ends(extents: InlineExtents) -> (f32, f32) {
    (extents.left, extents.right)
}

/// A context over Ahem, a Latin font with ligatures and a kerned pair, one
/// whose advances fall between layout's grid, and an Arabic one.
fn context() -> Context {
    let mut narrow = TestFont::new("Test Narrow", &[(0x20, 0x7E)]);
    narrow.advances = (0x20u8..=0x7E).map(|b| (char::from(b), 333)).collect();
    Context::new(collection(&[latin(), narrow, arabic()], ahem_fallback()))
}

/// Builds into `layout` in `block`.
fn build(
    cx: &mut Context,
    layout: &mut Layout,
    block: &ComputedBlockStyle<'_>,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) {
    let mut b = layout.builder(NodeKey(0), block, BuildOptions::default());
    calls(&mut b);
    assert!(b.finish(cx).is_complete());
}

/// Every text run of line `n`.
fn runs(layout: &Layout, n: usize) -> Vec<TextRun<'_>> {
    layout
        .line(n)
        .map(|line| {
            line.items()
                .filter_map(|item| match item {
                    Item::Text(run) => Some(run),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The room each line's items take, walked as a painter walks them: each
/// box's own edges before and after its part, and each leaf where it
/// stands. Checks that every leaf stands where the walk has got to, and
/// returns each line's pen at its end.
fn walk(layout: &Layout) -> Vec<f32> {
    let mut ends = Vec::new();
    for line in layout.lines() {
        let mut pen: Option<f32> = None;
        let mut before = 0.0;
        let mut open: Vec<(usize, f32)> = Vec::new();
        let close = |pen: &mut Option<f32>, before: &mut f32, room: f32| match pen {
            Some(pen) => *pen += room,
            None => *before += room,
        };
        for (index, item) in line.all_items().enumerate() {
            while let Some(&(end, room)) = open.last() {
                if end > index {
                    break;
                }
                open.pop();
                close(&mut pen, &mut before, room);
            }
            let leaf = match item {
                Item::Box(piece) => {
                    let InlineEdges { left, right } = piece.edges();
                    close(&mut pen, &mut before, left);
                    open.push((index + 1 + piece.descendants(), right));
                    None
                }
                Item::Text(run) | Item::Generated(run) => Some((run.inline(), run.advance())),
                Item::Atomic(atomic) => Some((atomic.inline(), atomic.advance())),
            };
            if let Some((InlineExtents { left, right }, advance)) = leaf {
                assert!((right - left - advance).abs() < 1e-3);
                if let Some(pen) = pen {
                    assert!(
                        (left - pen).abs() < 1e-3,
                        "line {}: a leaf at {left}, the walk at {pen}",
                        line.index()
                    );
                }
                pen = Some(right);
            }
        }
        while let Some((_, room)) = open.pop() {
            close(&mut pen, &mut before, room);
        }
        ends.push(pen.unwrap_or(before));
    }
    ends
}

/// A style at `size` pixels in Ahem that sets emphasis marks, skipping
/// `skip`.
fn marked(size: f32, skip: EmphasisSkip) -> ComputedStyle<'static> {
    let mut style = sized(&AHEM_FAMILY, size);
    style.text.emphasis.marks = true;
    style.text.emphasis.skip = skip;
    style
}

/// Before any break, and after a rebuild, there is nothing to read; and
/// every degenerate area reads, items and glyphs and boxes, without
/// panicking.
#[test]
fn any_area_gives_valid_reads() {
    let mut cx = context();
    let mut layout = Layout::new();
    assert_eq!(layout.lines().len(), 0);
    assert_eq!(layout.metrics(), crate::LayoutMetrics::default());
    assert_eq!(layout.box_fragments(NodeKey(0)).count(), 0);
    let root = sized(&LATIN, 16.0);
    let roomy = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(4.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "office\tAVA ");
        b.open_box(NodeKey(2), &roomy, None);
        b.text(NodeKey(3), "flat");
        b.close_box();
        b.open_box(NodeKey(4), &root, None);
        b.close_box();
        b.line_break(NodeKey(5));
    });
    assert_eq!(layout.lines().len(), 0, "nothing before a break");
    for width in [0.0, -5.0, f32::NAN, f32::INFINITY, 1.0, 33.3, 1000.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        for line in layout.lines() {
            let _ = (line.metrics(), line.text_range());
            for item in line.all_items() {
                match item {
                    Item::Text(run) | Item::Generated(run) => {
                        let _ = (run.inline(), run.block(), run.font(), run.key());
                        assert!(run.glyphs().all(|glyph| glyph.x.is_finite()));
                        assert!(run.clusters().all(|cluster| cluster.advance().is_finite()));
                    }
                    Item::Atomic(atomic) => {
                        let _ = (atomic.inline(), atomic.block());
                    }
                    Item::Box(piece) => {
                        let _ = (piece.inline(), piece.block(), piece.edges());
                    }
                }
            }
        }
        for key in 0..7 {
            for piece in layout.box_fragments(NodeKey(key)) {
                assert!(piece.line() < layout.lines().len());
            }
        }
        let _ = layout.metrics();
    }
    assert!(layout.line(usize::MAX).is_none());
    assert!(layout.line(LineId::MAX + 1).is_none());
    // A rebuild leaves nothing to read until the next break.
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "again")
    });
    assert_eq!(layout.lines().len(), 0);
    assert_eq!(layout.metrics().last_baseline, None);
}

mod boxes;
mod emphasis;
mod generated;
mod graphemes;
mod initial;
mod paint;
mod positioned;
mod ruby;
#[cfg(debug_assertions)]
mod seeks;
mod sizing;
mod text_runs;
mod vertical;
