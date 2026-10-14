//! Cost tests of long ruby columns split across many lines. They pin:
//! - the steps and seeks a relayout takes a line, the same for a column 1,
//!   2, 4 and 8 times as long;
//! - that bound with annotations longer and shorter than their base, under
//!   both `RubyBreakWithin` settings, and with ruby nested in the base.
//!
//! Each annotation word is in a box of its own.
//!
//! Steps and seeks are counted only in debug builds.

use super::*;
use crate::config::RubyBreakWithin;
use crate::work;

/// The words a column's base and annotations repeat.
const WORDS: &str = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ";

/// The length of a long column's annotation against its base.
#[derive(Copy, Clone, Debug)]
enum Note {
    /// Three times the base's words, at half its size: wider than the base.
    Longer,
    /// A quarter of the base's words, at half its size: narrower than the
    /// base.
    Shorter,
}

/// Builds one paragraph holding a ruby column whose base is `40 * scale`
/// words.
///
/// Where `nested`, each word of the base is a column of its own, with a
/// one-letter annotation, inside the long column's base.
fn long_column(fixture: &mut Fixture, layout: &mut Layout, scale: usize, note: Note, nested: bool) {
    let root = sized(&LATIN, 20.0);
    let small = sized(&LATIN, 10.0);
    let base = WORDS.repeat(4 * scale);
    let annotation = match note {
        Note::Longer => WORDS.repeat(12 * scale),
        Note::Shorter => WORDS.repeat(scale),
    };
    fixture.build(layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "before ");
        b.open_ruby(NodeKey(2), &root, None);
        if nested {
            let mut key = 100;
            for word in base.split_whitespace() {
                b.open_ruby(NodeKey(key), &root, None);
                b.text(NodeKey(key + 1), word);
                b.open_annotation(NodeKey(key + 2), &small, None);
                b.text(NodeKey(key + 3), "x");
                b.close_annotation();
                b.close_ruby();
                b.text(NodeKey(key + 4), " ");
                key += 5;
            }
        } else {
            b.text(NodeKey(3), &base);
        }
        b.open_annotation(NodeKey(4), &small, None);
        // Each word is in a box of its own, so the annotation has many
        // items.
        for (key, word) in (1_000_000..).zip(annotation.split_inclusive(' ')) {
            b.open_box(NodeKey(key), &small, None);
            b.text(NodeKey(key + 500_000), word);
            b.close_box();
        }
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), " after");
    });
}

/// Returns the steps and the seeks a relayout of a long column takes, each
/// per line, at 200 px.
fn relayout_work(
    fixture: &mut Fixture,
    scale: usize,
    note: Note,
    nested: bool,
    within: RubyBreakWithin,
) -> [f64; 2] {
    let mut config = *fixture.cx.config();
    config.ruby_break_within = within;
    fixture.cx.set_config(config);
    let mut layout = Layout::new();
    long_column(fixture, &mut layout, scale, note, nested);
    let area = Area::new(200.0);
    layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
    work::take();
    work::take_seeks();
    layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
    let (steps, seeks) = (work::take(), work::take_seeks());
    let lines = layout.lines().len() as f64;
    assert!(lines > 8.0, "{lines} lines");
    [steps as f64 / lines, seeks as f64 / lines]
}

/// Asserts that a column 2, 4 and 8 times as long takes as many steps and
/// seeks a line as the shortest, give or take a fifth.
fn assert_linear(note: Note, nested: bool, within: RubyBreakWithin) {
    let mut fixture = fixture();
    let short = relayout_work(&mut fixture, 1, note, nested, within);
    for scale in [2, 4, 8] {
        let long = relayout_work(&mut fixture, scale, note, nested, within);
        for (measure, (short, long)) in ["steps", "seeks"].iter().zip(short.iter().zip(long)) {
            assert!(
                long <= 1.2 * short + 2.0,
                "{note:?}, nested {nested}, {within:?}: {short:.1} {measure} a line at 1x, \
                 {long:.1} at {scale}x"
            );
        }
    }
}

/// A column's base breaks where its share of the room ends, and its wider
/// annotation where its own share does.
#[test]
fn splitting_a_column_under_a_longer_annotation_costs_linear_steps() {
    assert_linear(Note::Longer, false, RubyBreakWithin::BaseOpportunities);
}

/// A narrower annotation runs out on the column's first lines, and the base
/// goes on alone.
#[test]
fn splitting_a_column_under_a_shorter_annotation_costs_linear_steps() {
    assert_linear(Note::Shorter, false, RubyBreakWithin::BaseOpportunities);
}

/// Under `AllLevels`, a column breaks only where every nonempty annotation
/// has an opportunity too.
#[test]
fn splitting_a_column_on_all_levels_costs_linear_steps() {
    assert_linear(Note::Longer, false, RubyBreakWithin::AllLevels);
    assert_linear(Note::Shorter, false, RubyBreakWithin::AllLevels);
}

/// Each word of the base is a column of its own, which the long column's
/// breaks step over.
#[test]
fn splitting_a_column_with_nested_ruby_costs_linear_steps() {
    for within in [
        RubyBreakWithin::BaseOpportunities,
        RubyBreakWithin::AllLevels,
    ] {
        assert_linear(Note::Longer, true, within);
        assert_linear(Note::Shorter, true, within);
    }
}
