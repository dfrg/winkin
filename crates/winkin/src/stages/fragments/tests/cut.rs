//! Hyphen, ellipsis and clamp tests for line layout, in Chrome 153's numbers:
//!
//! - a soft hyphen's hyphen is its own item after the text, in the soft
//!   hyphen's box and at its level, as wide as its style's hyphen;
//! - the ellipsis follows what fits of a cut line, and the tail is laid out
//!   and hidden, though the line's first item keeps something;
//! - a clamp ends the block after its last line, cut for an ellipsis only
//!   where text follows.
//!
//! Chrome 153 headless gives the figures, on a page of 20 px Ahem in a 200 px
//! column. They are read off a screenshot and the text's client rectangles,
//! with the ellipsis and the text in different colours.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::config::Config;
use crate::layout::Item;
use crate::stages::lines::{
    BlockExtents, ExclusionsCheckpoint, FloatRequest, InlineExtents, PlacedFloat,
};
use crate::style::{Hyphens, InitialLetter, LineClamp, TextOverflow, TextWrapStyle};

/// Returns what line `n` draws, left to right, with each start and end from the area's line-left.
///
/// A text run gives its text, a hyphen `‐`, an ellipsis `…` and an atomic
/// inline `[]`. What an ellipsis hides is left out, as a painter leaves it.
fn drawn(layout: &Layout, n: usize) -> Vec<(String, f32, f32)> {
    reading_line(layout, n, false)
}

/// Returns what line `n` lays out and hides, in [`drawn`]'s form.
fn hidden(layout: &Layout, n: usize) -> Vec<(String, f32, f32)> {
    let all = reading_line(layout, n, true);
    let shown = drawn(layout, n);
    all.into_iter()
        .filter(|item| !shown.contains(item))
        .collect()
}

fn reading_line(layout: &Layout, n: usize, all: bool) -> Vec<(String, f32, f32)> {
    let Some(line) = layout.line(n) else {
        return Vec::new();
    };
    let left = line.metrics().left;
    let text = layout.text();
    let items: Vec<Item<'_>> = if all {
        line.all_items().collect()
    } else {
        line.items().collect()
    };
    items
        .into_iter()
        .filter_map(|item| {
            let (
                label,
                InlineExtents {
                    left: from,
                    right: to,
                },
            ) = match item {
                Item::Text(run) => (String::from(&text[run.text_range()]), run.inline()),
                Item::Generated(run) => (
                    String::from(match run.generated() {
                        Some(Generated::Hyphen) => "‐",
                        _ => "…",
                    }),
                    run.inline(),
                ),
                Item::Atomic(atomic) => (String::from("[]"), atomic.inline()),
                Item::Box(_) => return None,
            };
            Some((label, left + from, left + to))
        })
        .collect()
}

/// Returns a block style with `text-overflow: ellipsis`.
fn ellipsized() -> ComputedBlockStyle<'static> {
    ComputedBlockStyle {
        text_overflow: TextOverflow::Ellipsis,
        ..ComputedBlockStyle::default()
    }
}

/// Returns 20 px Ahem that does not wrap.
fn nowrap() -> ComputedStyle<'static> {
    let mut style = ahem(20.0);
    style.text.wrap_mode = TextWrapMode::NoWrap;
    style
}

fn item(label: &str, from: f32, to: f32) -> (String, f32, f32) {
    (String::from(label), from, to)
}

// The ellipsis ---------------------------------------------------------------

/// A line too long for its 200 px keeps the nine Xs that fit, then the ellipsis.
///
/// The ellipsis is Ahem's U+2026, an em wide. The rest is laid out in place
/// and hidden. Chrome 153 puts the text at 100 to 280 and the ellipsis at 280
/// to 300, with the block's left at 100.
#[test]
fn an_overflowing_line_keeps_what_fits_before_its_ellipsis() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        "XXXXXXXXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 200.0);
    let line = layout.line(0).expect("a line");
    assert!(line.has_ellipsis() && !line.is_hyphenated());
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXXXXXXX", 0.0, 180.0), item("…", 180.0, 200.0)]
    );
    assert_eq!(hidden(&layout, 0), [item("XXXXXX", 180.0, 300.0)]);
    // A line that fits is not cut, and a block that wraps cuts only the
    // line that still overflows.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        "XXXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 200.0);
    assert!(!layout.line(0).expect("a line").has_ellipsis());
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(20.0),
            ..ellipsized()
        },
        "XXXX XXXXXXXXXXXXX XX",
    );
    fixture.lay_out(&mut layout, 200.0);
    let cut: Vec<bool> = layout.lines().map(|line| line.has_ellipsis()).collect();
    assert_eq!(cut, [false, true, false]);
    assert_eq!(
        drawn(&layout, 1),
        [item("XXXXXXXXX", 0.0, 180.0), item("…", 180.0, 200.0)]
    );
}

/// A space just before the cut stays before the ellipsis only when it is in the kept text.
///
/// A space that is an item of its own goes with the tail. Chrome 153 sets
/// `XXXXXXXX XXXXXXXXXX` with its ellipsis at 280, after the space. It sets
/// `<span>XXXXXXXXX</span> <span>X</span>` with it at 280 too, after the Xs,
/// with the space hidden.
#[test]
fn a_space_before_the_cut_stays_where_it_is_the_text_kept() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        "XXXXXXXX XXXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXXXXXX ", 0.0, 180.0), item("…", 180.0, 200.0)]
    );
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        |b| {
            b.text(NodeKey(1), "XXXXXXXXX");
            b.text(NodeKey(2), " ");
            b.text(NodeKey(3), "X");
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXXXXXXX", 0.0, 180.0), item("…", 180.0, 200.0)]
    );
    assert_eq!(
        hidden(&layout, 0),
        [item(" ", 180.0, 200.0), item("X", 200.0, 220.0)]
    );
}

/// The ellipsis is set in the block's font, not the font of the text it follows.
///
/// After two Xs and three 40 px Ys, it is 20 px wide at 160. Chrome 153 puts
/// it at 260 to 280 from a left of 100. It sits on the line's baseline.
#[test]
fn the_ellipsis_is_the_blocks() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut big = nowrap();
    big.font.size = 40.0;
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        |b| {
            b.text(NodeKey(1), "XX");
            b.open_box(NodeKey(2), &big, None);
            b.text(NodeKey(3), "YYYYYYYYYY");
            b.close_box();
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [
            item("XX", 0.0, 40.0),
            item("YYY", 40.0, 160.0),
            item("…", 160.0, 180.0)
        ]
    );
    let line = layout.line(0).expect("a line");
    let ellipsis = line
        .items()
        .find_map(|item| match item {
            Item::Generated(run) => Some(run),
            _ => None,
        })
        .expect("an ellipsis");
    assert_eq!(ellipsis.key(), NodeKey(0), "the block's");
    assert_eq!(ellipsis.baseline(), line.metrics().ascent);
    assert_eq!(ellipsis.font().map(|font| font.size), Some(20.0));
    assert_eq!(ellipsis.glyphs().count(), 1, "U+2026, which Ahem has");
}

/// `text-indent` narrows what is kept.
///
/// Seven Xs start at 40, and the ellipsis is at 180. Chrome 153 gives 140 to
/// 280 and 280 to 300.
#[test]
fn an_indent_narrows_what_is_kept() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage {
                px: 40.0,
                fraction: 0.0,
            },
            hanging: false,
            each_line: false,
        },
        ..ellipsized()
    };
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..block
        },
        "XXXXXXXXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXXXXX", 40.0, 180.0), item("…", 180.0, 200.0)]
    );
}

/// Right to left, the ellipsis is at the line's left and the kept text at its right.
///
/// Latin in a right-to-left block, plain or overridden, keeps nine Xs at 20
/// to 200 with the ellipsis at 0 to 20. Chrome 153 sets both so.
#[test]
fn right_to_left_the_ellipsis_is_at_the_left() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ellipsized()
    };
    for bidi in [UnicodeBidi::Normal, UnicodeBidi::BidiOverride] {
        let root = reading(&nowrap(), Direction::Rtl, bidi);
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &root,
                ..block
            },
            "XXXXXXXXXXXXXXX",
        );
        fixture.lay_out(&mut layout, 200.0);
        assert_eq!(
            drawn(&layout, 0),
            [item("…", 0.0, 20.0), item("XXXXXXXXX", 20.0, 200.0)],
            "{bidi:?}"
        );
    }
}

/// An ellipsis is shaped in its line's direction, as Chrome's `LineTruncator` shapes it.
///
/// So a font's `rtla` gives it another glyph on a right-to-left line, here
/// 14 px wide rather than 10. Where each paragraph sets its own direction,
/// the paragraph's counts, not the block's. That happens under `direction:
/// auto`, or under `unicode-bidi: plaintext` in either block direction.
///
/// Chrome 153, with 20 px Ahem whose U+2026 has an `rtla` alternate, draws
/// the alternate under `plaintext` on a Hebrew line and the plain glyph on a
/// Latin one. Each line of a block holding both gets its own, and a clamped
/// line gets its paragraph's.
#[test]
fn the_ellipsis_is_shaped_in_its_lines_direction() {
    const FAMILY: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Ellipsis"))];
    let mut font = TestFont::new(
        "Test Ellipsis",
        &[(0x20, 0x7E), (0x5D0, 0x5EA), (0x2026, 0x2026)],
    );
    font.alternates = vec![(*b"rtla", vec![('\u{2026}', 700)])];
    let (plain, turned) = (
        font.glyph('\u{2026}'),
        font.alternate_glyph(*b"rtla", '\u{2026}'),
    );
    let mut fixture = Fixture::new(&[font], ahem_fallback(), StageCheck::Placed(check));
    let mut layout = Layout::new();
    // Each line's ellipsis, as its glyph and its extent from the area's left.
    let ellipses = |layout: &Layout| -> Vec<(u32, f32, f32)> {
        layout
            .lines()
            .filter_map(|line| {
                let left = line.metrics().left;
                line.items().find_map(|item| match item {
                    Item::Generated(run) if run.generated() == Some(Generated::Ellipsis) => {
                        let glyph = run.glyphs().next()?;
                        Some((
                            glyph.id,
                            left + run.inline().left,
                            left + run.inline().right,
                        ))
                    }
                    _ => None,
                })
            })
            .collect()
    };
    let hebrew: String = (0x5D0..=0x5EA).filter_map(char::from_u32).collect();
    let mut style = sized(&FAMILY, 20.0);
    style.text.wrap_mode = TextWrapMode::NoWrap;
    // A Latin paragraph, then a Hebrew one, each in its own direction. A
    // line holds ten letters. The Latin keeps nine before a plain ellipsis at
    // the right, and the Hebrew eight before the turned one at the left.
    let auto = ComputedBlockStyle {
        direction: BaseDirection::Auto,
        style: &style,
        ..ellipsized()
    };
    fixture.build(&mut layout, &auto, |b| {
        b.text(NodeKey(1), "XXXXXXXXXXXXXXXXXXXX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), &hebrew);
    });
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(
        ellipses(&layout),
        [(plain, 90.0, 100.0), (turned, 6.0, 20.0)]
    );
    // Under `plaintext` in a right-to-left block, a Latin paragraph reads
    // left to right, and so does its ellipsis.
    let plaintext = reading(&style, Direction::Rtl, UnicodeBidi::Plaintext);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            direction: BaseDirection::Rtl,
            style: &plaintext,
            ..ellipsized()
        },
        "XXXXXXXXXXXXXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(ellipses(&layout), [(plain, 90.0, 100.0)]);
    // A clamp after a Latin line, falling on a Hebrew one.
    let mut wrapping = sized(&FAMILY, 20.0);
    wrapping.text.wrap_mode = TextWrapMode::Wrap;
    let words: String = hebrew
        .chars()
        .take(24)
        .enumerate()
        .flat_map(|(at, ch)| {
            (at > 0 && at % 4 == 0)
                .then_some(' ')
                .into_iter()
                .chain([ch])
        })
        .collect();
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            direction: BaseDirection::Auto,
            line_clamp: LineClamp::Lines(2),
            ..ComputedBlockStyle::new(&wrapping)
        },
        |b| {
            b.text(NodeKey(1), "XXXX");
            b.line_break(NodeKey(2));
            b.text(NodeKey(3), &words);
        },
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(ellipses(&layout).len(), 1);
    assert_eq!(ellipses(&layout)[0].0, turned, "the clamped line's");
}

/// An atomic inline is kept whole or hidden.
///
/// - One that fits with the ellipsis keeps the ellipsis after it. Chrome 153
///   puts the text at 100 to 180, the box to 280 and the ellipsis at 280.
/// - One that does not fit is hidden, and the ellipsis follows what precedes
///   it. Chrome 153 puts an X at 100 and the ellipsis at 120.
/// - One that starts the line is kept whole however wide, with the ellipsis
///   after it past the band.
#[test]
fn an_atomic_inline_is_kept_whole_or_hidden() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = nowrap();
    let size = |inline: f32| BoxSize {
        inline,
        block: 10.0,
        baseline: None,
    };
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &root,
            ..ellipsized()
        },
        |b| {
            b.text(NodeKey(1), "XXXX");
            b.atomic(NodeKey(2), &root, None, size(100.0));
            b.text(NodeKey(3), "XXXXXX");
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [
            item("XXXX", 0.0, 80.0),
            item("[]", 80.0, 180.0),
            item("…", 180.0, 200.0)
        ]
    );
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &root,
            ..ellipsized()
        },
        |b| {
            b.text(NodeKey(1), "X");
            b.atomic(NodeKey(2), &root, None, size(300.0));
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("X", 0.0, 20.0), item("…", 20.0, 40.0)]
    );
    assert_eq!(hidden(&layout, 0), [item("[]", 20.0, 320.0)]);
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &root,
            ..ellipsized()
        },
        |b| {
            b.atomic(NodeKey(1), &root, None, size(300.0));
            b.text(NodeKey(2), "XXXXXX");
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("[]", 0.0, 300.0), item("…", 300.0, 320.0)]
    );
}

/// The line's first item keeps at least one cluster.
///
/// One X at 300 px is kept whole, with the ellipsis after it. Chrome 153
/// paints the X to the block's edge and no ellipsis inside it.
#[test]
fn the_first_item_keeps_a_cluster_at_least() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut huge = nowrap();
    huge.font.size = 300.0;
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        |b| {
            b.open_box(NodeKey(1), &huge, None);
            b.text(NodeKey(2), "XX");
            b.close_box();
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("X", 0.0, 300.0), item("…", 300.0, 320.0)]
    );
    assert_eq!(hidden(&layout, 0), [item("X", 300.0, 600.0)]);
}

/// A cut closes no box.
///
/// The box the cut falls in keeps its laid-out extent, over its hidden text
/// and to its own end edge past the band. Chrome's box fragment keeps its
/// laid-out size the same way. The span has 4 px borders around nine Xs
/// after three. Chrome 153's `getClientRects` gives 160 to 348 from a left of
/// 100. The ellipsis follows the five Xs that fit, at 164.
#[test]
fn a_cut_closes_no_box() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = nowrap();
    let bordered = ComputedStyle {
        paints: true,
        edges: EdgesGroup {
            border: Sides {
                left: 4.0,
                right: 4.0,
                ..Sides::ZERO
            },
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &root,
            ..ellipsized()
        },
        |b| {
            b.text(NodeKey(1), "XXX");
            b.open_box(NodeKey(2), &bordered, None);
            b.text(NodeKey(3), "XXXXXXXXX");
            b.close_box();
            b.text(NodeKey(4), "XX");
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [
            item("XXX", 0.0, 60.0),
            item("XXXXX", 64.0, 164.0),
            item("…", 164.0, 184.0)
        ]
    );
    assert_eq!(box_spans(&layout, 0), [(60.0, 248.0)]);
    let rects: Vec<(f32, f32)> = layout
        .box_fragments(NodeKey(2))
        .map(|piece| (piece.inline().left, piece.inline().right))
        .collect();
    assert_eq!(rects, [(60.0, 248.0)]);
}

/// Right to left, a cut through boxes leads the line with its ellipsis and keeps the boxes whole.
///
/// Two nested painted spans over the cut text keep their box parts. They
/// stay linked in box order past the ellipsis, and [`check`] follows the
/// links.
#[test]
fn right_to_left_a_cut_through_boxes_keeps_them() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ellipsized()
    };
    let root = reading(&nowrap(), Direction::Rtl, UnicodeBidi::BidiOverride);
    let painted = ComputedStyle {
        paints: true,
        ..root
    };
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &root,
            ..block
        },
        |b| {
            b.text(NodeKey(1), "XXX");
            b.open_box(NodeKey(2), &painted, None);
            b.text(NodeKey(3), "XXX");
            b.open_box(NodeKey(4), &painted, None);
            b.text(NodeKey(5), "XXXXXX");
            b.close_box();
            b.close_box();
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    let line = items(&layout, 0);
    assert_eq!(
        line[0].kind(),
        FragmentItemKind::Generated,
        "the ellipsis leads"
    );
    assert_eq!(
        drawn(&layout, 0),
        [
            item("…", 0.0, 20.0),
            item("XXX", 20.0, 80.0),
            item("XXX", 80.0, 140.0),
            item("XXX", 140.0, 200.0)
        ]
    );
    assert_eq!(box_spans(&layout, 0), [(-40.0, 140.0), (-40.0, 80.0)]);
}

/// A relayout at another width moves the cut, and returning restores it as a fresh layout would.
#[test]
fn a_cut_follows_the_width() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        "XXXXXXXXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 200.0);
    let at_200 = drawn(&layout, 0);
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXX", 0.0, 80.0), item("…", 80.0, 100.0)]
    );
    fixture.lay_out(&mut layout, 400.0);
    assert!(!layout.line(0).expect("a line").has_ellipsis());
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(drawn(&layout, 0), at_200);
}

// The clamp ------------------------------------------------------------------

/// A block clamped to two lines ends after its second, cut for an ellipsis where text follows.
///
/// The ellipsis goes after `XXXX XXXX` at 180 where the line has room, and
/// over its last X where it has not. Chrome 153, under
/// `-webkit-line-clamp: 2`, puts it at 280 to 300 from a left of 100 both
/// times. The block ends where its lines do.
#[test]
fn a_clamp_ends_the_block_after_its_last_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block_style = ahem(20.0);
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(2),
        ..ComputedBlockStyle::new(&block_style)
    };
    fixture.block_text(&mut layout, &clamped, "XXXX XXXX XXXX XXXX XXXX XXXX");
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(layout.lines().len(), 2);
    let cut: Vec<bool> = layout.lines().map(|line| line.has_ellipsis()).collect();
    assert_eq!(cut, [false, true]);
    assert_eq!(
        drawn(&layout, 1),
        [item("XXXX XXXX", 0.0, 180.0), item("…", 180.0, 200.0)]
    );
    assert_eq!(layout.metrics().block_end, 40.0);
    fixture.block_text(&mut layout, &clamped, "XXXXXXXXXX XXXXXXXXXX XXXXXXXXXX");
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 1),
        [item("XXXXXXXXX", 0.0, 180.0), item("…", 180.0, 200.0)]
    );
}

/// A clamp the text does not pass cuts nothing.
///
/// Two lines clamped to two are drawn whole, as Chrome 153 draws them, even
/// ended by a final `<br>` or a space. A line ending at a forced break with
/// text after it is cut. Chrome puts that ellipsis at 180 to 200, after
/// `XXXX`.
#[test]
fn a_clamp_the_text_does_not_pass_cuts_nothing() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(2),
        ..ComputedBlockStyle::new(&root)
    };
    for text in [
        "XXXXXXXXXX XXXXXXXXXX",
        "XXXXXXXXXX XXXXXXXXXX\n",
        "XXXXXXXXXX XXXXXXXXXX ",
    ] {
        let mut pre_line = root;
        pre_line.text.white_space_collapse = WhiteSpaceCollapse::PreserveBreaks;
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &pre_line,
                ..clamped
            },
            text,
        );
        fixture.lay_out(&mut layout, 200.0);
        assert!(layout.lines().all(|line| !line.has_ellipsis()), "{text:?}");
    }
    fixture.build(&mut layout, &clamped, |b| {
        b.text(NodeKey(1), "XXXX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "XXXX");
        b.line_break(NodeKey(4));
        b.text(NodeKey(5), "XXXX");
    });
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(
        drawn(&layout, 1),
        [item("XXXX", 0.0, 80.0), item("…", 80.0, 100.0)]
    );
    // `Lines(0)` clamps nothing.
    let none = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(0),
        ..ComputedBlockStyle::new(&root)
    };
    fixture.block_text(&mut layout, &none, "XXXX XXXX XXXX XXXX XXXX XXXX");
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(layout.lines().len(), 3);
}

/// A clamped line set at its end keeps its ellipsis inside the band.
///
/// Chrome 153 measures the room from the line's start before alignment. It
/// puts the ellipsis past the band's right, where the block clips it whole.
/// That is a Chrome limitation, deliberately not matched.
#[test]
fn a_clamped_line_set_at_its_end_keeps_its_ellipsis_in_the_band() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(2),
        ..aligned(TextAlign::Right, TextAlignLast::Auto)
    };
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(20.0),
            ..clamped
        },
        "XXXXXXXXX XXXXXXXXX XXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 1),
        [item("XXXXXXXX", 20.0, 180.0), item("…", 180.0, 200.0)]
    );
}

/// A clamped line keeps its hyphen before its ellipsis.
///
/// Chrome 153, under `-webkit-line-clamp: 1` over `XX&shy;XXXXXXXXXXXXXX`,
/// puts the Xs and the hyphen at 100 to 160 and the ellipsis at 160.
#[test]
fn a_clamped_line_keeps_its_hyphen() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block_style = ahem(20.0);
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(1),
        ..ComputedBlockStyle::new(&block_style)
    };
    fixture.block_text(&mut layout, &clamped, "XX\u{AD}XXXXXXXXXXXXXX");
    fixture.lay_out(&mut layout, 200.0);
    let line = layout.line(0).expect("a line");
    assert!(line.is_hyphenated() && line.has_ellipsis());
    assert_eq!(
        drawn(&layout, 0),
        [
            item("XX\u{AD}", 0.0, 40.0),
            item("‐", 40.0, 60.0),
            item("…", 60.0, 80.0)
        ]
    );
}

// The hyphen -----------------------------------------------------------------

/// A line broken at a soft hyphen draws its hyphen after it, in its box.
///
/// The hyphen is Ahem's U+2010, an em wide. Chrome 153, with `XX&shy;XXXX` in
/// 80 px of 20 px Ahem, reports the soft hyphen's rectangle at 40 to 60.
/// Soft hyphens not broken at draw nothing.
#[test]
fn a_line_broken_at_a_soft_hyphen_draws_its_hyphen() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let painted = ComputedStyle {
        paints: true,
        ..root
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &painted, None);
        b.text(NodeKey(2), "XX\u{AD}XX\u{AD}XX");
        b.close_box();
    });
    fixture.lay_out(&mut layout, 80.0);
    let hyphenated: Vec<bool> = layout.lines().map(|line| line.is_hyphenated()).collect();
    assert_eq!(hyphenated, [true, false]);
    assert_eq!(
        drawn(&layout, 0),
        [item("XX\u{AD}", 0.0, 40.0), item("‐", 40.0, 60.0)]
    );
    // The hyphen is in the box, which covers it.
    assert_eq!(box_spans(&layout, 0), [(0.0, 60.0)]);
    let hyphen = layout
        .line(0)
        .and_then(|line| {
            line.items().find_map(|item| match item {
                Item::Generated(run) => Some(run),
                _ => None,
            })
        })
        .expect("a hyphen");
    assert_eq!(hyphen.key(), NodeKey(2), "the text it ends");
    assert_eq!(hyphen.generated(), Some(Generated::Hyphen));
    assert!(hyphen.text_range().is_empty());
}

/// The hyphen is paid for where the line is measured.
///
/// In 51 px of 10 px Ahem, `12 4&shy;xx` ends its line with `4` and the
/// hyphen. `12 45&shy;xx` breaks at the space, since the hyphen does not fit.
/// A word with only its soft hyphen to break at overflows there and shows the
/// hyphen, as in WPT `hyphens-overflow-001`.
#[test]
fn the_hyphen_is_paid_for_where_the_line_is_measured() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for (text, lines, hyphen) in [
        ("12 4\u{AD}xx", ["12 4\u{AD}", "xx"], Some(40.0)),
        ("12 45\u{AD}xx", ["12 ", "45\u{AD}xx"], None),
        ("12345\u{AD}xx", ["12345\u{AD}", "xx"], Some(50.0)),
    ] {
        fixture.block_text(&mut layout, &ComputedBlockStyle::new(&ahem(10.0)), text);
        fixture.lay_out(&mut layout, 51.0);
        assert_eq!(texts(&layout), lines, "{text:?}");
        let drew = drawn(&layout, 0);
        let at = drew
            .iter()
            .find(|(label, ..)| label == "‐")
            .map(|&(_, from, _)| from);
        assert_eq!(at, hyphen, "{text:?}");
    }
}

/// `hyphens: none` shows no hyphen, and an empty `hyphenate-character` hyphenates at no cost.
///
/// `hyphens: none` offers no break at a soft hyphen.
#[test]
fn hyphens_none_shows_none_and_an_empty_hyphen_pays_nothing() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut none = ahem(20.0);
    none.text.hyphens = Hyphens::None;
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle::new(&none),
        "XX\u{AD}XXXX XX",
    );
    fixture.lay_out(&mut layout, 80.0);
    assert!(layout.lines().all(|line| !line.is_hyphenated()));
    let mut empty = ahem(20.0);
    empty.text.hyphenate_character = Some("");
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle::new(&empty),
        "XXX\u{AD}XXXX",
    );
    fixture.lay_out(&mut layout, 60.0);
    assert!(layout.line(0).expect("a line").is_hyphenated());
    assert_eq!(texts(&layout), ["XXX\u{AD}", "XXXX"]);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXX\u{AD}", 0.0, 60.0), item("‐", 60.0, 60.0)]
    );
    // Two characters take two ems. Under `hyphenate-character: 'AB'`,
    // Chrome 153 reports the soft hyphen's rectangle at 40 to 80.
    let mut two = ahem(20.0);
    two.text.hyphenate_character = Some("AB");
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&two), "XX\u{AD}XXXX");
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XX\u{AD}", 0.0, 40.0), item("‐", 40.0, 80.0)]
    );
}

/// The config chooses whether a space before the ellipsis goes with the tail.
///
/// Under `EllipsisSpace::Hidden`, white space ending the kept text goes with
/// the tail, so the ellipsis follows the last word. Under Chrome's `Kept`, a
/// kept space stays before the ellipsis. A relayout reads the context's
/// current config.
#[test]
fn a_space_before_the_ellipsis_goes_with_the_tail_where_the_config_says() {
    use crate::config::EllipsisSpace;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        "XXXXXXXX XXXXXXXXXX",
    );
    let config = *fixture.cx.config();
    fixture.cx.set_config(Config {
        ellipsis_space: EllipsisSpace::Hidden,
        ..config
    });
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXXXXXX", 0.0, 160.0), item("…", 160.0, 180.0)]
    );
    assert_eq!(hidden(&layout, 0), [item(" XXXXXXXXXX", 160.0, 380.0)]);
    // A text node kept whole also gives up the space it ends with.
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap(),
            ..ellipsized()
        },
        |b| {
            b.text(NodeKey(1), "XXXXXXXX ");
            b.text(NodeKey(2), "XXXXXXXXXX");
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXXXXXX", 0.0, 160.0), item("…", 160.0, 180.0)]
    );
    fixture.cx.set_config(config);
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("XXXXXXXX ", 0.0, 180.0), item("…", 180.0, 200.0)]
    );
}

/// Right to left, the hyphen takes the soft hyphen's level, so it sits left of the text it ends.
#[test]
fn right_to_left_the_hyphen_is_at_its_texts_left() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = reading(&ahem(20.0), Direction::Rtl, UnicodeBidi::BidiOverride);
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&root)
    };
    fixture.block_text(&mut layout, &block, "XX\u{AD}XXXX");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(
        drawn(&layout, 0),
        [item("‐", 40.0, 60.0), item("XX\u{AD}", 60.0, 100.0)]
    );
}

/// A justified line pays for its hyphen before spending its room, and ends with the hyphen at the edge.
///
/// The room goes to its one space.
#[test]
fn a_justified_line_ends_with_its_hyphen_at_the_edge() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = aligned(TextAlign::Justify, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(20.0),
            ..block
        },
        "XX XX\u{AD}XXXX",
    );
    fixture.lay_out(&mut layout, 150.0);
    assert_eq!(texts(&layout), ["XX XX\u{AD}", "XXXX"]);
    assert_eq!(
        drawn(&layout, 0),
        [item("XX XX\u{AD}", 0.0, 130.0), item("‐", 130.0, 150.0)]
    );
}

/// The first line's hyphen is in its first-line style, and later lines use the element's.
///
/// In Chrome 153, a `::first-line` at 40 px over 20 px Ahem draws its hyphen
/// 40 px wide.
#[test]
fn the_first_lines_hyphen_is_in_its_first_line_style() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let first = ahem(40.0);
    let mut b = layout.builder(
        &mut fixture.cx,
        NodeKey(0),
        &ComputedBlockStyle {
            first_line: Some(&first),
            ..ComputedBlockStyle::new(&root)
        },
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "XX\u{AD}XXXXXX XXXX\u{AD}XXXX");
    b.finish();
    fixture.lay_out(&mut layout, 150.0);
    assert_eq!(
        texts(&layout),
        ["XX\u{AD}", "XXXXXX ", "XXXX\u{AD}", "XXXX"]
    );
    assert_eq!(
        drawn(&layout, 0),
        [item("XX\u{AD}", 0.0, 80.0), item("‐", 80.0, 120.0)]
    );
    assert_eq!(
        drawn(&layout, 2),
        [item("XXXX\u{AD}", 0.0, 80.0), item("‐", 80.0, 100.0)]
    );
}

/// Min-content pays for the hyphen of a part ending at a soft hyphen.
///
/// `XXXX&shy;XX` in 20 px Ahem has a min-content of 100 px, not 80. Its
/// max-content has no hyphen.
#[test]
fn min_content_pays_the_hyphen() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle::new(&ahem(20.0)),
        "XXXX\u{AD}XX",
    );
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (100.0, 120.0));
}

// The initial letter ---------------------------------------------------------

/// A host that places floats at the line's left and narrows the lines beside them.
///
/// It gives an initial letter its room.
#[derive(Default)]
struct Kept {
    /// Each placed margin box as its top, bottom and right.
    placed: Vec<(f32, f32, f32)>,
}

impl Exclusions for Kept {
    fn band(&self, _line: usize, block: BlockExtents) -> InlineExtents {
        let left = self
            .placed
            .iter()
            .filter(|&&(top, bottom, _)| block.start < bottom && block.end > top)
            .map(|&(_, _, right)| right)
            .fold(f32::NEG_INFINITY, f32::max);
        InlineExtents {
            left,
            right: f32::INFINITY,
        }
    }

    fn below(&self, top: f32) -> Option<f32> {
        self.placed
            .iter()
            .map(|&(_, bottom, _)| bottom)
            .filter(|&bottom| bottom > top)
            .reduce(f32::min)
    }

    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        let (top, bottom) = (float.block_start, float.block_start + float.block_size);
        self.placed.push((top, bottom, float.inline_size));
        PlacedFloat {
            inline: InlineExtents {
                left: 0.0,
                right: float.inline_size,
            },
            block: BlockExtents {
                start: top,
                end: bottom,
            },
        }
    }

    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(u64::try_from(self.placed.len()).unwrap_or(u64::MAX))
    }

    fn rewind(&mut self, to: ExclusionsCheckpoint) {
        self.placed
            .truncate(usize::try_from(to.0).unwrap_or(usize::MAX));
    }
}

/// Returns a fixture configured as Chrome on Windows, where the numbers below were measured.
fn windows() -> Fixture {
    let mut fixture = fixture();
    fixture.cx.set_config(Config::chrome_windows());
    fixture
}

/// Returns a 20 px Ahem `::first-letter` style, an initial letter `size` lines tall sinking `sink`.
fn dropped(size: f32, sink: u32) -> ComputedStyle<'static> {
    let mut letter = ahem(20.0);
    letter.line.initial_letter = InitialLetter {
        size,
        sink,
        ..InitialLetter::NONE
    };
    letter
}

/// An initial letter on a line cut for an ellipsis is kept whole, as Chrome 153 keeps its letter box.
///
/// The ellipsis follows what fits beside the letter, or the letter itself
/// where nothing does, however far that overflows. The case is 20 px Ahem
/// with `initial-letter: 2`, a 45 px letter, read off a screenshot.
///
/// - In 200 px, the letter is at 0 to 45, six Xs at 45 to 165 and the
///   ellipsis at 165. Chrome on Windows is a pixel further, its box fitted to
///   DirectWrite's hinted ink, which the engine does not read.
/// - In 30 px, the ellipsis is at 45, after the letter.
/// - A letter with its punctuation, `"X`, is kept whole, with the ellipsis at
///   90. Otherwise the line's first item would keep only one cluster.
///
/// Chrome paints an ellipsis after the letter in the letter's colour, taking
/// the style of the preceding item's box. Here it is the block's, as CSS and
/// every other cut have it.
#[test]
fn a_cut_keeps_the_initial_letter_whole() {
    let mut fixture = windows();
    let mut layout = Layout::new();
    let letter = dropped(2.0, 2);
    let mut host = Kept::default();
    for (width, text, expected) in [
        (
            200.0,
            "XXXXXXXXXXXXXXXXXXXX",
            vec![
                item("X", 0.0, 45.0),
                item("XXXXXX", 45.0, 165.0),
                item("…", 165.0, 185.0),
            ],
        ),
        (
            30.0,
            "XXXXXXXXXXXXXXXXXXXX",
            vec![item("X", 0.0, 45.0), item("…", 45.0, 65.0)],
        ),
        (
            30.0,
            "\"XXXXXXXXXXXXXXXXXX",
            vec![item("\"X", 0.0, 90.0), item("…", 90.0, 110.0)],
        ),
    ] {
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &nowrap(),
                ..ellipsized()
            },
            |b| {
                b.set_first_letter(NodeKey(9), &letter, None);
                b.text(NodeKey(1), text);
            },
        );
        host.placed.clear();
        fixture.lay_out_with(&mut layout, Area::new(width), &mut host);
        assert_eq!(drawn(&layout, 0), expected, "{text} in {width}");
        assert_eq!(host.placed.len(), 1, "the letter's room is kept");
    }
}

/// A clamp counts the lines beside an initial letter, not the lines it spans.
///
/// The letter is set and its room kept whatever the clamp leaves. The values
/// are Chrome 153's, for `-webkit-line-clamp` over 20 px Ahem in 200 px with
/// `initial-letter: 3`, a 70 px letter.
///
/// - Clamped to two lines, the block is 40 px tall. Its second line holds
///   `XX XX` beside the letter, then the ellipsis.
/// - Clamped to one, it is 20 px, with `X XX` and the ellipsis.
/// - Raised to `3 1`, the first line moves down two lines. One line is 60 px
///   tall and two are 80, the second beside the letter's foot.
///
/// The text beside the letter starts at 70 here. Chrome on Windows starts it
/// at 71, its box fitted to DirectWrite's hinted ink, which the engine does
/// not read.
#[test]
fn a_clamp_counts_the_lines_beside_an_initial_letter() {
    let mut fixture = windows();
    let mut layout = Layout::new();
    let text = "XX XX XX XX XX XX XX XX XX XX XX XX XX XX XX XX XX XX XX XX";
    for (letter, lines, block_end, last) in [
        (
            dropped(3.0, 3),
            2,
            40.0,
            vec![item("XX XX", 70.0, 170.0), item("…", 170.0, 190.0)],
        ),
        (
            dropped(3.0, 3),
            1,
            20.0,
            vec![
                item("X", 0.0, 70.0),
                item("X XX", 70.0, 150.0),
                item("…", 150.0, 170.0),
            ],
        ),
        (
            dropped(3.0, 1),
            1,
            60.0,
            vec![
                item("X", 0.0, 70.0),
                item("X XX", 70.0, 150.0),
                item("…", 150.0, 170.0),
            ],
        ),
        (
            dropped(3.0, 1),
            2,
            80.0,
            vec![item("XX XX", 70.0, 170.0), item("…", 170.0, 190.0)],
        ),
    ] {
        let block_style = ahem(20.0);
        let clamped = ComputedBlockStyle {
            line_clamp: LineClamp::Lines(lines),
            ..ComputedBlockStyle::new(&block_style)
        };
        fixture.build(&mut layout, &clamped, |b| {
            b.set_first_letter(NodeKey(9), &letter, None);
            b.text(NodeKey(1), text);
        });
        let mut host = Kept::default();
        fixture.lay_out_with(&mut layout, Area::new(200.0), &mut host);
        let sink = letter.line.initial_letter.sink;
        let count = usize::try_from(lines).unwrap_or(usize::MAX);
        assert_eq!(layout.lines().len(), count, "{sink} in {lines}");
        assert_eq!(layout.metrics().block_end, block_end, "{sink} in {lines}");
        assert_eq!(drawn(&layout, count - 1), last, "{sink} in {lines}");
        assert_eq!(host.placed.len(), 1, "the letter's room is kept");
    }
}

// Clamping by height ----------------------------------------------------------

/// Returns a block style under `line-clamp: auto`, trimmed as `trim` says.
fn clamped_by_height(trim: TextBoxTrim) -> ComputedBlockStyle<'static> {
    ComputedBlockStyle {
        line_clamp: LineClamp::Auto,
        text_box_trim: trim,
        ..ComputedBlockStyle::default()
    }
}

/// Returns Ahem at `size` on lines twice as tall, as `line-height: 2` sets them.
fn double_spaced(size: f32) -> ComputedStyle<'static> {
    let mut style = ahem(size);
    style.line.height = LineHeight::Px(size * 2.0);
    style
}

/// Builds four lines of one letter each, parted by forced breaks.
fn four_lines(b: &mut LayoutBuilder<'_>) {
    for (at, letter) in ["A", "B", "C", "D"].into_iter().enumerate() {
        let key = u64::try_from(at).unwrap_or_default() * 2;
        if at > 0 {
            b.line_break(NodeKey(key));
        }
        b.text(NodeKey(key + 1), letter);
    }
}

/// `line-clamp: auto` keeps the lines that end within the block end the host gives.
///
/// The host sets the end from `height` or `max-height`, as CSS Overflow 4's
/// auto clamp point does. Chrome 153 lays it out so behind its `CSSLineClamp`
/// flag, and does nothing without it. The values are Chrome's, for four
/// one-letter lines of 50 px Ahem on 100 px lines:
///
/// - trimmed at the end, in 285 px (WPT `text-box-trim-line-clamp-auto-001`):
///   three lines, 275 tall, the third cut for an ellipsis after its `C`;
/// - untrimmed, in 285 px: two lines, 200 tall;
/// - trimmed at both ends, in 260 px: three lines, 250 tall;
/// - in 50 px or 0: no line, the block's end at its start;
/// - in 500 px, or with no end: all four, nothing cut.
///
/// The end is measured from the area's block start.
#[test]
fn a_clamp_by_height_keeps_the_lines_that_end_within_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for (trim, end, lines, block_end) in [
        (TextBoxTrim::TrimEnd, Some(285.0), 3, 275.0),
        (TextBoxTrim::None, Some(285.0), 2, 200.0),
        (TextBoxTrim::TrimBoth, Some(260.0), 3, 250.0),
        (TextBoxTrim::None, Some(50.0), 0, 0.0),
        (TextBoxTrim::None, Some(0.0), 0, 0.0),
        (TextBoxTrim::None, Some(500.0), 4, 400.0),
        (TextBoxTrim::None, None, 4, 400.0),
        (TextBoxTrim::None, Some(f32::NAN), 4, 400.0),
    ] {
        let block = clamped_by_height(trim);
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &double_spaced(50.0),
                ..block
            },
            four_lines,
        );
        let area = Area {
            block_end: end,
            ..Area::new(800.0)
        };
        fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
        let says = alloc::format!("{trim:?} in {end:?}");
        assert_eq!(layout.lines().len(), lines, "{says}");
        assert_eq!(layout.metrics().block_end, block_end, "{says}");
        let cut: Vec<bool> = layout.lines().map(|line| line.has_ellipsis()).collect();
        let mut expected = vec![false; lines];
        if lines < 4
            && let Some(last) = expected.last_mut()
        {
            *last = true;
        }
        assert_eq!(cut, expected, "{says}");
    }
    let block = clamped_by_height(TextBoxTrim::TrimEnd);
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &double_spaced(50.0),
            ..block
        },
        four_lines,
    );
    let area = Area {
        block_start: 30.0,
        block_end: Some(315.0),
        ..Area::new(800.0)
    };
    fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
    assert_eq!(layout.lines().len(), 3);
    assert_eq!(layout.metrics().block_end, 305.0);
    assert_eq!(
        drawn(&layout, 2),
        [item("C", 0.0, 50.0), item("…", 50.0, 100.0)]
    );
}

/// A clamp by height keeps soft-wrapped lines by where each would end the block.
///
/// `AA BB CC DD EE` in 20 px Ahem on 40 px lines, 110 px wide and clamped to
/// 100 px, keeps two lines and ends at 80. The second is cut for an ellipsis.
/// Chrome 153 keeps the same lines behind its flag.
///
/// Chrome there puts the ellipsis after `CC` and pushes `DD` past the clamp,
/// as CSS Overflow 4's `block-ellipsis` says. Here the clamped line is cut
/// like every clamped line, as Chrome's `LineTruncator` cuts a
/// `-webkit-line-clamp`.
#[test]
fn a_clamp_by_height_keeps_soft_wrapped_lines() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = clamped_by_height(TextBoxTrim::None);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &double_spaced(20.0),
            ..block
        },
        "AA BB CC DD EE",
    );
    let area = Area {
        block_end: Some(100.0),
        ..Area::new(110.0)
    };
    fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(layout.metrics().block_end, 80.0);
    let cut: Vec<bool> = layout.lines().map(|line| line.has_ellipsis()).collect();
    assert_eq!(cut, [false, true]);
}

/// Balanced lines that would reach past a clamp by height give way to the greedy lines.
///
/// Balancing chooses among the lines the clamp keeps. Where balanced lines
/// would pass the block's end and the measured greedy lines did not, the
/// greedy ones are kept. CSS Overflow 4 lets the UA rebalance only where the
/// clamp point's constraint still holds.
///
/// The text is eight words of 10 px Ahem on 20 px lines in 100 px, then a
/// forced break and two more. The first and fifth words are in a span of
/// 60 px lines.
///
/// - Greedy, the two tall words share the first line of five words, and the
///   second line is plain: 80 px in all.
/// - Balanced, four words a line, both lines are tall: 120 px.
/// - Clamped at 80, balancing would pass the end, so the greedy two lines
///   are kept.
/// - Clamped at 140, every line ends within it either way, so the balanced
///   lines are kept.
#[test]
fn balanced_lines_reaching_past_a_clamp_by_height_stay_greedy() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = ahem(10.0);
    style.line.height = LineHeight::Px(20.0);
    let mut tall = style;
    tall.line.height = LineHeight::Px(60.0);
    let words = |b: &mut LayoutBuilder<'_>| {
        for at in 0..8u64 {
            if at > 0 {
                b.text(NodeKey(100 + at), " ");
            }
            if at == 0 || at == 4 {
                b.open_box(NodeKey(300 + at), &tall, None);
                b.text(NodeKey(at), "X");
                b.close_box();
            } else {
                b.text(NodeKey(at), "X");
            }
        }
        b.line_break(NodeKey(200));
        b.text(NodeKey(201), "X X");
    };
    let ends = |layout: &Layout| -> Vec<usize> {
        layout.lines().map(|line| line.text_range().end).collect()
    };
    let laid = |fixture: &mut Fixture, layout: &mut Layout, block: &ComputedBlockStyle| {
        fixture.build(
            layout,
            &ComputedBlockStyle {
                style: &style,
                ..*block
            },
            words,
        );
        fixture.lay_out(layout, 100.0);
        (ends(layout), layout.metrics().block_end)
    };
    let greedy = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(2),
        ..ComputedBlockStyle::default()
    };
    let balanced = ComputedBlockStyle {
        text_wrap_style: TextWrapStyle::Balance,
        ..greedy
    };
    let (greedy_two, greedy_end) = laid(&mut fixture, &mut layout, &greedy);
    let (balanced_two, balanced_end) = laid(&mut fixture, &mut layout, &balanced);
    assert_eq!((greedy_end, balanced_end), (80.0, 120.0));
    assert_ne!(balanced_two, greedy_two);
    let all = ComputedBlockStyle {
        line_clamp: LineClamp::None,
        ..balanced
    };
    let (balanced_all, all_end) = laid(&mut fixture, &mut layout, &all);
    assert_eq!(all_end, 140.0);
    let auto = ComputedBlockStyle {
        line_clamp: LineClamp::Auto,
        ..balanced
    };
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..auto
        },
        words,
    );
    for (end, kept, block_end) in [(80.0, &greedy_two, 80.0), (140.0, &balanced_all, 140.0)] {
        let area = Area {
            block_end: Some(end),
            ..Area::new(100.0)
        };
        fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
        assert_eq!(&ends(&layout), kept, "in {end}");
        assert_eq!(layout.metrics().block_end, block_end, "in {end}");
    }
}

/// A relayout clamped by height equals a fresh layout, record for record, at every width and end.
///
/// Wherever it clamps, the measured lines past the clamp are taken back.
#[test]
fn a_relayout_clamped_by_height_equals_a_fresh_layout() {
    let mut fixture = fixture();
    let block = clamped_by_height(TextBoxTrim::TrimBoth);
    let style = double_spaced(20.0);
    let text = "AA BB CC DD EE FF GG HH II JJ KK LL";
    let mut layout = Layout::new();
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..block
        },
        text,
    );
    for (width, end) in [
        (110.0, Some(100.0)),
        (70.0, Some(250.0)),
        (300.0, Some(10.0)),
        (110.0, None),
        (45.0, Some(1000.0)),
        (110.0, Some(100.0)),
    ] {
        let area = Area {
            block_end: end,
            ..Area::new(width)
        };
        fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
        let mut fresh = Layout::new();
        fixture.block_text(
            &mut fresh,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            text,
        );
        fixture.lay_out_with(&mut fresh, area, &mut NoExclusions);
        let (a, b) = (layout.line_records(), fresh.line_records());
        assert_eq!(a.lines.as_slice(), b.lines.as_slice(), "{width} {end:?}");
        assert_eq!(a.block, b.block);
        assert_eq!(layout.metrics(), fresh.metrics());
    }
}
