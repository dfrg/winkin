//! Tests of offsets drawn at two places, and of a line's edges on the screen. They pin:
//! - a soft wrap as two stops each way: after white space the wrap removed,
//!   inside a word, at a `<wbr>` and inside a box with room, in
//!   left-to-right, right-to-left and vertical lines;
//! - End past white space a wrap removed or hung;
//! - a change of direction as two stops, one at the edge of each side's
//!   text, in left-to-right and right-to-left paragraphs;
//! - left and right to a line's edges on the screen, in each direction, in
//!   wrapped lines and in vertical lines.
//!
//! Chrome stops once at each of these offsets, and its End stops before
//! white space a wrap removed.

use super::*;
use crate::style::WritingMode;

const U: Affinity = Affinity::Upstream;
const D: Affinity = Affinity::Downstream;

/// A caret stop: its offset, its affinity, and its caret's line and x.
type Stop = (usize, Affinity, usize, f32);

/// Returns `position` as its offset, affinity, line and caret x.
fn stop(layout: &Layout, position: Position) -> Stop {
    let (line, x) = position_caret(layout, position);
    (position.offset, position.affinity, line, x)
}

/// Returns the stops a caret visits moving by `motion` from `from`, `from` first, until it stops.
fn steps(layout: &Layout, from: Position, motion: Motion) -> Vec<Stop> {
    walk_positions(layout, from, motion)
        .into_iter()
        .map(|position| stop(layout, position))
        .collect()
}

/// Returns a character motion in `direction`.
fn character(direction: MotionDirection) -> Motion {
    direction.moving(Granularity::Character)
}

/// Returns where a line-boundary motion in `direction` from `from` lands.
fn line_edge(layout: &Layout, from: usize, direction: MotionDirection) -> Stop {
    let to = moved(
        layout,
        Selection::from(Position::from(from)),
        direction.moving(Granularity::LineBoundary),
    );
    stop(layout, to.focus())
}

/// Returns `text` in Ahem in a block in `direction` and `mode`, broken at `width`.
fn laid_in(direction: BaseDirection, mode: WritingMode, width: f32, text: &str) -> Layout {
    let style = ahem();
    let block = ComputedBlockStyle {
        direction,
        writing_mode: mode,
        ..ComputedBlockStyle::new(&style)
    };
    laid_with(&block, width, |b| b.text(NodeKey(1), text))
}

/// Four Latin words of four letters, which a 100px line wraps after each space.
const LATIN: &str = "abcd efgh ijkl";

/// Three Hebrew words of four letters, two bytes each, which a 100px line wraps after each space.
const HEBREW: &str =
    "\u{5D0}\u{5D1}\u{5D2}\u{5D3} \u{5D4}\u{5D5}\u{5D6}\u{5D7} \u{5D8}\u{5D9}\u{5DB}\u{5DC}";

/// At a wrap after a space the wrap removed, a motion stops at the upper line's end past the space, then the next line's start.
///
/// Backward and left go the other way round. That holds in vertical lines,
/// along the line. In a right-to-left line the upper line's end is at its
/// left, past the space, and left stops there.
#[test]
fn a_wrap_after_a_space_is_two_stops_each_way() {
    for mode in [WritingMode::HorizontalTb, WritingMode::VerticalRl] {
        let layout = laid_in(BaseDirection::Ltr, mode, 100.0, LATIN);
        let forward = [
            (3, D, 0, 60.0),
            (4, U, 0, 80.0),
            (5, U, 0, 100.0),
            (5, D, 1, 0.0),
            (6, U, 1, 20.0),
        ];
        let backward = [
            (6, D, 1, 20.0),
            (5, D, 1, 0.0),
            (5, U, 0, 100.0),
            (4, D, 0, 80.0),
            (3, D, 0, 60.0),
        ];
        for direction in [MotionDirection::Forward, MotionDirection::Right] {
            let stops = steps(&layout, Position::from(3), character(direction));
            assert_eq!(stops[..5], forward, "{mode:?} {direction:?}");
        }
        for direction in [MotionDirection::Backward, MotionDirection::Left] {
            let stops = steps(&layout, Position::from(6), character(direction));
            assert_eq!(stops[..5], backward, "{mode:?} {direction:?}");
        }
    }
    // Hebrew set right to left in line boxes as wide as their text, 80, at
    // the area's right: the upper line's end is past the space, left of its
    // box.
    let layout = laid_in(BaseDirection::Rtl, WritingMode::HorizontalTb, 100.0, HEBREW);
    let forward = [
        (6, D, 0, 20.0),
        (8, U, 0, 0.0),
        (9, U, 0, -20.0),
        (9, D, 1, 80.0),
        (11, U, 1, 60.0),
    ];
    let stops = steps(
        &layout,
        Position::from(6),
        character(MotionDirection::Forward),
    );
    assert_eq!(stops[..5], forward);
    let stops = steps(
        &layout,
        Position::from(11),
        character(MotionDirection::Backward),
    );
    assert_eq!(
        stops[..5],
        [
            (11, D, 1, 60.0),
            (9, D, 1, 80.0),
            (9, U, 0, -20.0),
            (8, D, 0, 0.0),
            (6, D, 0, 20.0)
        ]
    );
    // Left stops at the upper line's left end, past the space, and goes no
    // further: left continues onto the line above.
    let stops = steps(&layout, Position::from(6), character(MotionDirection::Left));
    assert_eq!(stops, forward[..3]);
}

/// A wrap inside a word, at a `<wbr>` and inside a box with room is two stops each way too.
#[test]
fn every_wrap_is_two_stops_each_way() {
    // Inside a word: the upper line's end, then the next line's start.
    let broken = styled(|style| style.text.word_break = WordBreak::BreakAll);
    let layout = laid_with(&ComputedBlockStyle::new(&broken), 80.0, |b| {
        b.text(NodeKey(1), "abcdefgh")
    });
    let forward = [
        (3, D, 0, 60.0),
        (4, U, 0, 80.0),
        (4, D, 1, 0.0),
        (5, U, 1, 20.0),
    ];
    for direction in [MotionDirection::Forward, MotionDirection::Right] {
        let stops = steps(&layout, Position::from(3), character(direction));
        assert_eq!(stops[..4], forward, "{direction:?}");
    }
    let backward = [
        (5, D, 1, 20.0),
        (4, D, 1, 0.0),
        (4, U, 0, 80.0),
        (3, D, 0, 60.0),
    ];
    for direction in [MotionDirection::Backward, MotionDirection::Left] {
        let stops = steps(&layout, Position::from(5), character(direction));
        assert_eq!(stops[..4], backward, "{direction:?}");
    }
    // At a `<wbr>`: before it at the upper line's end, after it at the next
    // line's start.
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 80.0, |b| {
        b.text(NodeKey(1), "abcd");
        b.break_opportunity();
        b.text(NodeKey(1), "efgh");
    });
    let forward = [
        (3, D, 0, 60.0),
        (4, U, 0, 80.0),
        (7, D, 1, 0.0),
        (8, U, 1, 20.0),
    ];
    for direction in [MotionDirection::Forward, MotionDirection::Right] {
        let stops = steps(&layout, Position::from(3), character(direction));
        assert_eq!(stops[..4], forward, "{direction:?}");
    }
    // Inside a box with 10px of padding: `ab`, the padding, `cd ` and the
    // wrap, then `efgh` and the closing padding on the next line.
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 10.0,
                right: 10.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..ahem()
    };
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 100.0, |b| {
        b.text(NodeKey(1), "ab");
        b.open_box(NodeKey(2), &padded, None);
        b.text(NodeKey(3), "cd efgh");
        b.close_box();
    });
    assert_eq!(layout.lines().len(), 2);
    let forward = [
        (3, D, 0, 70.0),
        (4, U, 0, 90.0),
        (5, U, 0, 110.0),
        (5, D, 1, 0.0),
        (6, U, 1, 20.0),
    ];
    for direction in [MotionDirection::Forward, MotionDirection::Right] {
        let stops = steps(&layout, Position::from(3), character(direction));
        assert_eq!(stops[..5], forward, "{direction:?}");
    }
    let backward = [
        (6, D, 1, 20.0),
        (5, D, 1, 0.0),
        (5, U, 0, 110.0),
        (4, D, 0, 90.0),
        (3, D, 0, 70.0),
    ];
    for direction in [MotionDirection::Backward, MotionDirection::Left] {
        let stops = steps(&layout, Position::from(6), character(direction));
        assert_eq!(stops[..5], backward, "{direction:?}");
    }
    // The upper line's end keeps the box's node, after its space.
    let end = layout.node_position(Position::new(5, U)).unwrap();
    assert_eq!((end.key, end.offset), (NodeKey(3), 3));
}

/// End at a wrap goes past the white space the wrap removed or hung, so typed text follows the space.
///
/// Chrome's End stops before removed white space, at 4 here, and after the
/// first space hanging. A line that ends the paragraph keeps Chrome's End.
#[test]
fn end_goes_past_white_space_at_a_wrap() {
    for mode in [WritingMode::HorizontalTb, WritingMode::VerticalRl] {
        let layout = laid_in(BaseDirection::Ltr, mode, 100.0, LATIN);
        let end = line_edge(&layout, 1, MotionDirection::Forward);
        assert_eq!(end, (5, U, 0, 100.0), "{mode:?}");
        let node = layout.node_position(Position::new(5, U)).unwrap();
        assert_eq!((node.key, node.offset), (NodeKey(1), 5), "{mode:?}");
        // Home on the next line is its start.
        let home = line_edge(&layout, 7, MotionDirection::Backward);
        assert_eq!(home, (5, D, 1, 0.0), "{mode:?}");
    }
    // In a right-to-left line, past the space at the line's left.
    let layout = laid_in(BaseDirection::Rtl, WritingMode::HorizontalTb, 100.0, HEBREW);
    assert_eq!(
        line_edge(&layout, 2, MotionDirection::Forward),
        (9, U, 0, -20.0)
    );
    // Preserved spaces hanging past the line: past all three.
    let kept = styled(|style| style.text.white_space_collapse = WhiteSpaceCollapse::Preserve);
    let layout = laid_with(&ComputedBlockStyle::new(&kept), 100.0, |b| {
        b.text(NodeKey(1), "abcd   efgh")
    });
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(
        line_edge(&layout, 1, MotionDirection::Forward),
        (7, U, 0, 140.0)
    );
    // The paragraph's last line ends before the space it removed, as in
    // Chrome.
    let layout = laid("abcd ", 400.0);
    assert_eq!(
        line_edge(&layout, 1, MotionDirection::Forward),
        (4, U, 0, 80.0)
    );
}

/// Where the text changes direction, a motion stops at the edge of the text on each side, the offset upstream and downstream.
///
/// Left and right reach every place on the screen once, both ends of the
/// line included. Chrome's arrows stop once at each offset, on the
/// paragraph's side, and never reach a line end that a run against the
/// paragraph draws.
#[test]
fn a_change_of_direction_is_two_stops() {
    // "abc " 0..80, then alef, bet, gimel drawn right to left 140..80,
    // then " def" 140..220.
    let layout = laid("abc \u{5D0}\u{5D1}\u{5D2} def", 400.0);
    let stops = steps(
        &layout,
        Position::from(3),
        character(MotionDirection::Forward),
    );
    assert_eq!(
        stops[..8],
        [
            (3, D, 0, 60.0),
            (4, U, 0, 80.0),
            (4, D, 0, 140.0),
            (6, U, 0, 120.0),
            (8, U, 0, 100.0),
            (10, U, 0, 80.0),
            (10, D, 0, 140.0),
            (11, U, 0, 160.0),
        ]
    );
    let stops = steps(
        &layout,
        Position::from(11),
        character(MotionDirection::Backward),
    );
    assert_eq!(
        stops[..8],
        [
            (11, D, 0, 160.0),
            (10, D, 0, 140.0),
            (10, U, 0, 80.0),
            (8, D, 0, 100.0),
            (6, D, 0, 120.0),
            (4, D, 0, 140.0),
            (4, U, 0, 80.0),
            (3, D, 0, 60.0),
        ]
    );
    let stops = steps(
        &layout,
        Position::from(3),
        character(MotionDirection::Right),
    );
    assert_eq!(
        stops[..6],
        [
            (3, D, 0, 60.0),
            (4, U, 0, 80.0),
            (8, D, 0, 100.0),
            (6, D, 0, 120.0),
            (4, D, 0, 140.0),
            (11, U, 0, 160.0),
        ]
    );
    // A line ending in text against the paragraph: right reaches its right
    // end, the Hebrew's start, and End is the Hebrew's end at its left.
    let layout = laid("abc \u{5D0}\u{5D1}\u{5D2}", 400.0);
    let stops = steps(
        &layout,
        Position::from(0),
        character(MotionDirection::Right),
    );
    assert_eq!(stops.last(), Some(&(4, D, 0, 140.0)));
    assert_eq!(stops.len(), 8);
    assert_eq!(
        line_edge(&layout, 0, MotionDirection::Forward),
        (10, U, 0, 80.0)
    );
    // Latin in a right-to-left paragraph: alef 160..180, bet 140..160, a
    // space 120..140, abc 60..120, a space 40..60, gimel and dalet 20..40
    // and 0..20. Offset 5 is the Hebrew's end upstream at 120 and the
    // Latin's start downstream at 60; 8 is the Latin's end at 120 and the
    // Hebrew's start at 60.
    let layout = laid_in(
        BaseDirection::Rtl,
        WritingMode::HorizontalTb,
        400.0,
        "\u{5D0}\u{5D1} abc \u{5D2}\u{5D3}",
    );
    let stops = steps(
        &layout,
        Position::from(4),
        character(MotionDirection::Forward),
    );
    assert_eq!(
        stops[..4],
        [
            (4, D, 0, 140.0),
            (5, U, 0, 120.0),
            (5, D, 0, 60.0),
            (6, U, 0, 80.0)
        ]
    );
    let left = steps(&layout, Position::from(0), character(MotionDirection::Left));
    let xs: Vec<f32> = left.iter().map(|stop| stop.3).collect();
    assert_eq!(
        xs,
        [
            180.0, 160.0, 140.0, 120.0, 100.0, 80.0, 60.0, 40.0, 20.0, 0.0
        ]
    );
}

/// Left and right by line boundary go to the line's left and right edges on the screen, whatever its direction.
///
/// The line's logical start is downstream and its end upstream. On a line
/// that wraps after white space it removed, the space stands at the line's
/// end in the paragraph's direction, and the edge there is past it. In a
/// vertical line, left is the line's top and right its bottom, as for a
/// character.
#[test]
fn left_and_right_go_to_a_lines_edges_on_the_screen() {
    use MotionDirection::{Left, Right};
    let layout = laid("abc def", 400.0);
    assert_eq!(line_edge(&layout, 3, Left), (0, D, 0, 0.0));
    assert_eq!(line_edge(&layout, 3, Right), (7, U, 0, 140.0));
    // Right to left, in a line box as wide as its text: the start is on the
    // right, and the end on the left.
    let layout = laid_in(
        BaseDirection::Rtl,
        WritingMode::HorizontalTb,
        400.0,
        "\u{5D0}\u{5D1}\u{5D2} \u{5D3}\u{5D4}\u{5D5}",
    );
    assert_eq!(line_edge(&layout, 2, Right), (0, D, 0, 140.0));
    assert_eq!(line_edge(&layout, 2, Left), (13, U, 0, 0.0));
    // Wrapped: the end past the removed space, on the line's right in
    // left-to-right text and on its left in right-to-left text.
    let layout = laid(LATIN, 100.0);
    assert_eq!(line_edge(&layout, 2, Left), (0, D, 0, 0.0));
    assert_eq!(line_edge(&layout, 2, Right), (5, U, 0, 100.0));
    let layout = laid_in(BaseDirection::Rtl, WritingMode::HorizontalTb, 100.0, HEBREW);
    assert_eq!(line_edge(&layout, 2, Right), (0, D, 0, 80.0));
    assert_eq!(line_edge(&layout, 2, Left), (9, U, 0, -20.0));
    // A left-to-right line ending in Hebrew, wrapped after a space: "ab "
    // 0..60, gimel, bet, alef 60..120, and the removed space past them.
    let layout = laid("ab \u{5D0}\u{5D1}\u{5D2} de", 120.0);
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(line_edge(&layout, 1, Right), (10, U, 0, 140.0));
    assert_eq!(line_edge(&layout, 1, Left), (0, D, 0, 0.0));
    // Vertical: left is the line's top and right its bottom, as a step by
    // character goes. Right to left, the start is at the bottom.
    let cases = [
        (BaseDirection::Ltr, LATIN, (0, D, 0, 0.0), (5, U, 0, 100.0)),
        (
            BaseDirection::Rtl,
            HEBREW,
            (9, U, 0, -20.0),
            (0, D, 0, 80.0),
        ),
    ];
    for (direction, text, top, bottom) in cases {
        let layout = laid_in(direction, WritingMode::VerticalRl, 100.0, text);
        assert_eq!(line_edge(&layout, 2, Left), top, "{direction:?}");
        assert_eq!(line_edge(&layout, 2, Right), bottom, "{direction:?}");
        let step = steps(&layout, Position::from(2), character(Right))[1];
        assert_eq!(step.3, 20.0 + position_caret(&layout, Position::from(2)).1);
    }
}
