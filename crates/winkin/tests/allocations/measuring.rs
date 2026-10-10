//! Measurement: measuring again allocates nothing, through box edges,
//! atomic inlines and every kind of line height.

use super::test_fonts::{self, TestFont, han, han_fallback};
use super::{arabic, count_allocations, text};
use fontwich::Collection;
use winkin::style::{
    ComputedStyle, EdgesGroup, FontFamilyName, FontGroup, Language, LineGroup, LineHeight, Sides,
    TextGroup,
};
use winkin::{BoxSize, BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey};

/// Ahem; a Latin font with ligatures, a kern and a character drawn as two
/// glyphs; an Arabic font with joining forms; and a CJK font with metrics
/// of its own, which Han and kana fall back to.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i']];
    latin.splits = vec!['Q'];
    latin.kerning = vec![('A', 'V', -100)];
    let mut han = han();
    han.win = (700, 400);
    han.hhea = (700, -400, 200);
    test_fonts::collection(&[latin, arabic(), han], han_fallback("Test Han"))
}

/// `text` in paragraphs, in `family` and `language`, each paragraph with
/// its text again in a box with edges at another line height, an atomic
/// inline with margins between, and a line at a fixed height, so that
/// every path of the measure stage is taken: edges at a boundary, the
/// used fonts of `normal`, a margin box, a leaded box. The intrinsic
/// sizes are read after, as a host reads them.
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
    let boxed = ComputedStyle {
        font: FontGroup {
            size: 20.0,
            ..root.font
        },
        line: LineGroup {
            height: LineHeight::Factor(1.5),
            ..LineGroup::INITIAL
        },
        edges: EdgesGroup {
            margin: Sides::from_px(2.0),
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let fixed = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(30.0),
            ..LineGroup::INITIAL
        },
        ..root
    };
    let size = BoxSize {
        inline: 24.0,
        block: 18.0,
        baseline: Some(14.0),
    };
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    for (at, paragraph) in (0..).zip(text.split('\n')) {
        b.text(NodeKey(8 * at), paragraph);
        b.open_box(NodeKey(8 * at + 1), &boxed, None);
        b.text(NodeKey(8 * at + 2), paragraph);
        b.atomic(NodeKey(8 * at + 3), &boxed, None, size);
        b.close_box();
        b.open_box(NodeKey(8 * at + 4), &fixed, None);
        b.text(NodeKey(8 * at + 5), paragraph);
        b.close_box();
        b.line_break(NodeKey(8 * at + 6));
    }
    assert!(b.finish(cx).is_complete());
    assert!(layout.intrinsic_sizes().max_content > 0.0);
}

/// Measuring again allocates nothing: the prefix is built in the buffer the
/// last build's was, which shaping filled with advances, and every other
/// table of the measurements keeps its capacity. So for Latin with
/// ligatures and kerns, Arabic in a left-to-right paragraph, Han falling
/// back to a font of other metrics, and the three mixed.
#[test]
fn measuring_allocates_nothing_warm() {
    let arabic =
        "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{646}\u{64A} \u{628}\u{644}\u{62A}\u{645}\u{633}.";
    let documents = [
        (
            ("Test Latin", "en"),
            "To office AVAIL fit, flat Quiet: 1,000 times ffi.",
        ),
        (("Test Arabic", "ar"), arabic),
        (("Ahem", "ja"), "日本語の文章は、漢字と仮名で書かれる。"),
        (
            ("Test Latin", "en"),
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
