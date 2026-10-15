//! Content tests: what the builder writes.
//! - white space: collapsing, kept, trimmed, control characters and break
//!   opportunities;
//! - nodes and items, and unbalanced or unfinished calls;
//! - ruby containers and annotations;
//! - styles lowered into interned facts, the first-line styles and the flags;
//! - limits, record sizes, and calls in any order;
//! - text transforms and masks, and the first line's own text.
//!
//! `map` pins the offset map and `first_letter` pins `::first-letter`.
//!
//! Every build here finishes, and finishing asserts in debug builds that the
//! items tile the text and each node's items are its own.

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::facts::{ShapingFacts, TextFacts};
use super::lists::{FeatureSettingsId, Languages, VariationSettingsId};
use super::memo::StyleKey;
use super::writer::ContentLimits;
use super::*;
use crate::data::{HeapBytes, Id};
use crate::stages::fonts::NamedListId;
use crate::style::LengthPercentage;
use crate::style::{
    ComputedStyle, EdgesGroup, FirstLineVariant, FontFamilyName, FontFeature, FontGroup,
    GenericFamily, Language, LineBreak, Sides, Tag, TextAutospace, TextCase, TextCombineUpright,
    TextGroup, TextOrientation, TextSecurity, TextSpacingTrim, TextTransform, TextWrapMode,
    UnicodeBidi, WhiteSpaceCollapse, WhiteSpaceTrim, WordBreak, WritingMode,
};
use crate::tests::{key, no_fonts, nowrap, styled, white_space};
use crate::unit::TextUnit;
use crate::{BuildOptions, BuildReport, ComputedBlockStyle, Context, Layout, LayoutBuilder};
// Read only by the step-count tests, which run in debug builds.
#[cfg(debug_assertions)]
use crate::work;

fn trimmed(trim: WhiteSpaceTrim) -> ComputedStyle<'static> {
    styled(|style| style.text.white_space_trim = trim)
}

/// Builds a layout by `calls` under the initial style.
fn build(calls: impl FnOnce(&mut LayoutBuilder<'_>)) -> Layout {
    build_with(
        &ComputedBlockStyle::default(),
        BuildOptions::default(),
        calls,
    )
    .0
}

/// Builds a layout by `calls` in `block` with `options`, and returns what
/// finishing reported.
fn build_with(
    block: &ComputedBlockStyle<'_>,
    options: BuildOptions,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) -> (Layout, BuildReport) {
    let mut layout = Layout::new();
    let mut cx = no_fonts();
    let mut builder = layout.builder(&mut cx, key(0), block, options);
    calls(&mut builder);
    let report = builder.finish();
    (layout, report)
}

/// Builds a layout by `calls` under the initial style with `cx`, whose
/// content scratch keeps the build's lists after it.
fn build_in(cx: &mut Context, calls: impl FnOnce(&mut LayoutBuilder<'_>)) -> Layout {
    let mut layout = Layout::new();
    let block = ComputedBlockStyle::default();
    let mut builder = layout.builder(cx, key(0), &block, BuildOptions::default());
    calls(&mut builder);
    builder.finish();
    layout
}

/// Builds a layout by `calls` within `limits`.
fn build_limited(
    limits: ContentLimits,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) -> (Layout, BuildReport) {
    let mut layout = Layout::new();
    let block = ComputedBlockStyle::default();
    let mut cx = no_fonts();
    let mut builder =
        layout.builder_within(&mut cx, key(0), &block, BuildOptions::default(), limits);
    calls(&mut builder);
    let report = builder.finish();
    (layout, report)
}

/// Returns the content as a string: each text item's text, and a mark for
/// every other item.
fn shown(layout: &Layout) -> String {
    let content = layout.content();
    let mut out = String::new();
    for (_, item) in content.items.iter() {
        let text = &content.text[item.start.get()..item.end.get()];
        out.push_str(match item.kind {
            ItemKind::Text => text,
            ItemKind::Open => "[",
            ItemKind::Close => "]",
            ItemKind::Atomic => "{obj}",
            ItemKind::Float => "{float}",
            ItemKind::Absolute => "{abs}",
            ItemKind::Break => "{br}",
            ItemKind::RubyOpen => "<ruby>",
            ItemKind::RubyClose => "</ruby>",
            ItemKind::AnnotationOpen => "<rt>",
            ItemKind::AnnotationClose => "</rt>",
        });
    }
    out
}

/// Returns the text alone.
fn text(layout: &Layout) -> &str {
    &layout.content().text
}

/// Returns each node's kind and key, in pre-order.
fn nodes(layout: &Layout) -> Vec<(NodeKind, u64)> {
    let nodes = &layout.content().nodes;
    nodes
        .nodes
        .iter()
        .map(|(_, node)| (node.kind, node.key.0))
        .collect()
}

/// Returns each item's kind, its text, and the key of its node.
fn items(layout: &Layout) -> Vec<(ItemKind, String, u64)> {
    let content = layout.content();
    content
        .items
        .iter()
        .map(|(_, item)| {
            (
                item.kind,
                content.text[item.start.get()..item.end.get()].to_string(),
                content.nodes.key(item.node).0,
            )
        })
        .collect()
}

/// Returns the kinds of `node`'s items.
fn node_item_kinds(layout: &Layout, node: NodeId) -> Vec<ItemKind> {
    let content = layout.content();
    content
        .items
        .slice(content.nodes.items(node))
        .iter()
        .map(|item| item.kind)
        .collect()
}

// White space ---------------------------------------------------------------

/// A run of spaces, tabs, segment breaks and carriage returns collapses to
/// one space.
#[test]
fn whitespace_collapses_within_a_run() {
    let layout = build(|b| b.text(key(1), "a   b\t\tc\n\nd \r e"));
    assert_eq!(shown(&layout), "a b c d e");
}

/// WPT `control-chars-00D`: carriage returns in every white space mode are
/// written as the reference's spaces are.
#[test]
fn carriage_returns_are_written_as_the_spaces_of_control_chars_00d() {
    let modes = [
        WhiteSpaceCollapse::Preserve,
        WhiteSpaceCollapse::Preserve,
        WhiteSpaceCollapse::PreserveBreaks,
        WhiteSpaceCollapse::BreakSpaces,
        WhiteSpaceCollapse::Collapse,
    ];
    let runs = [3, 3, 6, 3, 5];
    let after = ["D", "E", "F", "G", "H"];
    let page = |white: &str| {
        build(|b| {
            b.text(key(1), &["A", &white.repeat(6), "B", white, "C"].concat());
            for (at, ((mode, count), letter)) in
                (0u64..).zip(modes.into_iter().zip(runs).zip(after))
            {
                let mut style = white_space(mode);
                if at == 4 {
                    style.text.wrap_mode = TextWrapMode::NoWrap;
                }
                b.open_box(key(10 + 2 * at), &style, None);
                b.text(key(11 + 2 * at), &white.repeat(count));
                b.close_box();
                b.text(key(30 + at), letter);
            }
        })
    };
    let test = page("\r");
    assert_eq!(shown(&test), shown(&page(" ")));
    assert_eq!(shown(&test), "A B C[   ]D[   ]E[ ]F[   ]G[ ]H");
}

/// Control characters are written as CSS Text 3 has them.
///
/// - A lone CR is a space in all respects (CSS Text 3, section 4):
///   collapsible where spaces collapse, a kept space where they are kept.
/// - A CRLF is one segment break, its LF.
/// - VT, FF, NEL, U+2028 and U+2029 force a line break in every white space
///   mode (section 5.1). They are written as given and take the collapsible
///   white space on both sides, as a `<br>` does.
///
/// Chrome 153 differs and is not followed: it drops CR and FF under preserved
/// white space, draws FF as a glyph under `normal`, and breaks at no BK or NL
/// character.
#[test]
fn control_characters_are_as_css_has_them() {
    use WhiteSpaceCollapse as W;
    let under = |mode: W, source: &str| {
        let (layout, _) = build_with(
            &ComputedBlockStyle::new(&white_space(mode)),
            BuildOptions::default(),
            |b| {
                b.text(key(1), source);
            },
        );
        text(&layout).to_string()
    };
    // A lone CR.
    assert_eq!(under(W::Collapse, "a\rb"), "a b");
    assert_eq!(under(W::Collapse, "a \r b"), "a b");
    assert_eq!(under(W::PreserveBreaks, "a\r\rb"), "a b");
    assert_eq!(under(W::Discard, "a\r b"), "ab");
    for mode in [W::Preserve, W::BreakSpaces, W::PreserveSpaces] {
        assert_eq!(under(mode, "a\rb"), "a b", "{mode:?}");
        assert_eq!(under(mode, "a \r\r b"), "a    b", "{mode:?}");
    }
    // A CRLF.
    assert_eq!(under(W::Collapse, "a\r\nb"), "a b");
    assert_eq!(under(W::PreserveBreaks, "a \r\n b"), "a\nb");
    assert_eq!(under(W::Preserve, "a\r\nb"), "a\nb");
    assert_eq!(under(W::BreakSpaces, "a\r\n\r\nb"), "a\n\nb");
    assert_eq!(under(W::PreserveSpaces, "a\r\nb"), "a b");
    // The characters that force a break, alone and with spaces around them.
    for separator in ["\u{B}", "\u{C}", "\u{85}", "\u{2028}", "\u{2029}"] {
        let alone = format!("a{separator}b");
        let spaced = format!("a {separator} b");
        for mode in [W::Collapse, W::PreserveBreaks, W::Discard] {
            assert_eq!(under(mode, &alone), alone, "{separator:?} {mode:?}");
            assert_eq!(under(mode, &spaced), alone, "{separator:?} {mode:?}");
        }
        for mode in [W::Preserve, W::BreakSpaces, W::PreserveSpaces] {
            assert_eq!(under(mode, &spaced), spaced, "{separator:?} {mode:?}");
        }
    }
    // Across nodes: the collapsible space after a kept one goes too.
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &white_space(W::Preserve), None);
        b.text(key(3), "\u{2028}");
        b.close_box();
        b.text(key(4), " b");
    });
    assert_eq!(shown(&layout), "a[\u{2028}]b");
    // Inside a ruby container each is a space, as a `<br>` is there. The
    // white space before it stays; the white space after it collapses.
    let layout = build(|b| {
        b.open_ruby(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "a \u{C} b");
        b.close_ruby();
    });
    assert_eq!(shown(&layout), "<ruby>a  b</ruby>");
}

/// A box edge neither trims nor protects white space. The run becomes one
/// space where it began, inside the box if the box holds its start. Chrome
/// agrees (probe `collapse.html`).
#[test]
fn whitespace_collapses_across_a_box_edge() {
    for (inside, after, expected) in [
        (" ", "", "a[ ]b"),
        ("", " ", "a[] b"),
        (" ", " ", "a[ ]b"),
        ("  ", "  ", "a[ ]b"),
    ] {
        let layout = build(|b| {
            b.text(key(1), "a");
            b.open_box(key(2), &ComputedStyle::initial(), None);
            b.text(key(3), inside);
            b.close_box();
            b.text(key(4), after);
            b.text(key(4), "b");
        });
        assert_eq!(
            shown(&layout),
            expected,
            "{inside:?} inside, {after:?} after"
        );
    }
}

/// `a<span> </span> b` keeps one space: a box edge does not end a run.
#[test]
fn an_edge_does_not_end_a_run() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), " ");
        b.close_box();
        b.text(key(4), " b");
    });
    assert_eq!(shown(&layout), "a[ ]b");
}

/// A collapsed space is written when its run ends, before the edges and
/// anchors that arrived during the run, which shift after it.
#[test]
fn a_collapsed_space_goes_before_what_arrived_while_it_was_pending() {
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.close_box();
        b.float(
            key(3),
            &ComputedStyle::initial(),
            crate::FloatSide::Left,
            crate::BoxSize::default(),
        );
        b.text(key(4), " b");
    });
    assert_eq!(shown(&layout), "a []{float}b");
    let offsets: Vec<_> = layout
        .content()
        .items
        .iter()
        .map(|(_, item)| (item.start.get(), item.end.get()))
        .collect();
    assert_eq!(offsets, [(0, 2), (2, 2), (2, 2), (2, 2), (2, 3)]);
}

/// A white space run does not cross an atomic inline, and the atomic adds no
/// space.
#[test]
fn an_atomic_separates_the_words_it_sits_between() {
    let atomic = |b: &mut LayoutBuilder<'_>| {
        b.atomic(
            key(2),
            &ComputedStyle::initial(),
            None,
            crate::BoxSize::default(),
        );
    };
    let layout = build(|b| {
        b.text(key(1), "a ");
        atomic(b);
        b.text(key(3), " b");
    });
    assert_eq!(shown(&layout), "a {obj} b");
    let layout = build(|b| {
        b.text(key(1), "a");
        atomic(b);
        b.text(key(3), "b");
    });
    assert_eq!(shown(&layout), "a{obj}b");
    assert_eq!(text(&layout), "a\u{FFFC}b");
}

/// Collapsible white space at the block's start or end is dropped, inside
/// boxes too.
#[test]
fn nothing_collapsible_at_the_block_start_or_end() {
    let layout = build(|b| b.text(key(1), "   a b   "));
    assert_eq!(shown(&layout), "a b");
    let layout = build(|b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "  a  ");
        b.close_box();
        b.text(key(3), "  ");
    });
    assert_eq!(shown(&layout), "[a]");
}

/// `preserve` keeps every character. A segment break stays in the text for
/// analysis to read as a forced break.
#[test]
fn preserve_keeps_every_character() {
    let layout = build(|b| {
        b.open_box(key(1), &white_space(WhiteSpaceCollapse::Preserve), None);
        b.text(key(2), "a   b\nc\t ");
        b.close_box();
    });
    assert_eq!(shown(&layout), "[a   b\nc\t ]");
}

/// Each node reads its own `white-space-collapse`; inheritance is the
/// cascade's job. Text after a box's close follows the parent's rule.
#[test]
fn each_node_reads_its_own_white_space() {
    let pre = white_space(WhiteSpaceCollapse::Preserve);
    let layout = build(|b| {
        b.open_box(key(1), &pre, None);
        b.text(key(2), "a   b");
        b.open_box(key(3), &pre, None);
        b.text(key(4), "   c");
        b.close_box();
        b.close_box();
        b.text(key(5), "   d");
    });
    assert_eq!(shown(&layout), "[a   b[   c]] d");
}

/// Kept spaces do not absorb collapsible ones: each side keeps its own.
#[test]
fn kept_spaces_and_collapsible_spaces_meet() {
    let pre = white_space(WhiteSpaceCollapse::Preserve);
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &pre, None);
        b.text(key(3), "  b  ");
        b.close_box();
        b.text(key(4), " c");
    });
    assert_eq!(shown(&layout), "a [  b  ] c");
}

/// `pre-line` keeps segment breaks, collapses the rest, and drops the spaces
/// on either side of a kept break.
#[test]
fn pre_line_keeps_the_newlines_and_nothing_else() {
    let source = "one   two  \n   three";
    let under = |collapse| {
        let layout = build(|b| {
            b.open_box(key(1), &white_space(collapse), None);
            b.text(key(2), source);
            b.close_box();
        });
        text(&layout).to_string()
    };
    assert_eq!(under(WhiteSpaceCollapse::PreserveBreaks), "one two\nthree");
    assert_eq!(under(WhiteSpaceCollapse::Collapse), "one two three");
    assert_eq!(under(WhiteSpaceCollapse::Preserve), source);
}

/// `preserve-spaces` keeps every space and tab, and writes a segment break
/// as a space.
#[test]
fn preserve_spaces_keeps_the_spacing_and_not_the_lines() {
    let layout = build(|b| {
        b.open_box(
            key(1),
            &white_space(WhiteSpaceCollapse::PreserveSpaces),
            None,
        );
        b.text(key(2), "one   two\n\tthree");
        b.close_box();
    });
    assert_eq!(text(&layout), "one   two \tthree");
}

/// `discard` drops every space, tab and segment break, leaving no pending
/// space. The white space around the box collapses by the block's rule.
#[test]
fn discard_drops_every_space() {
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &white_space(WhiteSpaceCollapse::Discard), None);
        b.text(key(3), "one  two\n three ");
        b.close_box();
        b.text(key(4), " b");
    });
    assert_eq!(shown(&layout), "a [onetwothree] b");
}

/// A forced break takes the collapsible white space on both sides.
#[test]
fn a_forced_break_takes_the_white_space_on_both_sides() {
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), " ");
        b.line_break(key(4));
        b.text(key(3), "  b");
        b.close_box();
    });
    assert_eq!(shown(&layout), "a[{br}b]");
    // So does a kept segment break, to collapsible white space in other
    // nodes.
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &white_space(WhiteSpaceCollapse::Preserve), None);
        b.text(key(3), "\n");
        b.close_box();
        b.text(key(4), " b");
    });
    assert_eq!(shown(&layout), "a[\n]b");
}

/// White space from a wrapping node that collapses into a run begun in a
/// `nowrap` node keeps its wrap opportunity as a generated U+200B, as Blink's
/// `AppendGeneratedBreakOpportunity` does. The reverse needs nothing.
#[test]
fn a_wrap_opportunity_survives_collapsing_into_nowrap() {
    let layout = build(|b| {
        b.open_box(key(1), &nowrap(), None);
        b.text(key(2), "a ");
        b.close_box();
        b.text(key(3), " b");
    });
    assert_eq!(shown(&layout), "[a ]\u{200B}b");
    assert_eq!(
        items(&layout)[3..],
        [
            (ItemKind::Text, "\u{200B}".to_string(), 3),
            (ItemKind::Text, "b".to_string(), 3)
        ],
        "an item of the wrapping node's own"
    );
    let generated = &layout.content().items.as_slice()[3];
    assert!(generated.flags.contains(ItemFlags::GENERATED));

    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &nowrap(), None);
        b.text(key(3), " b");
        b.close_box();
    });
    assert_eq!(shown(&layout), "a [b]");
}

/// A segment break becomes a space, except beside a zero width space, where
/// it is removed, as in Chrome's `ShouldRemoveNewline`.
#[test]
fn a_segment_break_beside_a_zero_width_space_is_removed() {
    let collapsed = |source: &str| text(&build(|b| b.text(key(1), source))).to_string();
    assert_eq!(collapsed("a\n b"), "a b");
    assert_eq!(collapsed("a\u{200B}\nb"), "a\u{200B}b");
    assert_eq!(collapsed("a\n\u{200B}b"), "a\u{200B}b");
    assert_eq!(
        collapsed("a \u{200B} b"),
        "a \u{200B} b",
        "a space alone stays"
    );
    // A `<wbr>` before the break counts: its U+200B is in the text.
    let layout = build(|b| {
        b.text(key(1), "a");
        b.break_opportunity();
        b.text(key(1), "\nb");
    });
    assert_eq!(text(&layout), "a\u{200B}b");
}

/// A segment break between ideographs is a space, as in Chrome, which
/// compiles out CSS Text's rule that removes it.
#[test]
fn a_segment_break_between_ideographs_is_a_space_as_in_chrome() {
    let layout = build(|b| b.text(key(1), "日本\n語"));
    assert_eq!(text(&layout), "日本 語");
}

/// A run of white space collapses across a float, as Blink's
/// `kOpaqueToCollapsing` has it.
#[test]
fn floats_are_opaque_to_collapsing() {
    let layout = build(|b| {
        b.text(key(1), "a  ");
        b.float(
            key(2),
            &ComputedStyle::initial(),
            crate::FloatSide::Right,
            crate::BoxSize::default(),
        );
        b.text(key(3), "  b");
    });
    assert_eq!(shown(&layout), "a {float}b");
}

/// A `<wbr>` is U+200B in its own generated item. Within a text node it
/// belongs to that node and splits its items; between nodes it belongs to
/// the enclosing box. White space collapses across it.
#[test]
fn a_break_opportunity_is_text() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.break_opportunity();
        b.text(key(1), "b");
    });
    assert_eq!(
        items(&layout),
        [
            (ItemKind::Text, "a".to_string(), 1),
            (ItemKind::Text, "\u{200B}".to_string(), 1),
            (ItemKind::Text, "b".to_string(), 1)
        ]
    );
    let flagged: Vec<bool> = layout
        .content()
        .items
        .iter()
        .map(|(_, item)| item.flags.contains(ItemFlags::GENERATED))
        .collect();
    assert_eq!(flagged, [false, true, false]);

    let layout = build(|b| {
        b.text(key(1), "a ");
        b.break_opportunity();
        b.text(key(1), " b");
    });
    assert_eq!(text(&layout), "a \u{200B}b");

    let layout = build(|b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "a");
        b.close_box();
        b.break_opportunity();
        b.text(key(3), "b");
    });
    assert_eq!(shown(&layout), "[a]\u{200B}b");
    let content = layout.content();
    let generated = &content.items.as_slice()[3];
    assert_eq!(generated.node, NodeId::BLOCK, "the block's");
    assert!(generated.flags.contains(ItemFlags::GENERATED));
}

/// A combined unit collapses its white space on its own: leading and trailing
/// white space goes, and inner runs become one space.
///
/// Chrome's `LayoutTextCombine` is an inline block holding the text, so its
/// own line drops white space at its ends. Outside, the unit acts as an
/// atomic inline: the space before it stays. Outside vertical writing, text
/// is not combined and keeps its spaces.
#[test]
fn a_combined_unit_collapses_its_white_space_on_its_own() {
    let combined = styled(|style| {
        style.orientation.text_combine_upright = TextCombineUpright::All;
    });
    let vertical = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::default()
    };
    let run = |block: &ComputedBlockStyle<'_>| {
        let (layout, _) = build_with(block, BuildOptions::default(), |b| {
            b.text(key(1), "\u{3042}");
            b.open_box(key(2), &combined, None);
            b.text(key(3), "  12");
            b.close_box();
            b.text(key(4), "\u{3044}");
            b.open_box(key(5), &combined, None);
            b.text(key(6), "34  ");
            b.close_box();
            b.text(key(7), "\u{3046} ");
            b.open_box(key(8), &combined, None);
            b.text(key(9), " 5    6");
            b.close_box();
            b.text(key(10), "\u{3048}");
        });
        shown(&layout)
    };
    assert_eq!(
        run(&vertical),
        "\u{3042}[12]\u{3044}[34]\u{3046} [5 6]\u{3048}"
    );
    assert_eq!(
        run(&ComputedBlockStyle::default()),
        "\u{3042}[ 12]\u{3044}[34 ]\u{3046} [5 6]\u{3048}"
    );
}

// white-space-trim ------------------------------------------------------------

/// `white-space-trim` drops the collapsible white space on either side of an
/// element, or at the ends of its content. Chrome lacks the property.
#[test]
fn white_space_trim_discards_the_space_at_an_edge() {
    let trim = |before, after, inner| WhiteSpaceTrim {
        discard_before: before,
        discard_after: after,
        discard_inner: inner,
    };
    let around = |style: &ComputedStyle<'_>, outside: &str, inside: &str| {
        let layout = build(|b| {
            b.text(key(1), &["a", outside].concat());
            b.open_box(key(2), style, None);
            b.text(key(3), &[inside, "b", inside].concat());
            b.close_box();
            b.text(key(4), &[outside, "c"].concat());
        });
        shown(&layout)
    };
    let plain = ComputedStyle::initial();
    assert_eq!(around(&plain, " ", ""), "a [b] c");
    assert_eq!(
        around(&trimmed(trim(true, false, false)), " ", ""),
        "a[b] c"
    );
    assert_eq!(
        around(&trimmed(trim(false, true, false)), " ", ""),
        "a [b]c"
    );
    assert_eq!(around(&trimmed(trim(false, false, true)), "", " "), "a[b]c");
    assert_eq!(around(&trimmed(trim(true, true, false)), " ", ""), "a[b]c");
    // Not inherited: a box inside trims only if its own style says so.
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &trimmed(trim(true, false, false)), None);
        b.text(key(3), "b ");
        b.open_box(key(4), &plain, None);
        b.text(key(5), "c");
        b.close_box();
        b.text(key(6), " d");
        b.close_box();
    });
    assert_eq!(shown(&layout), "a[b [c] d]");
}

/// `discard-inner` on a box drops a run begun inside it, not one begun
/// before it.
#[test]
fn discard_inner_drops_only_the_run_inside() {
    let inner = trimmed(WhiteSpaceTrim {
        discard_inner: true,
        ..WhiteSpaceTrim::NONE
    });
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &inner, None);
        b.close_box();
        b.text(key(3), "c");
    });
    assert_eq!(shown(&layout), "a []c");
    // With `discard-before` too, the run before it goes, and the content's
    // leading white space begins no new run.
    let both = trimmed(WhiteSpaceTrim {
        discard_before: true,
        discard_inner: true,
        ..WhiteSpaceTrim::NONE
    });
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &both, None);
        b.text(key(3), " b ");
        b.close_box();
        b.text(key(4), "c");
    });
    assert_eq!(shown(&layout), "a[b]c");
}

/// `discard-inner` on a block of kept text trims through its leading segment
/// break and from its trailing one, keeping the indent after the break.
#[test]
fn the_block_trims_kept_white_space_through_its_segment_breaks() {
    let root = styled(|style| {
        style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
        style.text.white_space_trim.discard_inner = true;
    });
    let (layout, _) = build_with(
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
        |b| {
            b.text(key(1), "\n  ");
            b.text(key(1), "code\n  more\n  ");
        },
    );
    assert_eq!(text(&layout), "  code\n  more");
    // Without it, everything is kept.
    let root = white_space(WhiteSpaceCollapse::Preserve);
    let (layout, _) = build_with(
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
        |b| {
            b.text(key(1), "\n  code\n  more\n  ");
        },
    );
    assert_eq!(text(&layout), "\n  code\n  more\n  ");
}

// Nodes and items -------------------------------------------------------------

/// Every box puts both edges in the items, so a box with nothing to show can
/// still be found, hit and decorated.
#[test]
fn every_box_has_both_edges() {
    let layout = build(|b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "plain");
        b.close_box();
    });
    assert_eq!(shown(&layout), "[plain]");
}

/// A float is a textless anchor where it was written.
#[test]
fn a_float_is_an_anchor_in_the_flow() {
    let layout = build(|b| {
        b.float(
            key(1),
            &ComputedStyle::initial(),
            crate::FloatSide::Left,
            crate::BoxSize::default(),
        );
        b.text(key(2), "beside it");
    });
    assert_eq!(shown(&layout), "{float}beside it");
}

/// Finishing closes whatever is open, ruby included.
#[test]
fn finishing_closes_what_is_open() {
    let layout = build(|b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "unclosed");
    });
    assert_eq!(shown(&layout), "[unclosed]");
    let layout = build(|b| {
        b.open_ruby(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "a");
        b.open_annotation(key(3), &ComputedStyle::initial(), None);
        b.text(key(4), "b");
    });
    assert_eq!(shown(&layout), "<ruby>a<rt>b</rt></ruby>");
}

/// Dropping a builder without finishing closes its content too.
#[test]
fn a_dropped_builder_leaves_whole_content() {
    let mut layout = Layout::new();
    let mut cx = no_fonts();
    let mut builder = layout.builder(
        &mut cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&ComputedStyle::initial()),
        BuildOptions::default(),
    );
    builder.open_box(key(1), &ComputedStyle::initial(), None);
    builder.text(key(2), "left open");
    drop(builder);
    assert_eq!(shown(&layout), "[left open]");
}

/// A close with no open box is ignored, as is one that would close a box
/// around an open ruby container or annotation.
#[test]
fn an_unbalanced_close_is_ignored() {
    let layout = build(|b| {
        b.close_box();
        b.text(key(1), "a");
        b.open_ruby(key(2), &ComputedStyle::initial(), None);
        b.close_box();
        b.text(key(3), "b");
        b.close_ruby();
        b.close_box();
        b.close_ruby();
        b.close_annotation();
    });
    assert_eq!(shown(&layout), "a<ruby>b</ruby>");
}

/// Every node records its key, and text from a new key starts a new node.
#[test]
fn nodes_record_their_keys() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.text(key(2), "b");
        b.atomic(
            key(3),
            &ComputedStyle::initial(),
            None,
            crate::BoxSize::default(),
        );
        b.line_break(key(4));
        b.text(key(2), "c");
    });
    assert_eq!(
        nodes(&layout),
        [
            (NodeKind::Block, 0),
            (NodeKind::Text, 1),
            (NodeKind::Text, 2),
            (NodeKind::Atomic, 3),
            (NodeKind::LineBreak, 4),
            (NodeKind::Text, 2),
        ]
    );
}

/// A key reused by a later node makes a new node, never a merge.
///
/// Every opening call makes a node, and so does text under a new key. A key
/// used for a box and its text, or for text on both sides of a box, is two
/// nodes, each with its own items.
#[test]
fn a_key_reused_by_a_later_node_is_a_new_node() {
    let layout = build(|b| {
        b.open_box(key(5), &ComputedStyle::initial(), None);
        b.text(key(5), "x");
        b.close_box();
        b.text(key(1), "a");
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.close_box();
        b.text(key(1), "b");
    });
    assert_eq!(
        nodes(&layout),
        [
            (NodeKind::Block, 0),
            (NodeKind::Box, 5),
            (NodeKind::Text, 5),
            (NodeKind::Text, 1),
            (NodeKind::Box, 2),
            (NodeKind::Text, 1),
        ]
    );
    assert_eq!(
        node_item_kinds(&layout, NodeId::new(1)),
        [ItemKind::Open, ItemKind::Text, ItemKind::Close]
    );
    assert_eq!(node_item_kinds(&layout, NodeId::new(2)), [ItemKind::Text]);
    assert_eq!(node_item_kinds(&layout, NodeId::new(5)), [ItemKind::Text]);
}

/// A node's items are a contiguous range. A box's run from its open to its
/// close; the last text in a box does not take the box's close.
#[test]
fn a_nodes_items_are_its_own() {
    let layout = build(|b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "a");
        b.close_box();
        b.text(key(4), "   ");
        b.close_box();
        b.text(key(5), "b");
    });
    assert_eq!(shown(&layout), "[[a] ]b");
    use ItemKind::*;
    assert_eq!(
        node_item_kinds(&layout, NodeId::BLOCK),
        [Open, Open, Text, Close, Text, Close, Text]
    );
    assert_eq!(
        node_item_kinds(&layout, NodeId::new(1)),
        [Open, Open, Text, Close, Text, Close]
    );
    assert_eq!(
        node_item_kinds(&layout, NodeId::new(2)),
        [Open, Text, Close]
    );
    assert_eq!(node_item_kinds(&layout, NodeId::new(3)), [Text]);
    assert_eq!(node_item_kinds(&layout, NodeId::new(4)), [Text]);
    assert_eq!(node_item_kinds(&layout, NodeId::new(5)), [Text]);
}

/// White space alone after an atomic inline stays in its own node. A run's
/// space goes in the item of the node where it began, so the next node's
/// offsets do not shift.
#[test]
fn white_space_after_an_atomic_stays_in_its_own_node() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.atomic(
            key(2),
            &ComputedStyle::initial(),
            None,
            crate::BoxSize::default(),
        );
        b.text(key(3), "  ");
        b.text(key(4), "b");
    });
    assert_eq!(
        items(&layout),
        [
            (ItemKind::Text, "a".to_string(), 1),
            (ItemKind::Atomic, "\u{FFFC}".to_string(), 2),
            (ItemKind::Text, " ".to_string(), 3),
            (ItemKind::Text, "b".to_string(), 4),
        ]
    );
}

/// A white space run that collapses away writes no text.
#[test]
fn a_run_that_comes_to_nothing_writes_nothing() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.close_box();
        b.text(key(3), "   ");
    });
    assert_eq!(text(&layout), "a");
    assert_eq!(shown(&layout), "a[]");
}

// Ruby --------------------------------------------------------------------------

/// A ruby container's items bracket its base and annotations. An
/// annotation's items, marks included, carry the annotation flag.
#[test]
fn ruby_marks_bracket_and_annotations_are_flagged() {
    let layout = build(|b| {
        b.open_ruby(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "漢");
        b.open_annotation(key(3), &ComputedStyle::initial(), None);
        b.text(key(4), "かん");
        b.close_annotation();
        b.close_ruby();
        b.text(key(5), "字");
    });
    assert_eq!(shown(&layout), "<ruby>漢<rt>かん</rt></ruby>字");
    let flagged: Vec<_> = layout
        .content()
        .items
        .iter()
        .map(|(_, item)| item.flags.contains(ItemFlags::ANNOTATION))
        .collect();
    assert_eq!(flagged, [false, false, true, true, true, false, false]);
    assert!(layout.content().flags.contains(ContentFlags::RUBY));
}

/// A second annotation on the same base closes the first, as successive
/// `<rtc>` levels do. Ending the ruby closes what is open inside it.
#[test]
fn annotations_follow_one_another() {
    let layout = build(|b| {
        b.open_ruby(key(1), &ComputedStyle::initial(), None);
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "a");
        b.close_box();
        b.open_annotation(key(4), &ComputedStyle::initial(), None);
        b.text(key(5), "x");
        b.open_annotation(key(6), &ComputedStyle::initial(), None);
        b.text(key(7), "y");
        b.close_ruby();
    });
    assert_eq!(shown(&layout), "<ruby>[a]<rt>x</rt><rt>y</rt></ruby>");
}

/// An annotation in a box in a base opens an anonymous container in the box,
/// over an empty base, as CSS Ruby wraps an annotation whose parent is no
/// ruby container (`<ruby><span>a<rt>x</rt></span></ruby>`). Ending the
/// annotation closes both, and the box goes on.
#[test]
fn an_annotation_in_a_base_box_opens_an_anonymous_container() {
    let layout = build(|b| {
        b.open_ruby(key(1), &ComputedStyle::initial(), None);
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "a");
        b.open_annotation(key(4), &ComputedStyle::initial(), None);
        b.text(key(5), "x");
        b.close_annotation();
        b.text(key(6), "b");
        b.close_box();
        b.close_ruby();
    });
    assert_eq!(shown(&layout), "<ruby>[a<ruby><rt>x</rt></ruby>b]</ruby>");
}

/// `start_column` flags the next annotation's open item `EMPTY_BASE` where
/// no base text came since the last annotation, so it starts a column rather
/// than stacking. With base text since, before the first annotation, or with
/// anything written between, it flags nothing.
#[test]
fn start_column_flags_an_empty_base_only_where_none_came() {
    let plain = ComputedStyle::initial();
    let layout = build(|b| {
        b.open_ruby(key(1), &plain, None);
        b.start_column();
        b.text(key(2), "a");
        b.open_annotation(key(3), &plain, None);
        b.text(key(4), "x");
        b.close_annotation();
        b.start_column();
        b.open_annotation(key(5), &plain, None);
        b.text(key(6), "y");
        b.close_annotation();
        b.text(key(7), "b");
        b.start_column();
        b.open_annotation(key(8), &plain, None);
        b.text(key(9), "z");
        b.close_annotation();
        b.start_column();
        b.open_box(key(10), &plain, None);
        b.close_box();
        b.open_annotation(key(11), &plain, None);
        b.close_ruby();
    });
    assert_eq!(
        shown(&layout),
        "<ruby>a<rt>x</rt><rt>y</rt>b<rt>z</rt>[]<rt></rt></ruby>"
    );
    let content = layout.content();
    let empty: Vec<u64> = content
        .items
        .iter()
        .filter(|(_, item)| item.flags.contains(ItemFlags::EMPTY_BASE))
        .map(|(_, item)| content.nodes.key(item.node).0)
        .collect();
    assert_eq!(empty, [5]);
}

/// An annotation outside any ruby container opens in an anonymous one over
/// an empty base, as Chrome wraps an orphan `ruby-text` box. Ending the
/// annotation closes both.
#[test]
fn an_annotation_outside_ruby_opens_an_anonymous_container() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.open_annotation(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "b");
        b.close_annotation();
        b.text(key(4), "c");
    });
    assert_eq!(shown(&layout), "a<ruby><rt>b</rt></ruby>c");
}

/// A forced break inside a ruby container is a space, as in Blink
/// (`kDisableForcedBreakInRubyColumn`): no paragraph ends inside a column.
///
/// The break keeps its node. White space after it collapses into it; white
/// space before it stays, since Blink appends the break's space as a text
/// item of its own.
#[test]
fn a_forced_break_in_ruby_is_a_space() {
    let layout = build(|b| {
        b.open_ruby(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "a ");
        b.line_break(key(3));
        b.text(key(4), " b");
        b.open_annotation(key(5), &ComputedStyle::initial(), None);
        b.text(key(6), "x");
        b.line_break(key(7));
        b.text(key(8), "y");
        b.close_ruby();
        b.line_break(key(9));
        b.text(key(10), "c");
    });
    assert_eq!(shown(&layout), "<ruby>a  b<rt>x y</rt></ruby>{br}c");
    assert!(nodes(&layout).contains(&(NodeKind::LineBreak, 3)));
}

/// A segment break inside a ruby container ends no paragraph either. It
/// collapses as white space under `collapse` and `preserve-breaks`, and is a
/// kept space under `preserve`.
#[test]
fn a_newline_in_ruby_ends_no_paragraph() {
    use crate::style::WhiteSpaceCollapse;
    for (collapse, want) in [
        (WhiteSpaceCollapse::Collapse, "<ruby>a b</ruby>"),
        (WhiteSpaceCollapse::PreserveBreaks, "<ruby>a b</ruby>"),
        (WhiteSpaceCollapse::Preserve, "<ruby>a  b</ruby>"),
    ] {
        let style = styled(|style| style.text.white_space_collapse = collapse);
        let layout = build_with(
            &ComputedBlockStyle::new(&style),
            BuildOptions::default(),
            |b| {
                b.open_ruby(key(1), &style, None);
                b.text(key(2), "a \nb");
                b.close_ruby();
            },
        )
        .0;
        assert_eq!(shown(&layout), want, "{collapse:?}");
        assert!(!text(&layout).contains('\n'), "{collapse:?}");
    }
}

/// A ruby in another's base keeps its own container and annotation. Closing
/// it leaves the outer base open for the outer annotation.
#[test]
fn a_ruby_in_a_base_keeps_its_container_and_annotations() {
    let plain = ComputedStyle::initial();
    let layout = build(|b| {
        b.open_ruby(key(1), &plain, None);
        b.open_ruby(key(2), &plain, None);
        b.text(key(3), "a");
        b.open_annotation(key(4), &plain, None);
        b.text(key(5), "x");
        b.close_annotation();
        b.close_ruby();
        b.open_annotation(key(6), &plain, None);
        b.text(key(7), "y");
        b.close_annotation();
        b.close_ruby();
        b.text(key(8), "b");
    });
    assert_eq!(
        shown(&layout),
        "<ruby><ruby>a<rt>x</rt></ruby><rt>y</rt></ruby>b"
    );
    // Malformed ruby builds without a panic and closes what it opened:
    // annotations with no ruby, a ruby left open, empty bases, deep levels.
    let layout = build(|b| {
        b.close_ruby();
        b.open_annotation(key(1), &plain, None);
        b.open_ruby(key(2), &plain, None);
        for level in 0..64 {
            b.open_annotation(key(3 + level), &plain, None);
        }
        b.open_ruby(key(100), &plain, None);
        b.open_ruby(key(101), &plain, None);
        b.open_annotation(key(102), &plain, None);
        b.text(key(103), "z");
    });
    let shown = shown(&layout);
    assert_eq!(
        shown.matches("<rt>").count(),
        shown.matches("</rt>").count()
    );
    assert_eq!(shown.matches('[').count(), shown.matches(']').count());
    assert_eq!(shown.matches("<ruby>").count(), 1);
    assert!(shown.ends_with("</ruby>"), "{shown}");
}

/// White space collapses across ruby marks, as across box edges.
#[test]
fn white_space_collapses_across_ruby_marks() {
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_ruby(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), " b");
        b.close_ruby();
    });
    assert_eq!(shown(&layout), "a <ruby>b</ruby>");
}

// Styles ------------------------------------------------------------------------

/// Returns `node`'s text facts in `variant`.
fn node_text_facts(content: &Content, node: NodeId, variant: FirstLineVariant) -> &TextFacts {
    content.facts.text(content.nodes.text_facts(node, variant))
}

/// Returns `node`'s font request in `variant`, through its text facts.
fn node_request(content: &Content, node: NodeId, variant: FirstLineVariant) -> &FontRequest {
    let facts = &content.facts;
    facts.request(facts.text_request(content.nodes.text_facts(node, variant)))
}

/// Returns `node`'s own text and box facts.
fn node_facts(content: &Content, node: NodeId) -> (TextFactsId, BoxFactsId) {
    let nodes = &content.nodes;
    (
        nodes.text_facts(node, FirstLineVariant::Standard),
        nodes.box_facts(node, FirstLineVariant::Standard),
    )
}

static SANS: [FontFamilyName<'static>; 2] = [
    FontFamilyName::named("Inter"),
    FontFamilyName::Generic(GenericFamily::SansSerif),
];

/// Equal styles lower into the same facts, whether their lists are borrowed
/// or owned: the builder interns by value.
#[test]
fn styles_are_interned_once() {
    let named = [
        FontFamilyName::Named(Cow::Owned("Inter".to_string())),
        FontFamilyName::Generic(GenericFamily::SansSerif),
    ];
    let features = [FontFeature::new(Tag::new(b"liga"), 0)];
    let one = styled(|style| style.font.families = &SANS);
    let two = ComputedStyle {
        font: FontGroup {
            families: &named,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut cx = no_fonts();
    let layout = build_in(&mut cx, |b| {
        b.open_box(key(1), &one, None);
        b.close_box();
        b.open_box(key(2), &two, None);
        b.close_box();
        b.open_box(
            key(3),
            &ComputedStyle {
                font: FontGroup {
                    features: &features,
                    ..two.font
                },
                ..two
            },
            None,
        );
        b.close_box();
    });
    let content = layout.content();
    let facts = |n| node_facts(content, NodeId::new(n));
    let request = |n| node_request(content, NodeId::new(n), FirstLineVariant::Standard);
    assert_eq!(facts(1), facts(2), "equal styles, the same facts");
    assert_ne!(facts(2), facts(3), "a feature tells them apart");
    assert_eq!(content.facts.text_count(), 3, "the block's and two");
    let fonts = cx.font_context();
    assert!(fonts.is_named(request(1).font.families, &SANS));
    let block = request(0);
    assert_eq!(block.font.families, NamedListId::INITIAL);
    assert_eq!(block.language, LanguageId::UNDETERMINED);
    assert_eq!(
        cx.content_lists()
            .feature_settings()
            .get(request(3).font.features),
        features
    );
}

/// Interning stays exact as a table grows past a few values.
///
/// A small table finds a value by comparing each; a larger one builds a hash
/// index. On both sides of that switch, equal facts, family lists, settings
/// lists, languages and hyphenation strings share one id, and each id names
/// what was interned under it.
#[test]
fn many_styles_and_lists_intern_once_past_a_few() {
    const MANY: usize = 20;
    let names: Vec<String> = (0..MANY).map(|n| format!("Family {n}")).collect();
    let lists_named: Vec<[FontFamilyName<'_>; 2]> = names
        .iter()
        .map(|name| {
            [
                FontFamilyName::Named(Cow::Borrowed(name.as_str())),
                FontFamilyName::Generic(GenericFamily::SansSerif),
            ]
        })
        .collect();
    let features: Vec<[FontFeature; 1]> = (0u16..)
        .take(MANY)
        .map(|n| [FontFeature::new(Tag::new(b"liga"), n)])
        .collect();
    let tags = [
        "en", "fr", "de", "es", "it", "ja", "zh", "ko", "ar", "he", "ru", "el", "nl", "pt", "sv",
        "fi", "da", "pl", "tr", "cs",
    ];
    let hyphens: Vec<String> = (0..MANY).map(|n| format!("-{n}")).collect();
    let style = |n: usize| ComputedStyle {
        font: FontGroup {
            families: &lists_named[n],
            features: &features[n],
            ..FontGroup::INITIAL
        },
        text: TextGroup {
            language: Language::parse(tags[n]).ok(),
            hyphenate_character: Some(hyphens[n].as_str()),
            ..TextGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut cx = no_fonts();
    let layout = build_in(&mut cx, |b| {
        for (n, node) in (0..MANY).zip(1..) {
            b.open_box(key(node), &style(n), None);
            b.close_box();
        }
        for (n, node) in (0..MANY).rev().zip(101..) {
            b.open_box(key(node), &style(n), None);
            b.close_box();
        }
    });
    let content = layout.content();
    let lists = cx.content_lists();
    let fonts = cx.font_context();
    // The block's facts and twenty more; each family list once, the initial
    // one never stored.
    assert_eq!(content.facts.text_count(), MANY + 1);
    assert_eq!(fonts.counts()[0], MANY);
    for n in 0..MANY {
        let first = NodeId::new(1 + n);
        let again = NodeId::new(1 + MANY + (MANY - 1 - n));
        let id = node_facts(content, first);
        assert_eq!(id, node_facts(content, again), "style {n} lowered twice");
        let held = node_request(content, first, FirstLineVariant::Standard);
        assert!(fonts.is_named(held.font.families, &lists_named[n]));
        assert_eq!(
            lists.feature_settings().get(held.font.features),
            features[n]
        );
        assert_eq!(
            Some(content.languages.get(held.language)),
            Language::parse(tags[n]).ok()
        );
        let hyphen = node_text_facts(content, first, FirstLineVariant::Standard)
            .hyphen
            .map(|id| lists.hyphen_strings().get(id));
        assert_eq!(hyphen, Some(hyphens[n].as_str()));
    }
}

/// A style's floats compare by their bits: a NaN equals itself as a memo
/// key, and `-0.0` differs from `0.0`. On the grid both spacings are zero, so
/// every node's text facts are the block's.
#[test]
fn a_style_compares_its_floats_by_bits() {
    let nan = styled(|style| style.text.letter_spacing = LengthPercentage::from_px(f32::NAN));
    let negative_zero = styled(|style| style.text.letter_spacing = LengthPercentage::from_px(-0.0));
    let mut lists = Lists::new();
    let mut languages = Languages::new();
    let mut cx = no_fonts();
    let (fonts, ..) = cx.selecting_fonts();
    let mut replaced = 0;
    let mut keyed = |style: &ComputedStyle<'_>| -> StyleKey {
        lists.key(style, &mut languages, fonts, &mut replaced)
    };
    assert_eq!(keyed(&nan), keyed(&nan));
    assert_ne!(keyed(&negative_zero), keyed(&ComputedStyle::initial()));
    let layout = build(|b| {
        b.open_box(key(1), &nan, None);
        b.close_box();
        b.open_box(key(2), &nan, None);
        b.close_box();
        b.open_box(key(3), &negative_zero, None);
        b.close_box();
    });
    let content = layout.content();
    for node in 1..4 {
        assert_eq!(
            node_facts(content, NodeId::new(node)).0,
            node_facts(content, NodeId::BLOCK).0
        );
    }
}

/// Languages and hyphenation strings are interned as lists are.
#[test]
fn languages_and_hyphenation_strings_are_interned() {
    let japanese = Language::parse("ja").ok();
    let mut cx = no_fonts();
    let layout = build_in(&mut cx, |b| {
        b.open_box(
            key(1),
            &styled(|style| {
                style.text.language = japanese;
                style.text.hyphenate_character = Some("‐");
            }),
            None,
        );
        b.close_box();
    });
    let content = layout.content();
    let node = NodeId::new(1);
    let request = node_request(content, node, FirstLineVariant::Standard);
    assert_eq!(Some(content.languages.get(request.language)), japanese);
    let hyphen = node_text_facts(content, node, FirstLineVariant::Standard)
        .hyphen
        .map(|id| cx.content_lists().hyphen_strings().get(id));
    assert_eq!(hyphen, Some("‐"));
}

/// Empty settings and `und` take the first ids of their tables but are
/// stored only once another entry follows. A layout that sets no feature,
/// variation or language stores nothing; one that sets some still answers
/// the initial ids.
#[test]
fn the_empty_lists_and_und_are_stored_only_before_another() {
    let mut cx = no_fonts();
    let plain = build_in(&mut cx, |b| b.text(key(1), "plain"));
    let styles = cx.content_lists();
    let languages = &plain.content().languages;
    let block = node_request(plain.content(), NodeId::BLOCK, FirstLineVariant::Standard);
    assert_eq!(
        (block.font.features, block.font.variations, block.language),
        (
            FeatureSettingsId::new(0),
            VariationSettingsId::new(0),
            LanguageId::UNDETERMINED
        )
    );
    assert_eq!(styles.feature_settings().get(block.font.features), []);
    assert_eq!(languages.get(block.language), Language::UND);
    let kept = styles.feature_settings().heap_bytes()
        + styles.variation_settings().heap_bytes()
        + languages.heap_bytes();
    assert_eq!(kept, 0, "nothing stored for the initial lists");
    let features = [FontFeature::new(Tag::new(b"liga"), 0)];
    let japanese = Language::parse("ja").ok();
    let mut cx = no_fonts();
    let set = build_in(&mut cx, |b| {
        b.open_box(
            key(1),
            &ComputedStyle {
                font: FontGroup {
                    features: &features,
                    ..FontGroup::INITIAL
                },
                text: TextGroup {
                    language: japanese,
                    ..TextGroup::INITIAL
                },
                ..ComputedStyle::initial()
            },
            None,
        );
        b.close_box();
    });
    let styles = cx.content_lists();
    let languages = &set.content().languages;
    let block = node_request(set.content(), NodeId::BLOCK, FirstLineVariant::Standard);
    let boxed = node_request(set.content(), NodeId::new(1), FirstLineVariant::Standard);
    assert_eq!(block.font.features, FeatureSettingsId::new(0));
    assert_eq!(boxed.font.features, FeatureSettingsId::new(1));
    assert_eq!(styles.feature_settings().get(block.font.features), []);
    assert_eq!(styles.feature_settings().get(boxed.font.features), features);
    assert_eq!(block.language, LanguageId::UNDETERMINED);
    assert_eq!(languages.get(block.language), Language::UND);
    assert_eq!(Some(languages.get(boxed.language)), japanese);
}

/// The initial style is CSS's, with Chrome's values where Chrome computes
/// another, and equals `Default`.
#[test]
fn there_is_one_initial_style() {
    let initial = ComputedStyle::initial();
    assert_eq!(initial, ComputedStyle::default());
    assert_eq!(initial.font.size, 16.0);
    assert_eq!(initial.text.line_break, LineBreak::Normal);
    assert!(initial.font.families.is_empty(), "the Standard font alone");
}

/// A close names its own box's node, never its parent's. The node's
/// first-line facts take only what `::first-line` may change from the box's
/// first-line style.
#[test]
fn a_close_names_its_own_box_and_first_line_style() {
    let plain = ComputedStyle::initial();
    let block_first_line = styled(|style| style.font.size = 20.0);
    let element = styled(|style| style.text.language = Language::parse("en").ok());
    let first_line = styled(|style| {
        style.font.size = 40.0;
        style.text.letter_spacing = LengthPercentage::from_px(2.0);
        // What `::first-line` may not change.
        style.text.word_break = WordBreak::BreakAll;
        style.text.line_break = LineBreak::Strict;
        style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
        style.orientation.text_orientation = TextOrientation::Upright;
        style.bidi.unicode_bidi = UnicodeBidi::Isolate;
        style.text.language = Language::parse("fr").ok();
    });
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&block_first_line),
            ..ComputedBlockStyle::new(&plain)
        },
        BuildOptions::default(),
        |b| {
            b.open_box(key(1), &element, Some(&first_line));
            b.text(key(2), "text in a box");
            b.close_box();
        },
    );
    let content = layout.content();
    let close = content
        .items
        .iter()
        .find(|(_, item)| item.kind == ItemKind::Close)
        .map(|(_, item)| item.node);
    assert_eq!(close, Some(NodeId::new(1)), "the close is the box's");
    let nodes = &content.nodes;
    let facts = &content.facts;
    let boxed = NodeId::new(1);
    let (first, standard) = (FirstLineVariant::FirstLine, FirstLineVariant::Standard);
    let (pinned, own) = (
        node_text_facts(content, boxed, first),
        node_text_facts(content, boxed, standard),
    );
    let shaping = |text: &TextFacts| *facts.shaping(text.shaping);
    let request = node_request(content, boxed, first);
    assert_eq!(request.font.size, 40.0, "the size it may change is kept");
    assert_eq!(shaping(pinned).letter, TextUnit::from_px(2.0));
    let bidi = |variant| facts.box_facts(nodes.box_facts(boxed, variant)).bidi;
    assert_eq!(
        (
            pinned.word_break,
            pinned.line_break,
            pinned.collapse,
            shaping(pinned).orientation,
            bidi(first),
            request.language,
        ),
        (
            own.word_break,
            own.line_break,
            own.collapse,
            shaping(own).orientation,
            bidi(standard),
            node_request(content, boxed, standard).language,
        ),
        "what it may not change is the element's own"
    );
    assert_eq!(
        nodes.text_facts(NodeId::new(2), first),
        nodes.text_facts(boxed, first),
        "the box's text has its box's"
    );
    assert_eq!(
        node_request(content, NodeId::BLOCK, first).font.size,
        20.0,
        "the block's is its own"
    );
    assert!(content.flags.contains(ContentFlags::FIRST_LINE_RESTYLE));
}

/// Without a block `::first-line`, every node's first-line facts are its
/// own, whatever a call passes.
#[test]
fn without_first_line_every_node_is_its_own_on_the_first_line() {
    let layout = build(|b| {
        b.open_box(
            key(1),
            &ComputedStyle::initial(),
            Some(&styled(|s| s.font.size = 9.0)),
        );
        b.close_box();
    });
    let nodes = &layout.content().nodes;
    assert_eq!(nodes.len(), 2);
    assert!(nodes.nodes.iter().all(|(_, node)| {
        (node.text, node.box_) == (node.first_line_text, node.first_line_box)
    }));
    assert!(
        !layout
            .content()
            .flags
            .contains(ContentFlags::FIRST_LINE_RESTYLE)
    );
}

/// The flags say what the content holds. A plain block sets only
/// `TRIMS_PUNCTUATION`.
#[test]
fn the_flags_say_what_the_content_holds() {
    // The initial `text-spacing-trim: normal` trims punctuation.
    let plain = build(|b| b.text(key(1), "plain"));
    assert_eq!(plain.content().flags, ContentFlags::TRIMS_PUNCTUATION);
    let spaced = styled(|style| style.text.spacing_trim = TextSpacingTrim::SpaceAll);
    let untrimmed = build_with(
        &ComputedBlockStyle::new(&spaced),
        BuildOptions::default(),
        |b| {
            b.open_box(key(1), &spaced, None);
            b.text(key(2), "plain");
            b.close_box();
        },
    )
    .0;
    assert_eq!(untrimmed.content().flags, ContentFlags::INLINE_BOXES);
    // Each flag's property, set on a box or a float. No line holds the float,
    // but its style still counts.
    let set = build(|b| {
        b.open_box(key(1), &nowrap(), None);
        b.close_box();
        b.open_box(
            key(2),
            &styled(|style| style.text.spacing_trim = TextSpacingTrim::TrimStart),
            None,
        );
        b.close_box();
        b.float(
            key(3),
            &styled(|style| style.text.autospace = TextAutospace::NORMAL),
            crate::FloatSide::Left,
            crate::BoxSize::default(),
        );
    });
    for flag in [
        ContentFlags::INLINE_BOXES,
        ContentFlags::NOWRAP,
        ContentFlags::TRIMS_WRAPPED_START,
        ContentFlags::TRIMS_PUNCTUATION,
        ContentFlags::AUTOSPACE,
    ] {
        assert!(set.content().flags.contains(flag), "{flag:?}");
    }

    let edged = styled(|style| {
        style.edges = EdgesGroup {
            padding: Sides::from_px(2.0),
            ..EdgesGroup::INITIAL
        };
    });
    let rich = build(|b| {
        b.open_box(key(1), &edged, None);
        b.open_box(
            key(2),
            &styled(|s| s.bidi.unicode_bidi = UnicodeBidi::Isolate),
            None,
        );
        b.open_box(
            key(3),
            &styled(|s| s.text.letter_spacing = LengthPercentage::from_px(1.0)),
            None,
        );
        b.atomic(
            key(4),
            &ComputedStyle::initial(),
            None,
            crate::BoxSize::default(),
        );
        b.float(
            key(5),
            &ComputedStyle::initial(),
            crate::FloatSide::Left,
            crate::BoxSize::default(),
        );
    });
    let flags = rich.content().flags;
    for flag in [
        ContentFlags::BOXES_WITH_EDGES,
        ContentFlags::NONZERO_SPACING,
        ContentFlags::ATOMICS,
        ContentFlags::FLOATS,
    ] {
        assert!(flags.contains(flag), "{flag:?}");
    }
    assert!(!flags.contains(ContentFlags::RUBY));
}

/// Atomic inline and float sizes are kept as given, except that a negative
/// or non-finite size becomes zero (and such a baseline, `None`).
#[test]
fn box_sizes_are_kept_and_made_sane() {
    let layout = build(|b| {
        b.atomic(
            key(1),
            &ComputedStyle::initial(),
            None,
            crate::BoxSize {
                inline: 20.0,
                block: 10.0,
                baseline: Some(8.0),
            },
        );
        b.float(
            key(2),
            &ComputedStyle::initial(),
            crate::FloatSide::Right,
            crate::BoxSize {
                inline: f32::NAN,
                block: -3.0,
                baseline: Some(f32::INFINITY),
            },
        );
    });
    let content = layout.content();
    let atomic = content.atomics().as_slice()[0];
    assert_eq!(atomic.size.inline, 20.0);
    assert_eq!(atomic.size.baseline, Some(8.0));
    assert_eq!(content.items[atomic.item].kind, ItemKind::Atomic);
    let float = content.floats().as_slice()[0];
    assert_eq!((float.size.inline, float.size.block), (0.0, 0.0));
    assert_eq!(float.size.baseline, None);
    assert_eq!(float.side, crate::FloatSide::Right);
}

// Limits ------------------------------------------------------------------------

/// Text past the limit is dropped in whole characters and reported, without
/// a panic, leaving valid content.
#[test]
fn text_past_the_limit_is_dropped_and_reported() {
    let limits = ContentLimits::MAX.with_text(10);
    let (layout, built) = build_limited(limits, |b| {
        b.text(key(1), "abcdefgh");
        // é is two bytes and would end at 11.
        b.text(key(1), "  éclair");
        b.atomic(
            key(2),
            &ComputedStyle::initial(),
            None,
            crate::BoxSize::default(),
        );
        b.text(key(3), "more");
        b.line_break(key(4));
        b.break_opportunity();
    });
    assert_eq!(text(&layout), "abcdefgh ");
    assert_eq!(built.dropped_bytes, "éclair".len() + "more".len());
    assert_eq!(built.dropped_nodes, 3, "the atomic, the text, the break");
    assert!(!built.is_complete());
}

/// Nodes past the limit are dropped with their contents. A dropped box still
/// consumes its own close, so the enclosing box closes in the right place.
#[test]
fn nodes_past_the_limit_are_dropped_and_reported() {
    let limits = ContentLimits::MAX.with_nodes(3);
    let (layout, built) = build_limited(limits, |b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.text(key(2), "a");
        b.open_box(key(3), &ComputedStyle::initial(), None);
        b.text(key(4), "dropped");
        b.close_box();
        b.text(key(2), "also dropped");
        b.close_box();
    });
    assert_eq!(shown(&layout), "[a]");
    assert_eq!(built.dropped_nodes, 3);
    assert_eq!(built.dropped_bytes, "dropped".len() + "also dropped".len());
}

/// Items past the limit are refused, but an open box always has room for its
/// close.
#[test]
fn items_past_the_limit_leave_every_box_closed() {
    let limits = ContentLimits::MAX.with_items(5);
    let (layout, built) = build_limited(limits, |b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.open_box(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "a");
        b.open_box(key(4), &ComputedStyle::initial(), None);
        b.text(key(5), "b");
    });
    assert_eq!(shown(&layout), "[[a]]");
    assert_eq!(built.dropped_nodes, 2, "the third box and the text in it");
}

// Sizes -------------------------------------------------------------------------

/// An item is 16 bytes and a node, with its four fact ids, 32. A style key is
/// at most 244 bytes, mostly margins and padding with their percentages.
#[test]
fn the_records_are_the_size_the_design_says() {
    assert_eq!(size_of::<Item>(), 16);
    assert_eq!(size_of::<Node>(), 32);
    let style = size_of::<StyleKey>();
    assert!(style <= 244, "a style's key is {style} bytes");
    let text = size_of::<TextFacts>();
    assert!(text <= 44, "text facts are {text} bytes");
    let shaping = size_of::<ShapingFacts>();
    assert!(shaping <= 16, "shaping facts are {shaping} bytes");
    let request = size_of::<FontRequest>();
    assert!(request <= 72, "a font request is {request} bytes");
    let facts = size_of::<BoxFacts>();
    assert!(facts <= 48, "box facts are {facts} bytes");
}

/// Once the fact tables are full, a node takes its parent's text facts and
/// the initial box facts, each counted as a replaced style. The content stays
/// valid.
#[test]
fn full_fact_tables_take_stand_ins_and_say_so() {
    // Sizes and paddings 1/64 px apart give each box new text and box facts.
    let mut step = 0.0;
    let styles: Vec<ComputedStyle<'static>> = (0..TextFactsId::MAX + 10)
        .map(|_| {
            step += 1.0 / 64.0;
            styled(|style| {
                style.font.size = 1.0 + step;
                style.edges.padding.left = LengthPercentage::from_px(step);
            })
        })
        .collect();
    let (layout, report) = build_with(
        &ComputedBlockStyle::default(),
        BuildOptions::default(),
        |b| {
            for (at, style) in (0u64..).zip(&styles) {
                b.open_box(key(2 * at + 1), style, None);
                b.text(key(2 * at + 2), "a");
                b.close_box();
            }
        },
    );
    assert!(report.replaced_styles >= 20, "{report:?}");
    assert_eq!(report.dropped_nodes, 0);
    let content = layout.content();
    let nodes = &content.nodes;
    let last = NodeId::new(nodes.len() - 2);
    assert_eq!(nodes.kind(last), Some(NodeKind::Box));
    let standard = FirstLineVariant::Standard;
    assert_eq!(
        nodes.text_facts(last, standard),
        nodes.text_facts(NodeId::BLOCK, standard)
    );
    assert_eq!(nodes.box_facts(last, standard), BoxFactsId::INITIAL);
    check(content);
}

// Anything a caller does ----------------------------------------------------------

/// `break-spaces` builds as `preserve` does. They differ only in where lines
/// may break, which analysis decides.
#[test]
fn break_spaces_keeps_what_preserve_keeps() {
    let under = |collapse| {
        let layout = build(|b| {
            b.open_box(key(1), &white_space(collapse), None);
            b.text(key(2), " a  b\n\t");
            b.close_box();
        });
        shown(&layout)
    };
    assert_eq!(
        under(WhiteSpaceCollapse::BreakSpaces),
        under(WhiteSpaceCollapse::Preserve)
    );
}

/// A generated break opportunity stays even when its run collapses away, as
/// in Blink.
#[test]
fn a_generated_opportunity_outlives_its_run() {
    let layout = build(|b| {
        b.open_box(key(1), &nowrap(), None);
        b.text(key(2), "a ");
        b.close_box();
        b.text(key(3), " ");
    });
    assert_eq!(shown(&layout), "[a]\u{200B}");
}

/// Calls in any order with any text never panic, under limits small enough
/// to reach. The content stays whole and the offset map round-trips.
#[test]
fn any_calls_in_any_order_leave_whole_content() {
    // A deterministic generator.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, below: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap_or(0) % below.max(1)
        }
    }
    let texts = [
        "",
        " ",
        "  a",
        "b ",
        "\n",
        " \n ",
        "\t",
        "\u{200B}",
        "日本",
        "é",
        "x\u{200B}\ny",
        "\r\x0C",
        "  word  ",
        "straße ΣΑΣ",
        "o'brien ǆ ŉ",
        "x.y İ ﬁ ｶ",
    ];
    let cases = [
        TextCase::None,
        TextCase::Uppercase,
        TextCase::Lowercase,
        TextCase::Capitalize,
    ];
    let collapses = [
        WhiteSpaceCollapse::Collapse,
        WhiteSpaceCollapse::Preserve,
        WhiteSpaceCollapse::BreakSpaces,
        WhiteSpaceCollapse::PreserveBreaks,
        WhiteSpaceCollapse::PreserveSpaces,
        WhiteSpaceCollapse::Discard,
    ];
    let mut random = Lcg(7);
    for round in 0..3000_u32 {
        let style_of = |random: &mut Lcg| {
            let n = random.next(64);
            styled(|style| {
                style.text.white_space_collapse = collapses[n % collapses.len()];
                if n & 8 != 0 {
                    style.text.wrap_mode = TextWrapMode::NoWrap;
                }
                style.text.white_space_trim = WhiteSpaceTrim {
                    discard_before: n & 16 != 0,
                    discard_after: n & 32 != 0,
                    discard_inner: n.is_multiple_of(3),
                };
                style.text.transform.case = cases[(n / 4) % cases.len()];
                style.text.transform.full_width = n % 7 == 1;
            })
        };
        let limits = if round.is_multiple_of(2) {
            ContentLimits::MAX
        } else {
            ContentLimits::MAX
                .with_text(1 + random.next(40))
                .with_nodes(1 + random.next(12))
                .with_items(random.next(16))
        };
        let root = style_of(&mut random);
        let first_line = style_of(&mut random);
        // Most rounds record the offset map, which finishing checks too.
        let options = BuildOptions {
            map_source: !round.is_multiple_of(4),
            ..BuildOptions::default()
        };
        let mut layout = Layout::new();
        let mut cx = no_fonts();
        let mut b = layout.builder_within(
            &mut cx,
            NodeKey(0),
            &ComputedBlockStyle {
                first_line: round.is_multiple_of(3).then_some(&first_line),
                ..ComputedBlockStyle::new(&root)
            },
            options,
            limits,
        );
        // Some rounds give every call its own key, so the map's round trips
        // can be checked exactly.
        let unique = round % 8 == 1;
        for call in 0..random.next(40) {
            let reused = key(u64::try_from(random.next(4)).unwrap_or(0));
            let k = if unique {
                key(u64::try_from(call).unwrap_or(0))
            } else {
                reused
            };
            let style = style_of(&mut random);
            match random.next(13) {
                0..=3 => b.text(k, texts[random.next(texts.len())]),
                12 => b.set_first_letter(k, &style, Some(&first_line)),
                4 => b.open_box(k, &style, Some(&first_line)),
                5 => b.close_box(),
                6 => b.atomic(k, &style, None, crate::BoxSize::default()),
                7 => b.float(k, &style, crate::FloatSide::Left, crate::BoxSize::default()),
                8 => b.line_break(k),
                9 => b.break_opportunity(),
                10 => match random.next(4) {
                    0 => b.open_ruby(k, &style, None),
                    1 => b.open_annotation(k, &style, None),
                    2 => b.close_annotation(),
                    _ => b.close_ruby(),
                },
                _ => b.text(k, texts[random.next(texts.len())]),
            }
        }
        if round.is_multiple_of(5) {
            drop(b);
        } else {
            b.finish();
        }
        let content = layout.content();
        assert!(content.text.len() <= limits.text);
        assert!(content.nodes.len() <= limits.nodes.max(1));
        map::round_trips(content, unique);
    }
}

/// Ruby calls find the container they close past any depth of boxes without
/// visiting the boxes.
///
/// Each ruby container and annotation on the builder's stack links to the
/// next one out. `open_annotation`, `close_annotation` and `close_ruby`
/// follow those links, so a thousand calls that close nothing inside a
/// thousand boxes take a few steps each.
///
/// An annotation in a box in a base opens an anonymous container there. A
/// ruby container opened inside its annotation is a box. With no annotation
/// of its own open, `close_annotation` closes nothing. Its annotation is the
/// anonymous column's next level and closes the box, so its `close_ruby`
/// then closes nothing.
#[test]
fn ruby_calls_look_past_no_box() {
    let plain = ComputedStyle::initial();
    let layout = build(|b| {
        b.open_ruby(key(1), &plain, None);
        b.open_box(key(2), &plain, None);
        b.text(key(3), "a");
        // Opens an anonymous container in the box.
        b.open_annotation(key(4), &plain, None);
        b.open_ruby(key(5), &plain, None);
        b.open_box(key(6), &plain, None);
        b.text(key(7), "b");
        // No annotation is open in the innermost ruby: nothing closes.
        b.close_annotation();
        // The column's second level: closes the inner boxes and the first
        // annotation, then opens.
        b.open_annotation(key(8), &plain, None);
        b.text(key(9), "c");
        // The inner ruby is already closed: nothing closes.
        b.close_ruby();
        b.text(key(10), "d");
        // Closes the annotation and its anonymous container, then the box
        // and the ruby.
        b.close_annotation();
        b.close_ruby();
        b.text(key(11), "e");
        // Nothing is open to close.
        b.close_ruby();
        b.close_annotation();
    });
    assert_eq!(
        shown(&layout),
        "<ruby>[a<ruby><rt>[[b]]</rt><rt>cd</rt></ruby>]</ruby>e"
    );
    let depth = 1000;
    for in_ruby in [true, false] {
        let mut layout = Layout::new();
        let mut cx = no_fonts();
        let mut b = layout.builder(
            &mut cx,
            NodeKey(0),
            &ComputedBlockStyle::new(&plain),
            BuildOptions::default(),
        );
        if in_ruby {
            b.open_ruby(key(1), &plain, None);
        }
        for n in 0..depth {
            b.open_box(key(2 + n), &plain, None);
        }
        // Steps are counted only in debug builds.
        #[cfg(debug_assertions)]
        let _ = work::take();
        for _ in 0..depth {
            b.close_annotation();
            if !in_ruby {
                b.close_ruby();
                b.open_annotation(key(0), &plain, None);
            }
        }
        #[cfg(debug_assertions)]
        let steps = work::take();
        // Outside a ruby, each annotation opens an anonymous container, and
        // each end closes both.
        #[cfg(debug_assertions)]
        let per_call = if in_ruby { 3 } else { 6 };
        #[cfg(debug_assertions)]
        assert!(steps <= per_call * depth, "{in_ruby}: {steps} steps");
        b.finish();
        let boxes = nodes(&layout)
            .iter()
            .filter(|&&(kind, _)| kind == NodeKind::Box)
            .count();
        assert_eq!(boxes, usize::try_from(depth).unwrap_or(0));
    }
}

// Transforms ---------------------------------------------------------------------

/// Returns a style that transforms its text as `case` says, in `language`.
fn cased(case: TextCase, language: &str) -> ComputedStyle<'static> {
    styled(|style| {
        style.text.transform.case = case;
        style.text.language = Language::parse(language).ok();
    })
}

/// Returns the text of one call in `style`.
fn transformed(style: &ComputedStyle<'_>, text_: &str) -> String {
    let layout = build(|b| {
        b.open_box(key(1), style, None);
        b.text(key(2), text_);
        b.close_box();
    });
    text(&layout).to_string()
}

/// Every entry in MathML Core's italic mappings table, in table order.
#[test]
fn math_auto_italic_mappings() {
    let original = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyzıȷΑΒΓΔΕΖΗΘΙΚΛΜΝΞΟΠΡϴΣΤΥΦΧΨΩ∇αβγδεζηθικλμνξοπρςστυφχψω∂ϵϑϰϕϱϖ";
    let italic = "𝐴𝐵𝐶𝐷𝐸𝐹𝐺𝐻𝐼𝐽𝐾𝐿𝑀𝑁𝑂𝑃𝑄𝑅𝑆𝑇𝑈𝑉𝑊𝑋𝑌𝑍𝑎𝑏𝑐𝑑𝑒𝑓𝑔ℎ𝑖𝑗𝑘𝑙𝑚𝑛𝑜𝑝𝑞𝑟𝑠𝑡𝑢𝑣𝑤𝑥𝑦𝑧𝚤𝚥𝛢𝛣𝛤𝛥𝛦𝛧𝛨𝛩𝛪𝛫𝛬𝛭𝛮𝛯𝛰𝛱𝛲𝛳𝛴𝛵𝛶𝛷𝛸𝛹𝛺𝛻𝛼𝛽𝛾𝛿𝜀𝜁𝜂𝜃𝜄𝜅𝜆𝜇𝜈𝜉𝜊𝜋𝜌𝜍𝜎𝜏𝜐𝜑𝜒𝜓𝜔𝜕𝜖𝜗𝜘𝜙𝜚𝜛";
    assert_eq!(original.chars().count(), 112);
    assert_eq!(italic.chars().count(), 112);
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    for (from, to) in original.chars().zip(italic.chars()) {
        assert_eq!(
            transformed(&math, from.encode_utf8(&mut [0; 4])),
            to.to_string(),
            "{from}"
        );
    }
}

#[test]
fn math_auto_uses_whole_source_text_nodes() {
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    for unchanged in [
        "", "hi", " h", "h ", "h\ni", "h\u{301}", "∞", "1", "𝑖", "\u{3A2}",
    ] {
        assert_eq!(
            transformed(&math, unchanged),
            unchanged.trim().replace('\n', " ")
        );
    }
    let layout = build_with(
        &ComputedBlockStyle::new(&math),
        BuildOptions::default(),
        |b| {
            b.text(key(1), "h");
            b.text(key(2), "i");
            b.text(key(3), "h");
            b.text(key(3), "");
            b.text(key(3), "i");
            b.text(key(3), "j");
            b.text(key(4), "");
            b.text(key(4), "i");
            b.text(key(4), "");
            b.text(key(5), "");
            b.text(key(4), "i");
        },
    )
    .0;
    assert_eq!(text(&layout), "ℎ𝑖hij𝑖𝑖");
}

/// `math-auto` measures each source text node as given, before white space
/// collapses, as Chrome 155 does:
/// - ` x ` and `\nx\n` keep their letter, though each collapses to it alone;
/// - `x<!---->y`, `x<b></b>y` and `x<wbr>y` are two nodes, each mapped;
/// - `x` beside an empty node or a node of collapsed space is mapped;
/// - `x ` before `<b>y</b>` keeps its letter, and the `y` is mapped.
#[test]
fn math_auto_measures_each_source_node_before_white_space_collapses() {
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    assert_eq!(transformed(&math, " x "), "x");
    assert_eq!(transformed(&math, "\nx\n"), "x");
    let kept = styled(|s| {
        s.text.transform = TextTransform::MATH_AUTO;
        s.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    });
    assert_eq!(transformed(&kept, " x "), " x ");
    let in_math = |calls: &dyn Fn(&mut LayoutBuilder<'_>)| {
        let block = ComputedBlockStyle::new(&math);
        let layout = build_with(&block, BuildOptions::default(), calls).0;
        text(&layout).to_string()
    };
    // `x<!---->y`: the comment splits the text into two nodes.
    assert_eq!(
        in_math(&|b| {
            b.text(key(1), "x");
            b.text(key(2), "y");
        }),
        "𝑥𝑦"
    );
    // `x<b></b>y`.
    assert_eq!(
        in_math(&|b| {
            b.text(key(1), "x");
            b.open_box(key(2), &math, None);
            b.close_box();
            b.text(key(3), "y");
        }),
        "𝑥𝑦"
    );
    // `x<wbr>y`.
    assert_eq!(
        in_math(&|b| {
            b.text(key(1), "x");
            b.break_opportunity();
            b.text(key(2), "y");
        }),
        "𝑥\u{200B}𝑦"
    );
    // Text nodes appended one by one: `x` and an empty node, either way
    // round, and `x` before a node of one space.
    for (first, second) in [("x", ""), ("", "x"), ("x", " ")] {
        let written = in_math(&|b| {
            b.text(key(1), first);
            b.text(key(2), second);
        });
        assert_eq!(written, "𝑥", "{first:?} {second:?}");
    }
    // `x <b>y</b>`.
    assert_eq!(
        in_math(&|b| {
            b.text(key(1), "x ");
            b.open_box(key(2), &math, None);
            b.text(key(3), "y");
            b.close_box();
        }),
        "x 𝑦"
    );
}

#[test]
fn math_auto_excludes_other_transforms() {
    let math = styled(|s| {
        s.text.transform = TextTransform {
            full_width: true,
            full_size_kana: true,
            ..TextTransform::MATH_AUTO
        }
    });
    assert_eq!(transformed(&math, "h"), "ℎ");
    assert_eq!(transformed(&math, "hi"), "hi");
    assert_eq!(transformed(&math, "ｧ"), "ｧ");
}

#[test]
fn math_auto_stops_at_the_text_limit() {
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    for (limit, expected, dropped) in [(3, "", 1), (4, "𝑖", 0)] {
        let mut layout = Layout::new();
        let mut cx = no_fonts();
        let mut b = layout.builder_within(
            &mut cx,
            key(0),
            &ComputedBlockStyle::new(&math),
            BuildOptions::default(),
            ContentLimits::MAX.with_text(limit),
        );
        b.text(key(1), "i");
        let report = b.finish();
        assert_eq!(text(&layout), expected);
        assert_eq!(report.dropped_bytes, dropped);
    }
    let mut layout = Layout::new();
    let mut cx = no_fonts();
    let mut b = layout.builder_within(
        &mut cx,
        key(0),
        &ComputedBlockStyle::new(&math),
        BuildOptions::default(),
        ContentLimits::MAX.with_text(1),
    );
    b.text(key(1), "i");
    b.text(key(1), "j");
    let report = b.finish();
    assert_eq!(text(&layout), "i");
    assert_eq!(report.dropped_bytes, 1);
}

/// Masked text stops at the text limit after the last grapheme whose masks
/// fit, and reports the bytes it drops.
#[test]
fn masked_text_stops_at_the_text_limit() {
    let masked = styled(|s| s.text.security = TextSecurity::Disc);
    for (limit, expected, dropped) in [(2, "", 3), (3, "\u{2022}", 2), (8, "\u{2022}\u{2022}", 1)] {
        let mut layout = Layout::new();
        let mut cx = no_fonts();
        let mut b = layout.builder_within(
            &mut cx,
            key(0),
            &ComputedBlockStyle::new(&masked),
            BuildOptions::default(),
            ContentLimits::MAX.with_text(limit),
        );
        b.text(key(1), "abc");
        let report = b.finish();
        assert_eq!(text(&layout), expected, "{limit}");
        assert_eq!(report.dropped_bytes, dropped, "{limit}");
    }
}

/// A case mapping or `full-width` needs room for three bytes per source byte,
/// not the four `math-auto` needs, so text whose transform just fits is
/// written whole:
/// - lowercased whole, a final sigma knows it ends its word;
/// - the first line draws its own full-width text.
#[test]
fn a_case_transform_at_the_text_limit_is_written_whole() {
    let within = |block: &ComputedBlockStyle<'_>, limit: usize, text_: &str| {
        let mut layout = Layout::new();
        let mut cx = no_fonts();
        let mut b = layout.builder_within(
            &mut cx,
            key(0),
            block,
            BuildOptions::default(),
            ContentLimits::MAX.with_text(limit),
        );
        b.text(key(1), text_);
        let report = b.finish();
        assert_eq!(report.dropped_bytes, 0);
        layout
    };
    let lower = cased(TextCase::Lowercase, "el");
    // Four bytes of Greek, with room for three times as many.
    let layout = within(&ComputedBlockStyle::new(&lower), 12, "ΑΣ");
    assert_eq!(text(&layout), "ας");
    let wide = styled(|s| s.text.transform.full_width = true);
    let plain = ComputedStyle::initial();
    let block = ComputedBlockStyle {
        first_line: Some(&wide),
        ..ComputedBlockStyle::new(&plain)
    };
    let layout = within(&block, 6, "ab");
    assert_eq!(text(&layout), "ab");
    assert_eq!(first_line_text(&layout).0, "ａｂ");
}

#[test]
fn math_auto_first_line_uses_the_source_node_length() {
    let math = styled(|s| s.text.transform = TextTransform::MATH_AUTO);
    let plain = ComputedStyle::initial();
    for (own, first) in [(&plain, &math), (&math, &plain)] {
        let layout = build_with(
            &ComputedBlockStyle {
                first_line: Some(first),
                ..ComputedBlockStyle::new(own)
            },
            BuildOptions::default(),
            |b| {
                b.text(key(1), "i");
                b.text(key(2), "h i");
                b.text(key(3), "h");
                b.text(key(3), "i");
            },
        )
        .0;
        let (first_text, offsets) = first_line_text(&layout);
        if own.text.transform == TextTransform::MATH_AUTO {
            assert_eq!(text(&layout), "𝑖h ihi");
            assert_eq!(first_text, "ih ihi");
            assert_eq!(offsets, [0, 1, 2, 3, 4, 5, 6]);
        } else {
            assert_eq!(text(&layout), "ih ihi");
            assert_eq!(first_text, "𝑖h ihi");
            assert_eq!(offsets, [0, 4, 5, 6, 7, 8, 9]);
        }
    }
}

/// Upper and lower case are the full mappings, with their contexts, as Chrome
/// maps them through ICU. `ß` is two capitals, a ligature two letters, and a
/// final sigma its own letter.
#[test]
fn case_is_the_full_mapping_in_its_context() {
    use crate::style::TextCase::{Lowercase, Uppercase};
    assert_eq!(transformed(&cased(Uppercase, "de"), "straße"), "STRASSE");
    assert_eq!(transformed(&cased(Uppercase, "en"), "ﬁsh ŉ"), "FISH ʼN");
    assert_eq!(transformed(&cased(Lowercase, "el"), "ΣΑΣ ΑΣ."), "σας ας.");
    assert_eq!(
        transformed(&cased(Lowercase, "en"), "İSTANBUL"),
        "i\u{307}stanbul"
    );
}

/// The language tailors the case. Turkish `i` has its own capital, Greek
/// capitals drop their accents, and Dutch titlecases `ij` whole.
#[test]
fn the_language_tailors_the_case() {
    use crate::style::TextCase::{Capitalize, Lowercase, Uppercase};
    assert_eq!(transformed(&cased(Uppercase, "tr"), "istanbul"), "İSTANBUL");
    assert_eq!(transformed(&cased(Uppercase, "az"), "i"), "İ");
    assert_eq!(transformed(&cased(Lowercase, "tr"), "IŞIK"), "ışık");
    assert_eq!(transformed(&cased(Uppercase, "en"), "istanbul"), "ISTANBUL");
    assert_eq!(transformed(&cased(Uppercase, "el"), "άλφα"), "ΑΛΦΑ");
    assert_eq!(transformed(&cased(Capitalize, "nl"), "ijsland"), "IJsland");
    assert_eq!(transformed(&cased(Capitalize, "en"), "ijsland"), "Ijsland");
}

/// Capitalize titlecases the first letter of each word and nothing else.
///
/// Words are Unicode's, so an apostrophe or a mark inside one is passed over.
/// They take Chrome's `en_US_POSIX` tailoring, under which a full stop joins
/// digits alone (measured in Chrome 153: `x.y` is `X.Y`). The titlecase is
/// the full mapping, as CSS asks: `ß` is `Ss` and `ǆ` is `ǅ`.
#[test]
fn capitalize_titlecases_the_first_letter_of_each_word() {
    use crate::style::TextCase::Capitalize;
    let style = cased(Capitalize, "en");
    assert_eq!(
        transformed(&style, "o'brien o’brien (hello x-ray x.y 3.14 3rd a_b"),
        "O'brien O’brien (Hello X-Ray X.Y 3.14 3rd A_b"
    );
    assert_eq!(transformed(&style, "ßa ǆb ﬁsh"), "Ssa ǅb Fish");
    assert_eq!(transformed(&style, "e\u{301}cole"), "E\u{301}cole");
    // Nothing but the first letter changes.
    assert_eq!(transformed(&style, "mIxED CASE"), "MIxED CASE");
}

/// Capitalize finds words with the build's segmenter, as word motion does.
///
/// - A Latin word after ideographs, kana or Thai starts a word, however they
///   are segmented.
/// - With the `dictionaries` feature, so does one after `々`. ICU hands `々`
///   to its dictionary with the ideograph before it, and breaks after the
///   run, as Chrome 153 does.
/// - ICU4C's word rules count Thai as a letter joining the Latin after it,
///   so Chrome leaves `ghi` as it is. This test pins the difference.
/// - Without dictionaries, UAX #29's rules find Chinese and Japanese words.
///   Under them `々` is a letter (ALetter) and carries its word into the
///   Latin after it, within one call and across two.
#[test]
fn capitalize_finds_words_as_word_motion_does() {
    let style = cased(TextCase::Capitalize, "en");
    assert_eq!(
        transformed(
            &style,
            "\u{65E5}\u{672C}abc \u{304B}\u{306A}def \u{E20}\u{E32}\u{E29}\u{E32}ghi"
        ),
        "\u{65E5}\u{672C}Abc \u{304B}\u{306A}Def \u{E20}\u{E32}\u{E29}\u{E32}Ghi"
    );
    let within = transformed(&style, "\u{6642}\u{3005}abc");
    let across = build(|b| {
        b.open_box(key(1), &style, None);
        b.text(key(2), "\u{6642}\u{3005}");
        b.text(key(3), "abc");
        b.close_box();
    });
    #[cfg(feature = "dictionaries")]
    let expected = "\u{6642}\u{3005}Abc";
    #[cfg(not(feature = "dictionaries"))]
    let expected = "\u{6642}\u{3005}abc";
    assert_eq!(within, expected);
    assert_eq!(text(&across), expected);
}

/// Across calls only the last character of the text before counts. A word a
/// span divides is capitalized once. An atomic inline between two words
/// parts them, and so does a `<br>`, as in Chrome.
#[test]
fn capitalize_reads_the_last_character_of_the_text_before() {
    let style = cased(TextCase::Capitalize, "en");
    let layout = build(|b| {
        b.open_box(key(1), &style, None);
        b.text(key(2), "al");
        b.open_box(key(3), &style, None);
        b.text(key(4), "pha");
        b.close_box();
        b.text(key(5), " o");
        b.text(key(6), "'brien ");
        b.atomic(key(7), &style, None, crate::BoxSize::default());
        b.text(key(8), "bravo");
        b.line_break(key(9));
        b.text(key(10), "charlie");
        b.close_box();
    });
    assert_eq!(text(&layout), "Alpha O'brien \u{FFFC}Bravo\nCharlie");
}

/// `full-width` maps ASCII to its full-width forms after the case mapping. A
/// space becomes U+3000 only where white space is kept; a collapsible space
/// is left to collapse, as CSS says and Blink's `ApplyFullwidthTransform`
/// does.
#[test]
fn full_width_leaves_collapsible_spaces_to_collapsing() {
    let wide = |collapse: WhiteSpaceCollapse, case: TextCase| {
        styled(|style| {
            style.text.transform.full_width = true;
            style.text.transform.case = case;
            style.text.white_space_collapse = collapse;
        })
    };
    use crate::style::TextCase::{None as Keep, Uppercase};
    assert_eq!(
        transformed(&wide(WhiteSpaceCollapse::Collapse, Keep), "Ab  1!"),
        "Ａｂ １！"
    );
    assert_eq!(
        transformed(&wide(WhiteSpaceCollapse::Preserve, Keep), "a b"),
        "ａ\u{3000}ｂ"
    );
    assert_eq!(
        transformed(&wide(WhiteSpaceCollapse::Collapse, Uppercase), "straße ｶﾀ"),
        "ＳＴＲＡＳＳＥ カタ"
    );
}

/// A transform that grows the text stops after the last source character
/// whose transform fits the limit, and reports the source bytes dropped.
#[test]
fn a_transform_that_grows_the_text_stops_at_its_limit() {
    let limits = ContentLimits::MAX.with_text(10);
    let wide = styled(|style| style.text.transform.full_width = true);
    let mut layout = Layout::new();
    let mut cx = no_fonts();
    let mut b = layout.builder_within(
        &mut cx,
        key(0),
        &ComputedBlockStyle::new(&wide),
        BuildOptions::default(),
        limits,
    );
    // Three bytes each: three fit, and a fourth would reach 12.
    b.text(key(1), "abcdef");
    let built = b.finish();
    assert_eq!(text(&layout), "ａｂｃ");
    assert_eq!(built.dropped_bytes, 3);

    // `ß` in bulk: two capitals of its two bytes, cut at the limit.
    let upper = cased(TextCase::Uppercase, "de");
    let mut layout = Layout::new();
    let mut cx = no_fonts();
    let mut b = layout.builder_within(
        &mut cx,
        key(0),
        &ComputedBlockStyle::new(&upper),
        BuildOptions::default(),
        limits,
    );
    b.text(key(1), &"ß".repeat(100));
    let built = b.finish();
    assert_eq!(text(&layout), "SSSSSSSSSS");
    assert_eq!(built.dropped_bytes, 190);
}

/// White space collapses across a transformed box as across any other.
#[test]
fn white_space_collapses_across_a_transformed_box() {
    let upper = cased(TextCase::Uppercase, "en");
    let layout = build(|b| {
        b.text(key(1), "a ");
        b.open_box(key(2), &upper, None);
        b.text(key(3), " b ");
        b.close_box();
        b.text(key(4), " c");
    });
    assert_eq!(shown(&layout), "a [B ]c");
}

// The first line's text ------------------------------------------------------------

/// Returns the first line's text and, for each char boundary of the content
/// text, the matching offset in it.
fn first_line_text(layout: &Layout) -> (String, Vec<usize>) {
    let content = layout.content();
    let none = FirstLineSource::default();
    let source = content.first_line_source().unwrap_or(&none);
    let offsets = (0..=content.text.len())
        .filter(|&at| content.text.is_char_boundary(at))
        .map(|at| source.offset(TextOffset::new(at)).get())
        .collect();
    (source.text.to_string(), offsets)
}

/// The first line's transform replaces the element's, since the cascade
/// gives a text one value.
///
/// A first line that capitalizes a lowercased element transforms the source
/// text, not the lowercased text. Chrome composes the two, a limitation its
/// source marks as such.
#[test]
fn a_first_line_transform_replaces_the_elements() {
    use crate::style::TextCase::{Capitalize, Lowercase};
    let root = cased(Lowercase, "en");
    let first_line = styled(|style| style.text.transform.case = Capitalize);
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&first_line),
            ..ComputedBlockStyle::new(&root)
        },
        BuildOptions::default(),
        |b| b.text(key(1), "ALPHA BRAVO"),
    );
    assert_eq!(text(&layout), "alpha bravo");
    let (first, offsets) = first_line_text(&layout);
    assert_eq!(first, "ALPHA BRAVO");
    assert_eq!(offsets, (0..=11).collect::<Vec<_>>());
    assert!(
        layout
            .content()
            .flags
            .contains(ContentFlags::TRANSFORM_SIDE_TEXT)
    );
}

/// A character the two transforms make different lengths maps as a whole:
/// its first cluster draws all of it, and a cluster inside it draws nothing.
#[test]
fn a_character_made_differently_long_maps_as_a_whole() {
    use crate::style::TextCase::Uppercase;
    // The element in capitals, the first line as written.
    let root = cased(Uppercase, "de");
    let first_line = styled(|style| style.text.transform = TextTransform::NONE);
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&first_line),
            ..ComputedBlockStyle::new(&root)
        },
        BuildOptions::default(),
        |b| b.text(key(1), "straße"),
    );
    assert_eq!(text(&layout), "STRASSE");
    let (first, offsets) = first_line_text(&layout);
    assert_eq!(first, "straße");
    // S T R A S S E: the second S is inside the ß, and maps to its end.
    assert_eq!(offsets, [0, 1, 2, 3, 4, 6, 6, 7]);

    // The other way round: the first line in capitals, over text as written.
    let upper = cased(Uppercase, "de");
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&upper),
            ..ComputedBlockStyle::new(&ComputedStyle::initial())
        },
        BuildOptions::default(),
        |b| {
            b.text(key(1), "straße ﬁ!");
        },
    );
    assert_eq!(text(&layout), "straße ﬁ!");
    let (first, offsets) = first_line_text(&layout);
    assert_eq!(first, "STRASSE FI!");
    // The two-byte ß is two capitals, the three-byte ligature two letters.
    assert_eq!(offsets, [0, 1, 2, 3, 4, 6, 7, 8, 10, 11]);
}

/// A cursor over the first line's text maps each offset as the map does,
/// queried forwards, backwards and in jumps.
#[test]
fn the_first_lines_cursor_maps_as_its_map_does() {
    let upper = cased(TextCase::Uppercase, "de");
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&upper),
            ..ComputedBlockStyle::new(&ComputedStyle::initial())
        },
        BuildOptions::default(),
        |b| {
            b.text(key(1), &"straße ﬁ! ".repeat(20));
        },
    );
    let content = layout.content();
    let source = content.first_line_source().expect("a first line's text");
    assert!(source.map.len() > 40, "{} entries", source.map.len());
    let len = content.text.len();
    let forward = 0..=len;
    let backward = (0..=len).rev();
    let jumps = (0..=len).map(|at| at * 37 % (len + 1));
    for order in [
        forward.collect::<Vec<_>>(),
        backward.collect(),
        jumps.collect(),
    ] {
        let cursor = content.text(FirstLineVariant::FirstLine).cursor();
        for at in order.into_iter().map(TextOffset::new) {
            assert_eq!(cursor.offset(at), source.offset(at), "at {at:?}");
        }
    }
}

/// The first line's text copies the text so far when the first differently
/// transformed text arrives, puts a collapsed space where its run began, and
/// ends with the first paragraph: no first line reaches past a forced break.
#[test]
fn the_first_lines_text_follows_the_first_paragraph() {
    use crate::style::TextCase::Uppercase;
    let first_line = styled(|_| {});
    let upper_first = cased(Uppercase, "en");
    let plain = ComputedStyle::initial();
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&first_line),
            ..ComputedBlockStyle::new(&plain)
        },
        BuildOptions::default(),
        |b| {
            b.text(key(1), "ab ");
            b.open_box(key(2), &plain, Some(&upper_first));
            b.text(key(3), "  cd ");
            b.close_box();
            b.text(key(4), " ef");
            b.line_break(key(5));
            b.open_box(key(6), &plain, Some(&upper_first));
            b.text(key(7), "gh");
            b.close_box();
        },
    );
    assert_eq!(text(&layout), "ab cd ef\ngh");
    let (first, _) = first_line_text(&layout);
    assert_eq!(first, "ab CD ef\n");
}

/// A first line that transforms as its elements do keeps no text of its own.
#[test]
fn a_first_line_that_transforms_alike_keeps_no_text() {
    let upper = cased(TextCase::Uppercase, "en");
    // The first-line style inherits the block's transform, as a cascade
    // would.
    let mut first_line = upper;
    first_line.font.size = 30.0;
    let (layout, _) = build_with(
        &ComputedBlockStyle {
            first_line: Some(&first_line),
            ..ComputedBlockStyle::new(&upper)
        },
        BuildOptions::default(),
        |b| {
            b.text(key(1), "ab");
            b.open_box(key(2), &upper, None);
            b.text(key(3), "cd");
            b.close_box();
        },
    );
    assert_eq!(text(&layout), "ABCD");
    assert!(layout.content().first_line_source().is_none());
    assert!(
        !layout
            .content()
            .flags
            .contains(ContentFlags::TRANSFORM_SIDE_TEXT)
    );
}

mod first_letter;
mod map;
