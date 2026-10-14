//! Overflow tests. They pin:
//! - a word with nowhere to break overflowing, unless `overflow-wrap` or
//!   `word-break: break-all` lets it break;
//! - an emergency break before a box that allows it;
//! - spaces ending an overflowing line staying on it;
//! - a paragraph whose prefix goes back breaking, and measuring its
//!   min-content, as Chrome does;
//! - a negative margin after an overflow letting the line fit on.

use super::*;
use crate::style::FirstLineVariant;

/// A word with nowhere to break overflows its line, which holds it whole.
///
/// `overflow-wrap: anywhere` and `break-word` break it only where they must,
/// a line at a time. Under `nowrap` it overflows whatever its
/// `overflow-wrap`. The results match Chrome.
#[test]
fn a_word_with_nowhere_to_break_overflows_unless_it_may_break() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let word = "XXXXXXXXXX";
    for wrap in [
        OverflowWrap::Normal,
        OverflowWrap::BreakWord,
        OverflowWrap::Anywhere,
    ] {
        let mut style = ahem(20.0);
        style.text.overflow_wrap = wrap;
        fixture.text(&mut layout, &style, word);
        fixture.lay_out(&mut layout, 60.0);
        if wrap == OverflowWrap::Normal {
            assert_eq!(texts(&layout), [word]);
            assert!(records(&layout)[0].flags.contains(LineFlags::OVERFLOWS));
            assert_eq!(end_kinds(&layout)[0], EndKind::TextEnd);
        } else {
            assert_eq!(texts(&layout), ["XXX", "XXX", "XXX", "X"], "{wrap:?}");
            assert_eq!(end_kinds(&layout)[0], EndKind::Emergency);
            // In a band narrower than a glyph, each line holds one and
            // overflows.
            fixture.lay_out(&mut layout, 5.0);
            assert_eq!(layout.lines().len(), 10, "{wrap:?}");
            assert!(records(&layout)[0].flags.contains(LineFlags::OVERFLOWS));
        }
        style.text.wrap_mode = TextWrapMode::NoWrap;
        fixture.text(&mut layout, &style, word);
        fixture.lay_out(&mut layout, 60.0);
        assert_eq!(texts(&layout), [word], "{wrap:?} under nowrap");
        assert_eq!(widths(&layout), [200.0]);
    }
}

/// `overflow-wrap` is the last resort.
///
/// A word that fits on a line of its own moves there first. Only a word
/// that fits nowhere breaks. A word whose style forbids the break overflows
/// at its first opportunity, even where a later word may break.
#[test]
fn overflow_wrap_breaks_only_what_fits_nowhere() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut breaking = ahem(20.0);
    breaking.text.overflow_wrap = OverflowWrap::BreakWord;
    fixture.text(&mut layout, &breaking, "YY XXXXXXX");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["YY ", "XXXXX", "XX"]);
    // `XXXX` may not break and `YYYYYY` may, so the first word overflows
    // whole.
    let plain = ahem(20.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&plain), |b| {
        b.text(NodeKey(1), "XXXX ");
        b.open_box(NodeKey(2), &breaking, None);
        b.text(NodeKey(3), "YYYYYY");
        b.close_box();
    });
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout), ["XXXX ", "YYY", "YYY"]);
    assert_eq!(end_kinds(&layout)[0], EndKind::Overflow);
}

/// An emergency break before a box opens where the text after it may break.
///
/// In a 40 px band, `XX<span>XX</span>` with `overflow-wrap` on the span
/// alone breaks before the span. Chrome's retry breaks before the first
/// character of the text that allows it.
#[test]
fn an_emergency_break_before_a_box_reads_the_text_after_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let plain = ahem(20.0);
    for wrap in [OverflowWrap::Anywhere, OverflowWrap::BreakWord] {
        let mut breaking = ahem(20.0);
        breaking.text.overflow_wrap = wrap;
        fixture.build(&mut layout, &ComputedBlockStyle::new(&plain), |b| {
            b.text(NodeKey(1), "XX");
            b.open_box(NodeKey(2), &breaking, None);
            b.text(NodeKey(3), "XX");
            b.close_box();
        });
        fixture.lay_out(&mut layout, 40.0);
        assert_eq!(texts(&layout), ["XX", "XX"], "{wrap:?}");
        assert_eq!(end_kinds(&layout)[0], EndKind::Emergency, "{wrap:?}");
    }
}

/// `word-break: break-all` offers a break between every letter.
///
/// A long word fills its lines and overflows none.
#[test]
fn break_all_breaks_between_letters() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.text(&mut layout, &break_all(&ahem(20.0)), "XXXXXXXXXX");
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout), ["XXX", "XXX", "XXX", "X"]);
    assert!(
        end_kinds(&layout)
            .iter()
            .all(|&kind| kind != EndKind::Overflow)
    );
}

/// Under `break-spaces` the spaces are content and do not hang.
///
/// A break that fits wraps the space after it. Where the line overflows at
/// its first opportunity and only spaces follow, the spaces stay on that
/// line instead of making lines of their own.
#[test]
fn spaces_ending_an_overflowing_line_stay_on_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = ahem(20.0);
    style.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    for (text, characters, expected) in [
        ("XXXX  ", 4.0, &["XXXX  "][..]),
        ("XXXX   ", 4.0, &["XXXX   "]),
        ("XXXX  X", 4.0, &["XXXX ", " X"]),
        ("X  ", 2.0, &["X ", " "]),
    ] {
        fixture.text(&mut layout, &style, text);
        fixture.lay_out(&mut layout, 20.0 * characters);
        assert_eq!(texts(&layout), expected, "{text:?}");
    }
}

/// A paragraph whose prefix goes back, as with a negative margin, is walked
/// instead of searched, and stops at the first boundary that does not fit,
/// as Chrome does.
#[test]
fn a_paragraph_that_goes_back_ends_its_line_at_the_first_overflow() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let back = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: -40.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XXX");
        b.open_box(NodeKey(2), &back, None);
        b.text(NodeKey(3), "X");
        b.close_box();
        b.text(NodeKey(4), " YY");
    });
    let flags = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .paragraph(ParagraphId::new(0));
    assert!(flags.contains(MeasureFlags::NONMONOTONE));
    fixture.lay_out(&mut layout, 45.0);
    assert_eq!(texts(&layout), ["XXXX ", "YY"]);
    assert_eq!(widths(&layout), [40.0, 40.0]);
    assert_eq!(end_kinds(&layout)[0], EndKind::Soft);
}

/// Min-content measures a paragraph whose prefix goes back as Chrome's
/// breaker sets it in no room.
///
/// Chrome 155, 10px Ahem, `width: min-content` (WPT
/// `inline-negative-margin-001`, and cases like it):
/// - `123 <span -80>1234 </span>` and `123 <span -80>123 </span>`: 0;
/// - `XXXXX <span -50>X</span> X X X`: 10;
/// - `XXX<span -40>X</span> YY`: 20;
/// - `XX XX <span -30>X</span> XXXX` and with `-50`: 40.
#[test]
fn min_content_takes_a_negative_margin_back_as_chrome_does() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(10.0);
    let back = |px: f32| ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: -px,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    for (before, margin, inside, after, expected) in [
        ("123 ", 80.0, "1234 ", "", 0.0),
        ("123 ", 80.0, "123 ", "", 0.0),
        ("XXXXX ", 50.0, "X", " X X X", 10.0),
        ("XXX", 40.0, "X", " YY", 20.0),
        ("XX XX ", 30.0, "X", " XXXX", 40.0),
        ("XX XX ", 50.0, "X", " XXXX", 40.0),
    ] {
        let span = back(margin);
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), before);
            b.open_box(NodeKey(2), &span, None);
            b.text(NodeKey(3), inside);
            b.close_box();
            b.text(NodeKey(4), after);
        });
        assert_eq!(
            layout.intrinsic_sizes().min_content,
            expected,
            "{before}|{margin}|{inside}|{after}"
        );
    }
}

/// A negative margin after an overflow can bring the line back within its
/// room, and the line then fits on from there.
///
/// Chrome 155 sets these in 40 px of 10 px Ahem (WPT
/// `inline-negative-margin-001`, and cases like it):
/// - `XXXXX <span -60>XXX</span>` on one line;
/// - `XXXXX <span -40>XXX</span>` on two, the span starting the second;
/// - `XXXXX <span -50>X</span> X X X` as `XXXXX X X` and `X X`.
#[test]
fn a_negative_margin_after_an_overflow_lets_the_line_fit_on() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(10.0);
    let back = |px: f32| ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: -px,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    for (before, margin, inside, after, expected) in [
        ("XXXXX ", 60.0, "XXX", "", &["XXXXX XXX"][..]),
        ("XXXXX ", 40.0, "XXX", "", &["XXXXX ", "XXX"][..]),
        ("XXXXX ", 50.0, "X", " X X X", &["XXXXX X X ", "X X"][..]),
    ] {
        let span = back(margin);
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), before);
            b.open_box(NodeKey(2), &span, None);
            b.text(NodeKey(3), inside);
            b.close_box();
            b.text(NodeKey(4), after);
        });
        fixture.lay_out(&mut layout, 40.0);
        assert_eq!(
            texts(&layout),
            expected,
            "{before}|{margin}|{inside}|{after}"
        );
    }
}
