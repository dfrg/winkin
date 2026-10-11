//! Seam tests: a caret keeps the side it arrived from at a boundary between nodes.
//!
//! The text is `a`, a bold span with no room holding `bc`, and `d`, in Ahem
//! at 20px, as nodes 1, 3 and 4. They pin:
//! - character motion landing upstream going forward and downstream going
//!   backward, logically and on the screen, with one stop per boundary;
//! - clicks taking the side of the glyph half hit;
//! - node positions, and so typed text, following the side;
//! - both sides drawn at one place;
//! - word motion, nested spans and a right-to-left paragraph.
//!
//! Chrome lands downstream after every step, and so reports the node after
//! a seam reached going forward.

use super::*;
use crate::style::FontWeight;

const U: Affinity = Affinity::Upstream;
const D: Affinity = Affinity::Downstream;

/// A caret stop: its offset, its affinity, its caret's x, and its node and offset there.
type Stop = (usize, Affinity, f32, (u64, usize));

/// Returns the bold Ahem style, which has no margin, border or padding.
fn bold() -> ComputedStyle<'static> {
    styled(|style| style.font.weight = FontWeight::BOLD)
}

/// Returns `a`, a bold span holding `bc`, and `d`, as nodes 1 to 4, in `block`.
fn seamed(block: &ComputedBlockStyle<'_>, [before, inside, after]: [&str; 3]) -> Layout {
    laid_with(block, 400.0, |b| {
        b.text(NodeKey(1), before);
        b.open_box(NodeKey(2), &bold(), None);
        b.text(NodeKey(3), inside);
        b.close_box();
        b.text(NodeKey(4), after);
    })
}

/// Returns `position` as its offset, affinity, caret x and node position.
fn stop(layout: &Layout, position: Position) -> Stop {
    let (_, x) = position_caret(layout, position);
    let node = layout.node_position(position).unwrap();
    (
        position.offset,
        position.affinity,
        x,
        (node.key.0, node.offset),
    )
}

/// Returns the stops a caret visits moving by `motion` from `from`, until it stops.
fn steps(layout: &Layout, from: Position, motion: Motion) -> Vec<Stop> {
    let mut at = Selection::from(from);
    let mut visited = vec![stop(layout, from)];
    loop {
        let before = at.focus();
        at.modify(layout, motion);
        if at.focus() == before {
            return visited;
        }
        visited.push(stop(layout, at.focus()));
        assert!(visited.len() < 32, "a walk that does not end");
    }
}

/// Going forward or right the caret lands upstream, in the node it passed;
/// going backward or left, downstream, in the node it came from. Each
/// boundary is one stop each way.
#[test]
fn a_step_onto_a_seam_lands_on_the_side_it_came_from() {
    let layout = seamed(&ComputedBlockStyle::new(&ahem()), ["a", "bc", "d"]);
    let ahead = vec![
        (0, D, 0.0, (1, 0)),
        (1, U, 20.0, (1, 1)),
        (2, U, 40.0, (3, 1)),
        (3, U, 60.0, (3, 2)),
        (4, U, 80.0, (4, 1)),
    ];
    let behind = vec![
        (4, D, 80.0, (4, 1)),
        (3, D, 60.0, (4, 0)),
        (2, D, 40.0, (3, 1)),
        (1, D, 20.0, (3, 0)),
        (0, D, 0.0, (1, 0)),
    ];
    let character = |direction: MotionDirection| direction.moving(Granularity::Character);
    let (start, end) = (Position::from(0), Position::from(4));
    assert_eq!(
        steps(&layout, start, character(MotionDirection::Forward)),
        ahead
    );
    assert_eq!(
        steps(&layout, start, character(MotionDirection::Right)),
        ahead
    );
    assert_eq!(
        steps(&layout, end, character(MotionDirection::Backward)),
        behind
    );
    assert_eq!(
        steps(&layout, end, character(MotionDirection::Left)),
        behind
    );
}

/// A click in the right half of the glyph before a seam is upstream, in its
/// node; one in the left half of the glyph after is downstream, in that one.
/// Both are drawn at the seam.
#[test]
fn a_click_beside_a_seam_takes_the_side_of_the_glyph_hit() {
    let layout = seamed(&ComputedBlockStyle::new(&ahem()), ["a", "bc", "d"]);
    let expected = [
        (15.0, (1, U, 20.0, (1, 1))),
        (25.0, (1, D, 20.0, (3, 0))),
        (55.0, (3, U, 60.0, (3, 2))),
        (65.0, (3, D, 60.0, (4, 0))),
    ];
    for (x, at) in expected {
        assert_eq!(stop(&layout, hit(&layout, x, 0)), at, "a click at {x}");
    }
}

/// Without room the two sides of a seam are one place: the caret, strong
/// and weak, is the same for either affinity.
#[test]
fn both_sides_of_a_seam_are_drawn_at_one_place() {
    let layout = seamed(&ComputedBlockStyle::new(&ahem()), ["a", "bc", "d"]);
    for offset in [1, 3] {
        assert_eq!(
            layout.carets(Position::new(offset, U)),
            layout.carets(Position::new(offset, D)),
            "{offset}"
        );
    }
}

/// A position from a node is on that node's side of a seam, whatever
/// affinity is asked for, and maps back to the node.
#[test]
fn a_position_from_a_node_keeps_its_side_of_a_seam() {
    let layout = seamed(&ComputedBlockStyle::new(&ahem()), ["a", "bc", "d"]);
    let cases = [
        (1, 1, Position::new(1, U)),
        (3, 0, Position::new(1, D)),
        (3, 2, Position::new(3, U)),
        (4, 0, Position::new(3, D)),
    ];
    for affinity in [U, D] {
        for (key, offset, expected) in cases {
            let found = layout.position(NodeKey(key), offset, affinity).unwrap();
            assert_eq!(found, expected, "{key}@{offset} {affinity:?}");
            let node = layout.node_position(found).unwrap();
            assert_eq!((node.key.0, node.offset), (key, offset));
        }
    }
}

/// A word motion landing on a seam keeps the side it came from: forward at
/// a word's end in the node before, backward at a word's start in the node
/// after.
#[test]
fn a_word_motion_onto_a_seam_lands_on_the_side_it_came_from() {
    let layout = seamed(&ComputedBlockStyle::new(&ahem()), ["ab", " cd", ""]);
    let word = |direction: MotionDirection| {
        direction
            .moving(Granularity::Word)
            .with_word_motion(WordMotion::StopAtWordEnd)
    };
    let ahead = moved(
        &layout,
        Selection::from(Position::from(0)),
        word(MotionDirection::Forward),
    );
    assert_eq!(stop(&layout, ahead.focus()), (2, U, 40.0, (1, 2)));
    let layout = seamed(&ComputedBlockStyle::new(&ahem()), ["ab ", "cd", ""]);
    let behind = moved(
        &layout,
        Selection::from(Position::from(5)),
        word(MotionDirection::Backward),
    );
    assert_eq!(stop(&layout, behind.focus()), (3, D, 60.0, (3, 0)));
}

/// Nested spans with no room are one seam: the caret lands in the text of
/// the innermost node on the side it came from.
#[test]
fn nested_spans_are_one_seam() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "a");
        b.open_box(NodeKey(2), &bold(), None);
        b.open_box(NodeKey(5), &ahem(), None);
        b.text(NodeKey(3), "bc");
        b.close_box();
        b.close_box();
        b.text(NodeKey(4), "d");
    });
    let right = MotionDirection::Right.moving(Granularity::Character);
    let left = MotionDirection::Left.moving(Granularity::Character);
    let ahead: Vec<_> = steps(&layout, Position::from(0), right)
        .into_iter()
        .map(|(offset, affinity, _, node)| (offset, affinity, node))
        .collect();
    assert_eq!(
        ahead,
        [
            (0, D, (1, 0)),
            (1, U, (1, 1)),
            (2, U, (3, 1)),
            (3, U, (3, 2)),
            (4, U, (4, 1))
        ]
    );
    let behind: Vec<_> = steps(&layout, Position::from(4), left)
        .into_iter()
        .map(|(offset, affinity, _, node)| (offset, affinity, node))
        .collect();
    assert_eq!(
        behind,
        [
            (4, D, (4, 1)),
            (3, D, (4, 0)),
            (2, D, (3, 1)),
            (1, D, (3, 0)),
            (0, D, (1, 0))
        ]
    );
}

/// In a right-to-left paragraph the sides mirror: left, forward, lands
/// upstream in the node it passed; right, backward, downstream in the node
/// it came from.
///
/// Alef, a bold span holding bet and gimel, and dalet, from the left of the
/// line box: dalet 0..20, gimel 20..40, bet 40..60, alef 60..80. Each letter
/// is two bytes.
#[test]
fn seams_in_a_right_to_left_paragraph_mirror() {
    let style = ahem();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let layout = laid_with(&rtl, 80.0, |b| {
        b.text(NodeKey(1), "\u{5D0}");
        b.open_box(NodeKey(2), &bold(), None);
        b.text(NodeKey(3), "\u{5D1}\u{5D2}");
        b.close_box();
        b.text(NodeKey(4), "\u{5D3}");
    });
    let left = MotionDirection::Left.moving(Granularity::Character);
    let right = MotionDirection::Right.moving(Granularity::Character);
    assert_eq!(
        steps(&layout, Position::from(0), left),
        [
            (0, D, 80.0, (1, 0)),
            (2, U, 60.0, (1, 2)),
            (4, U, 40.0, (3, 2)),
            (6, U, 20.0, (3, 4)),
            (8, U, 0.0, (4, 2))
        ]
    );
    assert_eq!(
        steps(&layout, Position::from(8), right),
        [
            (8, D, 0.0, (4, 2)),
            (6, D, 20.0, (4, 0)),
            (4, D, 40.0, (3, 2)),
            (2, D, 60.0, (3, 0)),
            (0, D, 80.0, (1, 0))
        ]
    );
    assert_eq!(stop(&layout, hit(&layout, 65.0, 0)), (2, U, 60.0, (1, 2)));
    assert_eq!(stop(&layout, hit(&layout, 55.0, 0)), (2, D, 60.0, (3, 0)));
}
