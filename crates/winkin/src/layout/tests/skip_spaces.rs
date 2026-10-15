//! `text-decoration-skip-spaces` tests, on Chrome 155's bars in 40px Ahem.
//! They pin:
//! - `start`, `end` and the initial `start end` leaving out a line's edge
//!   spaces, preserved or not, across runs and at forced breaks;
//! - `all` splitting a run's bar at the spaces inside it, but not at a run's
//!   own ends;
//! - `all` leaving out the letter-spacing that ends the line's content;
//! - the space separators that count, and tabs, which never take a bar;
//! - the run's own value deciding, whichever box draws the bar.

use super::*;
use crate::style::{LengthPercentage, TextDecorationSkipSpaces, TextWrapMode};

/// Returns 40px Ahem under `white-space: pre` with `skip`.
fn pre(skip: TextDecorationSkipSpaces) -> ComputedStyle<'static> {
    let mut style = sized(&AHEM_FAMILY, 40.0);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    style.text.wrap_mode = TextWrapMode::NoWrap;
    style.text.decoration_skip_spaces = skip;
    style
}

/// Returns `style` with `letter-spacing: px`.
fn spaced(style: ComputedStyle<'static>, px: f32) -> ComputedStyle<'static> {
    let mut style = style;
    style.text.letter_spacing = LengthPercentage::from_px(px);
    style
}

/// Returns the underlines line `at` of `layout` draws for the block, as
/// their left and right ends.
fn underlines(layout: &Layout, at: usize) -> Vec<(f32, f32)> {
    let Some(line) = layout.line(at) else {
        return Vec::new();
    };
    let block = |key: NodeKey| {
        if key == NodeKey(0) {
            Decorates::BeforeText
        } else {
            Decorates::None
        }
    };
    line.paints(block)
        .filter_map(|paint| match paint {
            Paint::DecorationBeforeText(bar) => Some(ends(bar.inline())),
            _ => None,
        })
        .collect()
}

/// Lays out `text` in a block of `style`, one text node, 1000px wide, and
/// returns the block's underlines on each line.
fn laid_out(style: &ComputedStyle<'static>, text: &str) -> Vec<Vec<(f32, f32)>> {
    let mut cx = context();
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(style), |b| {
        b.text(NodeKey(1), text);
    });
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    (0..layout.lines().count())
        .map(|at| underlines(&layout, at))
        .collect()
}

/// Each value leaves out the preserved spaces at the ends it names, and the
/// initial value both. Chrome 155, `pre`, `  AB  `: `none` underlines 0 to
/// 240, `start` 80 to 240, `end` 0 to 160, `start end` and `all` 80 to 160.
#[test]
fn a_line_leaves_out_the_edge_spaces_its_value_names() {
    use TextDecorationSkipSpaces::*;
    for (skip, bars) in [
        (None, [(0.0, 240.0)]),
        (Start, [(80.0, 240.0)]),
        (End, [(0.0, 160.0)]),
        (StartEnd, [(80.0, 160.0)]),
        (All, [(80.0, 160.0)]),
    ] {
        assert_eq!(laid_out(&pre(skip), "  AB  "), [bars], "{skip:?}");
    }
    assert_eq!(
        ComputedStyle::initial().text.decoration_skip_spaces,
        StartEnd
    );
}

/// The edge spaces are found across text runs and at a forced break. Chrome
/// 155, `pre`: `<span> </span> AB <span> </span>` underlines 80 to 160;
/// `AB  ⏎  CD` underlines 0 to 80 on the first line and 80 to 160 on the
/// second.
#[test]
fn the_edge_spaces_reach_across_runs_and_forced_breaks() {
    let style = pre(TextDecorationSkipSpaces::StartEnd);
    let mut cx = context();
    let mut layout = Layout::new();
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&style),
        |b| {
            b.open_box(NodeKey(1), &style, None);
            b.text(NodeKey(2), " ");
            b.close_box();
            b.text(NodeKey(3), " AB ");
            b.open_box(NodeKey(4), &style, None);
            b.text(NodeKey(5), " ");
            b.close_box();
        },
    );
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    assert_eq!(underlines(&layout, 0), [(80.0, 160.0)]);
    assert_eq!(
        laid_out(&style, "AB  \n  CD"),
        [vec![(0.0, 80.0)], vec![(80.0, 160.0)]]
    );
}

/// An atomic inline is content: the spaces beside it at the line's ends
/// keep their bar. Chrome 155, `pre`, a 30px inline-block, a space, `AB`, a
/// space and another: 30 to 190 underlined.
#[test]
fn spaces_beside_an_atomic_inline_keep_their_bar() {
    let style = pre(TextDecorationSkipSpaces::StartEnd);
    let mut cx = context();
    let mut layout = Layout::new();
    let atomic = ComputedStyle::initial();
    let size = BoxSize {
        inline: 30.0,
        block: 10.0,
        baseline: None,
    };
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&style),
        |b| {
            b.atomic(NodeKey(1), &atomic, None, size);
            b.text(NodeKey(2), " AB ");
            b.atomic(NodeKey(3), &atomic, None, size);
        },
    );
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    assert_eq!(underlines(&layout, 0), [(30.0, 190.0)]);
}

/// `all` splits a run's bar at each run of spaces between two of its
/// characters, word-spacing included. Chrome 155, `pre`, `  AB  CD  ` under
/// `all`: 80 to 160 and 240 to 320; `AB CD` with 20px word-spacing: 0 to
/// 80 and 140 to 220.
#[test]
fn all_leaves_out_the_spaces_inside_a_run() {
    let all = pre(TextDecorationSkipSpaces::All);
    assert_eq!(
        laid_out(&all, "  AB  CD  "),
        [[(80.0, 160.0), (240.0, 320.0)]]
    );
    let mut worded = all;
    worded.text.word_spacing = LengthPercentage::from_px(20.0);
    assert_eq!(laid_out(&worded, "AB CD"), [[(0.0, 80.0), (140.0, 220.0)]]);
    // `start end` keeps them.
    assert_eq!(
        laid_out(&pre(TextDecorationSkipSpaces::StartEnd), "AB CD"),
        [[(0.0, 200.0)]]
    );
}

/// Under `all`, spaces at a run's own ends inside the line keep their bar.
/// Chrome 155, `pre`: `AB  CD <span>EF</span>` underlines 0 to 80, then
/// 160 to 360 with the space before the span; `AB<span>  </span>CD`
/// underlines all of it.
#[test]
fn all_keeps_the_spaces_at_a_runs_ends() {
    let style = pre(TextDecorationSkipSpaces::All);
    let line = |first: &str, second: &str, third: &str| {
        let mut cx = context();
        let mut layout = Layout::new();
        build(
            &mut cx,
            &mut layout,
            &ComputedBlockStyle::new(&style),
            |b| {
                b.text(NodeKey(1), first);
                b.open_box(NodeKey(2), &style, None);
                b.text(NodeKey(3), second);
                b.close_box();
                b.text(NodeKey(4), third);
            },
        );
        layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
        underlines(&layout, 0)
    };
    assert_eq!(
        line("AB  CD ", "EF", ""),
        [(0.0, 80.0), (160.0, 280.0), (280.0, 360.0)]
    );
    assert_eq!(
        line("AB", "  ", "CD"),
        [(0.0, 80.0), (80.0, 160.0), (160.0, 240.0)]
    );
}

/// `all` leaves the letter-spacing off the right end of the line's
/// content, and nowhere else; `start end` keeps it. Chrome 155, `pre`,
/// 10px letter-spacing: `AB  CD` under `all` underlines 0 to 100 and 200
/// to 290, under `start end` 0 to 300; `AB ` under `all` 0 to 90; `AB`
/// before an inline-block keeps it, 0 to 100; -5px is kept.
#[test]
fn all_leaves_out_the_letter_spacing_ending_the_line() {
    let all = spaced(pre(TextDecorationSkipSpaces::All), 10.0);
    assert_eq!(laid_out(&all, "AB  CD"), [[(0.0, 100.0), (200.0, 290.0)]]);
    assert_eq!(laid_out(&all, "AB "), [[(0.0, 90.0)]]);
    let start_end = spaced(pre(TextDecorationSkipSpaces::StartEnd), 10.0);
    assert_eq!(laid_out(&start_end, "AB  CD"), [[(0.0, 300.0)]]);
    assert_eq!(
        laid_out(&spaced(pre(TextDecorationSkipSpaces::All), -5.0), "AB CD"),
        [[(0.0, 70.0), (105.0, 175.0)]]
    );
    let mut cx = context();
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&all), |b| {
        b.text(NodeKey(1), "AB");
        b.atomic(
            NodeKey(2),
            &ComputedStyle::initial(),
            None,
            BoxSize {
                inline: 30.0,
                block: 10.0,
                baseline: None,
            },
        );
    });
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    assert_eq!(underlines(&layout, 0), [(0.0, 100.0)]);
}

/// The no-break spaces and the other space separators are spaces; a
/// zero-width space is not, and joins the spaces around it. Chrome 155,
/// `all`: each of U+3000, U+2003, U+00A0, U+202F, U+205F and U+1680 between
/// two letters splits the bar, a lone U+200B does not, and
/// `A␣U+200B␣B` splits once.
#[test]
fn the_space_separators_are_spaces() {
    let all = pre(TextDecorationSkipSpaces::All);
    for space in [
        '\u{3000}', '\u{2003}', '\u{a0}', '\u{202f}', '\u{205f}', '\u{1680}',
    ] {
        let text = format!("A{space}B");
        let mut cx = context();
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&all), |b| {
            b.text(NodeKey(1), &text);
        });
        layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
        let run = runs(&layout, 0)[0];
        let clusters: Vec<_> = run.clusters().map(|c| ends(c.inline())).collect();
        assert_eq!(
            underlines(&layout, 0),
            [clusters[0], clusters[2]],
            "U+{:04X}",
            u32::from(space)
        );
    }
    assert_eq!(laid_out(&all, "A\u{200b}B").len(), 1);
    assert_eq!(laid_out(&all, "A\u{200b}B")[0].len(), 1);
    assert_eq!(
        laid_out(&all, "A \u{200b} B"),
        [[(0.0, 40.0), (120.0, 160.0)]]
    );
}

/// A tab takes no bar under any value. Chrome 155, `pre`, `A⇥B` with 8
/// space tab stops: 0 to 40 and 320 to 360, also under `none`.
#[test]
fn a_tab_takes_no_bar() {
    for skip in [
        TextDecorationSkipSpaces::None,
        TextDecorationSkipSpaces::StartEnd,
    ] {
        assert_eq!(
            laid_out(&pre(skip), "A\tB"),
            [[(0.0, 40.0), (320.0, 360.0)]],
            "{skip:?}"
        );
    }
}

/// The run's own value decides, not that of the box drawing the bar.
/// Chrome 155, `pre`, a block under `none` holding `A<span>B C</span>D E`,
/// the span under `all`: the block's underline leaves out only the span's
/// space, 0 to 80 and 120 to 280.
#[test]
fn the_runs_own_value_decides() {
    let none = pre(TextDecorationSkipSpaces::None);
    let all = pre(TextDecorationSkipSpaces::All);
    let mut cx = context();
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&none), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &all, None);
        b.text(NodeKey(3), "B C");
        b.close_box();
        b.text(NodeKey(4), "D E");
    });
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    assert_eq!(
        underlines(&layout, 0),
        [(0.0, 40.0), (40.0, 80.0), (120.0, 160.0), (160.0, 280.0)]
    );
}

/// In a right-to-left line, the spaces at its start lie on the right and
/// those at its end on the left. Chrome 155, `pre`, `direction: rtl`,
/// `  AB `: `start end` underlines AB alone, 40 to 120.
#[test]
fn a_right_to_left_line_leaves_out_its_edge_spaces() {
    let mut style = pre(TextDecorationSkipSpaces::StartEnd);
    style.bidi = BidiGroup {
        direction: Direction::Rtl,
        ..style.bidi
    };
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let mut cx = context();
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "  AB ");
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    assert_eq!(underlines(&layout, 0), [(40.0, 120.0)]);
}
