//! CJK typography: `text-autospace` and every `text-spacing-trim` allocate
//! nothing warm.

use super::count_allocations;
use super::test_fonts::{self, TestFont, han_fallback};
use fontwich::Collection;
use winkin::config::{Config, PunctuationTrim, SmallKana};
use winkin::style::{
    ComputedStyle, FontFamilyName, FontGroup, LineBreak, TextAutospace, TextSpacingTrim, WordBreak,
};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

/// Ahem; a Japanese font whose punctuation `halt` sets in half its em;
/// and one without, which Han falls back to.
fn collection() -> Collection {
    let fonts = [
        TestFont::cjk("Test Punct", true),
        TestFont::cjk("Test Punct Plain", false),
    ];
    test_fonts::collection(&fonts, han_fallback("Test Punct Plain"))
}

/// Builds Japanese prose `repeat` times under each `text-spacing-trim` and
/// `line-break`: Latin words and digits, pairs of marks, marks wherever a
/// line may start or end, and a span in another size and font.
fn document(layout: &mut Layout, cx: &mut Context, repeat: usize) {
    let punct = [FontFamilyName::named("Test Punct")];
    let plain = [FontFamilyName::named("Test Punct Plain")];
    let root = ComputedStyle {
        font: FontGroup {
            families: &punct,
            size: 20.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let trims = [
        TextSpacingTrim::Normal,
        TextSpacingTrim::SpaceFirst,
        TextSpacingTrim::TrimStart,
        TextSpacingTrim::SpaceAll,
    ];
    let styled = |n: usize| {
        let mut style = root;
        style.text.autospace = if n % 5 == 4 {
            TextAutospace::NO_AUTOSPACE
        } else {
            TextAutospace::NORMAL
        };
        style.text.spacing_trim = trims[n % trims.len()];
        style.text.line_break = if n.is_multiple_of(2) {
            LineBreak::Normal
        } else {
            LineBreak::Strict
        };
        style.text.word_break = if n % 7 == 3 {
            WordBreak::KeepAll
        } else {
            WordBreak::Normal
        };
        style
    };
    let other = |n: usize| ComputedStyle {
        font: FontGroup {
            families: &plain,
            size: 14.0 + (n % 3) as f32 * 6.0,
            ..FontGroup::INITIAL
        },
        ..styled(n)
    };
    let mut b = layout.builder(
        cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    for n in 0..repeat {
        b.open_box(next(), &styled(n), None);
        b.text(next(), "漢字ABC漢字「「漢字」」、「漢字」漢字123漢字っと");
        b.open_box(next(), &other(n), None);
        b.text(next(), "」「字Word漢（字）");
        b.close_box();
        b.text(next(), "ニャー。漢字");
        b.close_box();
        if n % 3 == 2 {
            b.line_break(next());
        }
    }
    assert!(b.finish().is_complete());
}

/// Reads every line back: every item's place and every glyph's.
fn read(layout: &Layout) -> f32 {
    let mut sum = 0.0;
    for line in layout.lines() {
        sum += line.metrics().width;
        for item in line.items() {
            if let Item::Text(run) = item {
                sum += run.advance();
                for glyph in run.glyphs() {
                    sum += glyph.x;
                }
            }
        }
    }
    sum
}

/// A warm rebuild of Japanese with `text-autospace` and every
/// `text-spacing-trim` allocates nothing.
///
/// Covers fonts with and without `halt`, both `PunctuationTrim` rules and
/// both `SmallKana` rules. Nor does breaking again at any width used
/// before, or reading back.
#[test]
fn cjk_spacing_and_trimming_allocate_nothing_warm() {
    let widths = [60.0, 95.5, 180.0, 400.0, 1000.0, 7.0];
    for (trim, kana) in [
        (PunctuationTrim::FontFeature, SmallKana::MayStartLine),
        (PunctuationTrim::Always, SmallKana::Held),
    ] {
        let mut cx = Context::new(collection());
        let mut config = Config::chrome_windows();
        config.punctuation_trim = trim;
        config.small_kana = kana;
        cx.set_config(config);
        let mut layout = Layout::new();
        let cold = count_allocations(|| document(&mut layout, &mut cx, 24));
        assert!(cold > 0, "a cold layout grows");
        let warm = count_allocations(|| document(&mut layout, &mut cx, 24));
        assert_eq!(warm, 0, "{trim:?}: rebuilding allocated");
        let warm = count_allocations(|| document(&mut layout, &mut cx, 11));
        assert_eq!(warm, 0, "{trim:?}: rebuilding something smaller allocated");
        document(&mut layout, &mut cx, 24);
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            read(&layout);
        }
        let warm = count_allocations(|| {
            for &width in &widths {
                layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            }
        });
        assert_eq!(warm, 0, "{trim:?}: breaking again allocated");
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            let mut sum = 0.0;
            let warm = count_allocations(|| sum = read(&layout));
            assert!(sum.is_finite());
            assert_eq!(warm, 0, "{trim:?} at {width}: reading back allocated");
        }
    }
}
