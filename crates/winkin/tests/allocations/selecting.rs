//! Font selection: selecting fonts again allocates nothing, with fallback
//! and mixed scripts, and in contexts sharing their fonts.

use super::test_fonts::{self, TestFont, han_fallback};
use super::{count_allocations, text};
use fontwich::Collection;
use winkin::style::{ComputedStyle, FontFamilyName, FontGroup, FontWeight, Language, TextGroup};
use winkin::{BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey, SharedFonts};

/// Ahem, and a CJK font that Han and kana fall back to from it, which
/// has digits and spaces too, as Ahem is asked for first.
fn collection() -> Collection {
    let han = TestFont::new(
        "Test Han",
        &[
            (0x20, 0x20),
            (0x30, 0x39),
            (0x3000, 0x30FF),
            (0x4E00, 0x9FFF),
            (0xFF00, 0xFFEF),
        ],
    );
    test_fonts::collection(&[han], han_fallback("Test Han"))
}

/// `text` in paragraphs in `language`, asking for Ahem, some of it in a
/// bold span at another size: two styles, two used fonts, a fallback
/// font for the Han.
fn prose(layout: &mut Layout, cx: &mut Context, language: &str, text: &str) {
    let families = [FontFamilyName::named("Ahem")];
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
    let bold = ComputedStyle {
        font: FontGroup {
            weight: FontWeight::BOLD,
            size: 20.0,
            ..root.font
        },
        ..root
    };
    let mut b = layout.builder(
        cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    for (at, paragraph) in (0..).zip(text.split('\n')) {
        b.text(NodeKey(4 * at), paragraph);
        b.open_box(NodeKey(4 * at + 1), &bold, None);
        b.text(NodeKey(4 * at + 2), paragraph);
        b.close_box();
        b.line_break(NodeKey(4 * at + 3));
    }
    assert!(b.finish().is_complete());
}

/// Selecting fonts again allocates nothing: the family lists, the fonts
/// each tries and the instances are the context's, found by hash, and the
/// fonts and the scratch keep their capacity. So for Latin set in the
/// font asked for, for Han falling back from it, and for the two mixed,
/// where the fallback's runs end at every Latin letter, digit and space.
#[test]
fn selecting_fonts_allocates_nothing_warm() {
    let documents = [
        (
            "en",
            "The quick brown fox jumps over the lazy dog, 1,000 times.",
        ),
        ("ja", "日本語の文章は、漢字と仮名で書かれる。"),
        (
            "zh",
            "中文文本在标点符号之后换行，数字 123 与拉丁字母 abc 混排。",
        ),
        ("en", "Mixed: 漢字 and Latin, 12 digits, 中文 again."),
    ];
    for (language, line) in documents {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        let same = text(line, 12);
        let half: String = line.chars().take(line.chars().count() / 2).collect();
        let similar = text(&half, 10);
        let cold = count_allocations(|| prose(&mut layout, &mut cx, language, &same));
        assert!(cold > 0, "a cold context grows");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, language, &same));
        assert_eq!(warm, 0, "{language}: rebuilding the same content allocated");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, language, &similar));
        assert_eq!(warm, 0, "{language}: rebuilding similar content allocated");
        // A second layout in the same context finds everything warm too,
        // once it has grown its own tables: the caches are the context's.
        let mut other = Layout::new();
        prose(&mut other, &mut cx, language, &same);
        let warm = count_allocations(|| prose(&mut other, &mut cx, language, &same));
        assert_eq!(warm, 0, "{language}: a second layout allocated");
    }
}

/// A context sharing its fonts allocates nothing warm, and neither does a
/// second one once it has grown its own tables: a warm build finds every
/// face in the context and never asks the shared fonts.
#[test]
fn a_context_sharing_fonts_allocates_nothing_warm() {
    let fonts = collection();
    let shared = SharedFonts::default();
    let line = text("Mixed: 漢字 and Latin, 12 digits, 中文 again.", 12);
    for _ in 0..2 {
        let mut cx = Context::with_shared_fonts(fonts.clone(), shared.clone());
        let mut layout = Layout::new();
        let cold = count_allocations(|| prose(&mut layout, &mut cx, "en", &line));
        assert!(cold > 0, "a cold context grows");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, "en", &line));
        assert_eq!(warm, 0, "rebuilding in a context sharing fonts allocated");
    }
}
