//! Whole-analysis tests. They pin:
//! - `::first-line`, which changes no analysis;
//! - a rebuild replacing the last build, and a dropped builder leaving none;
//! - text of every kind analysed without a panic.

use super::*;
use crate::style::LengthPercentage;

/// `::first-line` changes nothing here. It cannot set the four properties
/// analysis reads.
#[test]
fn the_first_line_style_changes_no_analysis() {
    let document = |first_line: Option<&ComputedStyle<'_>>| {
        let mut layout = Layout::new();
        let initial = ComputedStyle::initial();
        let block = ComputedBlockStyle {
            first_line,
            ..ComputedBlockStyle::new(&initial)
        };
        let mut cx = no_fonts();
        let mut b = layout.builder(&mut cx, key(0), &block, BuildOptions::default());
        b.text(key(1), "The first line, and the rest: 日本語 text.");
        b.open_box(key(2), &ComputedStyle::initial(), first_line);
        b.text(key(3), " More words here.");
        b.close_box();
        b.finish();
        check(&layout);
        dump(analysis(&layout))
    };
    let first_line = styled(|s| {
        s.font.size = 30.0;
        s.text.letter_spacing = LengthPercentage::from_px(2.0);
        s.text.word_break = WordBreak::BreakAll;
        s.text.line_break = LineBreak::Anywhere;
        s.text.language = Language::parse("ja").ok();
    });
    assert_eq!(document(Some(&first_line)), document(None));
}

/// A second build replaces the first, and a layout reused for another
/// document holds exactly what a fresh one would.
#[test]
fn a_second_build_replaces_the_first() {
    let first = |b: &mut LayoutBuilder<'_>| {
        b.text(key(1), "日本語のテキスト。\n");
        b.atomic(key(2), &ComputedStyle::initial(), None, BoxSize::default());
        b.text(key(3), "\u{5D0}\u{5D1} and more 🙂 text here");
    };
    let second = |b: &mut LayoutBuilder<'_>| {
        b.text(key(1), "short");
    };
    let mut cx = no_fonts();
    let mut layout = Layout::new();
    let root = white_space(WhiteSpaceCollapse::Preserve);
    for calls in [&first as &dyn Fn(&mut LayoutBuilder<'_>), &second, &first] {
        let mut b = layout.builder(
            &mut cx,
            key(0),
            &ComputedBlockStyle::new(&root),
            BuildOptions::default(),
        );
        calls(&mut b);
        b.finish();
        let fresh = build_with(&ComputedBlockStyle::new(&root), calls);
        assert_eq!(dump(analysis(&layout)), dump(analysis(&fresh)));
    }
}

/// A builder dropped without finishing leaves its content and no analysis,
/// rather than the analysis of what was built before.
#[test]
fn a_dropped_builder_leaves_no_analysis() {
    let mut layout = Layout::new();
    let initial = ComputedStyle::initial();
    let block = ComputedBlockStyle::new(&initial);
    let mut cx = no_fonts();
    let mut b = layout.builder(&mut cx, key(0), &block, BuildOptions::default());
    b.text(key(1), "before");
    b.finish();
    assert_eq!(paragraphs(&layout), ["before"]);
    let mut cx = no_fonts();
    let mut b = layout.builder(&mut cx, key(0), &block, BuildOptions::default());
    b.text(key(1), "after");
    drop(b);
    assert_eq!(layout.content().text, "after");
    assert!(analysis(&layout).paragraphs.is_empty());
    assert!(analysis(&layout).clusters.is_empty());
}

/// Nothing the text holds makes analysis panic: controls, lone marks and
/// joiners, unpaired brackets, unassigned and private-use characters, every
/// separator, in every white space mode and every line key.
#[test]
fn no_text_makes_analysis_panic() {
    let texts = [
        "",
        "\u{301}\u{302}\u{200D}\u{200C}\u{FE0F}",
        "\u{0}\u{1}\u{7F}\u{9F}\u{1C}\u{1D}\u{1E}\u{1F}",
        "\n\n\r\r\n\u{B}\u{C}\u{85}\u{2028}\u{2029}",
        ")))]]]}}}\u{300D}\u{FF09}((([[[",
        "\u{E000}\u{F8FF}\u{FFFF}\u{10FFFF}\u{378}",
        "\u{202A}\u{202B}\u{202C}\u{202D}\u{202E}\u{2066}\u{2067}\u{2068}\u{2069}",
        "\u{E01}\u{E31}\u{E48}\u{E01}\u{E32}\u{E23}\u{1780}\u{17B6}",
        "🏴\u{E0067}\u{E007F}\u{1F1E6}\u{1F3FB}\u{20E3}\u{FE0E}",
        "\u{FFFC}\u{FFFC}\u{308}\u{AD}\u{AD}\u{200B}\u{200B}\u{2060}",
        "\u{1100}\u{1161}\u{11A8}\u{AC00}\u{3131}",
    ];
    let modes = [
        WhiteSpaceCollapse::Collapse,
        WhiteSpaceCollapse::Preserve,
        WhiteSpaceCollapse::BreakSpaces,
        WhiteSpaceCollapse::PreserveBreaks,
    ];
    for text in texts {
        for mode in modes {
            for line in [
                LineBreak::Loose,
                LineBreak::Normal,
                LineBreak::Strict,
                LineBreak::Anywhere,
            ] {
                for word in [WordBreak::Normal, WordBreak::BreakAll, WordBreak::KeepAll] {
                    let style = styled(|s| {
                        s.text.white_space_collapse = mode;
                        s.text.line_break = line;
                        s.text.word_break = word;
                        s.text.overflow_wrap = OverflowWrap::Anywhere;
                        s.text.language = Language::parse("ja").ok();
                    });
                    let layout = build_with(&ComputedBlockStyle::new(&style), |b| {
                        for (at, piece) in (1..).zip(text.split_inclusive(|c: char| c > '\u{2000}'))
                        {
                            b.text(key(at), piece);
                            b.atomic(key(1000 + at), &nowrap(), None, BoxSize::default());
                        }
                    });
                    assert!(!analysis(&layout).paragraphs.is_empty());
                }
            }
        }
    }
}
