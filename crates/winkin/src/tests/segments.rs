//! Segment walk tests: the walk agrees with the by-cluster lookups.
//! - Each segment's paragraph, item, script run and shaping run are those
//!   the lookups find for each of its clusters.
//! - Segments tile the clusters walked, each ending where one of its four
//!   runs ends.
//! - Items holding no cluster come in reading order, at their boundaries.
//! - A walk sought in the middle, started at a line's first item, or copied
//!   half way agrees with the whole walk cut there, and none allocates.
//!
//! The content is drawn at random as in `fast.rs`, with what the walk must
//! step over: empty paragraphs, atomic inlines, floats, ruby, `<wbr>`, empty
//! boxes, a generated `::first-letter`, bidi and CJK text, and restyled
//! first lines.

use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;

use super::allocator::count_allocations;
use super::fast::{RARE, Rng, WORDS, blocks, latin_1, roots, spans};
use super::{Fixture, StageCheck, han, han_fallback, key};
use crate::data::Id;
use crate::stages::analysis::ClusterId;
use crate::stages::content::ItemId;
use crate::stages::lines::{Area, NoExclusions};
use crate::stages::{LineStages, Segment, Segments, Step};
use crate::style::{ComputedStyle, FirstLineVariant, FontGroup};
use crate::{
    BoxSize, ComputedBlockStyle, FloatSide, Layout, LayoutBuilder, NodeKey, OriginalDisplay,
};

/// One step as compared: an item at a boundary, or a segment's clusters
/// with its runs' ids.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Seen {
    Item(ClusterId, usize),
    Segment(ClusterId, ClusterId, [usize; 3]),
}

fn seen(step: Step) -> Seen {
    match step {
        Step::Item { at, id } => Seen::Item(at, id.get()),
        Step::Segment(s) => Seen::Segment(
            s.start,
            s.end,
            [s.paragraph.get(), s.item.get(), s.run.get()],
        ),
    }
}

fn walk(walk: Segments<'_>) -> Vec<Seen> {
    walk.map(seen).collect()
}

/// Returns the steps of `whole`, a walk over every cluster, that a walk over
/// `range` takes.
///
/// These are the segments cut to the range and the items at its boundaries
/// before its end, and at its end too where that is `text_end`.
fn cut(whole: &[Seen], range: &Range<ClusterId>, text_end: ClusterId) -> Vec<Seen> {
    let (s, e) = (range.start, range.end);
    whole
        .iter()
        .filter_map(|step| match *step {
            Seen::Item(at, id) => {
                ((s <= at && at < e) || (at == e && e == text_end)).then_some(Seen::Item(at, id))
            }
            Seen::Segment(start, end, ids) => {
                let (start, end) = (start.max(s), end.min(e));
                (start < end).then_some(Seen::Segment(start, end, ids))
            }
        })
        .collect()
}

/// Builds one case's content into `b`: words, spans, forced breaks, empty
/// paragraphs, atomic inlines, floats, `<wbr>`, ruby, empty boxes and
/// sometimes a `::first-letter`.
fn content(
    rng: &mut Rng,
    b: &mut LayoutBuilder<'_>,
    root: &ComputedStyle<'_>,
    spans: &[ComputedStyle<'_>],
) {
    let mut n = 1u64;
    let mut next = || {
        n += 1;
        NodeKey(n)
    };
    if rng.chance(15) {
        b.line_break(next());
    }
    if rng.chance(10) {
        b.set_first_letter(next(), &spans[rng.below(spans.len())], None);
    }
    let mut open = 0;
    let word = |rng: &mut Rng| {
        if rng.chance(25) {
            RARE[rng.below(RARE.len())]
        } else {
            WORDS[rng.below(WORDS.len())]
        }
    };
    for _ in 0..3 + rng.below(40) {
        match rng.below(100) {
            0..55 => {
                b.text(next(), word(rng));
                b.text(next(), " ");
            }
            55..62 if open < 3 => {
                b.open_box(next(), &spans[rng.below(spans.len())], None);
                open += 1;
            }
            62..70 if open > 0 => {
                b.close_box();
                open -= 1;
            }
            70..75 => {
                b.line_break(next());
                if rng.chance(30) {
                    b.line_break(next());
                }
            }
            75..79 => b.atomic(
                next(),
                root,
                None,
                BoxSize {
                    inline: 12.0,
                    block: 10.0,
                    baseline: None,
                },
            ),
            79..80 => b.absolute(next(), OriginalDisplay::Inline),
            80..81 => b.float(
                next(),
                root,
                FloatSide::Left,
                BoxSize {
                    inline: 20.0,
                    block: 8.0,
                    baseline: None,
                },
            ),
            81..85 => b.break_opportunity(),
            85..91 => {
                b.open_ruby(next(), root, None);
                b.text(next(), word(rng));
                b.open_annotation(next(), root, None);
                b.text(next(), word(rng));
                b.close_annotation();
                b.close_ruby();
            }
            _ => {
                b.open_box(next(), &spans[rng.below(spans.len())], None);
                b.close_box();
            }
        }
    }
    for _ in 0..open {
        b.close_box();
    }
    if rng.chance(15) {
        b.line_break(next());
    }
}

/// Checks that the walk over `range` in `stages` agrees with the lookups.
///
/// - Each segment's runs are those found for each of its clusters, and its
///   accessors read their rows; the script run comes by the shaping run's
///   link.
/// - The segments tile the range, each ending where one of its runs ends.
/// - Each item comes once, in order; an item holding no cluster comes at
///   its boundary.
/// - The paragraph changes only where a shaping run ends, since no shaping
///   run crosses a paragraph.
fn agrees(stages: &LineStages<'_>, range: Range<ClusterId>, case: u64) {
    let analysis = stages.analysis;
    let items = &stages.content.items;
    let item_clusters = &analysis.item_clusters;
    let text_end = analysis.clusters.end_id();
    let mut at = range.start;
    let mut last: Option<[usize; 4]> = None;
    let mut seen_items = Vec::new();
    for step in Segments::new(stages, range.clone()) {
        match step {
            Step::Item { at: there, id } => {
                assert_eq!(there, at, "case {case}: an item where the walk stands");
                let clusters = item_clusters.range(id);
                assert!(clusters.is_empty(), "case {case}: {id:?} holds clusters");
                assert_eq!(clusters.start, at, "case {case}: {id:?} sits elsewhere");
                assert!(items.get(id).is_some(), "case {case}: {id:?} has a row");
                seen_items.push(id);
                last = None;
            }
            Step::Segment(s) => {
                assert_eq!(s.start, at, "case {case}: segments tile the clusters");
                assert!(s.start < s.end, "case {case}: an empty segment");
                let script = s.run_row(stages).map(|run| run.script_run);
                for c in Id::get(s.start)..Id::get(s.end) {
                    let c = ClusterId::new(c);
                    assert_eq!(
                        analysis.paragraphs.containing(c),
                        Some(s.paragraph),
                        "case {case} {c:?}"
                    );
                    let holding = item_clusters.range(s.item);
                    assert!(holding.contains(&c), "case {case} {c:?}");
                    assert_eq!(
                        stages.shaped.runs.containing(c),
                        Some(s.run),
                        "case {case} {c:?}"
                    );
                    assert_eq!(analysis.runs.containing(c), script, "case {case} {c:?}");
                }
                // Each accessor reads the row its id names.
                let item = items.get(s.item);
                let run = stages.shaped.runs.get(s.run);
                let script_row = script.and_then(|script| analysis.runs.get(script));
                assert!(item.is_some() && run.is_some() && script_row.is_some());
                assert_eq!(s.item_row(stages), item);
                assert_eq!(s.run_row(stages), run);
                assert_eq!(
                    s.paragraph_row(stages),
                    analysis.paragraphs.get(s.paragraph)
                );
                assert_eq!(s.script_row(stages), script_row);
                assert_eq!(
                    s.text_facts(stages),
                    item.map(|item| stages.text_facts(item.node))
                );
                let ids = [
                    s.paragraph.get(),
                    s.item.get(),
                    script.map_or(usize::MAX, Id::get),
                    s.run.get(),
                ];
                assert_ne!(
                    last,
                    Some(ids),
                    "case {case}: segments end where a run does"
                );
                last = Some(ids);
                if seen_items.last() != Some(&s.item) {
                    seen_items.push(s.item);
                }
                at = s.end;
            }
        }
    }
    assert_eq!(at, range.end, "case {case}: the walk reaches its end");
    // Each item with a cluster in the range, and each holding none at a
    // boundary before the range's end or at the text's end: once, in order.
    let expected: Vec<_> = items
        .iter()
        .map(|(id, _)| id)
        .filter(|&id| {
            let clusters = item_clusters.range(id);
            if clusters.is_empty() {
                (range.start <= clusters.start && clusters.start < range.end)
                    || (clusters.start == range.end && range.end == text_end)
            } else {
                clusters.start.max(range.start) < clusters.end.min(range.end)
            }
        })
        .collect();
    assert_eq!(seen_items, expected, "case {case}: the items, in order");
}

/// Over random contents:
/// - the whole walk in each variant agrees with the lookups;
/// - a walk sought anywhere is the whole walk cut there;
/// - each line's walk from its first item equals the one sought at its
///   start, with the breaker's cursor where the seek puts it;
/// - a copy taken half way walks on as the walk does;
/// - no walk allocates.
#[test]
fn a_walk_holds_the_runs_each_cluster_is_in() {
    let mut fixture = Fixture::new(
        &[latin_1(), han()],
        han_fallback("Test Han"),
        StageCheck::Placed(|_| {}),
    );
    let roots = roots();
    for case in 0..200u64 {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d ^ (case + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let root = roots[rng.below(roots.len())];
        let spans = spans(&root);
        let first = ComputedStyle {
            font: FontGroup {
                size: 22.0,
                ..root.font
            },
            ..root
        };
        let block = blocks(&root, &first, &mut rng);
        let mut layout = Layout::new();
        fixture.build(&mut layout, &block, |b| content(&mut rng, b, &root, &spans));
        let stages = layout.stages();
        let analysis = stages.analysis;
        let text_end = analysis.clusters.end_id();
        let text = stages.variant(FirstLineVariant::Standard);
        agrees(&text, ClusterId::new(0)..text_end, case);
        let whole = walk(Segments::new(&text, ClusterId::new(0)..text_end));
        if let Some(first) = stages.first_line()
            && let Some(reach) = analysis.first_line_reach(stages.content)
        {
            agrees(&first, reach, case);
        }
        // Sought anywhere.
        for _ in 0..8 {
            let a = rng.below(text_end.get() + 1);
            let b = rng.below(text_end.get() + 1);
            let range = ClusterId::new(a.min(b))..ClusterId::new(a.max(b));
            agrees(&text, range.clone(), case);
            let sought = walk(Segments::new(&text, range.clone()));
            assert_eq!(
                sought,
                cut(&whole, &range, text_end),
                "case {case}: {range:?}"
            );
            // From the paragraph and item a caller holds, with no search.
            if let Some(paragraph) = analysis.paragraphs.containing(range.start) {
                let item = analysis_cursor(&layout, range.start);
                assert_eq!(
                    walk(Segments::from_item(&text, paragraph, item, range.clone())),
                    sought,
                    "case {case}: {range:?} from its item"
                );
            }
        }
        // A copy walks on as the walk does.
        let mut half = Segments::new(&text, ClusterId::new(0)..text_end);
        for _ in 0..whole.len() / 2 {
            half.next();
        }
        let ahead = half;
        assert_eq!(walk(ahead), walk(half), "case {case}: a copy");
        assert_eq!(
            count_allocations(|| {
                let steps = Segments::new(&text, ClusterId::new(0)..text_end).count();
                assert_eq!(steps, whole.len());
            }),
            0,
            "case {case}: a walk allocates"
        );
        // The lines, each walked from its first item.
        for width in [23.0, 140.0, 1.0e6] {
            layout.break_lines(&mut fixture.cx, Area::new(width), &mut NoExclusions);
            let stages = layout.stages();
            for (id, line) in layout.line_records().lines.iter() {
                let line_stages = stages.variant(line.variant());
                let clusters = line.clusters();
                assert_eq!(
                    line.first_item,
                    analysis_cursor(&layout, clusters.start),
                    "case {case} at {width}: {id:?}'s first item"
                );
                let from_line = walk(Segments::from_item(
                    &line_stages,
                    line.paragraph,
                    line.first_item,
                    line.clusters(),
                ));
                assert_eq!(
                    from_line,
                    walk(Segments::new(&line_stages, clusters.clone())),
                    "case {case} at {width}: {id:?}"
                );
                agrees(&line_stages, clusters, case);
                assert_eq!(
                    count_allocations(|| {
                        assert_eq!(
                            Segments::from_item(
                                &line_stages,
                                line.paragraph,
                                line.first_item,
                                line.clusters()
                            )
                            .count(),
                            from_line.len()
                        );
                    }),
                    0,
                    "case {case}: a line's walk allocates"
                );
            }
        }
    }
}

/// Returns where a walk from boundary `at` starts among `layout`'s items,
/// found by search.
fn analysis_cursor(layout: &Layout, at: ClusterId) -> ItemId {
    layout.analysis().item_clusters.cursor_containing(at).id()
}

/// A walk over a box's edges, an atomic inline, a ruby with its annotation,
/// a forced break and the empty paragraph after it.
///
/// Items holding no cluster come at their boundaries, before the clusters
/// after them. The annotation's text is a segment of its own item. The
/// text's last items, at its end, belong to the walk.
#[test]
fn a_walk_meets_the_items_between_its_segments() {
    let mut fixture = Fixture::new(
        &[latin_1()],
        han_fallback("Test Latin-1"),
        StageCheck::Placed(|_| {}),
    );
    let root = roots()[0];
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(key(1), "ab");
        b.open_box(key(2), &root, None);
        b.text(key(3), "cd");
        b.close_box();
        b.atomic(
            key(4),
            &root,
            None,
            BoxSize {
                inline: 10.0,
                block: 10.0,
                baseline: None,
            },
        );
        b.open_ruby(key(5), &root, None);
        b.text(key(6), "e");
        b.open_annotation(key(7), &root, None);
        b.text(key(8), "f");
        b.close_annotation();
        b.close_ruby();
        b.line_break(key(9));
        b.open_box(key(10), &root, None);
        b.close_box();
    });
    let stages = layout.stages();
    let text = stages.variant(FirstLineVariant::Standard);
    let end = stages.analysis.clusters.end_id();
    let kinds: Vec<_> = Segments::new(&text, ClusterId::new(0)..end)
        .map(|step| match step {
            Step::Item { at, id } => vec![0, at.get(), id.get()],
            Step::Segment(s) => vec![1, s.start.get(), s.end.get(), s.item.get()],
        })
        .collect();
    // ab | open | cd | close | U+FFFC | ruby open | e | annotation open | f
    // | annotation close | ruby close | \n | open | close: the last two at
    // the text's end, in the empty paragraph after the break.
    assert_eq!(
        kinds,
        [
            vec![1, 0, 2, 0],
            vec![0, 2, 1],
            vec![1, 2, 4, 2],
            vec![0, 4, 3],
            vec![1, 4, 5, 4],
            vec![0, 5, 5],
            vec![1, 5, 6, 6],
            vec![0, 6, 7],
            vec![1, 6, 7, 8],
            vec![0, 7, 9],
            vec![0, 7, 10],
            vec![1, 7, 8, 11],
            vec![0, 8, 12],
            vec![0, 8, 13],
        ]
    );
    // A walk to before the text's end leaves its last items to the next.
    let to = ClusterId::new(7);
    assert!(matches!(
        Segments::new(&text, ClusterId::new(0)..to).last(),
        Some(Step::Segment(s)) if s.end == to
    ));
}

/// Content with no text walks no segment, but its items, all at the text's
/// end (which is its start), belong to the walk. A cleared layout walks
/// nothing.
#[test]
fn a_walk_over_no_text_meets_only_its_items() {
    let mut fixture = Fixture::new(
        &[latin_1()],
        han_fallback("Test Latin-1"),
        StageCheck::Placed(|_| {}),
    );
    let root = roots()[0];
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(key(1), &root, None);
        b.close_box();
    });
    let stages = layout.stages();
    let text = stages.variant(FirstLineVariant::Standard);
    let zero = ClusterId::new(0);
    let steps = walk(Segments::new(&text, zero..zero));
    assert_eq!(steps, [Seen::Item(zero, 0), Seen::Item(zero, 1)]);
    let empty = Layout::new();
    let stages = empty.stages();
    let text = stages.variant(FirstLineVariant::Standard);
    assert_eq!(Segments::new(&text, zero..ClusterId::new(3)).count(), 0);
}

/// A segment is its ends and three ids, and a step is one with a tag, small
/// enough to pass in registers. The walk is two references and three
/// cursors, which a look-ahead copies.
#[test]
fn a_walk_and_its_segments_are_small() {
    assert_eq!(size_of::<Segment>(), 20);
    assert_eq!(size_of::<Option<Step>>(), 24);
    assert_eq!(size_of::<Segments<'_>>(), 2 * size_of::<usize>() + 32);
}
