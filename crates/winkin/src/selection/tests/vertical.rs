//! Vertical and combined text tests. They pin:
//! - line-relative carets, hits and rectangles in vertical lines;
//! - carets, hits and rectangles across combined text.

use super::*;

/// In a vertical line, carets, hits and selections are line-relative, in
/// both vertical modes.
///
/// A caret spans its font box, centred on the central baseline. A point hits
/// by the halves of the character along the line. A selection is as tall as
/// the line box. Chrome 153, in Yu Gothic at 40 px on 80 px lines, draws
/// `日本AB`'s carets 14 to 65 px across, at 0, 40, 80, 105.95 and 133.13
/// along, and selects `本A` over the line box.
#[test]
fn a_vertical_lines_carets_are_line_relative() {
    use crate::style::{TextOrientation, WritingMode};
    for mode in [WritingMode::VerticalRl, WritingMode::VerticalLr] {
        for orientation in [TextOrientation::Mixed, TextOrientation::Upright] {
            let root = styled(|style| {
                style.line.height = LineHeight::Px(40.0);
                style.orientation.text_orientation = orientation;
            });
            let block = ComputedBlockStyle {
                writing_mode: mode,
                ..ComputedBlockStyle::new(&root)
            };
            let layout = laid_with(&block, 400.0, |b| b.text(NodeKey(1), "ABC"));
            let case = (mode, orientation);
            for (at, along) in [(0, 0.0), (1, 20.0), (3, 60.0)] {
                let caret = layout.caret(Position::from(at)).unwrap();
                assert_eq!(
                    caret.inline,
                    InlineExtents {
                        left: along,
                        right: along
                    },
                    "{case:?} {at}"
                );
                assert_eq!(caret.block, across(10.0, 30.0), "{case:?} {at}");
            }
            assert_eq!(
                layout
                    .hit_test(29.0, 20.0, PastLines::Column)
                    .unwrap()
                    .offset,
                1
            );
            assert_eq!(
                layout
                    .hit_test(31.0, 20.0, PastLines::Column)
                    .unwrap()
                    .offset,
                2
            );
            let rects: Vec<_> = layout
                .selection_rects(1..2)
                .map(|rect| pairs(rect.inline, rect.block))
                .collect();
            assert_eq!(rects, [((20.0, 40.0), (0.0, 40.0))], "{case:?}");
        }
    }
}

/// Returns `AB`, `12` combined, `CD` in a `vertical-rl` block of Ahem at 20 px.
///
/// The unit is 40 px of horizontal text narrowed to 22, an em and a tenth,
/// centred across the line on its central baseline, 10 down its 20.
fn combined() -> Layout {
    use crate::style::{TextCombineUpright, WritingMode};
    let style = ahem();
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    let unit = styled(|style| style.orientation.text_combine_upright = TextCombineUpright::All);
    laid_with(&block, 400.0, |b| {
        b.text(NodeKey(1), "AB");
        b.open_box(NodeKey(2), &unit, None);
        b.text(NodeKey(3), "12");
        b.close_box();
        b.text(NodeKey(4), "CD");
    })
}

/// Inside combined text a caret runs along the line over the unit's em,
/// between two characters.
///
/// Chrome 153, in MS Gothic at 40 px with `12` combined in `vertical-rl`,
/// draws carets at 20, 40 and 60 px across an 80 px line box, each 40 px
/// along. Here the unit's text runs from 21 px across to −1, its first
/// character ending at 10. At the unit's edges the affinity picks the side,
/// since Chrome has two DOM positions there.
#[test]
fn a_caret_in_combined_text_runs_along_the_line() {
    let layout = combined();
    let caret = |at: usize, affinity| {
        let caret = layout.caret(Position::new(at, affinity)).unwrap();
        pairs(caret.inline, caret.block)
    };
    use Affinity::{Downstream, Upstream};
    // Beside the unit, across the line; inside it, along it.
    assert_eq!(caret(1, Downstream), ((20.0, 20.0), (0.0, 20.0)));
    assert_eq!(caret(2, Upstream), ((40.0, 40.0), (0.0, 20.0)));
    assert_eq!(caret(2, Downstream), ((40.0, 60.0), (21.0, 21.0)));
    assert_eq!(caret(3, Downstream), ((40.0, 60.0), (10.0, 10.0)));
    assert_eq!(caret(4, Upstream), ((40.0, 60.0), (-1.0, -1.0)));
    assert_eq!(caret(4, Downstream), ((60.0, 60.0), (0.0, 20.0)));
}

/// A point on combined text hits the nearer character boundary across the
/// line, as Chrome's click does.
///
/// Past the unit's text across, it hits the nearer end. Chrome jumps to the
/// block's start there, a bug not matched. Line motion lands before or after
/// the unit by its halves. Character motion steps through it.
#[test]
fn a_point_on_combined_text_hits_across_it() {
    let layout = combined();
    // The line box's left and top are the area's: a point at 50 along the
    // line is in the unit, whose em is 40 to 60.
    let hit = |x: f32, y: f32| layout.hit_test(x, y, PastLines::Column).unwrap();
    use Affinity::{Downstream, Upstream};
    assert_eq!(hit(50.0, 20.5), Position::new(2, Downstream));
    assert_eq!(hit(42.0, 14.0), Position::new(3, Downstream));
    assert_eq!(hit(58.0, 5.0), Position::new(3, Downstream));
    assert_eq!(hit(50.0, 0.5), Position::new(4, Upstream));
    // Past the text's start, and every place along the em alike.
    for x in [40.5, 50.0, 59.5] {
        assert_eq!(hit(x, 21.5), Position::new(2, Downstream), "{x}");
    }
    // A hit's caret is drawn where the point is: in the unit.
    let caret = layout.caret(hit(50.0, 0.5)).unwrap();
    assert_eq!(caret.block, across(-1.0, -1.0));
    // Beside the unit along the line, the text there.
    assert_eq!(hit(35.0, 10.0), Position::new(2, Upstream));
    assert_eq!(hit(65.0, 10.0), Position::new(4, Downstream));
    // Motion by character, forward and back, a character a step.
    let forward = MotionDirection::Forward.moving(Granularity::Character);
    assert_eq!(walk(&layout, 0, forward), [0, 1, 2, 3, 4, 5, 6]);
    let back = MotionDirection::Backward.moving(Granularity::Character);
    assert_eq!(walk(&layout, 6, back), [6, 5, 4, 3, 2, 1, 0]);
}

/// A selection over combined text paints the unit's em along the line and
/// the selected characters across it.
///
/// Chrome 153 in MS Gothic at 40 px paints the first character 20 to 40 px
/// across and the whole 20 to 60, never the line box. Text beside the unit
/// is as tall as the line box.
#[test]
fn a_selection_paints_combined_text_across_what_it_selects() {
    let layout = combined();
    let painted = |range: Range<usize>| -> Vec<((f32, f32), (f32, f32))> {
        layout
            .selection_rects(range)
            .map(|rect| pairs(rect.inline, rect.block))
            .collect()
    };
    assert_eq!(painted(2..3), [((40.0, 60.0), (10.0, 21.0))]);
    assert_eq!(painted(3..4), [((40.0, 60.0), (-1.0, 10.0))]);
    assert_eq!(
        painted(1..5),
        [
            ((20.0, 40.0), (0.0, 20.0)),
            ((40.0, 60.0), (-1.0, 21.0)),
            ((60.0, 80.0), (0.0, 20.0)),
        ]
    );
}
