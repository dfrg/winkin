//! Graphemes across lines. They pin a break between a space and the marks
//! after it, as Chrome and Firefox make it, with the caret keeping to the
//! grapheme.

use super::*;
use crate::selection::{Affinity, Granularity, MotionDirection, Position, Selection};
use crate::style::WhiteSpaceCollapse;

/// The text of each line of `text`, set in `style` and broken at 60px.
fn lines_of_text(cx: &mut Context, style: &ComputedStyle<'_>, text: &str) -> (Layout, Vec<String>) {
    let mut layout = Layout::new();
    build(cx, &mut layout, &ComputedBlockStyle::new(style), |b| {
        b.text(NodeKey(1), text)
    });
    layout.break_lines(cx, Area::new(60.0), &mut NoExclusions);
    let lines = layout
        .lines()
        .map(|line| String::from(&layout.text()[line.text_range()]))
        .collect();
    (layout, lines)
}

/// Chrome's lines, measured with `getClientRects` on each character: a
/// line may break between a space and the marks or the joiner after it,
/// which UAX #14 sets on no space (LB9, LB10). After a no-break space they
/// stay, as a mark set on it.
#[test]
fn a_line_breaks_between_a_space_and_its_marks_as_chrome_does() {
    let mut cx = context();
    let root = sized(&AHEM_FAMILY, 10.0);
    let mut kept = root;
    kept.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    for (style, text, expected) in [
        (&root, "aaaa \u{308}bbbb", &["aaaa ", "\u{308}bbbb"][..]),
        (
            &root,
            "aaaa \u{308}\u{301}bbbb",
            &["aaaa ", "\u{308}\u{301}bbbb"],
        ),
        (&root, "aaaa \u{200D}bbbb", &["aaaa ", "\u{200D}bbbb"]),
        (&kept, "aaaa \u{308}bbbb", &["aaaa ", "\u{308}bbbb"]),
        (&root, "aaaa\u{A0}\u{308}bbbb", &["aaaa\u{A0}\u{308}bbbb"]),
        (&root, "aa \u{308}b ccccccc", &["aa \u{308}b ", "ccccccc"]),
    ] {
        let (_, lines) = lines_of_text(&mut cx, style, text);
        assert_eq!(lines, expected, "{text:?}");
    }
}

/// The caret keeps to the grapheme a space and its marks make, across the
/// line between them: one step goes from before the space to after the
/// marks.
#[test]
fn a_caret_steps_over_a_space_and_its_marks_whole() {
    let mut cx = context();
    let root = sized(&AHEM_FAMILY, 10.0);
    let (layout, _) = lines_of_text(&mut cx, &root, "aaaa \u{308}bbbb");
    let mut selection = Selection::from(Position::new(4, Affinity::Downstream));
    selection.modify(
        &layout,
        MotionDirection::Forward.moving(Granularity::Character),
    );
    assert_eq!(selection.focus().offset, 7);
    selection.modify(
        &layout,
        MotionDirection::Backward.moving(Granularity::Character),
    );
    assert_eq!(selection.focus().offset, 4);
}
