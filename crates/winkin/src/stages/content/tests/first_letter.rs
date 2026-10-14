//! `::first-letter` tests:
//! - where the split falls, and the punctuation and space it takes;
//! - what the box holds, and how each side is transformed;
//! - what must come first for a letter to be found;
//! - which box is the block's initial letter.

use alloc::string::{String, ToString};

use super::*;
use crate::config::Config;
use crate::stages::lines::{Area, NoExclusions};
use crate::style::{InitialLetter, InitialLetterAlign, LineGroup, LineHeight, VerticalAlign};
use crate::tests::{AHEM_FAMILY, ahem_fallback, collection, sized};
use crate::{Context, Item as LineItem};

/// The first-letter box's key in these tests.
const LETTER: NodeKey = NodeKey(99);

#[test]
fn math_auto_first_letter_uses_the_source_node_length() {
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    assert_eq!(lettered(&math, "i"), "[𝑖]");
    assert_eq!(lettered(&math, "hi"), "[h]i");
    let layout = build(|b| {
        b.set_first_letter(LETTER, &math, None);
        b.text(key(1), "h");
        b.text(key(1), "i");
    });
    assert_eq!(shown(&layout), "[h]i");
    let plain = ComputedStyle::initial();
    let layout = build_with(
        &ComputedBlockStyle::new(&math),
        BuildOptions::default(),
        |b| {
            b.set_first_letter(LETTER, &plain, None);
            b.text(key(1), "hi");
        },
    )
    .0;
    assert_eq!(shown(&layout), "[h]i");
}

/// Returns the font size of `node`'s own text.
fn font_size(content: &Content, node: NodeId) -> f32 {
    node_request(content, node, FirstLineVariant::Standard)
        .font
        .size
}

/// Returns the content shown for `text_` as one text node, with a
/// `::first-letter` in `letter` set before it.
fn lettered(letter: &ComputedStyle<'_>, text_: &str) -> String {
    let layout = build(|b| {
        b.set_first_letter(LETTER, letter, None);
        b.text(key(1), text_);
    });
    shown(&layout)
}

/// The first letter takes the punctuation before it, one grapheme cluster,
/// and the punctuation after it except opening marks and dashes (CSS
/// Pseudo-Elements 4, section 2.2.1, as Blink's `FirstLetterLength` reads it).
#[test]
fn the_first_letter_takes_its_punctuation() {
    let plain = ComputedStyle::initial();
    let cases = [
        ("Once upon", "[O]nce upon"),
        ("\u{201C}Once upon", "[\u{201C}O]nce upon"),
        ("A\u{201D} said", "[A\u{201D}] said"),
        ("(A) thing", "[(A)] thing"),
        // A dash may open a first letter, and not close one.
        ("-A thing", "[-A] thing"),
        ("A- thing", "[A]- thing"),
        ("A\u{2014}B", "[A]\u{2014}B"),
        // A digit is a first letter, and a symbol.
        ("67 million", "[6]7 million"),
        ("\u{2605}x", "[\u{2605}]x"),
        // A grapheme cluster whole: a combining mark, a flag, a conjunct.
        ("E\u{301}tait", "[E\u{301}]tait"),
        ("\u{1F1EB}\u{1F1F7}flag", "[\u{1F1EB}\u{1F1F7}]flag"),
        ("\u{915}\u{94D}\u{937}a", "[\u{915}\u{94D}\u{937}]a"),
    ];
    for (written, expected) in cases {
        assert_eq!(lettered(&plain, written), expected, "{written:?}");
    }
}

/// A space separator other than U+3000 between punctuation and the letter
/// goes with the letter. After the letter it goes only if punctuation
/// follows; a word separator never goes. Chrome 153 follows CSS 2.1 and takes
/// no space (`“ Once` has no first letter there).
#[test]
fn space_goes_with_the_first_letter_between_punctuation() {
    let plain = ComputedStyle::initial();
    let cases = [
        ("\u{201C} Once", "[\u{201C} O]nce"),
        ("\u{201C}\u{2009}Once", "[\u{201C}\u{2009}O]nce"),
        ("A\u{2009}\u{201D} said", "[A\u{2009}\u{201D}] said"),
        ("A\u{2009}x", "[A]\u{2009}x"),
        ("A \u{201D} said", "[A] \u{201D} said"),
        ("A\u{A0}\u{201D} said", "[A]\u{A0}\u{201D} said"),
        // U+3000 between them: no first letter.
        ("\u{201C}\u{3000}Once", "\u{201C}\u{3000}Once"),
    ];
    for (written, expected) in cases {
        assert_eq!(lettered(&plain, written), expected, "{written:?}");
    }
}

/// Collapsed white space before the first letter is not part of it; kept
/// white space is, as Blink's `FirstLetterLength` counts it. A text of white
/// space alone leaves the letter to the next text. A kept segment break ends
/// the first line, so no letter follows it.
#[test]
fn kept_white_space_before_the_first_letter_is_part_of_it() {
    let plain = ComputedStyle::initial();
    assert_eq!(lettered(&plain, "   Once"), "[O]nce");
    let pre = white_space(WhiteSpaceCollapse::Preserve);
    let layout = build_with(
        &ComputedBlockStyle::new(&pre),
        BuildOptions::default(),
        |b| {
            b.set_first_letter(LETTER, &pre, None);
            b.text(key(1), "  Once");
        },
    )
    .0;
    assert_eq!(shown(&layout), "[  O]nce");
    let layout = build_with(
        &ComputedBlockStyle::new(&pre),
        BuildOptions::default(),
        |b| {
            b.set_first_letter(LETTER, &pre, None);
            b.text(key(1), "\nOnce");
        },
    )
    .0;
    assert_eq!(shown(&layout), "\nOnce");
    let layout = build(|b| {
        b.set_first_letter(LETTER, &plain, None);
        b.text(key(1), "   ");
        b.text(key(2), " Once");
    });
    assert_eq!(shown(&layout), "[O]nce");
}

/// The letter is the first-letter box's own text, under the box's key. The
/// rest of the text is a new node under the text's key, so the tree stays a
/// tree.
#[test]
fn the_first_letter_is_its_boxs_own_text() {
    let plain = ComputedStyle::initial();
    let layout = build(|b| {
        b.set_first_letter(LETTER, &plain, None);
        b.open_box(key(1), &plain, None);
        b.text(key(2), "Once");
        b.close_box();
    });
    assert_eq!(
        nodes(&layout),
        [
            (NodeKind::Block, 0),
            (NodeKind::Box, 1),
            (NodeKind::FirstLetter, 99),
            (NodeKind::Text, 2),
        ]
    );
    assert_eq!(
        items(&layout),
        [
            (ItemKind::Open, String::new(), 1),
            (ItemKind::Open, String::new(), 99),
            (ItemKind::Text, "O".to_string(), 99),
            (ItemKind::Close, String::new(), 99),
            (ItemKind::Text, "nce".to_string(), 2),
            (ItemKind::Close, String::new(), 1),
        ]
    );
    let nodes = &layout.content().nodes;
    assert_eq!(nodes.parent(NodeId::new(2)), NodeId::new(1));
    assert_eq!(nodes.parent(NodeId::new(3)), NodeId::new(1));
}

/// Punctuation ending a text leaves the letter to the next text, and the box
/// holds only the first text's part, as in Chrome. If no letter follows, the
/// box keeps the punctuation in the plain style.
#[test]
fn a_first_letter_across_texts_is_boxed_in_the_first() {
    let big = styled(|style| style.font.size = 48.0);
    let plain = ComputedStyle::initial();
    let layout = build(|b| {
        b.set_first_letter(LETTER, &big, None);
        b.open_box(key(1), &plain, None);
        b.text(key(2), "\u{201C}");
        b.close_box();
        b.set_first_letter(LETTER, &big, None);
        b.text(key(3), "Once");
    });
    assert_eq!(shown(&layout), "[[\u{201C}]]Once");
    let content = layout.content();
    assert_eq!(font_size(content, NodeId::new(2)), 48.0);

    // No letter comes: an atomic inline ends the search.
    let layout = build(|b| {
        b.set_first_letter(LETTER, &big, None);
        b.text(key(1), "\u{201C}");
        b.atomic(key(2), &plain, None, crate::BoxSize::default());
        b.text(key(3), "Once");
    });
    assert_eq!(shown(&layout), "[\u{201C}]{obj}Once");
    let content = layout.content();
    assert_eq!(font_size(content, NodeId::new(1)), 16.0);
}

/// The first letter comes first or not at all: none follows an atomic inline
/// or a forced break, and a request after text is ignored. A request repeated
/// before the letter takes the later style.
#[test]
fn the_first_letter_comes_first() {
    let plain = ComputedStyle::initial();
    let layout = build(|b| {
        b.set_first_letter(LETTER, &plain, None);
        b.atomic(key(1), &plain, None, crate::BoxSize::default());
        b.text(key(2), "Once");
    });
    assert_eq!(shown(&layout), "{obj}Once");
    let layout = build(|b| {
        b.set_first_letter(LETTER, &plain, None);
        b.line_break(key(1));
        b.text(key(2), "Once");
    });
    assert_eq!(shown(&layout), "{br}Once");
    let layout = build(|b| {
        b.text(key(1), "Once ");
        b.set_first_letter(LETTER, &plain, None);
        b.text(key(2), "upon");
    });
    assert_eq!(shown(&layout), "Once upon");
    let big = styled(|style| style.font.size = 48.0);
    let layout = build(|b| {
        b.set_first_letter(LETTER, &plain, None);
        b.text(key(1), " ");
        b.set_first_letter(LETTER, &big, None);
        b.text(key(2), "Once");
    });
    assert_eq!(shown(&layout), "[O]nce");
    let letter = layout
        .content()
        .nodes
        .nodes
        .iter()
        .find(|(_, node)| node.kind == NodeKind::FirstLetter)
        .map(|(node, _)| node);
    let letter = letter.unwrap_or(NodeId::BLOCK);
    assert_eq!(font_size(layout.content(), letter), 48.0);
}

/// The split is made in the source text, as in Chrome, and each side is
/// transformed in its own style:
/// - the letter in the pseudo-element's `text-transform`;
/// - `ß` as two capitals in its box;
/// - `capitalize` reads the letter as the character before the rest.
#[test]
fn each_side_of_the_first_letter_is_transformed_in_its_own_style() {
    use crate::style::TextCase::{Capitalize, Uppercase};
    let upper = cased(Uppercase, "de");
    assert_eq!(lettered(&upper, "\u{DF}ax"), "[SS]ax");
    let capitalized = cased(Capitalize, "en");
    let layout = build_with(
        &ComputedBlockStyle::new(&capitalized),
        BuildOptions::default(),
        |b| {
            b.set_first_letter(LETTER, &capitalized, None);
            b.text(key(1), "once upon");
        },
    )
    .0;
    assert_eq!(shown(&layout), "[O]nce Upon");
}

/// Under `::first-line`, the letter takes its own first-line style, whose
/// transform makes the first line's text.
#[test]
fn the_first_letter_takes_its_first_line_style() {
    use crate::style::TextCase::Uppercase;
    let plain = ComputedStyle::initial();
    let first_line = styled(|_| {});
    let upper = cased(Uppercase, "en");
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&first_line),
            ..ComputedBlockStyle::new(&plain)
        },
        BuildOptions::default(),
        |b| {
            b.set_first_letter(LETTER, &plain, Some(&upper));
            b.text(key(1), "once");
        },
    );
    assert_eq!(shown(&layout), "[o]nce");
    let (first, _) = first_line_text(&layout);
    assert_eq!(first, "Once");
    let content = layout.content();
    let letter = NodeId::new(1);
    assert_eq!(content.nodes.kind(letter), Some(NodeKind::FirstLetter));
    // The block's first line restyles nothing; the letter's does.
    assert!(content.flags.contains(ContentFlags::FIRST_LINE_RESTYLE));
}

/// Returns each node's kind in pre-order, and whether its font request sets
/// `initial-letter`.
fn initial_letters(layout: &Layout) -> Vec<(NodeKind, bool)> {
    let content = layout.content();
    content
        .nodes
        .nodes
        .iter()
        .map(|(id, node)| {
            let request = node_request(content, id, FirstLineVariant::Standard);
            (node.kind, request.initial_letter.is_set())
        })
        .collect()
}

/// Returns whether `node`'s font request on the first line sets
/// `initial-letter`.
fn initial_letter_on_first_line(layout: &Layout, node: NodeId) -> bool {
    let content = layout.content();
    node_request(content, node, FirstLineVariant::FirstLine)
        .initial_letter
        .is_set()
}

/// Returns a dropped initial letter three lines tall, with a
/// `vertical-align` that no initial letter uses.
fn dropped() -> ComputedStyle<'static> {
    styled(|style| {
        style.line.initial_letter = InitialLetter {
            size: 3.0,
            sink: 3,
            align: InitialLetterAlign::Alphabetic,
        };
        style.line.vertical_align = VerticalAlign::Px(5.0);
    })
}

/// The block has one initial letter: the inline box at its start that sets
/// `initial-letter` (CSS Inline 3, section 7.3.1), with `vertical-align:
/// baseline`. Elsewhere the builder interns the used value `normal`, so only
/// the initial letter's facts set it.
#[test]
fn only_the_box_at_the_blocks_start_is_its_initial_letter() {
    use NodeKind::{Block, Box as InlineBox, Text};
    let letter = dropped();
    let plain = ComputedStyle::initial();
    // The first box to set it is the initial letter, even after collapsed
    // white space and inside a box that opens first. The box inside it and
    // the box after it are not.
    let layout = build(|b| {
        b.text(key(1), "  ");
        b.open_box(key(2), &plain, None);
        b.open_box(key(3), &letter, None);
        b.open_box(key(4), &letter, None);
        b.text(key(5), "O");
        b.close_box();
        b.close_box();
        b.close_box();
        b.open_box(key(6), &letter, None);
        b.text(key(7), "nce");
        b.close_box();
    });
    assert_eq!(
        initial_letters(&layout),
        [
            (Block, false),
            (Text, false),
            (InlineBox, false),
            (InlineBox, true),
            (InlineBox, false),
            (Text, false),
            (InlineBox, false),
            (Text, false),
        ]
    );
    let content = layout.content();
    let taken = content
        .nodes
        .box_facts(NodeId::new(3), FirstLineVariant::Standard);
    assert_eq!(
        content.facts.box_facts(taken).align,
        VerticalAlign::Baseline
    );
    // After text or kept white space, no box is one.
    let pre = white_space(WhiteSpaceCollapse::Preserve);
    for (root, before) in [(&plain, "Once "), (&pre, " ")] {
        let (layout, _) = build_with(
            &ComputedBlockStyle::new(root),
            BuildOptions::default(),
            |b| {
                b.text(key(1), before);
                b.open_box(key(2), &letter, None);
                b.text(key(3), "upon");
                b.close_box();
            },
        );
        let letters = initial_letters(&layout);
        assert!(letters.iter().all(|&(_, set)| !set), "{before:?}");
    }
    // Nor is the block, which is not an inline box.
    let (layout, _) = build_with(
        &ComputedBlockStyle::new(&letter),
        BuildOptions::default(),
        |b| {
            b.text(key(1), "Once");
        },
    );
    assert!(initial_letters(&layout).iter().all(|&(_, set)| !set));
}

/// `::first-letter` opening the block is the initial letter, in its
/// first-line style too. Kept white space before the letter is inside its
/// box, so the box still opens the block.
#[test]
fn a_first_letter_is_the_initial_letter_where_it_opens_the_block() {
    use NodeKind::{Block, FirstLetter, Text};
    let letter = dropped();
    let first_line = styled(|_| {});
    let pre = white_space(WhiteSpaceCollapse::Preserve);
    let cases = [
        (ComputedStyle::initial(), "Once", true, 1),
        (pre, " Once", true, 1),
    ];
    for (root, text_, taken, at) in cases {
        let (layout, _) = build_with(
            &ComputedBlockStyle {
                first_line: Some(&first_line),
                ..ComputedBlockStyle::new(&root)
            },
            BuildOptions::default(),
            |b| {
                b.set_first_letter(LETTER, &letter, Some(&letter));
                b.text(key(1), text_);
            },
        );
        let letters = initial_letters(&layout);
        assert_eq!(letters[at], (FirstLetter, taken), "{text_:?}");
        for (node, &(kind, set)) in letters.iter().enumerate() {
            if node != at {
                assert!(matches!(kind, Block | Text) && !set, "{text_:?}");
            }
        }
        let first = initial_letter_on_first_line(&layout, NodeId::new(at));
        assert_eq!(first, taken, "{text_:?}");
    }
}

/// The first letter's box edges (inline, then block), the inline start of
/// the text keyed 1, and the first line's height.
type LetterSet = (Option<((f32, f32), (f32, f32))>, Option<f32>, Option<f32>);

/// Returns how the first letter and first line are set in Ahem at 10 px,
/// broken at 100 px.
fn letter_set(
    root: &ComputedStyle<'_>,
    letter: &ComputedStyle<'_>,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) -> LetterSet {
    let mut cx = Context::new(collection(&[], ahem_fallback()));
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let block = ComputedBlockStyle::new(root);
    let mut b = layout.builder(&mut cx, key(0), &block, BuildOptions::default());
    b.set_first_letter(LETTER, letter, None);
    calls(&mut b);
    assert!(b.finish().is_complete());
    layout.break_lines(&mut cx, Area::new(100.0), &mut NoExclusions);
    let piece = layout.box_fragments(LETTER).next().map(|piece| {
        let (inline, block) = (piece.inline(), piece.block());
        ((inline.left, inline.right), (block.over, block.under))
    });
    let after = layout.line(0).and_then(|line| {
        line.items()
            .filter_map(|item| match item {
                LineItem::Text(run) if run.key() == key(1) => Some(run.inline().left),
                _ => None,
            })
            .next()
    });
    let height = layout.line(0).map(|line| line.metrics().height());
    (piece, after, height)
}

/// A `::first-letter` that sets `initial-letter` is the initial letter after
/// an empty box, as at the block's start, and ignores its `vertical-align`.
/// After kept white space in a box it is not an initial letter, and its
/// `vertical-align` applies.
///
/// Chrome 153 on Windows sets the letter after an empty box as at the
/// block's start, in Ahem at 10 px on 10 px lines: the box 37 px wide
/// (DirectWrite's hinted ink; 35 unhinted) and the text after it at 36
/// (35). Chrome also makes the letter after a kept space an initial letter,
/// its box from 10 to 47; this crate does not.
#[test]
fn a_first_letter_after_an_empty_box_is_the_initial_letter() {
    let root = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(10.0),
            ..LineGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, 10.0)
    };
    let mut letter = root;
    letter.line.initial_letter = InitialLetter {
        size: 3.0,
        sink: 3,
        ..InitialLetter::NONE
    };
    letter.line.vertical_align = VerticalAlign::Px(10.0);
    let text_ = "X XX XX XX XX XX XX XX XX";
    let start = letter_set(&root, &letter, |b| b.text(key(1), text_));
    assert_eq!(
        start,
        (Some(((0.0, 35.0), (0.0, 35.0))), Some(35.0), Some(10.0))
    );
    let after_box = letter_set(&root, &letter, |b| {
        b.open_box(key(2), &root, None);
        b.close_box();
        b.text(key(1), text_);
    });
    assert_eq!(after_box, start);
    let mut pre = root;
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let after_space = letter_set(&pre, &letter, |b| {
        b.open_box(key(2), &pre, None);
        b.text(key(3), " ");
        b.close_box();
        b.text(key(1), text_);
    });
    // The letter's own 10 px, raised 10 px, makes the line 20 px tall.
    assert_eq!(
        after_space,
        (Some(((10.0, 20.0), (0.0, 10.0))), Some(20.0), Some(20.0))
    );
}
