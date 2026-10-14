//! Seek tests, in `crate::work` seeks counted in debug builds. They pin:
//! - the searches each public call makes on prose, bidi text and ruby, none
//!   growing with the text;
//! - a ceiling on each call's searches.
//!
//! The layouts keep no offset map, whose searches are opt-in.

use super::*;
use crate::work;

/// The public calls a count is taken of, in the order [`seeks`] gives them.
const CALLS: [&str; 10] = [
    "caret",
    "hit test",
    "character forward",
    "character right",
    "word forward",
    "word right",
    "line down",
    "line end",
    "rects",
    "selected text",
];

/// Returns the seeks each of [`CALLS`] makes at byte `at` of `layout`.
///
/// The rectangles and the copy cover the 40 bytes from `at`.
fn seeks(layout: &Layout, at: usize) -> [u64; 10] {
    let position = Position::from(at);
    let caret = layout.caret(position).unwrap();
    let line = layout.line(caret.line).unwrap().metrics();
    let x = line.left + caret.inline.left;
    let y = line.top + line.height() / 2.0;
    let range = at..at + 40;
    let selection = Selection::from(position);
    let mut counts = [0; 10];
    let mut count = |call: usize, f: &dyn Fn()| {
        work::take_seeks();
        f();
        if let Some(count) = counts.get_mut(call) {
            *count = work::take_seeks();
        }
    };
    count(0, &|| {
        let _ = layout.caret(position);
    });
    count(1, &|| {
        let _ = layout.hit_test(x, y, PastLines::Column);
    });
    let motions = [
        MotionDirection::Forward.moving(Granularity::Character),
        MotionDirection::Right.moving(Granularity::Character),
        MotionDirection::Forward.moving(Granularity::Word),
        MotionDirection::Right.moving(Granularity::Word),
        MotionDirection::Forward.moving(Granularity::Line),
        MotionDirection::Forward.moving(Granularity::LineBoundary),
    ];
    for (call, motion) in (2..).zip(motions) {
        count(call, &|| {
            let _ = moved(layout, selection, motion);
        });
    }
    count(8, &|| {
        let _ = layout.selection_rects(range.clone()).count();
    });
    count(9, &|| {
        let _ = layout.selected_text(range.clone(), CopyKind::Text).count();
    });
    counts
}

/// Returns a layout of `calls` broken at `width`, with no offset map.
fn unmapped(width: f32, calls: impl FnOnce(&mut LayoutBuilder<'_>)) -> Layout {
    let mut cx = context();
    let mut layout = Layout::new();
    let style = ahem();
    let block = ComputedBlockStyle::new(&style);
    let mut builder = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
    calls(&mut builder);
    builder.finish();
    layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
    layout
}

/// The ragged article: ten words a sentence, `copies` times, at 900 px, and
/// a byte in its fifth sentence.
fn article(copies: usize) -> (Layout, usize) {
    let sentence = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ";
    let layout = unmapped(900.0, |b| b.text(NodeKey(1), &sentence.repeat(copies)));
    (layout, 4 * sentence.len() + 8)
}

/// Latin and Hebrew words, `copies` times, at 300 px, and a byte in the
/// Hebrew of the fifth.
fn bidi(copies: usize) -> (Layout, usize) {
    let words = "ab \u{5D0}\u{5D1}\u{5D2} cd ";
    let layout = unmapped(300.0, |b| b.text(NodeKey(1), &words.repeat(copies)));
    (layout, 4 * words.len() + 5)
}

/// Words around ruby bases under annotations, `copies` times, at 200 px, and
/// a byte in the fifth base.
fn ruby(copies: u64) -> (Layout, usize) {
    let small = styled(|style| style.font.size = 10.0);
    let layout = unmapped(200.0, |b| {
        for n in 0..copies {
            b.text(NodeKey(4 * n), "XX ");
            b.open_ruby(NodeKey(4 * n + 1), &ahem(), None);
            b.text(NodeKey(4 * n + 2), "AAAA BBBB");
            b.open_annotation(NodeKey(4 * n + 3), &small, None);
            b.text(NodeKey(4 * n + 3), "aaaa bbbb");
            b.close_ruby();
            b.text(NodeKey(4 * n), " ZZ ");
        }
    });
    let base = layout
        .text()
        .match_indices("AAAA")
        .nth(4)
        .map_or(0, |(at, _)| at);
    (layout, base + 2)
}

/// Each public selection call makes as many searches in a text four times as
/// long.
///
/// A call seeks where it enters and walks from there, so its searches do not
/// grow with the text. Seeks are counted in debug builds only.
#[test]
fn a_selection_call_searches_as_often_however_long_the_text() {
    let samples = [
        ("article", article(10), article(40)),
        ("bidi", bidi(20), bidi(80)),
        ("ruby", ruby(10), ruby(40)),
    ];
    for (name, (short, at), (long, same)) in samples {
        assert_eq!(at, same);
        let (short, long) = (seeks(&short, at), seeks(&long, at));
        for ((call, short), long) in CALLS.iter().zip(short).zip(long) {
            assert_eq!(short, long, "{name}: {call}");
        }
    }
}

/// Each public selection call stays within its count of searches.
///
/// - A caret seeks its cluster and its line, then a text run's font.
/// - A hit test finds the cluster in the run it hits.
/// - A motion seeks the focus's cluster. A motion forward by character or
///   word seeks whether a line starts at its stop. A screen motion seeks its
///   paragraph and the segments it crosses, and a walk on the screen each
///   caret's font. A word motion seeks its paragraph once.
/// - Rectangles seek their lines' ends and each text run's font. A copy seeks
///   its start.
/// - Ruby adds the search for an annotation's line.
///
/// Seeks are counted in debug builds only.
#[test]
fn a_selection_call_searches_no_more_than_it_does() {
    #[rustfmt::skip]
    let most: [(&str, (Layout, usize), [u64; 10]); 3] = [
        ("article", article(10), [3, 0, 2, 6, 3, 7, 3, 2, 5, 1]),
        ("bidi", bidi(20), [3, 0, 2, 8, 3, 14, 3, 2, 10, 1]),
        ("ruby", ruby(10), [8, 1, 2, 6, 3, 7, 8, 7, 27, 1]),
    ];
    for (name, (layout, at), most) in most {
        let counts = seeks(&layout, at);
        for ((call, count), most) in CALLS.iter().zip(counts).zip(most) {
            assert!(
                count <= most,
                "{name}: {call} seeks {count}, at most {most}"
            );
        }
    }
}
