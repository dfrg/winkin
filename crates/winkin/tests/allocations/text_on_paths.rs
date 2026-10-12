//! Lines broken to paths and placed along them allocate nothing warm.

use super::count_allocations;
use super::test_fonts::{self, ahem_fallback};
use fontwich::Collection;
use winkin::paint::Decorates;
use winkin::path::{
    BezierPath, PagePoint, PastEnds, PathPaint, PathRoom, Polyline, TextPath, paints,
};
use winkin::style::{ComputedStyle, FontFamilyName, FontGroup, TextAlign, WritingMode};
use winkin::{BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey};

/// Ahem alone.
fn collection() -> Collection {
    test_fonts::collection(&[], ahem_fallback())
}

/// Paragraphs `repeat` times over, in `mode`: words, a box, marks,
/// ruby, a soft hyphen, ideographs, an accented letter of more glyphs
/// than a run's walk holds, justified.
fn document(layout: &mut Layout, cx: &mut Context, mode: WritingMode, repeat: usize) {
    let families = [FontFamilyName::named("Ahem")];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            size: 20.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut marked = root;
    marked.text.emphasis.marks = true;
    let mut small = root;
    small.font.size = 10.0;
    let block = ComputedBlockStyle {
        writing_mode: mode,
        text_align: TextAlign::Justify,
        ..ComputedBlockStyle::new(&root)
    };
    let mut b = layout.builder(cx, NodeKey(0), &block, BuildOptions::default());
    let mut key = 0;
    let mut next = || {
        key += 1;
        NodeKey(key)
    };
    for _ in 0..repeat {
        b.text(next(), "along the path we go, hy\u{ad}phen\u{ad}ated ");
        b.open_box(next(), &marked, None);
        b.text(next(), "marked \u{6f22}\u{5b57} ");
        b.close_box();
        b.open_ruby(next(), &root, None);
        b.text(next(), "\u{6c34}");
        b.open_annotation(next(), &small, None);
        b.text(next(), "XX");
        b.close_annotation();
        b.close_ruby();
        b.text(
            next(),
            " and on e\u{301}\u{302}\u{303}\u{304}\u{308} again ",
        );
    }
    assert!(b.finish().is_complete());
}

/// Reads every line along its path, and every item it paints there.
fn read<P: TextPath>(layout: &Layout, paths: &[P], ends: PastEnds) -> f32 {
    let mut sum = 0.0;
    for (line, path) in layout.lines().zip(paths.iter().cycle()) {
        for item in paints(&line, path, ends, |_| Decorates::Both) {
            match item {
                PathPaint::Text(run) | PathPaint::Annotation(run) | PathPaint::Generated(run) => {
                    for placed in run.glyphs() {
                        sum += placed.place.x + placed.place.y + placed.glyph.advance;
                    }
                }
                PathPaint::DecorationBeforeText(bar) | PathPaint::DecorationAfterText(bar) => {
                    for piece in bar.pieces() {
                        sum += piece.inline.right - piece.inline.left + piece.place.cos;
                    }
                }
                PathPaint::Emphasis { place, .. } | PathPaint::Atomic { place, .. } => {
                    sum += place.sin;
                }
                _ => {}
            }
        }
    }
    sum
}

/// Breaking a warm layout along paths, each line to its own path's
/// length, allocates nothing, horizontal or vertical, nor does placing
/// every line's paint along a polyline and a Bézier path, whatever is
/// past their ends: the room answers from the caller's paths, and the
/// placing reads each run's glyphs twice, keeping none.
#[test]
fn text_on_paths_allocates_nothing_warm() {
    let points: Vec<PagePoint> = (0..40u8)
        .map(|n| {
            let x = f32::from(n) * 25.0;
            PagePoint::new(x, 200.0 + if n % 2 == 0 { 30.0 } else { -30.0 })
        })
        .collect();
    let curve = [
        PagePoint::new(0.0, 0.0),
        PagePoint::new(100.0, -80.0),
        PagePoint::new(200.0, 80.0),
        PagePoint::new(300.0, 0.0),
        PagePoint::new(400.0, -80.0),
        PagePoint::new(500.0, 80.0),
        PagePoint::new(600.0, 0.0),
    ];
    let mut ends = vec![0.0; points.len()];
    let mut table = vec![0.0; 65];
    let polyline = Polyline::new(&points, &mut ends);
    let bezier = BezierPath::new(&curve, &mut table);
    let polylines = [polyline, Polyline::from_measured(&points[..10], &[])];
    for mode in [WritingMode::HorizontalTb, WritingMode::VerticalRl] {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        document(&mut layout, &mut cx, mode, 12);
        let mut room = PathRoom::new(&polylines, 15.0);
        layout.break_lines(&mut cx, room.area(), &mut room);
        let beziers = [bezier];
        let mut on_bezier = PathRoom::new(&beziers, 0.0);
        layout.break_lines(&mut cx, on_bezier.area(), &mut on_bezier);
        read(&layout, &beziers, PastEnds::Hidden);
        let warm = count_allocations(|| {
            layout.break_lines(&mut cx, room.area(), &mut room);
            layout.break_lines(&mut cx, on_bezier.area(), &mut on_bezier);
        });
        assert_eq!(warm, 0, "{mode:?}: breaking along paths allocated");
        for ends in [PastEnds::Hidden, PastEnds::Straight] {
            let mut sum = 0.0;
            let warm = count_allocations(|| {
                sum = read(&layout, &beziers, ends) + read(&layout, &polylines, ends);
            });
            assert!(sum.is_finite() && sum != 0.0);
            assert_eq!(warm, 0, "{mode:?}, {ends:?}: placing along paths allocated");
        }
    }
}
