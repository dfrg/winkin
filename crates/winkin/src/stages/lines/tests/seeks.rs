//! Seek tests. They pin:
//! - how many searches preparing and breaking make: none to prepare plain
//!   prose or lay it out, one a line for hyphenated prose's line-edge
//!   costs, and one a hyphen for its generated text;
//! - the cursor over the line-edge costs, which finds what a search finds;
//! - none to prepare ruby, and a constant number a column or a line to lay
//!   it out again, its columns split across lines or not;
//! - a few searches to prepare a first line in capitals, however long its
//!   paragraph.
//!
//! Seeks are counted only in debug builds.

use super::*;
#[cfg(debug_assertions)]
use crate::work;

/// Words of prose, `words` of them, with soft hyphens in each long one
/// where `hyphenated`.
fn prose(words: usize, hyphenated: bool) -> String {
    let long = if hyphenated {
        [
            "con\u{AD}tin\u{AD}ues",
            "beau\u{AD}ti\u{AD}ful",
            "ar\u{AD}ti\u{AD}cle",
        ]
    } else {
        ["continues", "beautiful", "article"]
    };
    let short = ["the", "a", "of", "line", "text", "and"];
    let mut text = String::new();
    for word in 0..words {
        if word > 0 {
            text.push(' ');
        }
        let pick = if word % 3 == 1 {
            long.get(word % long.len())
        } else {
            short.get(word % short.len())
        };
        text.push_str(pick.copied().unwrap_or("x"));
    }
    text
}

/// Seeks made building `text`, laying it out at 180 px, and laying it out
/// again at 150 px, with the steps and lines that relayout made.
#[cfg(debug_assertions)]
struct Seeks {
    build: u64,
    first: u64,
    relayout: u64,
    steps: u64,
    lines: u64,
}

/// Counts the seeks building and laying out `text` make (see [`Seeks`]).
#[cfg(debug_assertions)]
fn seeks(fixture: &mut Fixture, text: &str) -> Seeks {
    let style = sized(&LATIN, 16.0);
    let mut layout = Layout::new();
    work::take_seeks();
    fixture.text(&mut layout, &style, text);
    let build = work::take_seeks();
    layout.break_lines(&mut fixture.cx, Area::new(180.0), &mut NoExclusions);
    let first = work::take_seeks();
    work::take();
    layout.break_lines(&mut fixture.cx, Area::new(150.0), &mut NoExclusions);
    let relayout = work::take_seeks();
    Seeks {
        build,
        first,
        relayout,
        steps: work::take(),
        lines: layout.lines().len() as u64,
    }
}

/// Plain prose is prepared and laid out without a search.
///
/// Every walk over it carries its runs, and no line's edge costs anything,
/// so the breaker looks nothing up.
#[cfg(debug_assertions)]
#[test]
fn plain_prose_is_laid_out_without_a_search() {
    let mut fixture = fixture();
    let seeks = seeks(&mut fixture, &prose(400, false));
    assert!(seeks.lines > 20, "{} lines", seeks.lines);
    assert_eq!(seeks.build, 0, "seeks to prepare");
    assert_eq!(seeks.first, 0, "seeks to lay out");
    assert_eq!(seeks.relayout, 0, "seeks to lay out again");
}

/// Hyphenated prose seeks its line-edge costs once a line.
///
/// The fit seeks the cost at a line's start, and its candidates move the
/// cursor from there, back as the fit steps back. The paragraph's end has
/// its cost found once, so no candidate moves the cursor past the line.
/// Preparing finds each soft hyphen's generated hyphen once.
#[cfg(debug_assertions)]
#[test]
fn hyphenated_prose_seeks_its_edge_costs_once_a_line() {
    let mut fixture = fixture();
    let text = prose(400, true);
    let hyphens = text.matches('\u{AD}').count() as u64;
    let seeks = seeks(&mut fixture, &text);
    assert!(seeks.lines > 20, "{} lines", seeks.lines);
    assert_eq!(seeks.build, hyphens, "seeks to prepare");
    // One a line, and the cost at the paragraph's end.
    assert_eq!(seeks.relayout, seeks.lines + 1, "seeks to lay out again");
    // A paragraph four times as long takes as many steps a line.
    let long = self::seeks(&mut fixture, &prose(1600, true));
    let (short, long) = (
        seeks.steps as f64 / seeks.lines as f64,
        long.steps as f64 / long.lines as f64,
    );
    assert!(
        long <= 1.2 * short + 1.0,
        "{short:.1} steps a line of 400 words, {long:.1} of 1600"
    );
}

/// The line-edge cost cursor finds what a search finds, across a
/// hyphenated paragraph's candidates.
///
/// Each line's cursor is sought at its start. It is asked every boundary of
/// the line from its end back, as the fit asks its candidates, then every
/// boundary on to the paragraph's end, as the scorer does.
#[test]
fn the_edge_cost_cursor_finds_what_a_search_finds() {
    let mut fixture = fixture();
    let style = sized(&LATIN, 16.0);
    let mut layout = Layout::new();
    fixture.text(&mut layout, &style, &prose(120, true));
    fixture.lay_out(&mut layout, 150.0);
    let costs = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .edge_costs();
    assert!(!costs.is_empty(), "the soft hyphens cost a line's end");
    let records = layout.line_records();
    let end = records
        .lines
        .as_slice()
        .last()
        .map_or(at(0), |line| line.clusters().end);
    let mut asked = 0;
    for line in records.lines.as_slice() {
        let clusters = line.clusters();
        let mut cursor = costs.cursor(clusters.start);
        assert_eq!(cursor.get(clusters.start), costs.get(clusters.start));
        let back = (clusters.start.get()..=clusters.end.get()).rev();
        let forward = clusters.start.get()..=end.get();
        for boundary in back.chain(forward).map(at) {
            assert_eq!(cursor.get(boundary), costs.get(boundary), "at {boundary:?}");
            asked += 1;
        }
    }
    assert!(asked > costs.as_slice().len(), "{asked} boundaries asked");
}

/// Builds `count` sentences of ruby prose: three short columns every ten
/// words, one annotation wider than its base and two narrower.
#[cfg(debug_assertions)]
fn ruby_prose(fixture: &mut Fixture, layout: &mut Layout, count: u64) {
    let root = sized(&LATIN, 16.0);
    let small = sized(&LATIN, 8.0);
    let columns = [
        ("alpha ", "bravo", "b"),
        (" charlie delta ", "echo", "an answer"),
        (" foxtrot golf hotel ", "india", "in"),
    ];
    fixture.build(layout, &ComputedBlockStyle::new(&root), |b| {
        for sentence in 0..count {
            for (column, (before, base, note)) in (0..).zip(columns) {
                let key = 20 * sentence + 5 * column;
                b.text(NodeKey(key + 1), before);
                b.open_ruby(NodeKey(key + 2), &root, None);
                b.text(NodeKey(key + 3), base);
                b.open_annotation(NodeKey(key + 4), &small, None);
                b.text(NodeKey(key + 5), note);
                b.close_annotation();
                b.close_ruby();
            }
            b.text(NodeKey(20 * sentence + 16), " juliet ");
        }
    });
}

/// Builds `count` ruby columns of ten words over ten words, which split
/// across lines 200 px long.
#[cfg(debug_assertions)]
fn breaking_ruby(fixture: &mut Fixture, layout: &mut Layout, count: u64) {
    let root = sized(&LATIN, 20.0);
    let small = sized(&LATIN, 10.0);
    let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet";
    fixture.build(layout, &ComputedBlockStyle::new(&root), |b| {
        for column in 0..count {
            let key = 5 * column;
            b.open_ruby(NodeKey(key + 1), &root, None);
            b.text(NodeKey(key + 2), words);
            b.open_annotation(NodeKey(key + 3), &small, None);
            b.text(NodeKey(key + 4), words);
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(key + 5), " ");
        }
    });
}

/// Counts the seeks `build` makes, then those of laying its content out
/// at `width` and again at `width` (see [`Seeks`]).
#[cfg(debug_assertions)]
fn ruby_seeks(
    fixture: &mut Fixture,
    build: impl FnOnce(&mut Fixture, &mut Layout),
    width: f32,
) -> Seeks {
    let mut layout = Layout::new();
    work::take_seeks();
    build(fixture, &mut layout);
    let built = work::take_seeks();
    layout.break_lines(&mut fixture.cx, Area::new(width), &mut NoExclusions);
    let first = work::take_seeks();
    work::take();
    layout.break_lines(&mut fixture.cx, Area::new(width), &mut NoExclusions);
    Seeks {
        build: built,
        first,
        relayout: work::take_seeks(),
        steps: work::take(),
        lines: layout.lines().len() as u64,
    }
}

/// Ruby is prepared without a search, and laid out again with a constant
/// number of searches a column.
///
/// The measure walks each column's items from its opening item, its font
/// runs from where the last column's ended, and its nested columns from
/// itself. The breaker walks the line's columns from the last line's.
/// What placement seeks for each column's boxes, and the line-edge costs,
/// are the rest.
#[cfg(debug_assertions)]
#[test]
fn ruby_seeks_a_constant_number_a_column() {
    let mut fixture = fixture();
    for sentences in [10, 40] {
        let seeks = ruby_seeks(&mut fixture, |f, l| ruby_prose(f, l, sentences), 700.0);
        let columns = 3 * sentences;
        assert!(seeks.lines > sentences / 4, "{} lines", seeks.lines);
        assert_eq!(seeks.build, 0, "seeks to prepare");
        // Five a column, one a line, and the cost at the paragraph's end.
        assert!(
            seeks.relayout <= 5 * columns + seeks.lines + 1,
            "{} seeks to lay {columns} columns out again on {} lines",
            seeks.relayout,
            seeks.lines
        );
    }
}

/// Ruby columns split across lines are laid out again with a constant
/// number of searches a line.
///
/// The breaker reads the pieces of the line it fits, and of the line
/// before, from the end of their table, and a rewind truncates it to its
/// mark. Placement seeks each line's pieces once. Preparing seeks the font
/// runs once, past the last column.
#[cfg(debug_assertions)]
#[test]
fn breaking_ruby_seeks_a_constant_number_a_line() {
    let mut fixture = fixture();
    let short = ruby_seeks(&mut fixture, |f, l| breaking_ruby(f, l, 10), 200.0);
    let long = ruby_seeks(&mut fixture, |f, l| breaking_ruby(f, l, 40), 200.0);
    assert!(short.lines > 20, "{} lines", short.lines);
    assert!(short.build <= 1, "{} seeks to prepare", short.build);
    assert_eq!(
        long.build, short.build,
        "seeks to prepare four times as much"
    );
    for seeks in [&short, &long] {
        assert!(
            seeks.relayout <= 6 * seeks.lines,
            "{} seeks to lay {} lines out again",
            seeks.relayout,
            seeks.lines
        );
    }
    let (short, long) = (
        short.relayout as f64 / short.lines as f64,
        long.relayout as f64 / long.lines as f64,
    );
    assert!(
        long <= short + 0.5,
        "{short:.1} seeks a line of 10 columns, {long:.1} of 40"
    );
}
/// Seeks made building `text` with a first line in capitals, and laying it
/// out at 180 px.
#[cfg(debug_assertions)]
fn first_line_seeks(fixture: &mut Fixture, text: &str) -> (u64, u64) {
    use crate::style::TextCase;
    let root = sized(&LATIN, 16.0);
    let mut first = root;
    first.text.transform.case = TextCase::Uppercase;
    let mut layout = Layout::new();
    work::take_seeks();
    let mut b = layout.builder(
        &mut fixture.cx,
        NodeKey(0),
        &ComputedBlockStyle {
            first_line: Some(&first),
            ..ComputedBlockStyle::new(&root)
        },
        BuildOptions::default(),
    );
    b.text(NodeKey(1), text);
    b.finish();
    let build = work::take_seeks();
    layout.break_lines(&mut fixture.cx, Area::new(180.0), &mut NoExclusions);
    (build, work::take_seeks())
}

/// A first line in capitals is prepared with a few searches, however long
/// its paragraph.
///
/// Each `ß` is two capitals on the first line, so its text differs in
/// length from the content's and its map has an entry for each. Font
/// selection and shaping read it through cursors that step through the map
/// in order.
#[cfg(debug_assertions)]
#[test]
fn a_transformed_first_line_is_prepared_with_few_searches() {
    let mut fixture = fixture();
    let text = |words| prose(words, false).replace("beautiful", "straße");
    let (short, short_lines) = first_line_seeks(&mut fixture, &text(400));
    let (long, long_lines) = first_line_seeks(&mut fixture, &text(1600));
    assert!(short <= 2, "{short} seeks to prepare 400 words");
    assert_eq!(short, long, "seeks to prepare 400 words and 1600");
    assert_eq!((short_lines, long_lines), (0, 0), "seeks to lay out");
}

/// Line layout finds a hyphenated line's hyphen once.
///
/// Laid out at 70 px, a third of the lines end at a soft hyphen. The
/// relayout seeks each line's edge costs once, the paragraph end's cost,
/// and each hyphen's generated text once: its piece keeps it for its count
/// and its items.
#[cfg(debug_assertions)]
#[test]
fn a_hyphenated_line_finds_its_hyphen_once() {
    use crate::stages::fragments::FragmentItemKind;
    let mut fixture = fixture();
    let style = sized(&LATIN, 16.0);
    let mut layout = Layout::new();
    fixture.text(&mut layout, &style, &prose(400, true));
    layout.break_lines(&mut fixture.cx, Area::new(70.0), &mut NoExclusions);
    work::take_seeks();
    layout.break_lines(&mut fixture.cx, Area::new(70.0), &mut NoExclusions);
    let relayout = work::take_seeks();
    let lines = layout.lines().len() as u64;
    let hyphens = layout
        .fragments()
        .items
        .as_slice()
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Generated)
        .count() as u64;
    assert!(hyphens > 20, "{hyphens} hyphens on {lines} lines");
    assert_eq!(
        relayout,
        lines + 1 + hyphens,
        "{lines} lines, {hyphens} hyphens"
    );
}
