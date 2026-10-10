//! Seek tests. They pin how many searches line layout and reading a
//! layout make:
//! - none to lay out the lines of plain prose, boxed prose or vertical
//!   text;
//! - none to paint them, their glyphs, their paint in Chrome's order, or
//!   along a path;
//! - none for a run's font, extents, orientation, clusters and marks, or a
//!   box part's extents;
//! - at most one a line to find a decorating box's culled parts, and one a
//!   combined run for its fit.
//!
//! Seeks are counted only in debug builds, so these tests build only there.

use super::*;
use crate::RunOrientation;
use crate::config::{EmphasisRoom, Pretty, RubyBreakWithin};
use crate::path::{PagePoint, PastEnds, PathPaint, Polyline, paints as path_paints};
use crate::stages::fragments::{self, Fragments, PlaceInput, PlaceScratch};
use crate::stages::lines::{self, BreakInput, Lines};
use crate::stages::shape::ShapeSession;
use crate::work;
use core::hint::black_box;

/// Words of prose, `words` of them.
fn prose(words: usize) -> String {
    let pick = ["the", "continues", "a", "line", "beautiful", "of", "text"];
    let mut text = String::new();
    for word in 0..words {
        if word > 0 {
            text.push(' ');
        }
        text.push_str(pick.get(word % pick.len()).copied().unwrap_or("x"));
    }
    text
}

/// Plain prose in Test Latin, broken at 180 px.
fn plain(cx: &mut Context) -> Layout {
    let style = sized(&LATIN, 16.0);
    let mut layout = Layout::new();
    build(cx, &mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), &prose(400));
    });
    layout.break_lines(cx, Area::new(180.0), &mut NoExclusions);
    layout
}

/// Prose marked up as prose is, broken at 180 px: a padded box that paints,
/// an underlined box inside it, another painted box, a box with emphasis
/// marks and one that draws nothing, repeated.
fn boxed(cx: &mut Context) -> Layout {
    let style = sized(&LATIN, 16.0);
    let painted = ComputedStyle {
        paints: true,
        edges: EdgesGroup {
            padding: Sides::from_px(2.0),
            ..EdgesGroup::INITIAL
        },
        ..style
    };
    let underlined = ComputedStyle {
        decorates: true,
        ..style
    };
    let mut marked = style;
    marked.text.emphasis.marks = true;
    let mut layout = Layout::new();
    build(cx, &mut layout, &ComputedBlockStyle::new(&style), |b| {
        for n in 0..60u64 {
            let key = |k: u64| NodeKey(10 * n + k);
            b.text(key(1), "alpha bravo ");
            b.open_box(key(2), &painted, None);
            b.text(key(3), "charlie delta ");
            b.open_box(key(4), &underlined, None);
            b.text(key(5), "echo foxtrot");
            b.close_box();
            b.text(key(6), " golf");
            b.close_box();
            b.text(key(7), " hotel ");
            b.open_box(key(8), &painted, None);
            b.text(key(9), "india");
            b.close_box();
            b.text(key(1), " juliet ");
            b.open_box(key(0), &marked, None);
            b.text(key(1), "kilo");
            b.close_box();
            b.open_box(key(0), &style, None);
            b.text(key(1), " lima ");
            b.close_box();
        }
    });
    layout.break_lines(cx, Area::new(180.0), &mut NoExclusions);
    layout
}

/// Ideographs, sideways Latin and combined digits down `vertical-rl`
/// lines of Ahem, broken at 300 px.
fn vertical(cx: &mut Context) -> Layout {
    use crate::style::{TextCombineUpright, WritingMode};
    let style = sized(&AHEM_FAMILY, 20.0);
    let mut combined = style;
    combined.orientation.text_combine_upright = TextCombineUpright::All;
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    let mut layout = Layout::new();
    build(cx, &mut layout, &block, |b| {
        for n in 0..40u64 {
            b.text(NodeKey(3 * n + 1), "\u{6C34}\u{6C34}Latin\u{6C34}\u{3042}");
            b.open_box(NodeKey(3 * n + 2), &combined, None);
            b.text(NodeKey(3 * n + 3), "26");
            b.close_box();
        }
    });
    layout.break_lines(cx, Area::new(300.0), &mut NoExclusions);
    layout
}

/// Seeks made reading a layout, and what the reads walked.
#[derive(Debug)]
struct Seeks {
    /// Every text run's glyphs.
    paint: u64,
    /// Everything each line paints, every box but the block decorated.
    paints: u64,
    /// Everything each line paints along a path.
    path: u64,
    /// Each run's font, extents, orientation, combined scale, clusters and
    /// marks, and each box part's and atomic's extents.
    readers: u64,
    lines: u64,
    fragments: u64,
    /// The runs set as combined text.
    combined: u64,
    /// The emphasis marks the readers handed out.
    marks: u64,
}

/// Counts the seeks of a glyph walk, a paint walk, a paint walk along a
/// path and the readers over every line of `layout`.
fn seeks(layout: &Layout) -> Seeks {
    let (mut fragments, mut combined, mut marks) = (0, 0, 0);
    work::take_seeks();
    for line in layout.lines() {
        for item in line.items() {
            fragments += 1;
            if let Item::Text(run) = item {
                run.glyphs().for_each(|glyph| {
                    black_box(glyph);
                });
            }
        }
    }
    let paint = work::take_seeks();
    let decorates = |key: NodeKey| {
        if key == NodeKey(0) {
            Decorates::None
        } else {
            Decorates::Both
        }
    };
    for line in layout.lines() {
        for paint in line.paints(decorates) {
            if let Paint::Text(run) = paint {
                run.glyphs().for_each(|glyph| {
                    black_box(glyph);
                });
            }
        }
    }
    let paints = work::take_seeks();
    let points = [PagePoint::new(0.0, 0.0), PagePoint::new(400.0, 300.0)];
    let mut ends = [0.0; 2];
    let path = Polyline::new(&points, &mut ends);
    for line in layout.lines() {
        for paint in path_paints(&line, &path, PastEnds::Hidden, decorates) {
            if let PathPaint::Text(run) = paint {
                run.glyphs().for_each(|glyph| {
                    black_box(glyph);
                });
            }
        }
    }
    let path = work::take_seeks();
    for line in layout.lines() {
        for item in line.items() {
            match item {
                Item::Text(run) | Item::Generated(run) => {
                    if run.orientation() == RunOrientation::Combined {
                        combined += 1;
                    }
                    black_box((
                        run.font().map(|font| font.size),
                        run.block(),
                        run.combine_scale(),
                    ));
                    run.clusters().for_each(|cluster| {
                        black_box(cluster);
                    });
                    run.emphasis_marks().for_each(|mark| {
                        black_box(mark);
                        marks += 1;
                    });
                }
                Item::Box(part) => {
                    black_box(part.block());
                }
                Item::Atomic(atomic) => {
                    black_box(atomic.block());
                }
            }
        }
    }
    let readers = work::take_seeks();
    Seeks {
        paint,
        paints,
        path,
        readers,
        lines: layout.lines().len() as u64,
        fragments,
        combined,
        marks,
    }
}

/// Counts the seeks line layout makes placing `layout`'s lines broken in
/// `width`, apart from breaking them.
fn placing_seeks(cx: &mut Context, layout: &Layout, width: f32) -> u64 {
    let area = Area::new(width);
    let mut lines = Lines::new();
    let (shaping, scratch) = cx.breaking();
    let input = BreakInput {
        stages: layout.stages(),
        area,
        emphasis_room: EmphasisRoom::Shared,
        pretty: Pretty::Limited,
        ruby_break_within: RubyBreakWithin::BaseOpportunities,
    };
    lines::break_lines(
        &input,
        &mut ShapeSession::new(shaping, None),
        scratch,
        &mut NoExclusions,
        &mut lines,
    );
    let config = *cx.config();
    let input = PlaceInput {
        stages: layout.stages(),
        lines: &lines,
        placements: cx.placing().0,
        area,
        tab_justification: config.tab_justification,
        ellipsis_space: config.ellipsis_space,
    };
    let mut out = Fragments::new();
    work::take_seeks();
    fragments::place_fragments(&input, &mut PlaceScratch::new(), &mut out);
    let seeks = work::take_seeks();
    cx.placing().0.clear();
    seeks
}

/// Plain prose is laid out, painted and read without a search.
///
/// Each text item names its shaping run and its content item, and its line
/// its paragraph, so no reader looks them up.
#[test]
fn plain_prose_is_laid_out_painted_and_read_without_a_search() {
    let mut cx = context();
    let layout = plain(&mut cx);
    assert_eq!(placing_seeks(&mut cx, &layout, 180.0), 0);
    let seeks = seeks(&layout);
    assert!(seeks.lines > 20, "{seeks:?}");
    assert_eq!(
        (seeks.paint, seeks.paints, seeks.path, seeks.readers),
        (0, 0, 0, 0),
        "{seeks:?}"
    );
}

/// Boxed prose is laid out without a search, and painted and read without
/// one a fragment.
///
/// Line layout walks the kept boxes and their shifts as it opens them. A
/// box part names its kept box, so its extent is a load, and a text item's
/// ends find their items by walking from its own. A line's paint walks the
/// kept boxes to tell the culled ones that decorate. It starts at the
/// line's first box part, or where the line has none, seeks once.
#[test]
fn boxed_prose_is_laid_out_without_a_search_and_painted_with_one_a_line_at_most() {
    let mut cx = context();
    let layout = boxed(&mut cx);
    assert_eq!(placing_seeks(&mut cx, &layout, 180.0), 0);
    let seeks = seeks(&layout);
    assert!(seeks.lines > 20, "{seeks:?}");
    assert!(
        seeks.fragments > 3 * seeks.lines && seeks.marks > 20,
        "{seeks:?}"
    );
    assert_eq!((seeks.paint, seeks.readers), (0, 0), "{seeks:?}");
    assert!(seeks.paints > 0 && seeks.paints < seeks.lines, "{seeks:?}");
    assert_eq!(seeks.path, seeks.paints, "{seeks:?}");
}

/// Vertical text is laid out, painted and read without a search, but for
/// each combined run's fit, which its scale looks up by its script run.
#[test]
fn vertical_text_seeks_only_its_combined_fits() {
    let mut cx = context();
    let layout = vertical(&mut cx);
    assert_eq!(placing_seeks(&mut cx, &layout, 300.0), 0);
    let seeks = seeks(&layout);
    assert!(seeks.lines > 10 && seeks.combined > 10, "{seeks:?}");
    assert_eq!((seeks.paint, seeks.paints), (0, 0), "{seeks:?}");
    assert_eq!(seeks.path, seeks.combined, "{seeks:?}");
    assert_eq!(seeks.readers, seeks.combined, "{seeks:?}");
}
