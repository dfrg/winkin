//! `::first-line` tests:
//! - the first line is fitted, measured and drawn in its own styles, and the
//!   lines after it in the text's;
//! - its end moves with the width, with nothing prepared again;
//! - a first line restyled only in its height keeps the text's glyphs;
//! - a first-line transform draws its own characters in place of the
//!   element's;
//! - the intrinsic sizes measure the first line in its styles, as Chrome's
//!   breaker does.

use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::style::LengthPercentage;
use crate::style::{LineHeight, TextCase};

/// Builds `text` in `root` into `layout` with `first` as the block's
/// `::first-line` style, and lays it out `width` pixels wide.
fn first_line(
    fixture: &mut Fixture,
    layout: &mut Layout,
    root: &ComputedStyle<'_>,
    first: &ComputedStyle<'_>,
    text: &str,
    width: f32,
) {
    let mut b = layout.builder(
        &mut fixture.cx,
        NodeKey(0),
        &ComputedBlockStyle {
            first_line: Some(first),
            ..ComputedBlockStyle::new(root)
        },
        BuildOptions::default(),
    );
    b.text(NodeKey(1), text);
    b.finish();
    fixture.lay_out(layout, width);
}

/// Returns line `n`'s glyphs as id, inline position and advance, in pixels.
fn line_glyphs(layout: &Layout, n: usize) -> Vec<(u32, f32, f32)> {
    let mut out = Vec::new();
    for item in layout.line(n).expect("a line").items() {
        if let crate::Item::Text(run) = item {
            for glyph in run.glyphs() {
                out.push((glyph.id, glyph.x, glyph.advance));
            }
        }
    }
    out
}

/// Returns each line's height, in pixels.
fn heights(layout: &Layout) -> Vec<f32> {
    layout.lines().map(|line| line.metrics().height()).collect()
}

/// The first line alone is fitted and set in its own styles.
///
/// The first line box is as tall as its 40px font.
#[test]
fn the_first_line_is_fitted_in_its_own_styles() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let text = "aa aa aa aa aa aa aa aa aa aa aa aa aa aa";
    first_line(
        &mut fixture,
        &mut layout,
        &ahem(20.0),
        &ahem(40.0),
        text,
        400.0,
    );
    assert_eq!(
        texts(&layout),
        ["aa aa aa ", "aa aa aa aa aa aa aa ", "aa aa aa aa"]
    );
    assert_eq!(widths(&layout), [320.0, 400.0, 220.0]);
    assert_eq!(heights(&layout), [40.0, 20.0, 20.0]);
    let flags: Vec<bool> = records(&layout)
        .iter()
        .map(|line| line.flags.contains(LineFlags::FIRST_LINE))
        .collect();
    assert_eq!(flags, [true, false, false]);
    // Its glyphs are drawn at its font size.
    let glyphs = line_glyphs(&layout, 0);
    assert_eq!(glyphs.len(), 8);
    assert!(glyphs.iter().all(|&(_, _, advance)| advance == 40.0));
    assert_eq!(glyphs.last().map(|&(_, x, _)| x), Some(280.0));
    assert!(
        line_glyphs(&layout, 1)
            .iter()
            .all(|&(_, _, advance)| advance == 20.0)
    );
}

/// The first line's end moves with the width, and the lines after it start
/// where it ends.
///
/// A relayout at every width equals a fresh layout, and each first line
/// matches the text set whole in its styles, glyph for glyph.
#[test]
fn the_first_line_moves_with_the_width() {
    let mut fixture = fixture();
    let root = sized(&LATIN, 16.0);
    let mut first = sized(&LATIN, 24.0);
    first.text.letter_spacing = LengthPercentage::from_px(1.5);
    first.text.word_break = WordBreak::BreakAll;
    let mut root = root;
    root.text.word_break = WordBreak::BreakAll;
    let text = "AVAV office flight baffle AVAV afflict fifty AV fluffier officials";
    let mut relaid = Layout::new();
    first_line(&mut fixture, &mut relaid, &root, &first, text, 300.0);
    for width in (40..420).step_by(13) {
        let width = width as f32;
        let mut fresh = Layout::new();
        first_line(&mut fixture, &mut fresh, &root, &first, text, width);
        fixture.lay_out(&mut relaid, width);
        assert_eq!(texts(&relaid), texts(&fresh), "at {width}");
        assert_eq!(widths(&relaid), widths(&fresh), "at {width}");
        // The first line matches the text set whole in its styles.
        let mut whole = Layout::new();
        fixture.text(&mut whole, &first, text);
        fixture.lay_out(&mut whole, width);
        assert_eq!(texts(&fresh).first(), texts(&whole).first(), "at {width}");
        let (ours, theirs) = (
            fresh.line(0).expect("a line"),
            whole.line(0).expect("a line"),
        );
        assert_eq!(line_glyphs(&fresh, 0), line_glyphs(&whole, 0), "at {width}");
        assert_eq!(ours.metrics().width, theirs.metrics().width, "at {width}");
    }
}

/// A first line restyled only in its line height reuses the text's shaping
/// and is taller.
#[test]
fn a_first_line_restyled_in_height_keeps_the_texts_glyphs() {
    let mut fixture = fixture();
    let root = ahem(20.0);
    let mut first = root;
    first.line.height = LineHeight::Factor(3.0);
    let text = "aa aa aa aa aa aa aa aa aa aa aa aa aa aa";
    let mut layout = Layout::new();
    first_line(&mut fixture, &mut layout, &root, &first, text, 200.0);
    let mut plain = Layout::new();
    fixture.text(&mut plain, &root, text);
    fixture.lay_out(&mut plain, 200.0);
    assert_eq!(texts(&layout), texts(&plain));
    assert!(layout.shaped().first_line().is_none());
    assert!(layout.measured().first_line().is_some());
    assert_eq!(heights(&layout).first(), Some(&60.0));
    assert_eq!(heights(&layout).get(1), Some(&20.0));
}

/// A first-line transform draws its own characters on the first line, and
/// the element's on the rest, both from the text as given.
///
/// Capitalize over a lower-case element capitalizes the text as written.
/// Chrome composes the two transforms, which its source marks as broken.
#[test]
fn a_first_line_transform_draws_its_own_characters() {
    let mut fixture = fixture();
    let mut root = sized(&LATIN, 20.0);
    root.text.transform.case = TextCase::Lowercase;
    let mut first = root;
    first.text.transform.case = TextCase::Capitalize;
    let text = "AVA VAV AVA VAV AVA VAV AVA VAV AVA VAV";
    let mut layout = Layout::new();
    first_line(&mut fixture, &mut layout, &root, &first, text, 100.0);
    assert_eq!(layout.text(), text.to_lowercase());
    // The first line draws the text as written, which is in capitals.
    let mut written = Layout::new();
    fixture.text(&mut written, &sized(&LATIN, 20.0), text);
    fixture.lay_out(&mut written, 100.0);
    assert_eq!(line_glyphs(&layout, 0), line_glyphs(&written, 0));
    // The second line draws as its text in lower case, set alone.
    let second = layout.line(1).expect("a second line");
    let rest = String::from(&layout.text()[second.text_range()]);
    let mut lower = Layout::new();
    fixture.text(&mut lower, &sized(&LATIN, 20.0), &rest);
    fixture.lay_out(&mut lower, 1000.0);
    let ids = |glyphs: Vec<(u32, f32, f32)>| glyphs.iter().map(|g| g.0).collect::<Vec<_>>();
    assert_eq!(ids(line_glyphs(&layout, 1)), ids(line_glyphs(&lower, 0)));
}

/// The intrinsic sizes measure the first line in its own styles, as Chrome
/// does.
///
/// Max-content sets the first paragraph, and min-content its first
/// unbreakable part, in the first line's styles.
#[test]
fn the_intrinsic_sizes_measure_the_first_line_in_its_styles() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    first_line(
        &mut fixture,
        &mut layout,
        &ahem(20.0),
        &ahem(40.0),
        "aaa bb ccccccccc",
        400.0,
    );
    let sizes = layout.intrinsic_sizes();
    // Max-content sets the whole paragraph at 40px. The widest unbreakable
    // part is the last word at 20px, wider than the first word at 40px.
    assert_eq!(sizes.max_content, 16.0 * 40.0);
    assert_eq!(sizes.min_content, 9.0 * 20.0);
    let mut layout = Layout::new();
    first_line(
        &mut fixture,
        &mut layout,
        &ahem(20.0),
        &ahem(40.0),
        "aaaaa bb cc",
        400.0,
    );
    let sizes = layout.intrinsic_sizes();
    assert_eq!(sizes.min_content, 5.0 * 40.0);
}
