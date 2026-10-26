//! Cost tests, in `crate::work` steps counted in debug builds. They pin:
//! - each query linear in its line;
//! - a word motion costing the distance it moves;
//! - a step by character inside a right-to-left line costing no walk of it.

use super::*;

/// Each query costs linear steps in its line, and a word motion in its paragraph.
///
/// Twice the line takes at most about twice the steps. Steps are counted in
/// debug builds only.
#[cfg(debug_assertions)]
#[test]
fn a_query_costs_linear_line_length() {
    use crate::work;
    // Words of Latin, or of Latin and Hebrew, which left and right walk on
    // the screen.
    for word in ["ab ", "ab \u{5D0}\u{5D1} "] {
        let steps = |words: usize| {
            let text = word.repeat(words);
            let layout = laid(&text, 1e6);
            assert_eq!(layout.lines().len(), 1);
            let len = layout.text().len();
            let middle = snapped(&layout, Position::from(len / 2));
            let _ = work::take();
            let _ = layout.caret(middle.position);
            let _ = hit(&layout, 30.0 * words as f32, 0);
            let granularities = [
                Granularity::Character,
                Granularity::Word,
                Granularity::Line,
                Granularity::LineBoundary,
            ];
            for granularity in granularities {
                for direction in [
                    MotionDirection::Forward,
                    MotionDirection::Left,
                    MotionDirection::Right,
                ] {
                    let _ = moved(
                        &layout,
                        Selection::from(middle.position),
                        direction.moving(granularity),
                    );
                }
            }
            // And the walk on the screen that text changing direction takes.
            for right in [false, true] {
                let _ = visual::character_on_screen(&layout, middle, right);
                let _ = visual::word_on_screen(&layout, middle, right, WordMotion::SkipSpaces);
            }
            let _ = layout.selection_rects(0..len).count();
            let _ = layout.selected_text(0..len, CopyKind::Clipboard).count();
            work::take()
        };
        let (small, large) = (steps(300), steps(600));
        assert!(small > 0);
        assert!(
            large <= small * 5 / 2,
            "{word:?}: {small} steps, then {large}"
        );
    }
}

/// A word motion costs the distance it moves, however long the paragraph or word.
///
/// - Across a long word, from its start, middle and end each way, and over
///   base64 end to end, twice the text takes at most about twice the steps.
/// - A spaceless Thai or ideograph run costs a motion one segmenting of the
///   run, in text order and on screen, never one per stop.
/// - Walking Thai and Japanese of short runs end to end costs its length.
///
/// Steps are counted in debug builds only.
#[cfg(debug_assertions)]
#[test]
fn a_word_motion_costs_the_distance_it_moves() {
    use crate::work;
    let directions = [
        MotionDirection::Forward,
        MotionDirection::Backward,
        MotionDirection::Left,
        MotionDirection::Right,
    ];
    let broken = styled(|style| style.text.word_break = WordBreak::BreakAll);
    let laid_broken = |text: &str| {
        laid_with(&ComputedBlockStyle::new(&broken), 400.0, |b| {
            b.text(NodeKey(1), text)
        })
    };
    // Every word motion from `text`'s start, its middle and its end, and
    // the walk on the screen that text changing direction takes.
    let across = |text: &str| {
        let layout = laid_broken(text);
        let len = layout.text().len();
        let _ = work::take();
        for from in [0, len / 2, len] {
            let from = snapped(&layout, Position::from(from));
            for direction in directions {
                for stops in [WordMotion::SkipSpaces, WordMotion::StopAtWordEnd] {
                    let motion = direction.moving(Granularity::Word).with_word_motion(stops);
                    let _ = moved(&layout, Selection::from(from.position), motion);
                }
            }
            for right in [false, true] {
                let _ = visual::word_on_screen(&layout, from, right, WordMotion::SkipSpaces);
            }
        }
        work::take()
    };
    // One word of `letters` letters, crossed whole or half.
    let letters = |letters: usize| {
        let text: String = (0..letters)
            .map(|at| char::from(b'a' + (at % 26) as u8))
            .collect();
        across(&text)
    };
    let (small, large) = (letters(10_000), letters(20_000));
    assert!(small > 0);
    assert!(large <= small * 5 / 2, "{small} steps, then {large}");
    // "Thai" and "Chinese characters" with nothing between them, and Tai
    // Tham. No dictionary has Tai Tham, so a run is one word however long.
    // A motion crosses it a stop at a time.
    for word in [
        "\u{E44}\u{E17}\u{E22}",
        "\u{6F22}\u{5B57}",
        "\u{1A20}\u{1A21}",
    ] {
        let (small, large) = (across(&word.repeat(100)), across(&word.repeat(200)));
        assert!(small > 0);
        assert!(
            large <= small * 5 / 2,
            "{word}: {small} steps, then {large}"
        );
    }
    // A run of base64, its `+` and `/` and `=` words of their own, and Thai
    // and Japanese of short runs between spaces and marks of punctuation,
    // walked by words end to end, each way.
    let walked = |text: &str, copies: usize| {
        let layout = laid_broken(&text.repeat(copies));
        let len = layout.text().len();
        let _ = work::take();
        for direction in directions {
            let from = match direction {
                MotionDirection::Forward | MotionDirection::Right => 0,
                MotionDirection::Backward | MotionDirection::Left => len,
            };
            let stops = walk(&layout, from, direction.moving(Granularity::Word));
            assert!(stops.len() > copies, "{direction:?}: {} stops", stops.len());
        }
        work::take()
    };
    // The Thai sentence's first clause, which ends in a space.
    // Fewer copies of the Thai and the Japanese: the dictionaries, slow in
    // a debug build, segment their runs at each motion.
    let clause = THAI.split_inclusive(' ').next().unwrap_or(THAI);
    for (text, copies) in [
        ("TWFuIGlz+ZGlzdGluZ3Vp/c2hlZA==", 300),
        (clause, 10),
        (JAPANESE, 10),
    ] {
        let (small, large) = (walked(text, copies), walked(text, 2 * copies));
        assert!(small > 0);
        assert!(
            large <= small * 5 / 2,
            "{text}: {small} steps, then {large}"
        );
    }
}

/// A step by character on the screen inside a line of Arabic costs the same
/// however long the line.
///
/// Where the line reads one way, the step is found in text order and
/// neither caret is placed, so the line is not walked. Sixteen steps right
/// and sixteen back left from the middle of a line twice as long take the
/// same steps and seeks. So do sixteen steps right from the text's start,
/// the right end of its only line, where the caret stays. Steps and seeks
/// are counted in debug builds only.
#[cfg(debug_assertions)]
#[test]
fn a_character_step_inside_an_arabic_line_costs_the_same_however_long_the_line() {
    use crate::work;
    let style = ahem();
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let word = "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{627}\u{628}. ";
    let cost = |words: usize| {
        let text = word.repeat(words);
        let layout = laid_with(&block, 1e6, |b| b.text(NodeKey(1), &text));
        assert_eq!(layout.lines().len(), 1);
        let middle = snapped(&layout, Position::from(text.len() / 2)).position;
        let _ = (work::take(), work::take_seeks());
        let mut selection = Selection::from(middle);
        for direction in [MotionDirection::Right, MotionDirection::Left] {
            for _ in 0..16 {
                selection.modify(&layout, direction.moving(Granularity::Character));
            }
        }
        // Left is forward here, so the caret lands upstream, with the text it
        // passed.
        assert_eq!(
            selection.focus(),
            Position::new(middle.offset, Affinity::Upstream)
        );
        let start = Position::from(0);
        let mut selection = Selection::from(start);
        for _ in 0..16 {
            selection.modify(
                &layout,
                MotionDirection::Right.moving(Granularity::Character),
            );
        }
        assert_eq!(selection.focus(), start);
        (work::take(), work::take_seeks())
    };
    let ((small, small_seeks), (large, large_seeks)) = (cost(100), cost(200));
    assert!(small > 0);
    assert_eq!(large, small, "steps");
    assert!(
        large_seeks <= small_seeks + 32,
        "{small_seeks} seeks, then {large_seeks}"
    );
}
