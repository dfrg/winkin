//! Caret tests. They pin:
//! - a caret at each boundary, and two at a wrap;
//! - positions holding across relayouts;
//! - carets at soft hyphens, direction changes, atomic inlines and empty
//!   lines;
//! - a divided grapheme as one stop but at an edge.

use super::*;

/// A caret stands at each boundary, a character's width apart, as tall as its text.
#[test]
fn a_caret_stands_at_each_boundary() {
    let layout = laid("abcd efgh", 400.0);
    for at in 0..=9 {
        assert_eq!(caret(&layout, at), (0, 20.0 * at as f32), "{at}");
    }
    let caret = layout.caret(Position::from(2)).unwrap();
    assert_eq!(caret.block, across(0.0, 20.0));
    assert!(!caret.rtl);
    // Past the text, and inside a character, a caret is still somewhere.
    assert_eq!(position_caret(&layout, Position::from(99)), (0, 180.0));
    let layout = laid("a\u{E9}b", 400.0);
    assert_eq!(position_caret(&layout, Position::from(2)), (0, 20.0));
    assert_eq!(
        position_caret(&layout, Position::new(2, Affinity::Upstream)),
        (0, 40.0)
    );
}

/// At a wrap after a space, the line's end is before the space and the next line's start after it.
///
/// That holds whatever the affinity. At a wrap inside a word, the affinity
/// chooses, as Blink's `InlineCaretPositionAtSoftLineWrap` does.
#[test]
fn the_two_sides_of_a_wrap_are_two_places() {
    let layout = laid("abcd efgh ijkl", 100.0);
    assert_eq!(layout.lines().len(), 3);
    for affinity in [Affinity::Downstream, Affinity::Upstream] {
        assert_eq!(
            position_caret(&layout, Position::new(4, affinity)),
            (0, 80.0)
        );
        assert_eq!(
            position_caret(&layout, Position::new(5, affinity)),
            (1, 0.0)
        );
    }
    let broken = styled(|style| style.text.word_break = WordBreak::BreakAll);
    let layout = laid_with(&ComputedBlockStyle::new(&broken), 80.0, |b| {
        b.text(NodeKey(1), "abcdefgh")
    });
    assert_eq!(
        position_caret(&layout, Position::new(4, Affinity::Upstream)),
        (0, 80.0)
    );
    assert_eq!(caret(&layout, 4), (1, 0.0));
}

/// A position holds across relayouts at other widths.
#[test]
fn a_position_holds_across_relayouts() {
    let mut cx = context();
    let mut layout = Layout::new();
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&ahem()),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "abcd efgh ijkl");
    b.finish(&mut cx);
    let position = Position::from(7);
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(position_caret(&layout, position), (0, 140.0));
    layout.break_lines(&mut cx, Area::new(100.0), &mut NoExclusions);
    assert_eq!(position_caret(&layout, position), (1, 40.0));
}

/// After a soft hyphen that ends a line, the caret is before the hyphen upstream and at the next line's start downstream.
///
/// Chrome's generated hyphen keeps that position on the upper line both
/// ways, so the next line's start is no caret position. That is not matched.
#[test]
fn a_soft_hyphen_ends_its_line_before_the_hyphen() {
    let layout = laid("abcd\u{AD}efgh", 100.0);
    assert_eq!(layout.lines().len(), 2);
    assert!(layout.line(0).unwrap().is_hyphenated());
    assert_eq!(caret(&layout, 4), (0, 80.0));
    assert_eq!(
        position_caret(&layout, Position::new(6, Affinity::Upstream)),
        (0, 80.0)
    );
    assert_eq!(caret(&layout, 6), (1, 0.0));
    // A point on the hyphen, or past it, is after the soft hyphen, on the
    // upper line (in Chrome, 80..115 hits UTF-16 offset 5).
    for x in [80.0, 95.0, 110.0] {
        assert_eq!(hit(&layout, x, 0), Position::new(6, Affinity::Upstream));
    }
    assert_eq!(hit(&layout, 75.0, 0), Position::from(4));
    // Mid-line, it is a stop at the same x as before it.
    let layout = laid("abcd\u{AD}efgh", 400.0);
    assert_eq!(caret(&layout, 4), (0, 80.0));
    assert_eq!(caret(&layout, 6), (0, 80.0));
}

/// Where the text changes direction, the caret is on the side nearer the paragraph's level.
///
/// The split caret's other side is the weak caret.
#[test]
fn a_change_of_direction_draws_the_caret_on_the_paragraphs_side() {
    // "abc " 0..80, then alef, bet, gimel drawn right to left 140..80,
    // then " def" 140..220.
    let layout = laid("abc \u{5D0}\u{5D1}\u{5D2} def", 400.0);
    assert_eq!(caret(&layout, 4), (0, 80.0));
    assert_eq!(caret(&layout, 6), (0, 120.0));
    assert_eq!(caret(&layout, 8), (0, 100.0));
    assert_eq!(caret(&layout, 10), (0, 140.0));
    let carets = layout.carets(Position::from(4)).unwrap();
    assert_eq!(carets.strong.inline, along(80.0, 80.0));
    assert_eq!(
        carets.weak.map(|weak| weak.inline),
        Some(along(140.0, 140.0))
    );
    assert!(carets.weak.unwrap().rtl);
    let inside = layout.carets(Position::from(6)).unwrap();
    assert!(inside.strong.rtl && inside.weak.is_none());
    // A paragraph read right to left, with Latin in it, from its line box's
    // left: alef 160..180, bet 140..160, a space 120..140, abc 60..120, a
    // space 40..60, gimel and dalet 20..40 and 0..20.
    let style = ahem();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let layout = laid_with(&rtl, 400.0, |b| {
        b.text(NodeKey(1), "\u{5D0}\u{5D1} abc \u{5D2}\u{5D3}")
    });
    assert_eq!(caret(&layout, 0), (0, 180.0));
    assert_eq!(caret(&layout, 5), (0, 120.0));
    assert_eq!(caret(&layout, 6), (0, 80.0));
    assert_eq!(caret(&layout, 8), (0, 60.0));
}

/// An atomic inline has a caret before and after it, beside its box, as tall as the line.
///
/// Blink's `ComputeLocalCaretRectByBoxSide` sizes it so.
#[test]
fn an_atomic_inline_has_a_caret_on_each_side() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "ab");
        let size = BoxSize {
            inline: 40.0,
            block: 40.0,
            baseline: None,
        };
        b.atomic(NodeKey(2), &ahem(), None, size);
        b.text(NodeKey(3), "cd");
    });
    assert_eq!(layout.text(), "ab\u{FFFC}cd");
    assert_eq!(caret(&layout, 2), (0, 40.0));
    let after = layout.caret(Position::from(5)).unwrap();
    assert_eq!(after.inline, along(80.0, 80.0));
    assert_eq!(
        after.block,
        across(0.0, layout.line(0).unwrap().metrics().height())
    );
    assert_eq!(caret(&layout, 6), (0, 100.0));
    // Its halves decide: the middle is before it.
    assert_eq!(hit(&layout, 60.0, 0), Position::from(2));
    assert_eq!(hit(&layout, 61.0, 0), Position::new(5, Affinity::Upstream));
}

/// An empty line between two breaks has one caret, at its start.
#[test]
fn an_empty_line_has_a_caret() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "ab");
        b.line_break(NodeKey(2));
        b.line_break(NodeKey(3));
        b.text(NodeKey(4), "cd");
    });
    assert_eq!(layout.text(), "ab\n\ncd");
    assert_eq!(layout.lines().len(), 3);
    assert_eq!(caret(&layout, 2), (0, 40.0));
    assert_eq!(caret(&layout, 3), (1, 0.0));
    assert_eq!(caret(&layout, 4), (2, 0.0));
    assert_eq!(hit(&layout, 50.0, 1), Position::from(3));
}

/// A position inside a grapheme split by a style boundary is no stop.
///
/// Where a box's edges part the two halves, the edge is a stop at its own
/// place (the `splitcaret` and `edgecaret` probes).
#[test]
fn a_divided_grapheme_is_one_stop_but_at_an_edge() {
    let plain = ahem();
    let bigger = styled(|style| style.font.size = 40.0);
    let layout = laid_with(&ComputedBlockStyle::new(&plain), 400.0, |b| {
        b.text(NodeKey(1), "a");
        b.open_box(NodeKey(2), &bigger, None);
        b.text(NodeKey(3), "\u{327}");
        b.close_box();
        b.text(NodeKey(4), "z");
    });
    let forward = walk(
        &layout,
        0,
        MotionDirection::Forward.moving(Granularity::Character),
    );
    assert_eq!(forward, [0, 3, 4]);
    assert_eq!(
        layout.caret(Position::from(1)),
        layout.caret(Position::from(0))
    );
    let padded = styled(|style| {
        style.edges = EdgesGroup {
            padding: Sides::from_px(4.0),
            ..EdgesGroup::INITIAL
        }
    });
    let layout = laid_with(&ComputedBlockStyle::new(&plain), 400.0, |b| {
        b.text(NodeKey(1), "a");
        b.open_box(NodeKey(2), &padded, None);
        b.text(NodeKey(3), "\u{327}");
        b.close_box();
        b.text(NodeKey(4), "z");
    });
    let forward = walk(
        &layout,
        0,
        MotionDirection::Forward.moving(Granularity::Character),
    );
    assert_eq!(forward, [0, 1, 3, 4]);
}
