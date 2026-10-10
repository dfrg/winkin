//! Tests of the room a line makes for ruby annotations and emphasis marks:
//! - what reaches past a line box moves the line down, less what the line
//!   before left under its text;
//! - the block's first line uses the room above the block the host names;
//! - what reaches under moves the next line down;
//! - the room a block lends below, none past a trimmed end;
//! - under `Config::emphasis_room = Uniform` every marked line grows alike.
//!
//! Figures are measured with Chrome 153 on 40px Ahem, where an annotation
//! or mark at half the size is 20 tall.

use alloc::vec::Vec;

use super::*;
use crate::config::{Config, EmphasisRoom};
use crate::style::{LineHeight, RubyPosition, TextBoxTrim};
use crate::tests::no_fonts;

/// Returns `style` at a line height of `factor` times its size.
fn leaded(style: &ComputedStyle<'static>, factor: f32) -> ComputedStyle<'static> {
    let mut style = *style;
    style.line.height = LineHeight::Factor(factor);
    style
}

/// Returns `style` setting emphasis marks over its text.
fn marked(style: &ComputedStyle<'static>) -> ComputedStyle<'static> {
    let mut style = *style;
    style.text.emphasis.marks = true;
    style
}

/// Returns each line's top and height, in pixels.
fn boxes(layout: &Layout) -> Vec<(f32, f32)> {
    layout
        .lines()
        .map(|line| {
            let metrics = line.metrics();
            (metrics.top, metrics.height())
        })
        .collect()
}

/// Builds `A<ruby>BB<rt>CC</rt></ruby>D ` `count` times into `layout` and
/// breaks it 200 wide, a column a line.
///
/// Each ruby's annotation goes on the side `position` gives it in turn.
fn columns(
    fixture: &mut Fixture,
    layout: &mut Layout,
    block: &ComputedBlockStyle<'_>,
    positions: &[RubyPosition],
) {
    let root = leaded(&ahem(40.0), 1.0);
    let small = leaded(&ahem(20.0), 1.0);
    fixture.build(
        layout,
        &ComputedBlockStyle {
            style: &root,
            ..*block
        },
        |b| {
            for (n, &position) in positions.iter().enumerate() {
                let mut ruby = root;
                ruby.ruby.position = position;
                let key = 10 * n as u64;
                b.text(NodeKey(key + 1), "A");
                b.open_ruby(NodeKey(key + 2), &ruby, None);
                b.text(NodeKey(key + 3), "BB");
                b.open_annotation(NodeKey(key + 4), &small, None);
                b.text(NodeKey(key + 5), "CC");
                b.close_annotation();
                b.close_ruby();
                b.text(NodeKey(key + 6), "D ");
            }
        },
    );
    fixture.lay_out(layout, 200.0);
}

/// At `line-height: 1` each line of a column takes the annotation's 20 over
/// its 40, as in Chrome.
///
/// Three lines come to 180. The annotation stands on the base's em box.
#[test]
fn a_line_with_an_annotation_grows_by_what_reaches_past_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let over = RubyPosition::Over;
    columns(
        &mut fixture,
        &mut layout,
        &ComputedBlockStyle::default(),
        &[over; 3],
    );
    assert_eq!(boxes(&layout), [(0.0, 60.0), (60.0, 60.0), (120.0, 60.0)]);
    assert_eq!(layout.metrics().block_end, 180.0);
    for line in layout.lines() {
        assert_eq!(line.metrics().ascent, 52.0);
        let annotation = line.annotations().next().expect("an annotation");
        assert_eq!(annotation.baseline(), 16.0);
    }
}

/// An annotation under one line and one over the next take their room
/// apart, as in Chrome.
///
/// The two lines come to 120, since neither leaves room on the side the
/// other reaches into.
#[test]
fn an_annotation_under_moves_the_next_line_down() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = ComputedBlockStyle::default();
    columns(
        &mut fixture,
        &mut layout,
        &block,
        &[RubyPosition::Under, RubyPosition::Over],
    );
    assert_eq!(boxes(&layout), [(0.0, 60.0), (60.0, 60.0)]);
    let first = layout.line(0).expect("a line");
    assert_eq!(first.metrics().descent, 28.0);
    let annotation = first.annotations().next().expect("an annotation");
    assert!(annotation.is_under());
    assert_eq!(annotation.baseline(), 56.0);
}

/// Padding over the block is room its first line's annotation may take, as
/// in Chrome.
///
/// The padding gives 12 of the 20 here, so the line grows by 8 and is 60
/// tall with the 12. The host names the room as `Area::room_above`, since
/// what the block before lends changes with the width.
#[test]
fn the_first_line_takes_the_room_above_the_block() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = ComputedBlockStyle::default();
    columns(&mut fixture, &mut layout, &block, &[RubyPosition::Over]);
    let area = Area {
        room_above: 12.0,
        ..Area::new(200.0)
    };
    fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
    assert_eq!(boxes(&layout), [(0.0, 48.0)]);
    // A negative room, or one that is not a number, counts as none.
    for room_above in [-5.0, f32::NAN] {
        let area = Area {
            room_above,
            ..Area::new(200.0)
        };
        fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
        assert_eq!(boxes(&layout), [(0.0, 60.0)]);
    }
}

/// A block reports what its last line leaves under its text as
/// `Layout::room_below`, and the host lends it to the next block.
///
/// Blocks of 20px Ahem on 30px lines carry a string mark
/// (`text-emphasis: 'X'`, 10 tall), measured in Chrome 153:
/// - The first block's marks reach 5 past its line box, so its line moves
///   down and it is 35 tall.
/// - The next block takes those 5 from the 5 the first left under its
///   text, and is 30.
/// - A plain block lends its room too: a marked block after it is 30.
#[test]
fn a_block_lends_the_next_the_room_under_its_last_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let plain = leaded(&ahem(20.0), 1.5);
    let loose = marked(&plain);
    fixture.text(&mut layout, &loose, "XX");
    fixture.lay_out(&mut layout, 400.0);
    assert_eq!(boxes(&layout), [(0.0, 35.0)]);
    let first = layout.metrics();
    assert_eq!((first.block_end, layout.room_below()), (35.0, 5.0));
    let lent = Area {
        room_above: layout.room_below(),
        ..Area::new(400.0)
    };
    fixture.lay_out_with(&mut layout, lent, &mut NoExclusions);
    assert_eq!(boxes(&layout), [(0.0, 30.0)]);
    // A plain block: its half leading under its text.
    fixture.text(&mut layout, &plain, "XX XX");
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(boxes(&layout), [(0.0, 30.0), (30.0, 30.0)]);
    assert_eq!(layout.room_below(), 5.0);
    // An empty block lends nothing.
    fixture.text(&mut layout, &plain, "");
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(layout.room_below(), 0.0);
}

/// An annotation under a block's last line reports a negative room below,
/// as in Chrome.
///
/// The annotation reaches 20 past the line box, which takes it, so the
/// block is 60 tall. The block reports a room below of −20, for its host to
/// let into the end padding, as Chrome's "Collapse annotation overflow and
/// padding" does. Where `text-box-trim` trims the block's end, the
/// annotation is trimmed with the line and the block lends nothing, as in
/// Chrome.
#[test]
fn a_block_reports_what_its_last_annotation_reaches_past_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let under = [RubyPosition::Under];
    columns(
        &mut fixture,
        &mut layout,
        &ComputedBlockStyle::default(),
        &under,
    );
    assert_eq!(boxes(&layout), [(0.0, 60.0)]);
    assert_eq!(layout.room_below(), -20.0);
    let trimmed = ComputedBlockStyle {
        text_box_trim: TextBoxTrim::TrimEnd,
        ..ComputedBlockStyle::default()
    };
    columns(&mut fixture, &mut layout, &trimmed, &under);
    let block = layout.metrics();
    assert_eq!((block.block_end, layout.room_below()), (40.0, 0.0));
    let plain = leaded(&ahem(20.0), 1.5);
    let trimmed = ComputedBlockStyle {
        style: &plain,
        ..trimmed
    };
    fixture.build(&mut layout, &trimmed, |b| b.text(NodeKey(1), "XX"));
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(layout.room_below(), 0.0);
}

/// A block in a context with no fonts trims its end by the metrics its
/// text falls back to, and then lends no room below, as a block with fonts
/// does. Untrimmed, it lends the half leading under its last line.
#[test]
fn a_block_with_no_fonts_lends_no_room_below_a_trimmed_end() {
    let mut fixture = fixture();
    fixture.cx = no_fonts();
    let mut layout = Layout::new();
    let plain = leaded(&ahem(20.0), 1.5);
    let mut below = Vec::new();
    for trim in [TextBoxTrim::None, TextBoxTrim::TrimEnd] {
        let block = ComputedBlockStyle {
            style: &plain,
            text_box_trim: trim,
            ..ComputedBlockStyle::default()
        };
        fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), "XX"));
        fixture.lay_out(&mut layout, 100.0);
        let trims_end = layout.line_records().block.trims_end;
        below.push((trims_end, layout.room_below()));
    }
    assert_eq!(below, [(false, 5.0), (true, 0.0)]);
}

/// Marked lines take their room from what the line before leaves, as in
/// Chrome.
///
/// - Four marked lines of 40px text at `line-height: 1.5` come to 250. The
///   marks reach 10 past the first line box, and each later line takes its
///   10 from the room the line before left.
/// - At `line-height: 1` there is no such room, so each line takes 20.
/// - At 1.25 the first line takes 15 and the rest 10.
/// - Under `Config::emphasis_room = Uniform` (beyond Chrome), every marked
///   line grows alike, 10 at 1.25, so the lines stay evenly spaced.
#[test]
fn marked_lines_share_the_room_between_them_as_chrome_does() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let loose = marked(&leaded(&ahem(40.0), 1.5));
    fixture.text(&mut layout, &loose, "XX XX XX XX");
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(
        boxes(&layout),
        [(0.0, 70.0), (70.0, 60.0), (130.0, 60.0), (190.0, 60.0)]
    );
    assert_eq!(layout.metrics().block_end, 250.0);
    let tight = marked(&leaded(&ahem(40.0), 1.0));
    fixture.text(&mut layout, &tight, "XX XX");
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(boxes(&layout), [(0.0, 60.0), (60.0, 60.0)]);
    let between = marked(&leaded(&ahem(40.0), 1.25));
    fixture.text(&mut layout, &between, "XX XX XX");
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(boxes(&layout), [(0.0, 65.0), (65.0, 60.0), (125.0, 60.0)]);
    // The uniform rule, beyond Chrome.
    fixture.cx.set_config(Config {
        emphasis_room: EmphasisRoom::Uniform,
        ..*fixture.cx.config()
    });
    fixture.text(&mut layout, &between, "XX XX XX");
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(boxes(&layout), [(0.0, 60.0), (60.0, 60.0), (120.0, 60.0)]);
}
