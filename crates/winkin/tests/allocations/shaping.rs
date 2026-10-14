//! Shaping: shaping again allocates nothing, through ligatures, kerns,
//! Arabic joining and CJK fallback.

use super::test_fonts::{self, TestFont, han, han_fallback};
use super::{arabic, count_allocations, text};
use fontwich::Collection;
use winkin::style::{BaseDirection, ComputedStyle, FontFamilyName, FontGroup, Language, TextGroup};
use winkin::{BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey};

/// Ahem; a Latin font with ligatures, a kerning pair, a character drawn
/// as two glyphs and one drawn raised; an Arabic font with joining
/// forms; and a CJK font that Han and kana fall back to.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    latin.splits = vec!['Q'];
    latin.raised = vec![('R', 200)];
    latin.kerning = vec![('A', 'V', -100), ('T', 'o', -80)];
    test_fonts::collection(&[latin, arabic(), han()], han_fallback("Test Han"))
}

/// `text` in paragraphs, in `family` and `language`, some of it in a
/// span at another size, in a block set in `direction`.
fn prose(
    layout: &mut Layout,
    cx: &mut Context,
    (family, language, direction): (&str, &str, BaseDirection),
    text: &str,
) {
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
    let larger = ComputedStyle {
        font: FontGroup {
            size: 20.0,
            ..root.font
        },
        ..root
    };
    let block = ComputedBlockStyle {
        direction,
        ..ComputedBlockStyle::new(&root)
    };
    let mut b = layout.builder(cx, NodeKey(0), &block, BuildOptions::default());
    for (at, paragraph) in (0..).zip(text.split('\n')) {
        b.text(NodeKey(4 * at), paragraph);
        b.open_box(NodeKey(4 * at + 1), &larger, None);
        b.text(NodeKey(4 * at + 2), paragraph);
        b.close_box();
        b.line_break(NodeKey(4 * at + 3));
    }
    assert!(b.finish().is_complete());
}

/// Shaping again allocates nothing.
///
/// harfrust's per-font and per-instance data, the plans and the buffer are
/// the context's, and the shaped text and advances keep their capacity.
/// Covers Latin with ligatures, kerns and expanded clusters, Arabic in
/// paragraphs of both directions, Han falling back, and the three mixed.
#[test]
fn shaping_allocates_nothing_warm() {
    let arabic =
        "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{646}\u{64A} \u{628}\u{644}\u{62A}\u{645}\u{633}.";
    let documents = [
        (
            ("Test Latin", "en", BaseDirection::Ltr),
            "To office AVAIL fit, flat Quiet R: 1,000 times ffi.",
        ),
        (("Test Arabic", "ar", BaseDirection::Ltr), arabic),
        (("Test Arabic", "ar", BaseDirection::Rtl), arabic),
        (
            ("Test Latin", "ja", BaseDirection::Ltr),
            "日本語の文章は、漢字と仮名で書かれる。",
        ),
        (
            ("Test Latin", "en", BaseDirection::Ltr),
            "Mixed: 漢字 and office, 12 digits, \u{628}\u{62A}\u{633}\u{645} again.",
        ),
    ];
    for (how, line) in documents {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        let same = text(line, 12);
        let half: String = line.chars().take(line.chars().count() / 2).collect();
        let similar = text(&half, 10);
        let cold = count_allocations(|| prose(&mut layout, &mut cx, how, &same));
        assert!(cold > 0, "a cold context grows");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, how, &same));
        assert_eq!(warm, 0, "{how:?}: rebuilding the same content allocated");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, how, &similar));
        assert_eq!(warm, 0, "{how:?}: rebuilding similar content allocated");
        let mut other = Layout::new();
        prose(&mut other, &mut cx, how, &same);
        let warm = count_allocations(|| prose(&mut other, &mut cx, how, &same));
        assert_eq!(warm, 0, "{how:?}: a second layout allocated");
    }
}
