//! Overflow tests. They pin:
//! - a word with nowhere to break overflowing, unless `overflow-wrap` or
//!   `word-break: break-all` lets it break;
//! - spaces ending an overflowing line staying on it;
//! - a paragraph whose prefix goes back breaking as Chrome's walk does.

use super::*;
use crate::style::FirstLineVariant;

/// A word with nowhere to break overflows its line, which holds it whole.
///
/// `overflow-wrap: anywhere` and `break-word` break it where they must, a
/// line at a time. Under `nowrap` there is no break to offer, so it
/// overflows however it may break. The results match Chrome.
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
            // Narrower than a glyph, a line keeps one, overflowing.
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
/// A word that fits on a line of its own goes there first, and only one
/// that fits nowhere is broken. A word in a style that may not break
/// overflows at its first opportunity even where a later one might.
#[test]
fn overflow_wrap_breaks_only_what_fits_nowhere() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut breaking = ahem(20.0);
    breaking.text.overflow_wrap = OverflowWrap::BreakWord;
    fixture.text(&mut layout, &breaking, "YY XXXXXXX");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["YY ", "XXXXX", "XX"]);
    // `XX` may not break and `YYYY` may: the first word overflows whole.
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
/// `XX<span>XX</span>` 40 wide, the span alone allowing `overflow-wrap`,
/// breaks before the span, as Chrome's retry breaks before the first
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
/// A break that fits wraps the space after it. But where the line already
/// overflows at its first opportunity and only spaces follow, they stay on
/// that line rather than make lines of their own.
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
/// rather than searched.
///
/// The search needs a prefix that only goes forward. The walk goes from the
/// line's start to the first boundary that does not fit, as Chrome's
/// breaker walks every paragraph. Here the third `X` overflows before the
/// margin takes it back, so the line ends at its first opportunity, which
/// fits.
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
