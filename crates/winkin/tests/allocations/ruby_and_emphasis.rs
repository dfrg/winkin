//! Ruby and emphasis marks, split ruby continuations included, allocate
//! nothing warm.

use super::count_allocations;
use super::test_fonts::{self, TestFont, ahem_fallback};
use fontwich::Collection;
use winkin::config::{Config, EmphasisRoom, RubyOverhangRule};
use winkin::paint::{Decorates, Paint};
use winkin::style::{
    ComputedStyle, EmphasisSide, FontFamilyName, FontGroup, LineGroup, LineHeight, RubyAlign,
    RubyOverhang, RubyPosition, Sides, TextAlign,
};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

/// Ahem, and Latin with ligatures.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E)]);
    latin.ligatures = vec![vec!['f', 'i'], vec!['f', 'l']];
    test_fonts::collection(&[latin], ahem_fallback())
}

/// Builds paragraphs of ruby `repeat` times in a justified block.
///
/// Columns have annotations on both sides and stacked levels, under every
/// `ruby-align` and `ruby-overhang`, with a ruby in a base, a forced break
/// in one, and emphasis marks over and under bases and over annotation
/// text. Measurement sizes columns and levels, the breaker makes each
/// line's room, and line layout writes annotation lines.
fn document(layout: &mut Layout, cx: &mut Context, repeat: usize) {
    let families = [
        FontFamilyName::named("Test Latin"),
        FontFamilyName::named("Ahem"),
    ];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            size: 20.0,
            ..FontGroup::INITIAL
        },
        line: LineGroup {
            height: LineHeight::Factor(1.25),
            ..LineGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let aligns = [
        RubyAlign::SpaceAround,
        RubyAlign::SpaceBetween,
        RubyAlign::Center,
        RubyAlign::Start,
    ];
    let overhangs = [RubyOverhang::Auto, RubyOverhang::Spaces, RubyOverhang::None];
    let positions = [
        RubyPosition::Over,
        RubyPosition::Under,
        RubyPosition::Alternate,
    ];
    let ruby = |n: usize| {
        let mut style = root;
        style.ruby.align = aligns[n % aligns.len()];
        style.ruby.overhang = overhangs[n % overhangs.len()];
        style.ruby.position = positions[n % positions.len()];
        style
    };
    let mut small = root;
    small.font.size = 10.0;
    let mut boxed = small;
    boxed.edges.padding = Sides::from_px(1.5);
    boxed.paints = true;
    let marked = |n: usize| {
        let mut style = root;
        style.text.emphasis.marks = true;
        style.text.emphasis.position.side = if n.is_multiple_of(2) {
            EmphasisSide::Over
        } else {
            EmphasisSide::Under
        };
        style
    };
    let block = ComputedBlockStyle {
        text_align: TextAlign::Justify,
        ..ComputedBlockStyle::new(&root)
    };
    let mut b = layout.builder(cx, NodeKey(0), &block, BuildOptions::default());
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    for n in 0..repeat {
        b.text(next(), "words before ");
        b.open_ruby(next(), &ruby(n), None);
        b.open_box(next(), &marked(n), None);
        b.text(next(), "fit base");
        b.close_box();
        b.open_annotation(next(), &small, None);
        b.text(next(), "a longer ");
        // A box inside the annotation, with edges and a fragment of its
        // own on the annotation line.
        b.open_box(next(), &boxed, None);
        b.text(next(), "reading");
        b.close_box();
        b.close_annotation();
        b.open_annotation(next(), &small, None);
        b.text(next(), "fl");
        b.close_annotation();
        b.text(next(), "B");
        b.open_annotation(next(), &marked(n + 1), None);
        b.text(next(), "bb");
        b.close_annotation();
        b.open_ruby(next(), &ruby(n + 1), None);
        b.text(next(), "in");
        b.line_break(next());
        b.open_annotation(next(), &small, None);
        b.text(next(), "nested");
        b.close_ruby();
        b.close_ruby();
        b.open_box(next(), &marked(n), None);
        b.text(next(), " and marked, text ");
        b.close_box();
        if n % 4 == 3 {
            b.line_break(next());
        }
    }
    assert!(b.finish().is_complete());
}

/// Reads every line back: its items' glyphs and marks, its annotations'
/// runs, glyphs and marks, and its paint.
fn read(layout: &Layout) -> f32 {
    let mut sum = 0.0;
    for line in layout.lines() {
        sum += line.metrics().baseline;
        for item in line.items() {
            if let Item::Text(run) = item {
                for glyph in run.glyphs() {
                    sum += glyph.x;
                }
                for mark in run.emphasis_marks() {
                    sum += mark.x + mark.baseline;
                }
            }
        }
        for annotation in line.annotations() {
            sum += annotation.baseline() + annotation.inline().left;
            for span in annotation.boxes() {
                sum += span.inline().left + span.block().under;
            }
            for run in annotation.runs() {
                for glyph in run.glyphs() {
                    sum += glyph.x;
                }
                for cluster in run.clusters() {
                    sum += cluster.advance();
                }
                for mark in run.emphasis_marks() {
                    sum += mark.x;
                }
            }
        }
        for item in line.paints(|_| Decorates::Both) {
            if let Paint::Emphasis(mark) = item {
                sum += mark.size;
            }
        }
    }
    sum
}

/// A warm rebuild of ruby and emphasis marks allocates nothing, under each
/// `Config::ruby_overhang` and `Config::emphasis_room`.
///
/// Nor does breaking it again at any width used before, or reading it
/// back. Columns and levels are ranges of one table each, and the breaker's
/// level bands and line layout's annotation pieces keep their capacity.
#[test]
fn ruby_and_emphasis_allocate_nothing_warm() {
    let widths = [45.0, 120.0, 333.3, 900.0, 4.0];
    // Room over the first line, which its annotations take first.
    let area = |width: f32| Area {
        room_above: 3.0,
        ..Area::new(width)
    };
    for (overhang, room) in [
        (RubyOverhangRule::AdjacentText, EmphasisRoom::Shared),
        (RubyOverhangRule::KanaOnly, EmphasisRoom::Uniform),
    ] {
        let mut cx = Context::new(collection());
        let mut config = Config::chrome_windows();
        config.ruby_overhang = overhang;
        config.emphasis_room = room;
        cx.set_config(config);
        let mut layout = Layout::new();
        let cold = count_allocations(|| document(&mut layout, &mut cx, 20));
        assert!(cold > 0, "a cold layout grows");
        let warm = count_allocations(|| document(&mut layout, &mut cx, 20));
        assert_eq!(warm, 0, "{overhang:?}: rebuilding allocated");
        let warm = count_allocations(|| document(&mut layout, &mut cx, 7));
        assert_eq!(
            warm, 0,
            "{overhang:?}: rebuilding something smaller allocated"
        );
        document(&mut layout, &mut cx, 20);
        for &width in &widths {
            layout.break_lines(&mut cx, area(width), &mut NoExclusions);
            read(&layout);
        }
        let warm = count_allocations(|| {
            for &width in &widths {
                layout.break_lines(&mut cx, area(width), &mut NoExclusions);
            }
        });
        assert_eq!(warm, 0, "{overhang:?}: breaking again allocated");
        for &width in &widths {
            layout.break_lines(&mut cx, area(width), &mut NoExclusions);
            let mut sum = 0.0;
            let warm = count_allocations(|| sum = read(&layout));
            assert!(sum.is_finite());
            assert_eq!(warm, 0, "{overhang:?} at {width}: reading back allocated");
        }
    }
}
/// Independent annotation cursors, retained edge shapes and selection
/// walks all reuse their capacity after every requested width is warmed.
#[test]
fn split_ruby_continuations_allocate_nothing_warm() {
    let mut cx = Context::new(collection());
    let mut layout = Layout::new();
    let base = ComputedStyle {
        font: FontGroup {
            size: 20.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let small = ComputedStyle {
        font: FontGroup {
            size: 10.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut b = layout.builder(
        &mut cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&base),
        BuildOptions::default(),
    );
    for n in 0..12 {
        b.open_ruby(NodeKey(n * 4 + 1), &base, None);
        b.text(NodeKey(n * 4 + 2), "AAAA BBBB CCCC DDDD");
        b.open_annotation(NodeKey(n * 4 + 3), &small, None);
        b.text(NodeKey(n * 4 + 4), "aaaa bbbb cccc dddd");
        b.close_ruby();
    }
    b.finish();
    let widths = [40.0, 120.0, 200.0, 400.0];
    for width in widths {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
    }
    assert_eq!(
        count_allocations(|| {
            for width in widths {
                layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
                read(&layout);
                for offset in 0..layout.text().len() {
                    let _ = layout.caret(offset.into());
                }
                let _ = layout.selection_rects(0..layout.text().len()).count();
            }
        }),
        0
    );
}
