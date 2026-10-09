//! Offset map tests:
//! - the units the writer records, for collapsed, dropped and kept white
//!   space;
//! - transforms that change a length, and `full-size-kana`;
//! - nodes answering by their keys, atomic inlines, breaks and break
//!   opportunities;
//! - the first letter's box;
//! - round trips from the content's text to the caller's and back.
//!
//! Collapsed white space maps as Blink's `offset_mapping_test.cc` maps it.

use alloc::vec::Vec;

use super::*;
use crate::stages::content::map::{MapSide, MapUnitKind};

use MapUnitKind::{Collapsed as C, Generated as G, Identity as I, Variable as V};

/// Builds a layout by `calls` under `root`, recording the offset map.
fn mapped_with(root: &ComputedStyle<'_>, calls: impl FnOnce(&mut LayoutBuilder<'_>)) -> Layout {
    let options = BuildOptions {
        map_source: true,
        ..BuildOptions::default()
    };
    build_with(&ComputedBlockStyle::new(root), options, calls).0
}

/// Builds a layout by `calls` under the initial style, recording the map.
fn mapped(calls: impl FnOnce(&mut LayoutBuilder<'_>)) -> Layout {
    mapped_with(&ComputedStyle::initial(), calls)
}

/// Returns the map's units: kind, content range, source range.
fn units(layout: &Layout) -> Vec<(MapUnitKind, Range<usize>, Range<u32>)> {
    let content = layout.content();
    let extras = content.extras.as_deref();
    extras.map_or_else(Vec::new, |extras| extras.map.listed(content.text.len()))
}

/// Returns where content offset `at` is in the caller's text, from `side`.
fn source(layout: &Layout, at: usize, side: MapSide) -> Option<(u64, u32)> {
    let map = layout.content().offset_map()?;
    map.source_position(TextOffset::new(at), side)
        .map(|(key, offset)| (key.0, offset))
}

/// Returns where offset `offset` of node `key`'s text is in the content's
/// text.
fn content_offset(layout: &Layout, key_: u64, offset: u32, side: MapSide) -> Option<usize> {
    let map = layout.content().offset_map()?;
    map.content_offset(key(key_), offset, side)
        .map(TextOffset::get)
}

use MapSide::{After, Before};

#[test]
fn math_auto_maps_each_source_character_as_one_stop() {
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    let layout = mapped_with(&math, |b| {
        b.text(key(1), "h");
        b.text(key(2), "i");
        b.text(key(3), "α");
        b.text(key(4), "hi");
    });
    assert_eq!(text(&layout), "ℎ𝑖𝛼hi");
    assert_eq!(
        units(&layout),
        [
            (V, 0..3, 0..1),
            (V, 3..7, 0..1),
            (V, 7..11, 0..2),
            (I, 11..13, 0..2)
        ]
    );
    for (key, from, to, len) in [(1, 0, 3, 1), (2, 3, 7, 1), (3, 7, 11, 2)] {
        assert_eq!(content_offset(&layout, key, 0, After), Some(from));
        assert_eq!(content_offset(&layout, key, len, Before), Some(to));
        for at in from + 1..to {
            assert_eq!(source(&layout, at, Before), Some((key, 0)));
            assert_eq!(source(&layout, at, After), Some((key, len)));
        }
    }
}

/// A plain document is one unit per text node. Nothing collapses or
/// transforms, and a lone space stays one unit.
#[test]
fn an_untouched_document_is_one_unit() {
    let layout = mapped(|b| b.text(key(1), "nothing to collapse here"));
    assert_eq!(units(&layout), [(I, 0..24, 0..24)]);
    for at in 0..=24 {
        for side in [Before, After] {
            assert_eq!(source(&layout, at, side), Some((1, at as u32)));
            assert_eq!(content_offset(&layout, 1, at as u32, side), Some(at));
        }
    }
}

/// Without `map_source` nothing is recorded, and there is no map to ask.
#[test]
fn the_map_is_recorded_only_where_asked() {
    let layout = build(|b| b.text(key(1), "a  b"));
    assert!(layout.content().offset_map().is_none());
    assert!(units(&layout).is_empty());
}

/// A collapsed run keeps its first character as the space. The rest maps to
/// after it, as Blink's `OneTextNodeWithCollapsedSpace` has it.
#[test]
fn a_collapsed_run_is_its_first_character_and_nothing() {
    let layout = mapped(|b| b.text(key(1), "one     two"));
    assert_eq!(text(&layout), "one two");
    assert_eq!(
        units(&layout),
        [(I, 0..4, 0..4), (C, 4..4, 4..8), (I, 4..7, 8..11)]
    );
    // Into the content: the run past its first character is after the space.
    assert_eq!(content_offset(&layout, 1, 3, After), Some(3));
    for offset in 4..=8 {
        assert_eq!(
            content_offset(&layout, 1, offset, After),
            Some(4),
            "{offset}"
        );
    }
    assert_eq!(content_offset(&layout, 1, 9, Before), Some(5));
    // Out of it: after the space, the run's two ends.
    assert_eq!(source(&layout, 4, Before), Some((1, 4)));
    assert_eq!(source(&layout, 4, After), Some((1, 8)));
    assert_eq!(source(&layout, 5, Before), Some((1, 9)));
}

/// White space dropped at the block's start maps to where the text starts.
#[test]
fn white_space_dropped_maps_to_where_the_text_starts() {
    let layout = mapped(|b| b.text(key(1), "   leading"));
    assert_eq!(text(&layout), "leading");
    assert_eq!(units(&layout), [(C, 0..0, 0..3), (I, 0..7, 3..10)]);
    for offset in 0..=3 {
        assert_eq!(content_offset(&layout, 1, offset, Before), Some(0));
    }
    assert_eq!(source(&layout, 0, Before), Some((1, 0)));
    assert_eq!(source(&layout, 0, After), Some((1, 3)));
    // And at the block's end, after the text.
    let layout = mapped(|b| b.text(key(1), "trailing  "));
    assert_eq!(units(&layout), [(I, 0..8, 0..8), (C, 8..8, 8..10)]);
    assert_eq!(source(&layout, 8, Before), Some((1, 8)));
    assert_eq!(source(&layout, 8, After), Some((1, 10)));
    assert_eq!(content_offset(&layout, 1, 10, After), Some(8));
}

/// A space owed across a box edge is the unit of the white space it came
/// from. The white space after the edge collapses to nothing.
#[test]
fn a_space_owed_across_an_edge_is_its_own_nodes() {
    let layout = mapped(|b| {
        b.text(key(1), "one  ");
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "  two");
        b.close_box();
    });
    assert_eq!(text(&layout), "one two");
    assert_eq!(
        units(&layout),
        [
            (I, 0..4, 0..4),
            (C, 4..4, 4..5),
            (C, 4..4, 0..2),
            (I, 4..7, 2..5)
        ]
    );
    assert_eq!(source(&layout, 3, After), Some((1, 3)));
    assert_eq!(source(&layout, 4, Before), Some((1, 4)));
    assert_eq!(source(&layout, 4, After), Some((3, 2)));
    assert_eq!(content_offset(&layout, 1, 5, Before), Some(4));
    assert_eq!(content_offset(&layout, 3, 0, Before), Some(4));
    assert_eq!(content_offset(&layout, 3, 1, Before), Some(4));
}

/// A transform that keeps each character's length maps one to one. One that
/// does not is one caret stop: an offset inside it maps to an end.
#[test]
fn a_transform_that_changes_a_length_is_one_stop() {
    let upper = cased(TextCase::Uppercase, "en");
    let layout = mapped_with(&upper, |b| b.text(key(1), "Stra\u{DF}e ab"));
    assert_eq!(text(&layout), "STRASSE AB");
    // `ß` is two bytes, and so is `SS`, but two characters: one unit.
    assert_eq!(
        units(&layout),
        [(I, 0..4, 0..4), (V, 4..6, 4..6), (I, 6..10, 6..10)]
    );
    assert_eq!(source(&layout, 5, Before), Some((1, 4)));
    assert_eq!(source(&layout, 5, After), Some((1, 6)));
    assert_eq!(content_offset(&layout, 1, 4, After), Some(4));
    assert_eq!(content_offset(&layout, 1, 6, Before), Some(6));
    let map = layout.content().offset_map().unwrap();
    assert_eq!(
        map.variable_around(TextOffset::new(5)),
        Some(TextOffset::new(4)..TextOffset::new(6))
    );
    assert_eq!(map.variable_around(TextOffset::new(4)), None);
    // ASCII in capitals is the text as given.
    let layout = mapped_with(&upper, |b| b.text(key(1), "plain words"));
    assert_eq!(units(&layout), [(I, 0..11, 0..11)]);
    // A full-width letter is three bytes of one.
    let wide = styled(|style| {
        style.text.transform = TextTransform {
            full_width: true,
            ..TextTransform::NONE
        }
    });
    let layout = mapped_with(&wide, |b| b.text(key(1), "ab"));
    assert_eq!(units(&layout), [(V, 0..3, 0..1), (V, 3..6, 1..2)]);
    assert_eq!(content_offset(&layout, 1, 1, Before), Some(3));
}

/// CSS Text 3's table of small kana and their full-size forms (Appendix G,
/// "Small Kana Mappings"), as the editor's draft lists it.
///
/// It holds the hiragana, the katakana and the half-width katakana, 58 in
/// all. Blink's `Character::FullSizeKanaVariant` holds the same 58.
const SMALL_KANA: [(char, char); 58] = [
    ('\u{3041}', '\u{3042}'),
    ('\u{3043}', '\u{3044}'),
    ('\u{3045}', '\u{3046}'),
    ('\u{3047}', '\u{3048}'),
    ('\u{3049}', '\u{304A}'),
    ('\u{3095}', '\u{304B}'),
    ('\u{3096}', '\u{3051}'),
    ('\u{1B132}', '\u{3053}'),
    ('\u{3063}', '\u{3064}'),
    ('\u{3083}', '\u{3084}'),
    ('\u{3085}', '\u{3086}'),
    ('\u{3087}', '\u{3088}'),
    ('\u{308E}', '\u{308F}'),
    ('\u{1B150}', '\u{3090}'),
    ('\u{1B151}', '\u{3091}'),
    ('\u{1B152}', '\u{3092}'),
    ('\u{30A1}', '\u{30A2}'),
    ('\u{30A3}', '\u{30A4}'),
    ('\u{30A5}', '\u{30A6}'),
    ('\u{30A7}', '\u{30A8}'),
    ('\u{30A9}', '\u{30AA}'),
    ('\u{30F5}', '\u{30AB}'),
    ('\u{31F0}', '\u{30AF}'),
    ('\u{30F6}', '\u{30B1}'),
    ('\u{1B155}', '\u{30B3}'),
    ('\u{31F1}', '\u{30B7}'),
    ('\u{31F2}', '\u{30B9}'),
    ('\u{30C3}', '\u{30C4}'),
    ('\u{31F3}', '\u{30C8}'),
    ('\u{31F4}', '\u{30CC}'),
    ('\u{31F5}', '\u{30CF}'),
    ('\u{31F6}', '\u{30D2}'),
    ('\u{31F7}', '\u{30D5}'),
    ('\u{31F8}', '\u{30D8}'),
    ('\u{31F9}', '\u{30DB}'),
    ('\u{31FA}', '\u{30E0}'),
    ('\u{30E3}', '\u{30E4}'),
    ('\u{30E5}', '\u{30E6}'),
    ('\u{30E7}', '\u{30E8}'),
    ('\u{31FB}', '\u{30E9}'),
    ('\u{31FC}', '\u{30EA}'),
    ('\u{31FD}', '\u{30EB}'),
    ('\u{31FE}', '\u{30EC}'),
    ('\u{31FF}', '\u{30ED}'),
    ('\u{30EE}', '\u{30EF}'),
    ('\u{1B164}', '\u{30F0}'),
    ('\u{1B165}', '\u{30F1}'),
    ('\u{1B166}', '\u{30F2}'),
    ('\u{1B167}', '\u{30F3}'),
    ('\u{FF67}', '\u{FF71}'),
    ('\u{FF68}', '\u{FF72}'),
    ('\u{FF69}', '\u{FF73}'),
    ('\u{FF6A}', '\u{FF74}'),
    ('\u{FF6B}', '\u{FF75}'),
    ('\u{FF6F}', '\u{FF82}'),
    ('\u{FF6C}', '\u{FF94}'),
    ('\u{FF6D}', '\u{FF95}'),
    ('\u{FF6E}', '\u{FF96}'),
];

/// `full-size-kana` maps every small kana of CSS Text 3's table to its
/// full-size form, and every other Unicode scalar value to itself.
///
/// A small kana that `full-width` makes full-width is then made full-size,
/// as CSS orders the two and Blink applies them. A supplementary small kana
/// of four UTF-8 bytes becomes three bytes, one character for one, and the
/// offset map finds it.
#[test]
fn full_size_kana_is_css_text_3s_table() {
    for code in 0..=0x10_FFFF_u32 {
        let Some(ch) = char::from_u32(code) else {
            continue;
        };
        let want = SMALL_KANA
            .iter()
            .find(|&&(small, _)| small == ch)
            .map_or(ch, |&(_, full)| full);
        assert_eq!(
            super::super::transform::full_size_kana(ch),
            want,
            "U+{code:04X}"
        );
    }
    let kana = |full_width: bool, case: TextCase| {
        styled(|style| {
            style.text.transform = TextTransform {
                case,
                full_width,
                full_size_kana: true,
            };
        })
    };
    assert_eq!(
        transformed(&kana(false, TextCase::None), "\u{3063}\u{30E3}\u{1B132}a"),
        "\u{3064}\u{30E4}\u{3053}a"
    );
    // Half-width small katakana: full-size alone, and full-width first.
    assert_eq!(
        transformed(&kana(false, TextCase::None), "\u{FF67}\u{FF6F}"),
        "\u{FF71}\u{FF82}"
    );
    assert_eq!(
        transformed(&kana(true, TextCase::Uppercase), "a\u{FF67}\u{FF6F}"),
        "\u{FF21}\u{30A2}\u{30C4}"
    );
    let layout = mapped_with(&kana(false, TextCase::None), |b| {
        b.text(key(1), "\u{1B132}x")
    });
    assert_eq!(text(&layout), "\u{3053}x");
    assert_eq!(units(&layout), [(V, 0..3, 0..4), (I, 3..4, 4..5)]);
}

/// Greek capitals drop their accents in context, which each letter alone
/// does not. The span whose characters do not add up is one unit.
#[test]
fn a_transform_in_context_is_one_unit_where_it_must_be() {
    let upper = cased(TextCase::Uppercase, "el");
    let layout = mapped_with(&upper, |b| b.text(key(1), "\u{3AC}\u{3BB}\u{3C6}\u{3B1}"));
    let listed = units(&layout);
    let covered: u32 = listed
        .iter()
        .map(|(_, _, source)| source.len() as u32)
        .sum();
    assert_eq!(
        covered, 8,
        "every byte of the caller's is mapped: {listed:?}"
    );
    assert_eq!(
        content_offset(&layout, 1, 8, Before),
        Some(text(&layout).len())
    );
}

/// Nodes answer by their keys, each counting its own text.
#[test]
fn the_map_answers_in_nodes() {
    let layout = mapped(|b| {
        b.text(key(1), "first");
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "second");
        b.close_box();
        b.text(key(4), "third");
    });
    assert_eq!(text(&layout), "firstsecondthird");
    assert_eq!(source(&layout, 5, Before), Some((1, 5)));
    assert_eq!(source(&layout, 5, After), Some((3, 0)));
    assert_eq!(source(&layout, 12, Before), Some((4, 1)));
    assert_eq!(content_offset(&layout, 3, 0, After), Some(5));
    assert_eq!(content_offset(&layout, 4, 1, After), Some(12));
    // An offset past a node's text is its end; a key with no text, none.
    assert_eq!(content_offset(&layout, 3, 60, After), Some(11));
    assert_eq!(content_offset(&layout, 2, 0, After), None);
    assert_eq!(content_offset(&layout, 9, 0, After), None);
}

/// An atomic inline and a `<br>` span their node's offsets 0 and 1, before
/// and after them, as Blink's `ReplacedElement` and `BRBetweenTextNodes`
/// have them.
#[test]
fn an_atomic_inline_and_a_break_are_before_and_after() {
    let layout = mapped(|b| {
        b.text(key(1), "ab");
        b.atomic(key(2), &ComputedStyle::initial(), None, BoxSize::default());
        b.text(key(3), "cd");
        b.line_break(key(4));
        b.text(key(5), "ef");
    });
    assert_eq!(text(&layout), "ab\u{FFFC}cd\nef");
    assert_eq!(
        units(&layout),
        [
            (I, 0..2, 0..2),
            (G, 2..5, 0..1),
            (I, 5..7, 0..2),
            (G, 7..8, 0..1),
            (I, 8..10, 0..2)
        ]
    );
    assert_eq!(source(&layout, 2, Before), Some((1, 2)));
    assert_eq!(source(&layout, 2, After), Some((2, 0)));
    assert_eq!(source(&layout, 5, Before), Some((2, 1)));
    assert_eq!(source(&layout, 5, After), Some((3, 0)));
    assert_eq!(content_offset(&layout, 2, 0, After), Some(2));
    assert_eq!(content_offset(&layout, 2, 1, Before), Some(5));
    assert_eq!(content_offset(&layout, 4, 0, After), Some(7));
    assert_eq!(content_offset(&layout, 4, 1, After), Some(8));
    assert_eq!(source(&layout, 8, After), Some((5, 0)));
}

/// A `<wbr>`'s U+200B holds none of the caller's text. Offsets on either
/// side of it map to the text node's one offset there.
#[test]
fn a_break_opportunity_holds_none_of_the_callers_text() {
    let layout = mapped(|b| {
        b.text(key(1), "ab");
        b.break_opportunity();
        b.text(key(1), "cd");
    });
    assert_eq!(text(&layout), "ab\u{200B}cd");
    assert_eq!(
        units(&layout),
        [(I, 0..2, 0..2), (G, 2..5, 2..2), (I, 5..7, 2..4)]
    );
    assert_eq!(content_offset(&layout, 1, 2, Before), Some(2));
    assert_eq!(content_offset(&layout, 1, 2, After), Some(5));
    assert_eq!(source(&layout, 2, After), Some((1, 2)));
    assert_eq!(source(&layout, 5, Before), Some((1, 2)));
    // Between nodes, after a box closed: the node written last's.
    let layout = mapped(|b| {
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "ab");
        b.close_box();
        b.break_opportunity();
        b.text(key(4), "cd");
    });
    assert_eq!(text(&layout), "ab\u{200B}cd");
    assert_eq!(source(&layout, 5, Before), Some((3, 2)));
    assert_eq!(source(&layout, 5, After), Some((4, 0)));
}

/// Collapsing keeps a break opportunity where white space that wraps joins a
/// run from a box that does not. The opportunity belongs to the joining node
/// and holds none of its text.
#[test]
fn the_opportunity_collapsing_keeps_is_empty_in_the_callers_text() {
    let layout = mapped(|b| {
        b.open_box(key(1), &nowrap(), None);
        b.text(key(2), "a ");
        b.close_box();
        b.text(key(3), " b");
    });
    assert_eq!(text(&layout), "a \u{200B}b");
    assert_eq!(
        units(&layout),
        [
            (I, 0..2, 0..2),
            (G, 2..5, 0..0),
            (C, 5..5, 0..1),
            (I, 5..6, 1..2)
        ]
    );
    assert_eq!(content_offset(&layout, 3, 0, Before), Some(2));
    assert_eq!(content_offset(&layout, 3, 1, Before), Some(5));
}

/// Kept white space is written as given, a kept segment break too. A CRLF is
/// one break: its CR collapses.
#[test]
fn kept_white_space_is_written_as_given() {
    let layout = mapped_with(&white_space(WhiteSpaceCollapse::Preserve), |b| {
        b.text(key(1), "a  b\r\nc\td");
    });
    assert_eq!(text(&layout), "a  b\nc\td");
    assert_eq!(
        units(&layout),
        [(I, 0..4, 0..4), (C, 4..4, 4..5), (I, 4..8, 5..9)]
    );
    assert_eq!(content_offset(&layout, 1, 5, After), Some(4));
    assert_eq!(content_offset(&layout, 1, 6, After), Some(5));
    // A segment break written as a space stands for itself.
    let layout = mapped_with(&white_space(WhiteSpaceCollapse::PreserveSpaces), |b| {
        b.text(key(1), "a\nb");
    });
    assert_eq!(text(&layout), "a b");
    assert_eq!(units(&layout), [(I, 0..3, 0..3)]);
    // Discarded white space is collapsed away.
    let layout = mapped_with(&white_space(WhiteSpaceCollapse::Discard), |b| {
        b.text(key(1), "a b");
    });
    assert_eq!(
        units(&layout),
        [(I, 0..1, 0..1), (C, 1..1, 1..2), (I, 1..2, 2..3)]
    );
}

/// The block's `white-space-trim: discard-inner` trims its kept white space
/// at both ends. What it trims collapses to nothing, and the rest maps as it
/// would untrimmed.
#[test]
fn a_trimmed_block_maps_what_it_kept() {
    let root = styled(|style| {
        style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
        style.text.white_space_trim = WhiteSpaceTrim {
            discard_inner: true,
            ..WhiteSpaceTrim::NONE
        };
    });
    let layout = mapped_with(&root, |b| b.text(key(1), "\n  ab\n  cd  \n  "));
    assert_eq!(text(&layout), "  ab\n  cd  ");
    assert_eq!(content_offset(&layout, 1, 0, Before), Some(0));
    assert_eq!(content_offset(&layout, 1, 1, After), Some(0));
    assert_eq!(content_offset(&layout, 1, 3, After), Some(2));
    assert_eq!(content_offset(&layout, 1, 9, After), Some(8));
    assert_eq!(content_offset(&layout, 1, 15, Before), Some(11));
    assert_eq!(source(&layout, 2, After), Some((1, 3)));
    assert_eq!(source(&layout, 11, After), Some((1, 15)));
}

/// The first letter's box holds the caller's text node's text, as in Blink's
/// `FirstLetter`. Its units answer to the text node's key, and the rest of
/// that node counts on after it.
#[test]
fn a_first_letter_is_its_text_nodes_text() {
    let letter = ComputedStyle::initial();
    let layout = mapped(|b| {
        b.set_first_letter(key(9), &letter, None);
        b.text(key(1), "  \u{201C}Hello there");
    });
    assert_eq!(shown(&layout), "[\u{201C}H]ello there");
    for (offset, at) in [(2, 0), (5, 3), (6, 4), (8, 6), (9, 7), (10, 8)] {
        assert_eq!(
            content_offset(&layout, 1, offset, After),
            Some(at),
            "{offset}"
        );
        assert_eq!(source(&layout, at, After), Some((1, offset)), "{at}");
    }
    assert_eq!(content_offset(&layout, 9, 0, After), None);
    // A text ends with punctuation, and the letter is in the next text. The
    // box holds the first text's part, and the next text goes on after it.
    let layout = mapped(|b| {
        b.set_first_letter(key(9), &letter, None);
        b.text(key(1), "\u{201C}");
        b.text(key(1), "Hi");
    });
    assert_eq!(shown(&layout), "[\u{201C}]Hi");
    assert_eq!(content_offset(&layout, 1, 3, After), Some(3));
    assert_eq!(source(&layout, 4, Before), Some((1, 4)));
}

/// Every offset of the content maps into the caller's text and back to
/// where it started. A break opportunity the builder wrote is the exception:
/// its two sides are one offset in the caller's text. The documents use
/// every call and every white space rule.
#[test]
fn the_map_round_trips() {
    let upper = cased(TextCase::Uppercase, "en");
    let pre = white_space(WhiteSpaceCollapse::Preserve);
    let layouts = [
        mapped(|b| {
            b.text(key(1), "  one  two\n three ");
            b.open_box(key(2), &upper, None);
            b.text(key(3), " stra\u{DF}e  ");
            b.close_box();
            b.atomic(key(4), &ComputedStyle::initial(), None, BoxSize::default());
            b.text(key(5), "\tx");
            b.break_opportunity();
            b.text(key(5), "y  ");
            b.line_break(key(6));
            b.open_box(key(7), &pre, None);
            b.text(key(8), " a \r\n b ");
            b.close_box();
            b.text(key(9), " end");
        }),
        mapped(|b| {
            b.set_first_letter(key(10), &ComputedStyle::initial(), None);
            b.text(key(1), "Hello world");
        }),
    ];
    for layout in &layouts {
        let content = layout.content();
        round_trips(content, true);
    }
}

/// Checks that every offset of `content`'s text maps into the caller's text
/// and back to where it started, where the map was recorded.
///
/// A break opportunity the builder wrote is the exception: its two sides are
/// one offset in the caller's text. Where the caller gave a key to several
/// nodes (not `unique`), an offset comes back in the first of them that holds
/// it, so the check asks only that both ways answer.
pub(super) fn round_trips(content: &Content, unique: bool) {
    let text = &content.text;
    let Some(map) = content.offset_map() else {
        return;
    };
    let listed = content
        .extras
        .as_deref()
        .map_or_else(Vec::new, |extras| extras.map.listed(text.len()));
    let keys: Vec<(NodeKind, u64)> = content
        .nodes
        .nodes
        .iter()
        .map(|(_, node)| (node.kind, node.key.0))
        .collect();
    for at in (0..=text.len()).filter(|&at| text.is_char_boundary(at)) {
        let at = TextOffset::new(at);
        if map.variable_around(at).is_some() {
            continue;
        }
        for side in [Before, After] {
            let Some((node_key, offset)) = map.source_position(at, side) else {
                assert!(text.is_empty(), "{at:?} maps nowhere in {text:?}");
                continue;
            };
            let back = map.content_offset(node_key, offset, side).unwrap();
            if !unique {
                continue;
            }
            let (low, high) = (at.min(back).get(), at.max(back).get());
            assert!(
                text[low..high].chars().all(|ch| ch == '\u{200B}'),
                "{at:?} {side:?} came back as {back:?} in {text:?}: {listed:?} {keys:?}"
            );
        }
    }
}
