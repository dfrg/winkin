//! Rectangle and copy tests. They pin:
//! - a selection painting its text, its line ends, pieces, atomic inlines and
//!   hyphens;
//! - copied text as laid out, keeping the author's case and order.

use super::*;
use crate::style::TextTransform;

/// A selection paints each item's selected text as tall as the line box.
///
/// Where it goes on past a line's end, it paints a space's width more, as
/// Blink's `ExpandedSelectionRectForSoftLineBreakIfNeeded`.
#[test]
fn a_selection_paints_its_text_and_its_line_ends() {
    use SelectionRectKind::{LineEnd, Text};
    let tall = styled(|style| style.line.height = LineHeight::Px(40.0));
    let layout = laid_with(&ComputedBlockStyle::new(&tall), 400.0, |b| {
        b.text(NodeKey(1), "abc def")
    });
    let rect = layout.selection_rects(1..6).next().unwrap();
    assert_eq!(pairs(rect.inline, rect.block), ((20.0, 120.0), (0.0, 40.0)));
    let layout = laid("abcd efgh", 100.0);
    assert_eq!(
        rects(&layout, 1..7),
        [
            (0, (20.0, 80.0), Text),
            (0, (80.0, 100.0), LineEnd),
            (1, (0.0, 40.0), Text)
        ]
    );
    assert_eq!(rects(&layout, 1..4), [(0, (20.0, 80.0), Text)]);
    assert!(rects(&layout, 4..5).is_empty());
    assert_eq!(rects(&layout, 4..7), [(1, (0.0, 40.0), Text)]);
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "abcd");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "efgh");
    });
    assert_eq!(
        rects(&layout, 1..8),
        [
            (0, (20.0, 80.0), Text),
            (0, (80.0, 100.0), LineEnd),
            (1, (0.0, 60.0), Text)
        ]
    );
    assert_eq!(
        rects(&layout, 4..6),
        [(0, (80.0, 100.0), LineEnd), (1, (0.0, 20.0), Text)]
    );
}

/// Each run in its own direction is its own rectangle.
///
/// An atomic inline is selected whole or not at all. A hyphen goes with the
/// soft hyphen it ends.
#[test]
fn a_selection_paints_pieces_atomics_and_hyphens() {
    use SelectionRectKind::{Atomic, Text};
    let layout = laid("abc \u{5D0}\u{5D1}\u{5D2} def", 400.0);
    assert_eq!(
        rects(&layout, 2..6),
        [(0, (40.0, 80.0), Text), (0, (120.0, 140.0), Text)]
    );
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "ab");
        let size = BoxSize {
            inline: 40.0,
            block: 20.0,
            baseline: None,
        };
        b.atomic(NodeKey(2), &ahem(), None, size);
        b.text(NodeKey(3), "cd");
    });
    assert_eq!(
        rects(&layout, 1..6),
        [
            (0, (20.0, 40.0), Text),
            (0, (40.0, 80.0), Atomic),
            (0, (80.0, 100.0), Text)
        ]
    );
    // An atomic inline is one character: a selection ends before it or
    // after it.
    assert_eq!(rects(&layout, 5..6), [(0, (80.0, 100.0), Text)]);
    let layout = laid("abcd\u{AD}efgh", 100.0);
    assert_eq!(rects(&layout, 4..6), [(0, (80.0, 100.0), Text)]);
    assert_eq!(
        rects(&layout, 0..6),
        [(0, (0.0, 80.0), Text), (0, (80.0, 100.0), Text)]
    );
}

// Copy ------------------------------------------------------------------------

#[test]
fn a_selection_copies_math_auto_text() {
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    let layout = laid_with(&ComputedBlockStyle::new(&math), 400.0, |b| {
        b.text(NodeKey(1), "h");
        b.text(NodeKey(2), "i");
        b.text(NodeKey(3), "hi");
        b.text(NodeKey(4), "∞");
    });
    for kind in [CopyKind::Text, CopyKind::Clipboard] {
        assert_eq!(copied(&layout, kind), "ℎ𝑖hi∞");
        assert_eq!(layout.selected_text(0..3, kind).to_string(), "ℎ");
        assert_eq!(layout.selected_text(3..7, kind).to_string(), "𝑖");
    }
}

/// A selection copies the text as laid out.
///
/// White space is collapsed, each element's transform applied and `<br>` a
/// newline. What the builder inserted is left out and what the caller wrote
/// kept. A no-break space is copied as asked.
#[test]
fn a_selection_copies_the_text_as_laid_out() {
    let layout = laid("hello      world\n   again\ttab", 400.0);
    assert_eq!(copied(&layout, CopyKind::Text), "hello world again tab");
    let upper = styled(|style| style.text.transform.case = TextCase::Uppercase);
    let layout = laid_with(&ComputedBlockStyle::new(&upper), 400.0, |b| {
        b.text(NodeKey(1), "hello stra\u{DF}e")
    });
    assert_eq!(copied(&layout, CopyKind::Text), "HELLO STRASSE");
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "super");
        b.break_opportunity();
        b.text(NodeKey(1), "cali\u{200B}fragile");
        b.line_break(NodeKey(2));
        b.atomic(NodeKey(3), &ahem(), None, BoxSize::default());
        b.text(NodeKey(4), "a\u{A0}b\u{AD}c");
    });
    assert_eq!(
        copied(&layout, CopyKind::Text),
        "supercali\u{200B}fragile\na\u{A0}b\u{AD}c"
    );
    assert_eq!(
        copied(&layout, CopyKind::Clipboard),
        "supercali\u{200B}fragile\na b\u{AD}c"
    );
    let pieces: Vec<&str> = layout.selected_text(0..8, CopyKind::Text).collect();
    assert_eq!(pieces, ["super"]);
    assert_eq!(
        layout.selected_text(5..11, CopyKind::Text).to_string(),
        "cal"
    );
}

/// Small capitals are copied as written, and ruby annotation text in its place.
///
/// The first line's transform is not copied.
#[test]
fn copy_keeps_the_authors_case_and_order() {
    let small = styled(|style| style.font.variant_caps = FontVariantCaps::SmallCaps);
    let layout = laid_with(&ComputedBlockStyle::new(&small), 400.0, |b| {
        b.text(NodeKey(1), "Small Caps")
    });
    assert_eq!(copied(&layout, CopyKind::Text), "Small Caps");
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.open_ruby(NodeKey(1), &ahem(), None);
        b.text(NodeKey(2), "ab");
        b.open_annotation(NodeKey(3), &ahem(), None);
        b.text(NodeKey(4), "xyz");
        b.close_ruby();
        b.text(NodeKey(5), "cd");
    });
    assert_eq!(copied(&layout, CopyKind::Text), "abxyzcd");
    let long = "the quick brown fox jumps over the lazy dog";
    let first_line = styled(|style| style.text.transform.case = TextCase::Uppercase);
    let mut cx = context();
    let mut layout = Layout::new();
    let style = ahem();
    let block = ComputedBlockStyle {
        first_line: Some(&first_line),
        ..ComputedBlockStyle::new(&style)
    };
    let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
    b.text(NodeKey(1), long);
    b.finish(&mut cx);
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    assert_eq!(copied(&layout, CopyKind::Text), long);
}
