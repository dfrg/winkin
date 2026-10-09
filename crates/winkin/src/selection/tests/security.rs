//! `-webkit-text-security` tests, against Chrome 155. They pin:
//! - one mask for each grapheme, after `text-transform`, with white space
//!   masked rather than collapsed;
//! - Chrome's mask characters;
//! - caret stops, hit testing and selections in the caller's offsets;
//! - word motion over masked text, which finds no words inside it.

use super::*;
use crate::style::{TextSecurity, TextTransform};

/// Returns the Ahem style masked by `security`.
fn masked(security: TextSecurity) -> ComputedStyle<'static> {
    styled(|style| {
        style.text.security = security;
        style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    })
}

/// Returns `text` as one text node masked by discs, broken at `width`.
fn laid_masked(text: &str, width: f32) -> Layout {
    laid_with(
        &ComputedBlockStyle::new(&masked(TextSecurity::Disc)),
        width,
        |b| b.text(NodeKey(1), text),
    )
}

/// Returns how many discs `layout` holds, checking it holds nothing else.
fn discs(layout: &Layout) -> usize {
    let text = layout.text();
    assert!(text.chars().all(|ch| ch == '\u{2022}'), "{text:?}");
    text.chars().count()
}

/// Returns where content offset `at` is in node 1's text, from `affinity`'s side.
fn source(layout: &Layout, at: usize, affinity: Affinity) -> usize {
    let position = layout.node_position(Position::new(at, affinity)).unwrap();
    assert_eq!(position.key, NodeKey(1));
    position.offset
}

/// Chrome draws one mask for each extended grapheme cluster, measured by the
/// masked text's width at 40px.
///
/// A combining sequence, a precomposed letter, an astral character, a ZWJ
/// sequence, a flag, a Hangul syllable of jamo, a Thai cluster, a Devanagari
/// conjunct and an emoji with its variation selector are each one mask, as
/// are an ideograph, a space and a tab.
#[test]
fn each_grapheme_is_one_mask() {
    for (text, masks) in [
        ("a", 1),
        ("e\u{301}", 1),
        ("\u{E9}", 1),
        ("\u{1F600}", 1),
        ("\u{1D465}", 1),
        ("\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}", 1),
        ("\u{1F1EF}\u{1F1F5}", 1),
        ("\u{6F22}", 1),
        (" ", 1),
        ("  ", 2),
        ("\t", 1),
        ("\u{1100}\u{1161}\u{11A8}", 1),
        ("\u{E01}\u{E33}", 1),
        ("\u{915}\u{94D}\u{937}\u{93F}", 1),
        ("\u{2764}\u{FE0F}", 1),
    ] {
        assert_eq!(discs(&laid_masked(text, 400.0)), masks, "{text:?}");
    }
}

/// Chrome masks the text before white space is processed: spaces neither
/// collapse nor trim, and a segment break is a mask on the same line.
#[test]
fn white_space_is_masked_not_collapsed() {
    let collapsing = styled(|style| style.text.security = TextSecurity::Disc);
    for (text, masks) in [("a  b", 4), (" a ", 3), ("a\nb", 3), ("a\tb", 3)] {
        let layout = laid_with(&ComputedBlockStyle::new(&collapsing), 400.0, |b| {
            b.text(NodeKey(1), text)
        });
        assert_eq!(discs(&layout), masks, "{text:?}");
        assert_eq!(layout.lines().count(), 1, "{text:?}");
    }
    let layout = laid_masked("a\nb", 400.0);
    assert_eq!((discs(&layout), layout.lines().count()), (3, 1));
}

/// Chrome transforms the text before masking it: `ß` in capitals is two
/// masks, and the `ﬁ` ligature in none is one.
#[test]
fn masks_follow_the_transform() {
    let upper = styled(|style| {
        style.text.security = TextSecurity::Disc;
        style.text.transform = TextTransform {
            case: TextCase::Uppercase,
            ..TextTransform::NONE
        };
    });
    let layout = laid_with(&ComputedBlockStyle::new(&upper), 400.0, |b| {
        b.text(NodeKey(1), "\u{DF}")
    });
    assert_eq!(discs(&layout), 2);
    let forward = MotionDirection::Forward.moving(Granularity::Character);
    assert_eq!(walk(&layout, 0, forward), [0, 6]);
    assert_eq!(source(&layout, 6, Affinity::Upstream), 2);
    assert_eq!(discs(&laid_masked("\u{FB01}", 400.0)), 1);
}

/// Each text node is masked on its own, as Chrome masks each `LayoutText`:
/// a grapheme a span splits is two masks.
#[test]
fn a_grapheme_split_across_nodes_is_two_masks() {
    let style = masked(TextSecurity::Disc);
    let layout = laid_with(&ComputedBlockStyle::new(&style), 400.0, |b| {
        b.text(NodeKey(1), "e");
        b.open_box(NodeKey(2), &style, None);
        b.text(NodeKey(3), "\u{301}");
        b.close_box();
    });
    assert_eq!(discs(&layout), 2);
}

/// Chrome's masks: a disc is U+2022, a circle U+25E6 and a square U+25A0,
/// as their widths in Arial show.
#[test]
fn each_keyword_masks_with_chromes_character() {
    for (security, mask) in [
        (TextSecurity::Disc, "\u{2022}\u{2022}"),
        (TextSecurity::Circle, "\u{25E6}\u{25E6}"),
        (TextSecurity::Square, "\u{25A0}\u{25A0}"),
        (TextSecurity::None, "ab"),
    ] {
        let layout = laid_with(&ComputedBlockStyle::new(&masked(security)), 400.0, |b| {
            b.text(NodeKey(1), "ab")
        });
        assert_eq!(layout.text(), mask);
    }
}

/// A mask is one caret stop, and its ends are the grapheme's ends in the
/// caller's text.
///
/// In Chrome's password field the arrow keys stop at UTF-16 offsets 0, 1, 3
/// and 4 of `ae\u{301}b`, and 0, 1, 3 and 4 of `a😀b`: the grapheme's ends.
#[test]
fn a_mask_is_one_caret_stop() {
    let text = "ae\u{301}\u{1F600}\u{1F468}\u{200D}\u{1F469}b";
    let layout = laid_masked(text, 400.0);
    assert_eq!(discs(&layout), 5);
    let forward = MotionDirection::Forward.moving(Granularity::Character);
    let stops = walk(&layout, 0, forward);
    assert_eq!(stops, [0, 3, 6, 9, 12, 15]);
    let sources: Vec<usize> = stops
        .iter()
        .map(|&at| source(&layout, at, Affinity::Downstream))
        .collect();
    assert_eq!(sources, [0, 1, 4, 8, 19, 20]);
    let backward = MotionDirection::Backward.moving(Granularity::Character);
    assert_eq!(walk(&layout, 15, backward), [15, 12, 9, 6, 3, 0]);
    // Every offset inside a grapheme maps onto a mask's end.
    for (offset, upstream, downstream) in [(2, 3, 6), (5, 6, 9), (12, 9, 12)] {
        let to = |affinity| {
            layout
                .position(NodeKey(1), offset, affinity)
                .unwrap()
                .offset
        };
        assert_eq!(to(Affinity::Upstream), upstream, "{offset}");
        assert_eq!(to(Affinity::Downstream), downstream, "{offset}");
    }
}

/// A selection of masks is a selection of the caller's graphemes, and the
/// masks highlight as wide as they are drawn.
#[test]
fn a_selection_of_masks_is_one_of_graphemes() {
    let layout = laid_masked("ae\u{301}\u{1F600}b", 400.0);
    let selection = Selection::new(Position::from(3), Position::from(9));
    let range = selection.range();
    let start = source(&layout, range.start, Affinity::Downstream);
    let end = source(&layout, range.end, Affinity::Upstream);
    assert_eq!((start, end), (1, 8));
    assert_eq!(
        rects(&layout, range),
        [(0, (20.0, 60.0), SelectionRectKind::Text)]
    );
}

/// A point hits the nearer end of the mask it is over.
#[test]
fn a_point_hits_a_masks_nearer_end() {
    let layout = laid_masked("ae\u{301}\u{1F600}b", 400.0);
    for (x, at, offset) in [
        (5.0, 0, 0),
        (25.0, 3, 1),
        (35.0, 6, 4),
        (55.0, 9, 8),
        (79.0, 12, 9),
    ] {
        let position = hit(&layout, x, 0);
        assert_eq!(position.offset, at, "{x}");
        assert_eq!(
            source(&layout, position.offset, position.affinity),
            offset,
            "{x}"
        );
    }
}

/// Word motion finds no words in masked text: Chrome's Ctrl+Right from
/// offset 0 or 1 of a masked `ab cd` textarea goes to its end, and Ctrl+Left
/// back to its start, under each keyword.
#[test]
fn word_motion_crosses_masked_text_whole() {
    for security in [
        TextSecurity::Disc,
        TextSecurity::Circle,
        TextSecurity::Square,
    ] {
        let layout = laid_with(&ComputedBlockStyle::new(&masked(security)), 400.0, |b| {
            b.text(NodeKey(1), "ab cd")
        });
        for word_motion in [WordMotion::SkipSpaces, WordMotion::StopAtWordEnd] {
            let forward = MotionDirection::Forward
                .moving(Granularity::Word)
                .with_word_motion(word_motion);
            let backward = MotionDirection::Backward
                .moving(Granularity::Word)
                .with_word_motion(word_motion);
            let case = (security, word_motion);
            assert_eq!(walk(&layout, 0, forward), [0, 15], "{case:?}");
            assert_eq!(walk(&layout, 3, forward), [3, 15], "{case:?}");
            assert_eq!(walk(&layout, 15, backward), [15, 0], "{case:?}");
        }
    }
}
