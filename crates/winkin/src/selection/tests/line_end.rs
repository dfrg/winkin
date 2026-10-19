//! Tests of End that stops before trailing preserved white space first. They
//! pin:
//! - the two stops at a soft wrap with hanging spaces, at one after
//!   `break-spaces` spaces and before a forced break, and the one stop where the line has no such white space;
//! - End from the first stop, or past it, going to the line's end;
//! - left and right to the line's end on the screen, in right-to-left and
//!   vertical lines;
//! - extending motion through the same stops.
//!
//! Chrome has no such End, so these pin the behavior as defined.

use super::*;
use crate::style::WritingMode;

const U: Affinity = Affinity::Upstream;

/// A caret stop: its offset, its affinity, and its caret's line and x.
type Stop = (usize, Affinity, usize, f32);

/// Returns the stops End visits from `from` by line-boundary motion in
/// `direction` under `rule`, `from` excluded, until it stops.
fn ends(
    layout: &Layout,
    from: usize,
    direction: MotionDirection,
    rule: LineEndMotion,
) -> Vec<Stop> {
    let motion = direction
        .moving(Granularity::LineBoundary)
        .with_line_end_motion(rule);
    walk_positions(layout, Position::from(from), motion)
        .into_iter()
        .skip(1)
        .map(|position| {
            let (line, x) = position_caret(layout, position);
            (position.offset, position.affinity, line, x)
        })
        .collect()
}

/// Returns the stops End visits from `from` forward under the two-step rule.
fn two_step(layout: &Layout, from: usize) -> Vec<Stop> {
    let rule = LineEndMotion::BeforeTrailingSpace;
    ends(layout, from, MotionDirection::Forward, rule)
}

/// Returns `text` in preserved Ahem in a block in `direction` and `mode`,
/// broken at `width`.
fn preserved(direction: BaseDirection, mode: WritingMode, width: f32, text: &str) -> Layout {
    let style = styled(|style| style.text.white_space_collapse = WhiteSpaceCollapse::Preserve);
    let block = ComputedBlockStyle {
        direction,
        writing_mode: mode,
        ..ComputedBlockStyle::new(&style)
    };
    laid_with(&block, width, |b| b.text(NodeKey(1), text))
}

/// At a soft wrap after hanging spaces, End stops after the text first and
/// past the spaces next. The default End goes past the spaces at once.
///
/// From the first stop, or from inside the spaces, End goes past them.
#[test]
fn end_stops_before_hanging_spaces_at_a_wrap_first() {
    for mode in [WritingMode::HorizontalTb, WritingMode::VerticalRl] {
        let layout = preserved(BaseDirection::Ltr, mode, 100.0, "abcd   efgh");
        assert_eq!(layout.lines().len(), 2);
        let rule = LineEndMotion::LineEnd;
        let end = (7, U, 0, 140.0);
        assert_eq!(
            ends(&layout, 1, MotionDirection::Forward, rule),
            [end],
            "{mode:?}"
        );
        assert_eq!(two_step(&layout, 1), [(4, U, 0, 80.0), end], "{mode:?}");
        assert_eq!(two_step(&layout, 4), [end], "{mode:?}");
        assert_eq!(two_step(&layout, 5), [end], "{mode:?}");
        // Right is End in a left-to-right line, and Left stays Home.
        let rule = LineEndMotion::BeforeTrailingSpace;
        assert_eq!(
            ends(&layout, 1, MotionDirection::Right, rule),
            [(4, U, 0, 80.0), end],
            "{mode:?}"
        );
        let home = ends(&layout, 5, MotionDirection::Left, rule);
        assert_eq!(home, [(0, Affinity::Downstream, 0, 0.0)], "{mode:?}");
    }
}

/// Under `break-spaces` the spaces take room before the wrap, and End stops
/// before them first too.
#[test]
fn end_stops_before_spaces_kept_by_break_spaces_first() {
    let style = styled(|style| style.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces);
    let layout = laid_with(&ComputedBlockStyle::new(&style), 140.0, |b| {
        b.text(NodeKey(1), "abcd   efgh")
    });
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(two_step(&layout, 1), [(4, U, 0, 80.0), (7, U, 0, 140.0)]);
}

/// Before a forced break, End stops after the text first, and then after the
/// first hanging space, where the default End stops.
#[test]
fn end_stops_before_spaces_at_a_forced_break_first() {
    let layout = preserved(
        BaseDirection::Ltr,
        WritingMode::HorizontalTb,
        400.0,
        "abcd   \nefgh",
    );
    let rule = LineEndMotion::LineEnd;
    let end = ends(&layout, 1, MotionDirection::Forward, rule);
    assert_eq!(end, [(5, U, 0, 100.0)]);
    assert_eq!(two_step(&layout, 1), [(4, U, 0, 80.0), end[0]]);
    // From inside the spaces, End goes to the line's end.
    assert_eq!(two_step(&layout, 5), end);
    assert_eq!(two_step(&layout, 6), end);
    // At the text's end too.
    let layout = preserved(
        BaseDirection::Ltr,
        WritingMode::HorizontalTb,
        400.0,
        "abcd  ",
    );
    let end = ends(&layout, 1, MotionDirection::Forward, rule);
    assert_eq!(two_step(&layout, 1), [(4, U, 0, 80.0), end[0]]);
}

/// Where a line ends without preserved white space, both rules stop once at
/// its end. White space a wrap removes, or that collapses, adds no stop.
#[test]
fn end_without_trailing_preserved_space_stops_once() {
    let layout = laid("abcd efgh", 400.0);
    assert_eq!(two_step(&layout, 1), [(9, U, 0, 180.0)]);
    // Past the space a wrap removed, as the default End goes.
    let layout = laid("abcd efgh", 100.0);
    assert_eq!(two_step(&layout, 1), [(5, U, 0, 100.0)]);
    // The paragraph's last line ends before the space it removed.
    let layout = laid("abcd  ", 400.0);
    assert_eq!(two_step(&layout, 1), [(4, U, 0, 80.0)]);
    // A line of nothing but preserved spaces has one end.
    let layout = preserved(
        BaseDirection::Ltr,
        WritingMode::HorizontalTb,
        400.0,
        "   \nabcd",
    );
    let rule = LineEndMotion::LineEnd;
    let end = ends(&layout, 0, MotionDirection::Forward, rule);
    assert_eq!(two_step(&layout, 0), end);
}

/// In a right-to-left line the first stop is at the text's left edge, and
/// the line's end past the hanging spaces further left. Left is End there.
///
/// The text is drawn from 80 to 0 and the spaces from 0 to -60.
#[test]
fn right_to_left_end_stops_at_the_text_edge_first() {
    let text = "\u{5D0}\u{5D1}\u{5D2}\u{5D3}   \u{5D4}\u{5D5}\u{5D6}\u{5D7}";
    let layout = preserved(BaseDirection::Rtl, WritingMode::HorizontalTb, 100.0, text);
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(caret(&layout, 6), (0, 20.0));
    let stops = [(8, U, 0, 0.0), (11, U, 0, -60.0)];
    assert_eq!(two_step(&layout, 2), stops);
    let rule = LineEndMotion::BeforeTrailingSpace;
    assert_eq!(ends(&layout, 2, MotionDirection::Left, rule), stops);
    // Right is Home there.
    let home = ends(&layout, 2, MotionDirection::Right, rule);
    assert_eq!(home, [(0, Affinity::Downstream, 0, 80.0)]);
}

/// Extending End selects to the first stop, then to the line's end.
#[test]
fn extending_end_selects_to_each_stop() {
    let layout = preserved(
        BaseDirection::Ltr,
        WritingMode::HorizontalTb,
        100.0,
        "abcd   efgh",
    );
    let motion = MotionDirection::Forward
        .extending(Granularity::LineBoundary)
        .with_line_end_motion(LineEndMotion::BeforeTrailingSpace);
    let first = moved(&layout, Selection::from(Position::from(1)), motion);
    assert_eq!((first.range(), first.focus()), (1..4, Position::new(4, U)));
    let second = moved(&layout, first, motion);
    assert_eq!(
        (second.range(), second.focus()),
        (1..7, Position::new(7, U))
    );
    assert_eq!(moved(&layout, second, motion), second);
}
