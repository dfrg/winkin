//! Vertical text, upright, on its side and combined, allocates nothing warm.

use super::count_allocations;
use super::test_fonts::{self, TestFont, TestVertical, ahem_fallback};
use fontwich::Collection;
use winkin::style::LengthPercentage;
use winkin::style::{
    ComputedStyle, FontFamilyName, FontGroup, TextAlign, TextCombineUpright, TextOrientation,
    WritingMode,
};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
    RunOrientation,
};

/// Ahem, which has no vertical metrics, and a CJK font that has them.
fn collection() -> Collection {
    let mut vertical = TestFont::cjk("Test Vertical", false);
    vertical.vertical = Some(TestVertical {
        advance: 1000,
        top_side_bearing: 100,
        advances: vec![('\u{6C34}', 900)],
        origins: Some((880, vec![('\u{6F22}', 860)])),
    });
    test_fonts::collection(&[vertical], ahem_fallback())
}

/// Builds paragraphs of mixed vertical text `repeat` times in `mode`:
/// upright ideographs in fonts with and without vertical metrics, Latin on
/// its side, spans set upright and sideways, combined text and digits, all
/// spaced, justified and marked.
fn document(layout: &mut Layout, cx: &mut Context, mode: WritingMode, repeat: usize) {
    let families = [
        FontFamilyName::named("Test Vertical"),
        FontFamilyName::named("Ahem"),
    ];
    let ahem = [FontFamilyName::named("Ahem")];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            size: 20.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut upright = root;
    upright.orientation.text_orientation = TextOrientation::Upright;
    let mut sideways = root;
    sideways.orientation.text_orientation = TextOrientation::Sideways;
    let mut all = root;
    all.orientation.text_combine_upright = TextCombineUpright::All;
    let mut digits = root;
    digits.orientation.text_combine_upright = TextCombineUpright::Digits(3);
    let mut marked = root;
    marked.text.emphasis.marks = true;
    let mut spaced = ahem_style(&ahem);
    spaced.text.letter_spacing = LengthPercentage::from_px(2.0);
    let block = ComputedBlockStyle {
        writing_mode: mode,
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
        b.text(next(), "\u{6F22}\u{5B57}\u{3001}\u{6C34} Latin words ");
        b.open_box(next(), &upright, None);
        b.text(next(), "ABC");
        b.close_box();
        b.open_box(next(), &sideways, None);
        b.text(next(), "\u{6F22}\u{5B57}");
        b.close_box();
        b.open_box(next(), &all, None);
        b.text(next(), if n % 2 == 0 { "12" } else { "2026" });
        b.close_box();
        b.open_box(next(), &digits, None);
        b.text(next(), "\u{6C34}7\u{6C34}123\u{6C34}4567");
        b.close_box();
        b.open_box(next(), &marked, None);
        b.text(next(), "\u{6F22}\u{5B57}\u{6C34}");
        b.close_box();
        b.open_box(next(), &spaced, None);
        b.text(next(), "\u{6C34}ab\u{6C34} ");
        b.close_box();
    }
    assert!(b.finish().is_complete());
}

/// Ahem as a style of its own.
fn ahem_style<'a>(families: &'a [FontFamilyName<'a>]) -> ComputedStyle<'a> {
    ComputedStyle {
        font: FontGroup {
            families,
            size: 20.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    }
}

/// Reads every line back: its runs, how they stand, their glyphs and
/// their marks.
fn read(layout: &Layout) -> f32 {
    let mut sum = 0.0;
    for line in layout.lines() {
        for item in line.all_items() {
            if let Item::Text(run) = item {
                sum += run.combine_scale();
                if run.orientation() == RunOrientation::Upright {
                    sum += 1.0;
                }
                for glyph in run.glyphs() {
                    sum += glyph.x + glyph.y;
                }
                for mark in run.emphasis_marks() {
                    sum += mark.x + mark.baseline;
                }
            }
        }
    }
    sum
}

/// A warm rebuild of vertical text allocates nothing, in either vertical
/// mode and sideways.
///
/// Nor does breaking it again at any width used before, or reading it
/// back. The orientation runs, vertical metrics, combined units' fits and
/// orientation pieces keep no allocation of their own.
#[test]
fn vertical_text_allocates_nothing_warm() {
    let widths = [60.0, 133.5, 400.0, 2000.0, 5.0];
    for mode in [
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::SidewaysRl,
    ] {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        let cold = count_allocations(|| document(&mut layout, &mut cx, mode, 12));
        assert!(cold > 0, "a cold layout grows");
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            read(&layout);
        }
        let warm = count_allocations(|| document(&mut layout, &mut cx, mode, 12));
        assert_eq!(warm, 0, "{mode:?}: rebuilding allocated");
        let warm = count_allocations(|| document(&mut layout, &mut cx, mode, 7));
        assert_eq!(warm, 0, "{mode:?}: rebuilding something smaller allocated");
        document(&mut layout, &mut cx, mode, 12);
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            read(&layout);
        }
        let warm = count_allocations(|| {
            for &width in &widths {
                layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            }
        });
        assert_eq!(warm, 0, "{mode:?}: breaking again allocated");
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            let mut sum = 0.0;
            let warm = count_allocations(|| sum = read(&layout));
            assert!(sum.is_finite());
            assert_eq!(warm, 0, "{mode:?} at {width}: reading back allocated");
        }
    }
}
