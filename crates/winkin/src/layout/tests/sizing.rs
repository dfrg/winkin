//! Sizing tests. They pin that `size_lines` sizes as `break_lines` does,
//! and that a layout sized without placing reads as one with no items.

use super::*;
use crate::build::FloatSide;
use crate::config::PastLines;

/// Content with what positioning reads: inline boxes, an atomic inline, a
/// float, ruby, right-to-left text and a forced break.
fn varied(cx: &mut Context, layout: &mut Layout, root: &ComputedStyle<'_>) {
    let boxed = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..*root
    };
    let size = BoxSize {
        inline: 24.0,
        block: 18.0,
        baseline: Some(14.0),
    };
    build(cx, layout, &ComputedBlockStyle::new(root), |b| {
        b.text(NodeKey(1), "XX XXX ");
        b.open_box(NodeKey(2), &boxed, None);
        b.text(NodeKey(3), "XXXX \u{5d0}\u{5d1}\u{5d2} XX");
        b.close_box();
        b.atomic(NodeKey(4), root, None, size);
        b.float(NodeKey(5), root, FloatSide::Left, size);
        b.open_ruby(NodeKey(6), root, None);
        b.text(NodeKey(7), "XX");
        b.open_annotation(NodeKey(8), root, None);
        b.text(NodeKey(9), "X");
        b.close_annotation();
        b.close_ruby();
        b.line_break(NodeKey(10));
        b.text(NodeKey(11), "XXX XX XXXXX XX X");
    });
}

/// A layout sized without placing has the lines, block metrics and room
/// below of one broken and placed, and every view reads it as having no
/// items, at each width.
#[test]
fn sizing_sizes_as_breaking_does() {
    let root = sized(&AHEM_FAMILY, 10.0);
    let mut cx = context();
    let mut placed = Layout::new();
    let mut sized_only = Layout::new();
    varied(&mut cx, &mut placed, &root);
    varied(&mut cx, &mut sized_only, &root);
    for width in [30.0, 75.0, 140.0, 600.0] {
        placed.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        sized_only.size_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(sized_only.metrics(), placed.metrics(), "at {width}");
        assert_eq!(sized_only.room_below(), placed.room_below(), "at {width}");
        assert_eq!(sized_only.lines().len(), placed.lines().len(), "at {width}");
        for (sized, line) in sized_only.lines().zip(placed.lines()) {
            let (sized, line) = (sized.metrics(), line.metrics());
            assert_eq!(sized.width, line.width, "at {width}");
            assert_eq!(sized.ascent, line.ascent, "at {width}");
            assert_eq!(sized.descent, line.descent, "at {width}");
            assert_eq!(sized.band, line.band, "at {width}");
        }
        for line in sized_only.lines() {
            assert_eq!(line.items().count(), 0);
            assert_eq!(line.all_items().count(), 0);
            assert_eq!(line.paints(|_| Decorates::None).count(), 0);
            let _ = line.annotations().count();
            let _ = line.floats().count();
        }
        assert_eq!(sized_only.box_fragments(NodeKey(2)).count(), 0);
        let _ = sized_only.static_positions().count();
        let _ = sized_only.hit_test(5.0, 5.0, PastLines::Column);
        let _ = sized_only.selection_rects(0..8).count();
        // Breaking again places as a layout broken from the start does.
        sized_only.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(
            sized_only.lines().flat_map(|line| line.all_items()).count(),
            placed.lines().flat_map(|line| line.all_items()).count(),
            "at {width}"
        );
    }
}
