//! Line breaking:
//! - breaking again allocates nothing, at any width broken at before, around
//!   floats too;
//! - a relayout in a new context is warm again after its first.

use super::test_fonts::{self, TestFont, han, han_fallback};
use super::{Floats, arabic, count_allocations, relayout, text};
use fontwich::Collection;
use winkin::style::{
    ComputedStyle, EdgesGroup, FontFamilyName, FontGroup, Language, Sides, TextGroup, WordBreak,
};
use winkin::{Area, BuildOptions, ComputedBlockStyle, Context, Glyph, Item, Layout, NodeKey};

/// Ahem; a Latin font with ligatures and kerning pairs, one of them a
/// letter against the space after it, which a line ending there is
/// reshaped without; an Arabic font with joining forms; and a CJK font
/// that Han and kana fall back to.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    latin.kerning = vec![('A', 'V', -100), ('T', 'o', -80), ('Y', ' ', -120)];
    test_fonts::collection(&[latin, arabic(), han()], han_fallback("Test Han"))
}

/// `text` in paragraphs, in `family` and `language`, each paragraph
/// again in a span that breaks between any two letters, so lines end
/// and start inside kerned pairs, ligatures and joined words, whose
/// edges are reshaped, and again in a box with edges.
fn prose(layout: &mut Layout, cx: &mut Context, (family, language): (&str, &str), text: &str) {
    let families = [FontFamilyName::named(family)];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        text: TextGroup {
            language: Language::parse(language).ok(),
            ..TextGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut anywhere = root;
    anywhere.text.word_break = WordBreak::BreakAll;
    let boxed = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    for (at, paragraph) in (0..).zip(text.split('\n')) {
        b.text(NodeKey(8 * at), paragraph);
        b.open_box(NodeKey(8 * at + 1), &anywhere, None);
        b.text(NodeKey(8 * at + 2), paragraph);
        b.close_box();
        b.open_box(NodeKey(8 * at + 3), &boxed, None);
        b.text(NodeKey(8 * at + 4), paragraph);
        b.close_box();
        b.line_break(NodeKey(8 * at + 5));
    }
    assert!(b.finish(cx).is_complete());
}

/// Latin with kerning and ligatures, Arabic in a left-to-right
/// paragraph, Han falling back, and the three mixed: each document's
/// font and language, and a line of it.
const DOCUMENTS: [((&str, &str), &str); 4] = [
    (
        ("Test Latin", "en"),
        "To office AVAVAIL fit, flat waffle: 1,000 times ffi officially, YAY YAY.",
    ),
    (
        ("Test Arabic", "ar"),
        "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{646}\u{64A} \u{628}\u{644}\u{62A}\u{645}\u{633}.",
    ),
    (("Ahem", "ja"), "日本語の文章は、漢字と仮名で書かれる。"),
    (
        ("Test Latin", "en"),
        "Mixed: 漢字 and office, AVAV digits, \u{628}\u{62A}\u{633}\u{645} again.",
    ),
];

/// The widths each document is broken at.
const WIDTHS: [f32; 8] = [37.0, 55.5, 81.25, 12.0, 120.0, 250.0, 600.0, 3.0];

/// Breaking again allocates nothing, at any width broken at before:
/// the lines, the reshaped edges' words, glyphs and advances keep their
/// capacity, and the reshapes find the context's plans and buffer warm.
/// So for Latin with kerning and ligatures, Arabic in a left-to-right
/// paragraph, Han falling back, and the three mixed, with lines broken
/// inside words where the font shaped across the break.
#[test]
fn breaking_allocates_nothing_warm() {
    let widths = WIDTHS;
    for (how, line) in DOCUMENTS {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        prose(&mut layout, &mut cx, how, &text(line, 12));
        let cold = count_allocations(|| {
            relayout(&mut layout, &mut cx, &widths);
        });
        assert!(cold > 0, "a first break grows the lines");
        let warm = count_allocations(|| {
            assert!(relayout(&mut layout, &mut cx, &widths) > 0);
        });
        assert_eq!(warm, 0, "{how:?}: breaking again allocated");
        let warm = count_allocations(|| {
            relayout(&mut layout, &mut cx, &[81.25, 3.0, 600.0, 37.0]);
        });
        assert_eq!(warm, 0, "{how:?}: breaking in another order allocated");
    }
}

/// The context is a cache: a host frees every cache by
/// replacing it with a new one over the same fonts, which breaks a
/// layout built in the old into the same lines and glyphs, making what
/// the reshaped edges need again from the layout's own record of its
/// fonts, which allocates; and warm again after, breaking again
/// allocates nothing. The new context lays out the documents in other
/// fonts first, so that its tables hold their entries first and name the
/// layout's fonts by other ids.
#[test]
fn a_relayout_in_a_new_context_is_warm_again_after() {
    // Each line's text, and every glyph of it as a painter reads it.
    let lines = |layout: &Layout| {
        let drawn = layout.lines().map(|line| {
            let mut glyphs = Vec::<Glyph>::new();
            for item in line.items() {
                if let Item::Text(run) = item {
                    glyphs.extend(run.glyphs());
                }
            }
            (line.text_range(), glyphs)
        });
        drawn.collect::<Vec<_>>()
    };
    let mut made = 0;
    for (how, line) in DOCUMENTS {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        prose(&mut layout, &mut cx, how, &text(line, 12));
        relayout(&mut layout, &mut cx, &WIDTHS);
        let before = lines(&layout);
        assert!(before.iter().any(|(_, glyphs)| !glyphs.is_empty()));
        cx = Context::new(cx.collection().clone());
        for (other, line) in DOCUMENTS {
            if other.0 != how.0 {
                let mut layout = Layout::new();
                prose(&mut layout, &mut cx, other, &text(line, 2));
                relayout(&mut layout, &mut cx, &WIDTHS);
            }
        }
        made += count_allocations(|| {
            relayout(&mut layout, &mut cx, &WIDTHS);
        });
        assert_eq!(lines(&layout), before, "{how:?}");
        let warm = count_allocations(|| {
            assert!(relayout(&mut layout, &mut cx, &WIDTHS) > 0);
        });
        assert_eq!(warm, 0, "{how:?}: breaking again allocated");
    }
    assert!(made > 0, "the reshapes made harfrust's data again");
}

/// Breaking around floats again allocates nothing, at any width broken
/// at before: each paragraph opens beside a float, and
/// reaches two more inside it, one on each side, which go beside a line
/// or below it as the width has it; lines the floats leave no room move
/// below them, and trials taken back place their floats again; the
/// block opens with an initial letter, whose room is the host's to keep.
/// The host keeps its own floats in room it has.
#[test]
fn breaking_around_floats_allocates_nothing_warm() {
    let floated = |layout: &mut Layout, cx: &mut Context, (family, language), text: &str| {
        let families = [FontFamilyName::named(family)];
        let root = ComputedStyle {
            font: FontGroup {
                families: &families,
                ..FontGroup::INITIAL
            },
            text: TextGroup {
                language: Language::parse(language).ok(),
                ..TextGroup::INITIAL
            },
            ..ComputedStyle::initial()
        };
        let mut letter = root;
        letter.line.initial_letter = winkin::style::InitialLetter {
            size: 2.0,
            sink: 2,
            ..winkin::style::InitialLetter::NONE
        };
        let size = |inline, block| winkin::BoxSize {
            inline,
            block,
            baseline: None,
        };
        let mut b = layout.builder(
            NodeKey(0),
            &ComputedBlockStyle::new(&root),
            BuildOptions::default(),
        );
        b.open_box(NodeKey(1), &letter, None);
        b.text(NodeKey(2), "O");
        b.close_box();
        for (at, paragraph) in (1..).zip(text.split('\n')) {
            b.float(
                NodeKey(8 * at),
                &root,
                winkin::FloatSide::Left,
                size(40.0, 30.0),
            );
            let half = paragraph.chars().count() / 2;
            let middle = paragraph
                .char_indices()
                .nth(half)
                .map_or(paragraph.len(), |(at, _)| at);
            let (first, rest) = paragraph.split_at(middle);
            b.text(NodeKey(8 * at + 1), first);
            b.float(
                NodeKey(8 * at + 2),
                &root,
                winkin::FloatSide::Right,
                size(25.0, 50.0),
            );
            b.float(
                NodeKey(8 * at + 3),
                &root,
                winkin::FloatSide::Left,
                size(70.0, 12.0),
            );
            b.text(NodeKey(8 * at + 4), rest);
            b.line_break(NodeKey(8 * at + 5));
        }
        assert!(b.finish(cx).is_complete());
    };
    let mut host = Floats {
        width: 0.0,
        placed: Vec::with_capacity(256),
        lowers: true,
    };
    let mut relayout = |layout: &mut Layout, cx: &mut Context, widths: &[f32]| {
        let mut read = 0;
        for &width in widths {
            host.width = width;
            host.placed.clear();
            layout.break_lines(cx, Area::new(width), &mut host);
            for line in layout.lines() {
                read += line.text_range().len() + line.floats().count();
            }
            read += layout.floats().count();
        }
        read
    };
    for (how, line) in DOCUMENTS {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        floated(&mut layout, &mut cx, how, &text(line, 4));
        let cold = count_allocations(|| {
            relayout(&mut layout, &mut cx, &WIDTHS);
        });
        assert!(cold > 0, "a first break grows the lines");
        let warm = count_allocations(|| {
            assert!(relayout(&mut layout, &mut cx, &WIDTHS) > 0);
        });
        assert_eq!(warm, 0, "{how:?}: breaking around floats again allocated");
        assert_eq!(layout.floats().count(), 12, "every float is placed");
        let warm = count_allocations(|| {
            relayout(&mut layout, &mut cx, &[81.25, 3.0, 600.0, 37.0]);
        });
        assert_eq!(warm, 0, "{how:?}: breaking in another order allocated");
    }
}
