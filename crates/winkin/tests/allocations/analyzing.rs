//! Analysis:
//! - analyzing Latin, CJK or Arabic again allocates nothing;
//! - analyzing Thai again allocates nothing, with or without dictionaries.

use super::{count_allocations, text};
use winkin::style::{ComputedStyle, Language, TextGroup, WordBreak};
use winkin::{BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey};

/// Builds `text` in paragraphs in `language`, some of it in a span with
/// another word-break rule, so the line segmenter resumes at a seam.
fn prose(layout: &mut Layout, cx: &mut Context, language: &str, text: &str) {
    let root = ComputedStyle {
        text: TextGroup {
            language: Language::parse(language).ok(),
            ..TextGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut other = root;
    other.text.word_break = WordBreak::BreakAll;
    let mut b = layout.builder(
        cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    for (at, paragraph) in (0..).zip(text.split('\n')) {
        b.text(NodeKey(4 * at), paragraph);
        b.open_box(NodeKey(4 * at + 1), &other, None);
        b.text(NodeKey(4 * at + 2), paragraph);
        b.close_box();
        b.line_break(NodeKey(4 * at + 3));
    }
    assert!(b.finish().is_complete());
}

/// Analyzing again allocates nothing.
///
/// The analysis tables and scratch, the bidi resolver's included, keep
/// their capacity; the segmenters are the context's; and ICU's line and
/// grapheme iterators allocate nothing on the rule path. The Arabic is in
/// a left-to-right paragraph, so its levels are resolved and kept.
#[test]
fn analyzing_latin_cjk_or_arabic_allocates_nothing_warm() {
    let documents = [
        (
            "en",
            "The quick brown fox jumps over the lazy dog, 1,000 times (or so) \u{2014} well-known words.",
        ),
        (
            "ja",
            "日本語のテキストは、句読点「かぎかっこ」と共に折り返す。漢字かな交じり文です。",
        ),
        (
            "zh",
            "中文文本在标点符号（括号）之后换行，数字 123 与拉丁字母 abc 混排。",
        ),
        (
            "ar",
            "\u{627}\u{644}\u{646}\u{635} \u{627}\u{644}\u{639}\u{631}\u{628}\u{64A} \u{64A}\u{64F}\u{642}\u{631}\u{623} \u{661}\u{662}\u{663}\u{60C} (\u{645}\u{639}) \u{643}\u{644}\u{645}\u{627}\u{62A}.",
        ),
    ];
    let mut cx = Context::new(fontwich::Collection::new());
    for (language, line) in documents {
        let mut layout = Layout::new();
        let same = text(line, 12);
        let half: String = line.chars().take(line.chars().count() / 2).collect();
        let similar = text(&half, 10);
        let cold = count_allocations(|| prose(&mut layout, &mut cx, language, &same));
        assert!(cold > 0, "a cold layout grows");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, language, &same));
        assert_eq!(warm, 0, "{language}: rebuilding the same content allocated");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, language, &similar));
        assert_eq!(warm, 0, "{language}: rebuilding similar content allocated");
    }
}

/// Thai rebuilds without allocating, and so does Latin built after it in the
/// same layout.
///
/// Thai, Lao, Khmer and Myanmar lines break at the words of a dictionary the
/// scan reads in place; without the `dictionaries` feature a run breaks
/// only around it.
#[test]
fn thai_allocates_nothing_warm() {
    let thai = text(
        "\u{E20}\u{E32}\u{E29}\u{E32}\u{E44}\u{E17}\u{E22}\u{E40}\u{E1B}\u{E47}\u{E19}\u{E20}\u{E32}\u{E29}\u{E32}\u{E17}\u{E35}\u{E48}\u{E2A}\u{E27}\u{E22}\u{E07}\u{E32}\u{E21}",
        8,
    );
    let latin = text("Latin text afterwards, in the same layout.", 8);
    let mut cx = Context::new(fontwich::Collection::new());
    let mut layout = Layout::new();
    prose(&mut layout, &mut cx, "th", &thai);
    prose(&mut layout, &mut cx, "en", &latin);
    let warm = count_allocations(|| prose(&mut layout, &mut cx, "th", &thai));
    assert_eq!(warm, 0, "rebuilding Thai allocated");
    let warm = count_allocations(|| prose(&mut layout, &mut cx, "en", &latin));
    assert_eq!(warm, 0, "after Thai, Latin allocated");
}
