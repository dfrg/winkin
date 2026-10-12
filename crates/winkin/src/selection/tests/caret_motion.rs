//! Motion tests. They pin:
//! - character motion, one cluster or break opportunity a stop;
//! - left and right by the paragraph, as Chrome's arrows go;
//! - line motion keeping its column, and Home and End at a line's logical
//!   ends;
//! - an arrow on a selection collapsing it;
//! - motion on the screen where text reads one way, and word stops as each
//!   run reads.

use super::*;
use crate::selection::words::Words;

/// By character, a caret stops at every boundary, and twice at a wrap.
///
/// At a wrap after a space it stops before the space, then at the upper
/// line's end past it, then at the next line's start. Chrome stops once
/// after the space, at the next line's start. At the text's ends it stays.
#[test]
fn a_character_motion_crosses_one_cluster() {
    let layout = laid("abcd efgh ijkl", 100.0);
    let forward = walk(
        &layout,
        3,
        MotionDirection::Forward.moving(Granularity::Character),
    );
    assert_eq!(forward, [3, 4, 5, 5, 6, 7, 8, 9, 10, 10, 11, 12, 13, 14]);
    let positions = walk_positions(
        &layout,
        Position::from(3),
        MotionDirection::Forward.moving(Granularity::Character),
    );
    let carets: Vec<(usize, f32)> = positions
        .iter()
        .map(|&at| position_caret(&layout, at))
        .collect();
    assert_eq!(carets[1..4], [(0, 80.0), (0, 100.0), (1, 0.0)]);
    assert_eq!(
        positions[2..4],
        [
            Position::new(5, Affinity::Upstream),
            Position::new(5, Affinity::Downstream)
        ]
    );
    let backward = walk(
        &layout,
        14,
        MotionDirection::Backward.moving(Granularity::Character),
    );
    let mut reversed = forward.clone();
    reversed.reverse();
    assert_eq!(backward[..reversed.len()], reversed[..]);
    // Right is forward in a line read left to right, and left backward.
    let right = walk(
        &layout,
        3,
        MotionDirection::Right.moving(Granularity::Character),
    );
    assert_eq!(right, forward);
}

/// The two sides of a `<wbr>` are one stop each way.
///
/// Where the line wraps there, they are two places, and a motion stops at
/// each: forward at the upper line's end, then the lower line's start, and
/// backward the other way round. Chrome stops at one of them.
#[test]
fn a_break_opportunity_is_one_stop() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 80.0, |b| {
        b.text(NodeKey(1), "abcd");
        b.break_opportunity();
        b.text(NodeKey(1), "efgh");
    });
    assert_eq!(layout.text(), "abcd\u{200B}efgh");
    assert_eq!(layout.lines().len(), 2);
    let forward = walk(
        &layout,
        3,
        MotionDirection::Forward.moving(Granularity::Character),
    );
    assert_eq!(forward[..4], [3, 4, 7, 8]);
    assert_eq!(caret(&layout, 4), (0, 80.0));
    assert_eq!(caret(&layout, 7), (1, 0.0));
    let backward = walk(
        &layout,
        8,
        MotionDirection::Backward.moving(Granularity::Character),
    );
    assert_eq!(backward[..4], [8, 7, 4, 3]);
    // On one line the two sides are one place and one stop.
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "abcd");
        b.break_opportunity();
        b.text(NodeKey(1), "efgh");
    });
    let forward = walk(
        &layout,
        3,
        MotionDirection::Forward.moving(Granularity::Character),
    );
    assert_eq!(forward[..3], [3, 4, 8]);
}

/// In bidi text, left and right go to the nearest caret on the screen, unlike Chrome's arrows.
///
/// Chrome's arrows go through the text by the paragraph's direction, so the
/// caret jumps where the direction changes; here right is forward. Forward
/// stops at both places of each offset where the direction changes, where
/// Chrome stops once. Left and right stop at every place on the screen,
/// both edges of the right-to-left run included.
#[test]
fn left_and_right_go_the_way_they_point_and_chromes_arrows_by_the_paragraph() {
    let layout = laid("abc \u{5D0}\u{5D1}\u{5D2} def", 400.0);
    let chromes_right = chromes_arrow(&layout, Position::from(3), true);
    assert_eq!(chromes_right, MotionDirection::Forward);
    let right = walk(&layout, 3, chromes_right.moving(Granularity::Character));
    assert_eq!(right[..8], [3, 4, 4, 6, 8, 10, 10, 11]);
    let xs = |motion: Motion, from: usize| -> Vec<f32> {
        walk_positions(&layout, Position::from(from), motion)
            .iter()
            .map(|&at| position_caret(&layout, at).1)
            .collect()
    };
    let right = xs(MotionDirection::Right.moving(Granularity::Character), 3);
    assert_eq!(right[..6], [60.0, 80.0, 100.0, 120.0, 140.0, 160.0]);
    let left = xs(MotionDirection::Left.moving(Granularity::Character), 13);
    assert!(left.windows(2).all(|pair| pair[1] < pair[0]), "{left:?}");
    assert_eq!(left.len(), 11);
    assert_eq!(left.last(), Some(&0.0));
}

/// Up and down keep the column across a short line.
///
/// Past the last line they go to the text's end, and before the first to its
/// start. Any other motion forgets the column.
#[test]
fn line_motion_keeps_its_column() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "abcdefgh");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "ab");
        b.line_break(NodeKey(4));
        b.text(NodeKey(5), "abcdefgh");
    });
    let down = MotionDirection::Forward.moving(Granularity::Line);
    let up = MotionDirection::Backward.moving(Granularity::Line);
    let start = Selection::from(Position::from(6));
    let one = moved(&layout, start, down);
    assert_eq!(one.focus().offset, 11);
    let two = moved(&layout, one, down);
    assert_eq!(two.focus().offset, 18);
    assert_eq!(
        moved(&layout, moved(&layout, two, up), up).focus().offset,
        6
    );
    assert_eq!(moved(&layout, two, down).focus().offset, 20);
    assert_eq!(moved(&layout, start, up).focus().offset, 0);
    // A motion across the line forgets the column.
    let right = moved(
        &layout,
        one,
        MotionDirection::Right.moving(Granularity::Character),
    );
    assert_eq!(right.focus().offset, 12);
    assert_eq!(moved(&layout, right, down).focus().offset, 20);
}

/// Home and End go to a line's logical ends, as Chrome's do (probe 16g).
///
/// At a wrap, End goes past white space the wrap removed, upstream at the
/// wrap, so text typed there follows the space. Chrome's End stops before
/// it. After a preserved space hanging past a line, End is past it, as
/// Blink's `AdjustForSoftLineWrap` puts it.
#[test]
fn home_and_end_go_to_a_lines_logical_ends() {
    let home = MotionDirection::Backward.moving(Granularity::LineBoundary);
    let end = MotionDirection::Forward.moving(Granularity::LineBoundary);
    let layout = laid("abcd efgh ijkl", 100.0);
    let at = moved(&layout, Selection::from(Position::from(1)), end);
    assert_eq!(at.focus(), Position::new(5, Affinity::Upstream));
    assert_eq!(position_caret(&layout, at.focus()), (0, 100.0));
    assert_eq!(
        moved(&layout, Selection::from(Position::from(7)), home).focus(),
        Position::from(5)
    );
    let wrapping = styled(|style| style.text.white_space_collapse = WhiteSpaceCollapse::Preserve);
    let layout = laid_with(&ComputedBlockStyle::new(&wrapping), 100.0, |b| {
        b.text(NodeKey(1), "abcd efgh")
    });
    let at = moved(&layout, Selection::from(Position::from(1)), end);
    assert_eq!(at.focus(), Position::new(5, Affinity::Upstream));
    // A line starting in Hebrew: Home is its logical start, drawn at the
    // Hebrew's right edge, and Chrome's left arrow goes there, by the
    // paragraph's direction (probe 16g).
    let layout = laid("\u{5D0}\u{5D1}\u{5D2} abc def", 400.0);
    let at = moved(&layout, Selection::from(Position::from(10)), home);
    assert_eq!(at.focus().offset, 0);
    assert_eq!(position_caret(&layout, at.focus()), (0, 60.0));
    let chromes_left = chromes_arrow(&layout, Position::from(10), false);
    let at = moved(
        &layout,
        Selection::from(Position::from(10)),
        chromes_left.moving(Granularity::LineBoundary),
    );
    assert_eq!(at.focus().offset, 0);
    // Left on the screen: the line's left edge, the Hebrew's logical end,
    // upstream, drawn by the Hebrew at the edge, where Chrome draws its
    // caret on the Latin's side. Right: the line's right edge, its logical
    // end.
    let left = MotionDirection::Left.moving(Granularity::LineBoundary);
    let at = moved(&layout, Selection::from(Position::from(10)), left);
    assert_eq!(at.focus(), Position::new(6, Affinity::Upstream));
    let carets = layout.carets(at.focus()).unwrap();
    assert_eq!(carets.strong.inline, along(0.0, 0.0));
    assert_eq!(carets.weak.map(|weak| weak.inline), Some(along(60.0, 60.0)));
    let right = MotionDirection::Right.moving(Granularity::LineBoundary);
    let at = moved(&layout, Selection::from(Position::from(1)), right);
    assert_eq!(at.focus(), Position::new(14, Affinity::Upstream));
}

/// An arrow collapses a selection to the end it points at, without moving further (`selection.png`).
///
/// A word motion moves the focus, and extending moves only the focus. Left
/// and right collapse to the end drawn further that way. Chrome's arrows
/// collapse to the end they go to in the text.
#[test]
fn an_arrow_on_a_selection_collapses_it() {
    let layout = laid("hi there, big world", 400.0);
    let forward = Selection::new(Position::from(3), Position::from(8));
    let backward = Selection::new(Position::from(8), Position::from(3));
    for selection in [forward, backward] {
        let right = moved(
            &layout,
            selection,
            MotionDirection::Right.moving(Granularity::Character),
        );
        assert_eq!(right, Selection::from(Position::from(8)));
        let left = moved(
            &layout,
            selection,
            MotionDirection::Left.moving(Granularity::Character),
        );
        assert_eq!(left, Selection::from(Position::from(3)));
    }
    // In bidi text: inside the Hebrew, offset 6 is drawn at 120 and 8 at 100,
    // so right collapses a selection of the two to 6 and left to 8, where
    // Chrome's right arrow, on in the text, collapses it to 8, and its left
    // to 6.
    let bidi = laid("abc \u{5D0}\u{5D1}\u{5D2} def", 400.0);
    let across = Selection::new(Position::from(6), Position::from(8));
    let collapsed = |motion: MotionDirection| {
        moved(&bidi, across, motion.moving(Granularity::Character))
            .focus()
            .offset
    };
    assert_eq!(
        (
            collapsed(MotionDirection::Right),
            collapsed(MotionDirection::Left)
        ),
        (6, 8)
    );
    let chromes = |right: bool| chromes_arrow(&bidi, across.focus(), right);
    assert_eq!(
        (collapsed(chromes(true)), collapsed(chromes(false))),
        (8, 6)
    );
    let windows = WordMotion::SkipSpaces;
    let word = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(windows);
    assert_eq!(moved(&layout, forward, word).focus().offset, 10);
    assert_eq!(moved(&layout, backward, word).focus().offset, 8);
    let extend = MotionDirection::Forward
        .extending(Granularity::Word)
        .with_word_motion(windows);
    let once = moved(&layout, forward, extend);
    assert_eq!((once.anchor().offset, once.focus().offset), (3, 10));
    assert_eq!(moved(&layout, once, extend).range(), 3..14);
    let back = moved(
        &layout,
        forward,
        MotionDirection::Backward.extending(Granularity::Word),
    );
    assert!(back.is_collapsed());
    assert_eq!(back.focus().offset, 3);
}

/// Paragraphs that read one way throughout: Latin in a left-to-right block
/// and Hebrew and Arabic in a right-to-left one, with punctuation, spaces
/// collapsed and kept, `<wbr>`s, an atomic inline, forced breaks, a soft
/// hyphen and a ligature, on one line and wrapped, after a space, at a
/// `<wbr>`, at a soft hyphen and inside a word. Each with whether it reads
/// right to left.
fn one_way() -> Vec<(&'static str, bool, Layout)> {
    let plain = ahem();
    let preserved = styled(|style| style.text.white_space_collapse = WhiteSpaceCollapse::Preserve);
    let broken = styled(|style| style.text.word_break = WordBreak::BreakAll);
    let ltr = |style| ComputedBlockStyle::new(style);
    let rtl = |style| ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(style)
    };
    let size = BoxSize {
        inline: 20.0,
        block: 20.0,
        baseline: None,
    };
    let text = |text: &'static str| move |b: &mut LayoutBuilder<'_>| b.text(NodeKey(1), text);
    let wbr = |before: &'static str, after: &'static str| {
        move |b: &mut LayoutBuilder<'_>| {
            b.text(NodeKey(1), before);
            b.break_opportunity();
            b.text(NodeKey(1), after);
        }
    };
    let atomic = |before: &'static str, after: &'static str| {
        move |b: &mut LayoutBuilder<'_>| {
            b.text(NodeKey(1), before);
            b.atomic(NodeKey(2), &ahem(), None, size);
            b.text(NodeKey(3), after);
        }
    };
    let breaks = |first: &'static str, second: &'static str, third: &'static str| {
        move |b: &mut LayoutBuilder<'_>| {
            b.text(NodeKey(1), first);
            b.line_break(NodeKey(2));
            b.text(NodeKey(3), second);
            b.line_break(NodeKey(4));
            b.line_break(NodeKey(5));
            b.text(NodeKey(6), third);
        }
    };
    let hebrew = "\u{5D0}\u{5D1} \u{5D2}\u{5D3}\u{5D4}, \u{5D5}\u{5D6}!";
    let arabic = "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{627}\u{628}. \u{628}\u{644}\u{627}";
    let (letters, more) = ("\u{5D0}\u{5D1}\u{5D2}", "\u{5D3}\u{5D4} \u{5D5}");
    let spaced = "\u{5D0}\u{5D1}  \u{5D2}\u{5D3}  \u{5D4}";
    #[rustfmt::skip]
    let samples = vec![
        ("words", false, laid_with(&ltr(&plain), 400.0, text("hi there, big world"))),
        ("words wrapped", false, laid_with(&ltr(&plain), 100.0, text("hi there, big world"))),
        ("stops", false, laid_with(&ltr(&plain), 160.0, text("x.y 3.14 end! don't stop"))),
        ("wbr", false, laid_with(&ltr(&plain), 400.0, wbr("super", "cali fragile"))),
        ("wbr wrapped", false, laid_with(&ltr(&plain), 100.0, wbr("super", "cali fragile"))),
        ("atomic", false, laid_with(&ltr(&plain), 400.0, atomic("ab", "cd, ef"))),
        ("breaks", false, laid_with(&ltr(&plain), 400.0, breaks("ab", "cd ef", "gh"))),
        ("soft hyphen", false, laid_with(&ltr(&plain), 100.0, text("abcd\u{AD}efgh ij"))),
        ("kept spaces", false, laid_with(&ltr(&preserved), 100.0, text("abcd  efgh  ij"))),
        ("inside words", false, laid_with(&ltr(&broken), 80.0, text("abcdefghij kl"))),
        ("hebrew", true, laid_with(&rtl(&plain), 400.0, text(hebrew))),
        ("hebrew wrapped", true, laid_with(&rtl(&plain), 100.0, text(hebrew))),
        ("hebrew wbr", true, laid_with(&rtl(&plain), 400.0, wbr(letters, more))),
        ("hebrew wbr wrapped", true, laid_with(&rtl(&plain), 60.0, wbr(letters, more))),
        ("hebrew atomic", true, laid_with(&rtl(&plain), 400.0, atomic(letters, more))),
        ("hebrew breaks", true, laid_with(&rtl(&plain), 400.0, breaks(letters, more, letters))),
        ("hebrew kept spaces", true, laid_with(&rtl(&preserved), 60.0, text(spaced))),
        ("arabic", true, laid_with(&rtl(&plain), 400.0, text(arabic))),
        ("arabic wrapped", true, laid_with(&rtl(&plain), 100.0, text(arabic))),
    ];
    samples
}

/// Within a line that reads one way, left and right follow text order.
///
/// Left is backward and right forward in left-to-right text, and the
/// reverse in right-to-left text. That holds by character, by word with
/// either platform's stops, and to a line's ends, moving and extending. The
/// on-screen walk meets the same stops. RTL line crossings follow screen order.
#[test]
fn left_and_right_follow_text_order_within_a_line() {
    let granularities = [
        Granularity::Character,
        Granularity::Word,
        Granularity::LineBoundary,
    ];
    for (name, rtl, layout) in one_way() {
        // Left and right, and the text's order each is in this paragraph.
        let (left_is, right_is) = if rtl {
            (MotionDirection::Forward, MotionDirection::Backward)
        } else {
            (MotionDirection::Backward, MotionDirection::Forward)
        };
        let text = layout.text();
        let offsets: Vec<usize> = (0..=text.len())
            .filter(|&at| text.is_char_boundary(at))
            .collect();
        let middle = Position::from(offsets[offsets.len() / 2]);
        for &at in &offsets {
            for affinity in [Affinity::Downstream, Affinity::Upstream] {
                let position = Position::new(at, affinity);
                let selections = [Selection::from(position), Selection::new(middle, position)];
                for selection in selections {
                    for granularity in granularities {
                        for stops in [WordMotion::SkipSpaces, WordMotion::StopAtWordEnd] {
                            for extend in [false, true] {
                                let motion = |direction: MotionDirection| {
                                    let motion = if extend {
                                        direction.extending(granularity)
                                    } else {
                                        direction.moving(granularity)
                                    };
                                    motion.with_word_motion(stops)
                                };
                                for (screen, text_order) in [
                                    (MotionDirection::Left, left_is),
                                    (MotionDirection::Right, right_is),
                                ] {
                                    let mut expected =
                                        moved(&layout, selection, motion(text_order));
                                    let from = snapped(&layout, selection.focus());
                                    let collapses = granularity == Granularity::Character
                                        && !extend
                                        && !selection.is_collapsed();
                                    if rtl && !collapses && granularity != Granularity::LineBoundary
                                    {
                                        let right = screen == MotionDirection::Right;
                                        let to = if granularity == Granularity::Character {
                                            visual::character_on_screen(&layout, from, right)
                                        } else {
                                            visual::word_on_screen(&layout, from, right, stops)
                                        }
                                        .position;
                                        expected = Selection::new(
                                            if extend {
                                                snapped(&layout, selection.anchor()).position
                                            } else {
                                                to
                                            },
                                            to,
                                        );
                                    }
                                    assert_eq!(
                                        moved(&layout, selection, motion(screen)),
                                        expected,
                                        "{name}: {screen:?} as {text_order:?} by \
                                         {granularity:?}, {stops:?}, extending \
                                         {extend}, from {selection:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            // On the screen, a step or a word at a time.
            let from = snapped(&layout, Position::from(at));
            let lands = |position: Position| (position.offset, layout.caret(position));
            for right in [false, true] {
                let forward = right != rtl;
                let screen = visual::character_on_screen(&layout, from, right);
                assert_eq!(
                    lands(visual::by_character(&layout, from, right, rtl).position),
                    lands(screen.position),
                    "{name}: character fast path, right {right}, from {at}"
                );
                let logical = motion::by_character(&layout, from, forward);
                let stays = |to: Position| {
                    let from_caret = layout.caret(from.position).unwrap();
                    let to_caret = layout.caret(to).unwrap();
                    from_caret.line == to_caret.line
                        && if right {
                            to_caret.inline.left > from_caret.inline.right
                        } else {
                            to_caret.inline.right < from_caret.inline.left
                        }
                };
                if !rtl || (logical.offset() != from.offset() && stays(logical.position)) {
                    assert_eq!(
                        lands(screen.position),
                        lands(logical.position),
                        "{name}: character within line, right {right}, from {at}"
                    );
                }
                for stops in [WordMotion::SkipSpaces, WordMotion::StopAtWordEnd] {
                    let screen = visual::word_on_screen(&layout, from, right, stops);
                    assert_eq!(
                        lands(visual::by_word(&layout, from, right, rtl, stops).position),
                        lands(screen.position),
                        "{name}: word fast path, right {right}, {stops:?}, from {at}"
                    );
                    let logical = motion::by_word(&layout, from, forward, stops);
                    if !rtl
                        || (logical.offset() != from.offset()
                            && stays(logical.position)
                            && motion::is_word_stop(
                                &mut Words::new(&layout),
                                logical.cluster,
                                forward,
                                stops,
                            ))
                    {
                        assert_eq!(
                            lands(screen.position),
                            lands(logical.position),
                            "{name}: word within line, right {right}, {stops:?}, from {at}"
                        );
                    }
                }
            }
        }
    }
}

/// Screen motion crosses lines in screen order while logical motion keeps text order.
#[test]
fn rtl_line_edges_cross_right_down_and_left_up() {
    let mut style = ahem();
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut block = ComputedBlockStyle::new(&style);
    block.direction = BaseDirection::Auto;
    let layout = laid_with(&block, 400.0, |builder| {
        builder.text(NodeKey(1), "abc\nאבג\ndef");
    });
    let start = "abc\n".len();
    let end = "abc\nאבג".len();
    for extend in [false, true] {
        let motion = |direction: MotionDirection, granularity| {
            if extend {
                direction.extending(granularity)
            } else {
                direction.moving(granularity)
            }
        };
        let from = Selection::from(Position::from(start));
        let right = moved(
            &layout,
            from,
            motion(MotionDirection::Right, Granularity::Character),
        );
        assert_eq!(right.focus().offset, end + 1);
        assert_eq!(layout.caret(right.focus()).unwrap().line, 2);
        let logical = moved(
            &layout,
            from,
            motion(MotionDirection::Backward, Granularity::Character),
        );
        assert_eq!(layout.caret(logical.focus()).unwrap().line, 0);

        let from = Selection::from(Position::from(end));
        let left = moved(
            &layout,
            from,
            motion(MotionDirection::Left, Granularity::Character),
        );
        assert_eq!(left.focus().offset, 3);
        assert_eq!(layout.caret(left.focus()).unwrap().line, 0);
        let logical = moved(
            &layout,
            from,
            motion(MotionDirection::Forward, Granularity::Character),
        );
        assert_eq!(layout.caret(logical.focus()).unwrap().line, 2);

        for rule in [WordMotion::SkipSpaces, WordMotion::StopAtWordEnd] {
            let right = moved(
                &layout,
                Selection::from(Position::from(start)),
                motion(MotionDirection::Right, Granularity::Word).with_word_motion(rule),
            );
            assert_eq!(layout.caret(right.focus()).unwrap().line, 2);
            let left = moved(
                &layout,
                Selection::from(Position::from(end)),
                motion(MotionDirection::Left, Granularity::Word).with_word_motion(rule),
            );
            assert_eq!(left.focus().offset, 3);
            assert_eq!(layout.caret(left.focus()).unwrap().line, 0);
        }
    }
    style.text.word_break = WordBreak::BreakAll;
    let mut block = ComputedBlockStyle::new(&style);
    block.direction = BaseDirection::Rtl;
    let layout = laid_with(&block, 60.0, |builder| {
        builder.text(NodeKey(1), "אבג דהו");
    });
    assert_eq!(layout.lines().len(), 2);
    let right = moved(
        &layout,
        Selection::from(Position::from(0)),
        MotionDirection::Right.moving(Granularity::Character),
    );
    assert_eq!(right.focus().offset, layout.text().len());
    assert_eq!(layout.caret(right.focus()).unwrap().line, 1);
    let left = moved(
        &layout,
        right,
        MotionDirection::Left.moving(Granularity::Character),
    );
    assert_eq!(left.focus().offset, 0);
    assert_eq!(layout.caret(left.focus()).unwrap().line, 0);
}

/// Left and right by a word go on the screen, stopping in each run as a text-order word motion would.
///
/// Going the way a run reads, the motion's [`WordMotion`] decides; against
/// it, the stop is a word's start. So a right-to-left run swaps a word's
/// halves. The caret moves one way on the screen, where Chrome's arrows jump
/// about.
#[test]
fn a_word_on_the_screen_stops_as_each_run_reads() {
    let visited = |layout: &Layout, from: usize, direction: MotionDirection, stops| {
        let positions = walk_positions(
            layout,
            Position::from(from),
            direction.moving(Granularity::Word).with_word_motion(stops),
        );
        let offsets: Vec<usize> = positions.iter().map(|at| at.offset).collect();
        let xs: Vec<f32> = positions
            .iter()
            .map(|&at| position_caret(layout, at).1)
            .collect();
        (offsets, xs)
    };
    let one_way = |xs: &[f32], right: bool| {
        xs.windows(2)
            .all(|pair| (pair[1] > pair[0]) == right && pair[1] != pair[0])
    };
    use MotionDirection::{Left, Right};
    use WordMotion::{SkipSpaces, StopAtWordEnd};
    // Three Hebrew words in Latin: aleph-bet at 200 to 240, gimel-dalet at
    // 140 to 180 and he-vav at 80 to 120, their starts, 4, 9 and 14, drawn at
    // 240, 180 and 120. Offset 4 is also the Latin's end, upstream at 80, and
    // 18, the Hebrew's end, is drawn at 80 upstream and 240 downstream. Each
    // place is one word stop.
    let layout = laid(
        "abc \u{5D0}\u{5D1} \u{5D2}\u{5D3} \u{5D4}\u{5D5} def",
        400.0,
    );
    let cases = [
        (0, Right, SkipSpaces, vec![0, 4, 14, 9, 4, 19, 22]),
        (0, Right, StopAtWordEnd, vec![0, 3, 14, 9, 4, 22]),
        (22, Left, SkipSpaces, vec![22, 19, 4, 9, 14, 4, 0]),
        (22, Left, StopAtWordEnd, vec![22, 19, 8, 13, 18, 0]),
    ];
    for (from, direction, stops, expected) in cases {
        let (offsets, xs) = visited(&layout, from, direction, stops);
        assert_eq!(offsets, expected, "{direction:?}, {stops:?}");
        assert!(one_way(&xs, direction == Right), "{direction:?}: {xs:?}");
    }
    // Chrome's right arrow, on in the text: gimel-dalet before he-vav.
    let chromes_right = chromes_arrow(&layout, Position::from(0), true);
    let (offsets, xs) = visited(&layout, 0, chromes_right, SkipSpaces);
    assert_eq!(offsets, [0, 4, 9, 14, 19, 22]);
    assert!(!one_way(&xs, true), "{xs:?}");
    // Arabic before Latin: leaving the Arabic word to the right stops at its
    // start, its right edge, not past the Latin word; coming back left, at
    // the Latin word's start and then at the Arabic word's, which a change of
    // direction draws where the space's start is.
    let layout = laid("\u{628}\u{62A}\u{633} def", 400.0);
    let (offsets, _) = visited(&layout, 4, Right, SkipSpaces);
    assert_eq!(offsets[..3], [4, 0, 7]);
    let (offsets, _) = visited(&layout, 10, Left, SkipSpaces);
    assert_eq!(offsets[..3], [10, 7, 0]);
    // Latin in a right-to-left paragraph: its words' halves as it reads, left
    // to right. Offset 5, the Latin's start, is drawn at its left edge, 60,
    // and upstream at the Hebrew's end, 160, and a word stops at each.
    let plain = ahem();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&plain)
    };
    let layout = laid_with(&rtl, 400.0, |b| {
        b.text(NodeKey(1), "\u{5D0}\u{5D1} ab cd \u{5D2}\u{5D3}")
    });
    let cases = [
        (0, Left, SkipSpaces, vec![0, 5, 8, 5, 11, 15]),
        (0, Left, StopAtWordEnd, vec![0, 4, 8, 5, 15]),
        (15, Right, SkipSpaces, vec![15, 11, 5, 8, 5, 0]),
        (15, Right, StopAtWordEnd, vec![15, 11, 7, 10, 0]),
    ];
    for (from, direction, stops, expected) in cases {
        let (offsets, xs) = visited(&layout, from, direction, stops);
        assert_eq!(offsets, expected, "{direction:?}, {stops:?}");
        assert!(one_way(&xs, direction == Right), "{direction:?}: {xs:?}");
    }
}
