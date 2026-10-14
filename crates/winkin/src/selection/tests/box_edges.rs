//! Box edge tests: an inline box edge with room has a position on each side.
//!
//! The text is `a`, a box holding `bc`, and `d`, in Ahem at 20px, the box
//! with 10px of padding, border or margin at each end. They pin:
//! - clicks in the room taking the nearer side, and on glyphs the halves;
//! - character motion landing on the side it came from, logically and on the
//!   screen, in one stop each way;
//! - node positions following the side, and positions from nodes keeping it;
//! - a line's start and end, nested boxes, an empty box, a right-to-left
//!   paragraph and Hebrew in a left-to-right box.

use super::*;

const U: Affinity = Affinity::Upstream;
const D: Affinity = Affinity::Downstream;

/// A caret stop: its offset, its affinity and its caret's x.
type Stop = (usize, Affinity, f32);

/// A node position: the node's key and the offset in its text.
type Node = (u64, usize);

/// The three ways a box takes room at its inline ends.
#[derive(Copy, Clone, Debug)]
enum Room {
    Padding,
    Border,
    Margin,
}

const ROOMS: [Room; 3] = [Room::Padding, Room::Border, Room::Margin];

/// Returns `style` with 10px of `room` on its left and right.
fn with_room(style: ComputedStyle<'static>, room: Room) -> ComputedStyle<'static> {
    let sides = Sides::<f32> {
        left: 10.0,
        right: 10.0,
        ..Sides::ZERO
    };
    let edges = match room {
        Room::Padding => EdgesGroup {
            padding: sides.into(),
            ..EdgesGroup::INITIAL
        },
        Room::Border => EdgesGroup {
            border: sides,
            ..EdgesGroup::INITIAL
        },
        Room::Margin => EdgesGroup {
            margin: sides.into(),
            ..EdgesGroup::INITIAL
        },
    };
    ComputedStyle { edges, ..style }
}

/// Returns `before`, a box with `room` holding `inside`, and `after`, as
/// nodes 1 to 4, in `block`.
fn boxed(block: &ComputedBlockStyle<'_>, room: Room, [before, inside, after]: [&str; 3]) -> Layout {
    let spanned = with_room(ahem(), room);
    laid_with(block, 400.0, |b| {
        b.text(NodeKey(1), before);
        b.open_box(NodeKey(2), &spanned, None);
        b.text(NodeKey(3), inside);
        b.close_box();
        b.text(NodeKey(4), after);
    })
}

/// Returns `position` as its offset, affinity and caret x.
fn stop(layout: &Layout, position: Position) -> Stop {
    let (_, x) = position_caret(layout, position);
    (position.offset, position.affinity, x)
}

/// Returns the stops a caret visits moving by `motion` from `from`, until
/// it stops.
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

/// Returns the node and offset `position` maps to.
fn node(layout: &Layout, position: Position) -> Node {
    let found = layout.node_position(position).unwrap();
    (found.key.0, found.offset)
}

/// Returns the stops going forward from the text's start and backward from
/// its end.
fn both_ways(layout: &Layout) -> [Vec<Stop>; 2] {
    let end = Position::from(layout.text().len());
    [
        steps(
            layout,
            Position::from(0),
            MotionDirection::Forward.moving(Granularity::Character),
        ),
        steps(
            layout,
            end,
            MotionDirection::Backward.moving(Granularity::Character),
        ),
    ]
}

/// Returns the stops going right from the text's start and left from its end.
fn screen_ways(layout: &Layout, from_left: usize, from_right: usize) -> [Vec<Stop>; 2] {
    [
        steps(
            layout,
            Position::from(from_left),
            MotionDirection::Right.moving(Granularity::Character),
        ),
        steps(
            layout,
            Position::from(from_right),
            MotionDirection::Left.moving(Granularity::Character),
        ),
    ]
}

/// A click in a box's room takes the nearer side, the room split at its
/// middle and a tie going left. On a glyph the halves decide, so a click in
/// the box's first or last glyph lands inside the box.
///
/// From the left: `a` 0..20, room 20..30, `bc` 30..70, room 70..80, `d`
/// 80..100.
#[test]
fn a_click_in_a_boxs_room_takes_the_nearer_side() {
    let expected: [(f32, usize, Affinity, f32, Node); 11] = [
        (15.0, 1, U, 20.0, (1, 1)),
        (22.0, 1, U, 20.0, (1, 1)),
        (25.0, 1, U, 20.0, (1, 1)),
        (27.0, 1, D, 30.0, (3, 0)),
        (35.0, 1, D, 30.0, (3, 0)),
        (45.0, 2, D, 50.0, (3, 1)),
        (65.0, 3, U, 70.0, (3, 2)),
        (72.0, 3, U, 70.0, (3, 2)),
        (75.0, 3, U, 70.0, (3, 2)),
        (78.0, 3, D, 80.0, (4, 0)),
        (85.0, 3, D, 80.0, (4, 0)),
    ];
    for room in ROOMS {
        let layout = boxed(&ComputedBlockStyle::new(&ahem()), room, ["a", "bc", "d"]);
        for (x, offset, affinity, caret, at) in expected {
            let found = hit(&layout, x, 0);
            assert_eq!(
                stop(&layout, found),
                (offset, affinity, caret),
                "{room:?}: a click at {x}"
            );
            assert_eq!(node(&layout, found), at, "{room:?}: a click at {x}");
        }
    }
}

/// Character motion stops once at each box edge each way, on the side it
/// came from: forward at the trailing edge of what it passed, upstream, and
/// backward at the leading edge, downstream. The next step crosses the room
/// and the following character together. Right and left behave the same on
/// the screen.
#[test]
fn character_motion_lands_on_the_side_it_came_from() {
    let forward = vec![
        (0, D, 0.0),
        (1, U, 20.0),
        (2, U, 50.0),
        (3, U, 70.0),
        (4, U, 100.0),
    ];
    let backward = vec![
        (4, D, 100.0),
        (3, D, 80.0),
        (2, D, 50.0),
        (1, D, 30.0),
        (0, D, 0.0),
    ];
    for room in ROOMS {
        let layout = boxed(&ComputedBlockStyle::new(&ahem()), room, ["a", "bc", "d"]);
        let [ahead, behind] = both_ways(&layout);
        assert_eq!(ahead, forward, "{room:?}");
        assert_eq!(behind, backward, "{room:?}");
        let [right, left] = screen_ways(&layout, 0, 4);
        assert_eq!(right, forward, "{room:?}");
        assert_eq!(left, backward, "{room:?}");
        // The node reported is the one the caret is drawn beside.
        let nodes = [
            (Position::new(1, U), (1, 1)),
            (Position::new(1, D), (3, 0)),
            (Position::new(3, U), (3, 2)),
            (Position::new(3, D), (4, 0)),
        ];
        for (position, at) in nodes {
            assert_eq!(node(&layout, position), at, "{room:?}: {position:?}");
        }
    }
}

/// A position from a node is on that node's side of a box edge, whatever
/// affinity is asked for, and maps back to the node.
#[test]
fn a_position_from_a_node_keeps_its_side_of_a_box_edge() {
    let layout = boxed(
        &ComputedBlockStyle::new(&ahem()),
        Room::Padding,
        ["a", "bc", "d"],
    );
    for affinity in [U, D] {
        let cases = [
            (1, 1, Position::new(1, U)),
            (3, 0, Position::new(1, D)),
            (3, 2, Position::new(3, U)),
            (4, 0, Position::new(3, D)),
        ];
        for (key, offset, expected) in cases {
            let found = layout.position(NodeKey(key), offset, affinity).unwrap();
            assert_eq!(found, expected, "{key}@{offset} {affinity:?}");
            assert_eq!(node(&layout, found), (key, offset));
        }
    }
}

/// At a line's start or end only the side inside the line exists, so the
/// edge there is one position.
///
/// A box opening the text: room 0..10, `bc` 10..50, room 50..60, `d`
/// 60..80. A box closing it: `a` 0..20, room 20..30, `bc` 30..70, room
/// 70..80.
#[test]
fn a_box_edge_at_a_line_start_or_end_is_one_position() {
    let style = ahem();
    let block = ComputedBlockStyle::new(&style);
    let start = boxed(&block, Room::Padding, ["", "bc", "d"]);
    let [forward, backward] = both_ways(&start);
    assert_eq!(
        forward,
        [(0, D, 10.0), (1, U, 30.0), (2, U, 50.0), (3, U, 80.0)]
    );
    assert_eq!(
        backward,
        [(3, D, 80.0), (2, D, 60.0), (1, D, 30.0), (0, D, 10.0)]
    );
    assert_eq!(stop(&start, hit(&start, 5.0, 0)), (0, D, 10.0));
    assert_eq!(
        position_caret(&start, Position::new(0, U)),
        position_caret(&start, Position::new(0, D))
    );

    let end = boxed(&block, Room::Padding, ["a", "bc", ""]);
    let [forward, backward] = both_ways(&end);
    assert_eq!(
        forward,
        [(0, D, 0.0), (1, U, 20.0), (2, U, 50.0), (3, U, 70.0)]
    );
    assert_eq!(
        backward,
        [(3, D, 70.0), (2, D, 50.0), (1, D, 30.0), (0, D, 0.0)]
    );
    assert_eq!(stop(&end, hit(&end, 75.0, 0)), (3, U, 70.0));
    assert_eq!(
        position_caret(&end, Position::new(3, U)),
        position_caret(&end, Position::new(3, D))
    );
}

/// Nested boxes' edges at one boundary are crossed together, in one step.
///
/// `a` 0..20, room 20..40, `bc` 40..80, room 80..100, `d` 100..120.
#[test]
fn nested_box_edges_are_crossed_together() {
    let padded = with_room(ahem(), Room::Padding);
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "a");
        b.open_box(NodeKey(2), &padded, None);
        b.open_box(NodeKey(5), &padded, None);
        b.text(NodeKey(3), "bc");
        b.close_box();
        b.close_box();
        b.text(NodeKey(4), "d");
    });
    let [forward, backward] = both_ways(&layout);
    assert_eq!(
        forward,
        [
            (0, D, 0.0),
            (1, U, 20.0),
            (2, U, 60.0),
            (3, U, 80.0),
            (4, U, 120.0)
        ]
    );
    assert_eq!(
        backward,
        [
            (4, D, 120.0),
            (3, D, 100.0),
            (2, D, 60.0),
            (1, D, 40.0),
            (0, D, 0.0)
        ]
    );
    assert_eq!(stop(&layout, hit(&layout, 25.0, 0)), (1, U, 20.0));
    assert_eq!(stop(&layout, hit(&layout, 35.0, 0)), (1, D, 40.0));
    assert_eq!(node(&layout, Position::new(1, D)), (3, 0));
    assert_eq!(node(&layout, Position::new(3, D)), (4, 0));
}

/// An empty box holds no position. Its edges are one boundary, with a side
/// before the box and a side after it.
///
/// `a` 0..20, the box's room 20..40, `b` 40..60.
#[test]
fn an_empty_box_holds_no_position() {
    let layout = boxed(
        &ComputedBlockStyle::new(&ahem()),
        Room::Padding,
        ["a", "", "b"],
    );
    let [forward, backward] = both_ways(&layout);
    assert_eq!(forward, [(0, D, 0.0), (1, U, 20.0), (2, U, 60.0)]);
    assert_eq!(backward, [(2, D, 60.0), (1, D, 40.0), (0, D, 0.0)]);
    let [right, left] = screen_ways(&layout, 0, 2);
    assert_eq!(right, forward);
    assert_eq!(left, backward);
    assert_eq!(stop(&layout, hit(&layout, 28.0, 0)), (1, U, 20.0));
    assert_eq!(stop(&layout, hit(&layout, 32.0, 0)), (1, D, 40.0));
    assert_eq!(node(&layout, Position::new(1, U)), (1, 1));
    assert_eq!(node(&layout, Position::new(1, D)), (4, 0));
}

/// In a right-to-left paragraph the sides mirror: forward, which is leftward
/// on the screen, lands at the trailing edge of what it passed.
///
/// Alef, a box holding bet and gimel, and dalet, from the left of the line
/// box: dalet 0..20, room 20..30, gimel 30..50, bet 50..70, room 70..80,
/// alef 80..100. Each letter is two bytes.
#[test]
fn box_edges_in_a_right_to_left_paragraph_mirror() {
    let style = ahem();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let mut spanned = with_room(ahem(), Room::Padding);
    spanned.bidi.direction = Direction::Rtl;
    let layout = laid_with(&rtl, 100.0, |b| {
        b.text(NodeKey(1), "\u{5D0}");
        b.open_box(NodeKey(2), &spanned, None);
        b.text(NodeKey(3), "\u{5D1}\u{5D2}");
        b.close_box();
        b.text(NodeKey(4), "\u{5D3}");
    });
    let forward = vec![
        (0, D, 100.0),
        (2, U, 80.0),
        (4, U, 50.0),
        (6, U, 30.0),
        (8, U, 0.0),
    ];
    let backward = vec![
        (8, D, 0.0),
        (6, D, 20.0),
        (4, D, 50.0),
        (2, D, 70.0),
        (0, D, 100.0),
    ];
    let [ahead, behind] = both_ways(&layout);
    assert_eq!(ahead, forward);
    assert_eq!(behind, backward);
    let [right, left] = screen_ways(&layout, 8, 0);
    assert_eq!(right, backward);
    assert_eq!(left, forward);
    assert_eq!(stop(&layout, hit(&layout, 78.0, 0)), (2, U, 80.0));
    assert_eq!(stop(&layout, hit(&layout, 72.0, 0)), (2, D, 70.0));
    assert_eq!(stop(&layout, hit(&layout, 28.0, 0)), (6, U, 30.0));
    assert_eq!(stop(&layout, hit(&layout, 22.0, 0)), (6, D, 20.0));
    assert_eq!(node(&layout, Position::new(2, U)), (1, 2));
    assert_eq!(node(&layout, Position::new(2, D)), (3, 0));
    assert_eq!(node(&layout, Position::new(6, U)), (3, 4));
    assert_eq!(node(&layout, Position::new(6, D)), (4, 0));
}

/// Hebrew in a padded box in a left-to-right paragraph keeps each side of
/// its edges. The closing edge's inside is drawn at the run's left end, next
/// to the room before it; the opening edge's inside at its right end.
///
/// From the left: `a` 0..20, room 20..30, gimel 30..50, bet 50..70, room
/// 70..80, `d` 80..100.
#[test]
fn hebrew_in_a_left_to_right_box_keeps_each_side_of_its_edges() {
    let layout = boxed(
        &ComputedBlockStyle::new(&ahem()),
        Room::Padding,
        ["a", "\u{5D1}\u{5D2}", "d"],
    );
    assert_eq!(stop(&layout, Position::new(1, U)), (1, U, 20.0));
    assert_eq!(stop(&layout, Position::new(1, D)), (1, D, 70.0));
    assert_eq!(stop(&layout, Position::new(5, U)), (5, U, 30.0));
    assert_eq!(stop(&layout, Position::new(5, D)), (5, D, 80.0));
    assert_eq!(node(&layout, Position::new(5, U)), (3, 4));
    assert_eq!(node(&layout, Position::new(5, D)), (4, 0));
    // In text order the caret continues through the box's text.
    let [forward, backward] = both_ways(&layout);
    assert_eq!(
        forward,
        [
            (0, D, 0.0),
            (1, U, 20.0),
            (3, U, 50.0),
            (5, U, 30.0),
            (6, U, 100.0)
        ]
    );
    assert_eq!(
        backward,
        [
            (6, D, 100.0),
            (5, D, 80.0),
            (3, D, 50.0),
            (1, D, 70.0),
            (0, D, 0.0)
        ]
    );
    // On the screen it crosses each room once, landing on the side it left.
    let [right, left] = screen_ways(&layout, 0, 6);
    assert_eq!(
        right,
        [
            (0, D, 0.0),
            (1, U, 20.0),
            (3, D, 50.0),
            (1, D, 70.0),
            (6, U, 100.0)
        ]
    );
    assert_eq!(
        left,
        [
            (6, D, 100.0),
            (5, D, 80.0),
            (3, U, 50.0),
            (5, U, 30.0),
            (0, D, 0.0)
        ]
    );
    // A click in the room takes the nearer side, drawn where it is.
    assert_eq!(stop(&layout, hit(&layout, 22.0, 0)), (1, U, 20.0));
    assert_eq!(stop(&layout, hit(&layout, 28.0, 0)), (5, U, 30.0));
    assert_eq!(stop(&layout, hit(&layout, 72.0, 0)), (1, D, 70.0));
    assert_eq!(stop(&layout, hit(&layout, 78.0, 0)), (5, D, 80.0));
}
