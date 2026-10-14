//! Text on path tests. They pin:
//! - a level path placing every glyph where the straight line does;
//! - each character turning as one to the tangent at its middle, and
//!   characters past the ends left out;
//! - each line taking its own path's length, however often the breaker asks;
//! - upright text standing across a path, sideways text lying along it, and
//!   combined text narrowed along it;
//! - annotations, emphasis marks, decorations, atomic inlines and the
//!   ellipsis following the path;
//! - paths extended past their ends, and reversed paths;
//! - no panic on any path or buffer.
//!
//! The figures are in Ahem: every glyph an em square, 0.8 em above the
//! baseline.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

use super::{
    BezierPath, PagePoint, PastEnds, PathPaint, PathPiece, PathPoint, PathRoom, Placement,
    Polyline, ReversedPath, TextPath, paints,
};
use crate::paint::Decorates;
use crate::style::{
    ComputedStyle, FontFamilyName, TextAlign, TextAlignLast, TextCombineUpright, TextJustify,
    TextOverflow, TextWrapMode, TextWrapStyle, WritingMode,
};
use crate::tests::{AHEM_FAMILY, TestFont, ahem_fallback, collection, sized};
use crate::{
    Area, BlockExtents, BoxSize, BuildOptions, ComputedBlockStyle, Context, Exclusions,
    ExclusionsCheckpoint, FloatRequest, Generated, Glyph, InlineExtents, Item, Layout,
    LayoutBuilder, Line, NoExclusions, NodeKey, PlacedFloat, TextRun,
};

/// Returns the page point `(x, y)`.
fn point(x: f32, y: f32) -> PagePoint {
    PagePoint::new(x, y)
}

const SPLIT: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed("Test Split"))];

/// Returns a context with Ahem and "Test Split".
///
/// "Test Split" draws `s` as two glyphs, a half em and a quarter em wide,
/// and raises `r` by a fifth of an em.
fn context() -> Context {
    let mut split = TestFont::new("Test Split", &[(0x20, 0x7E)]);
    split.splits = vec!['s'];
    split.raised = vec![('r', 200)];
    Context::new(collection(&[split], ahem_fallback()))
}

/// Returns 20 px Ahem that never wraps.
fn nowrap() -> ComputedStyle<'static> {
    let mut style = sized(&AHEM_FAMILY, 20.0);
    style.text.wrap_mode = TextWrapMode::NoWrap;
    style
}

/// Builds a layout of `block` from `calls`.
fn built(
    cx: &mut Context,
    block: &ComputedBlockStyle<'_>,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) -> Layout {
    let mut layout = Layout::new();
    let mut b = layout.builder(cx, NodeKey(0), block, BuildOptions::default());
    calls(&mut b);
    assert!(b.finish().is_complete());
    layout
}

/// Breaks `layout` along `paths`, each line starting `start` along its path.
fn along<P: TextPath>(cx: &mut Context, layout: &mut Layout, paths: &[P], start: f32) {
    let mut room = PathRoom::new(paths, start);
    layout.break_lines(cx, room.area(), &mut room);
}

/// A straight path from `(x, y)` in direction `(dx, dy)`, `length` long.
#[derive(Copy, Clone, Debug)]
struct Straight {
    x: f32,
    y: f32,
    dx: f32,
    dy: f32,
    length: f32,
}

impl TextPath for Straight {
    fn length(&self) -> f32 {
        self.length
    }

    fn at(&self, distance: f32) -> PathPoint {
        PathPoint {
            x: self.x + self.dx * distance,
            y: self.y + self.dy * distance,
            dx: self.dx,
            dy: self.dy,
        }
    }
}

/// Returns a straight path from the origin in direction `(dx, dy)`, 1000
/// long.
fn straight(dx: f32, dy: f32) -> Straight {
    Straight {
        x: 0.0,
        y: 0.0,
        dx,
        dy,
        length: 1000.0,
    }
}

/// Returns each glyph that line `line` places along `path`, with its run and
/// placement.
fn glyphs_along<'a, P: TextPath>(
    layout: &'a Layout,
    line: usize,
    path: &'a P,
    ends: PastEnds,
) -> Vec<(TextRun<'a>, Glyph, Placement)> {
    let Some(line) = layout.line(line) else {
        return Vec::new();
    };
    let mut placed = Vec::new();
    for item in paints(&line, path, ends, |_| Decorates::None) {
        if let PathPaint::Text(placed_run)
        | PathPaint::Annotation(placed_run)
        | PathPaint::Generated(placed_run) = item
        {
            let run = placed_run.run();
            placed.extend(
                placed_run
                    .glyphs()
                    .map(|placed| (run, placed.glyph, placed.place)),
            );
        }
    }
    placed
}

/// Returns every placement line `line` makes along `path`, decorations
/// included.
fn placements<P: TextPath + ?Sized>(
    layout: &Layout,
    line: usize,
    path: &P,
    ends: PastEnds,
) -> Vec<Placement> {
    let Some(line) = layout.line(line) else {
        return Vec::new();
    };
    let mut placed = Vec::new();
    for item in paints(&line, path, ends, |_| Decorates::Both) {
        match item {
            PathPaint::Text(run) | PathPaint::Annotation(run) | PathPaint::Generated(run) => {
                placed.extend(run.glyphs().map(|glyph| glyph.place));
            }
            PathPaint::DecorationBeforeText(bar) | PathPaint::DecorationAfterText(bar) => {
                placed.extend(bar.pieces().map(|piece| piece.place));
            }
            PathPaint::Emphasis { place, .. } | PathPaint::Atomic { place, .. } => {
                placed.push(place)
            }
        }
    }
    placed
}

/// Returns each text glyph of `line` with its node and its straight-line
/// position from the line box's left and top.
fn glyphs_on_the_line(line: &Line<'_>) -> Vec<(NodeKey, f32, f32)> {
    line.items()
        .filter_map(|item| match item {
            Item::Text(run) | Item::Generated(run) => Some(run),
            _ => None,
        })
        .flat_map(|run| run.glyphs().map(move |glyph| (run.key(), glyph.x, glyph.y)))
        .collect()
}

/// Returns whether `a` and `b` are within a thousandth.
fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// Rounds `x` to a thousandth, halves away from zero, so float results
/// compare exactly.
fn thousandth(x: f32) -> f32 {
    (x * 1000.0 + 0.5 * x.signum()) as i32 as f32 / 1000.0
}

/// Returns a placement's rotation `(cos, sin)`, to a thousandth.
fn turn(place: &Placement) -> (f32, f32) {
    (thousandth(place.cos), thousandth(place.sin))
}

// Placing along a path ---------------------------------------------------

/// A level path at y = 100 places every glyph where the straight line does,
/// unrotated, with the baseline on the path.
#[test]
fn a_level_path_places_glyphs_where_the_line_does() {
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "set along a path");
    });
    let paths = [Straight {
        y: 100.0,
        ..straight(1.0, 0.0)
    }];
    along(&mut cx, &mut layout, &paths, 0.0);
    let line = layout.line(0).expect("a line");
    let metrics = line.metrics();
    assert_eq!((metrics.left, metrics.ascent), (0.0, 16.0));
    let on_the_line = glyphs_on_the_line(&line);
    let placed = glyphs_along(&layout, 0, &paths[0], PastEnds::Hidden);
    assert_eq!(placed.len(), on_the_line.len());
    assert_eq!(placed.len(), 16);
    for ((_, _, place), (_, x, y)) in placed.iter().zip(&on_the_line) {
        let (x, y) = (metrics.left + x, 100.0 - (metrics.ascent - y));
        assert!(
            near(place.x, x) && near(place.y, y),
            "{place:?} against {x}, {y}"
        );
        assert_eq!(turn(place), (1.0, 0.0), "not turned");
        assert_eq!(place.scale, 1.0);
    }
}

/// A path running down the page turns each glyph a quarter turn, and drops
/// glyphs past its end. Of `downward`'s eight 20 px squares, with middles at
/// 10, 30, 50 and so on, a path 30 long holds the first two.
#[test]
fn a_path_turns_its_glyphs_and_ends_where_it_ends() {
    let mut cx = context();
    let down = |length: f32| Straight {
        x: 50.0,
        length,
        ..straight(0.0, 1.0)
    };
    let placed = |cx: &mut Context, path: Straight| {
        let mut layout = built(cx, &ComputedBlockStyle::new(&nowrap()), |b| {
            b.text(NodeKey(1), "downward");
        });
        along(cx, &mut layout, &[path], 0.0);
        glyphs_along(&layout, 0, &path, PastEnds::Hidden)
            .iter()
            .map(|(_, _, place)| *place)
            .collect::<Vec<_>>()
    };
    let all = placed(&mut cx, down(1000.0));
    assert_eq!(all.len(), 8, "every glyph of `downward`");
    assert!(all.iter().all(|place| turn(place) == (0.0, 1.0)));
    // Each glyph's origin lies on the path, with the baseline running down
    // the page.
    let ys: Vec<f32> = all.iter().map(|place| place.y).collect();
    assert_eq!(ys, [0.0, 20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 140.0]);
    assert!(all.iter().all(|place| place.x == 50.0));
    let some = placed(&mut cx, down(30.0));
    assert_eq!(some.len(), 2, "{some:?}");
    assert_eq!(some, all[..2]);
}

/// Each line gets its path's length less the start, and lines past the last
/// path reuse its length. With a start of 20, the first line has 100 (room
/// for `one`) and the second 300 (room for `two three four`).
#[test]
fn lines_take_their_lengths_from_the_paths_in_turn() {
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(
            NodeKey(1),
            "one two three four five six seven eight nine ten eleven twelve thirteen \
             fourteen fifteen sixteen",
        );
    });
    let level = |length: f32| Straight {
        length,
        ..straight(1.0, 0.0)
    };
    let paths = [level(120.0), level(320.0)];
    along(&mut cx, &mut layout, &paths, 20.0);
    let lines: Vec<Line<'_>> = layout.lines().collect();
    assert_eq!(lines.len(), 8);
    let bands: Vec<(f32, f32)> = lines
        .iter()
        .map(|line| (line.metrics().band.left, line.metrics().band.right))
        .collect();
    assert_eq!(bands[..3], [(20.0, 120.0), (20.0, 320.0), (20.0, 320.0)]);
    assert!(
        bands[2..].iter().all(|&band| band == (20.0, 320.0)),
        "the last path again"
    );
    let text = |line: &Line<'_>| String::from(layout.text()[line.text_range()].trim_end());
    assert_eq!(text(&lines[0]), "one");
    assert_eq!(text(&lines[1]), "two three four");
    assert_eq!(text(&lines[2]), "five six seven");
    assert!(lines.iter().all(|line| line.metrics().left == 20.0));
}

/// In a vertical line, upright ideographs stand across the path with their
/// tops toward its start, and sideways text lies along it. On a path down
/// the page, upright glyphs are unrotated and every glyph lands where the
/// vertical line puts it, the central baseline 10 from the line's right.
#[test]
fn upright_glyphs_stand_across_a_path_and_sideways_ones_lie_along_it() {
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    let mut layout = built(&mut cx, &block, |b| {
        b.text(NodeKey(1), "\u{6f22}\u{5b57}");
        b.open_box(NodeKey(2), &style, None);
        b.text(NodeKey(3), "ab");
        b.close_box();
    });
    let down = straight(0.0, 1.0);
    along(&mut cx, &mut layout, &[down], 0.0);
    let line = layout.line(0).expect("a line");
    assert_eq!(line.metrics().ascent, 10.0, "the central baseline");
    let placed = glyphs_along(&layout, 0, &down, PastEnds::Hidden);
    assert_eq!(placed.len(), 4, "{placed:?}");
    for (run, glyph, place) in &placed {
        let expected = if run.key() == NodeKey(1) {
            (1.0, 0.0)
        } else {
            (0.0, 1.0)
        };
        assert_eq!(turn(place), expected, "{:?}", run.key());
        // Where the straight vertical line puts the glyph.
        assert!(
            near(place.x, 10.0 - glyph.y) && near(place.y, glyph.x),
            "{place:?}"
        );
    }
    assert!(placed.windows(2).all(|pair| pair[1].2.y > pair[0].2.y));
    // On a path across the page, the ideographs' tops face left.
    let across = straight(1.0, 0.0);
    let placed = glyphs_along(&layout, 0, &across, PastEnds::Hidden);
    assert_eq!(turn(&placed[0].2), (0.0, -1.0));
    assert_eq!(turn(&placed[3].2), (1.0, 0.0));
}

/// Combined text keeps its narrowing along a path and turns as one
/// character: four Ahem digits in one em. Other glyphs keep a scale of 1.
#[test]
fn combined_text_is_narrowed_along_a_path() {
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    let mut combined = style;
    combined.orientation.text_combine_upright = TextCombineUpright::All;
    let mut layout = built(&mut cx, &block, |b| {
        b.text(NodeKey(1), "\u{6f22}");
        b.open_box(NodeKey(2), &combined, None);
        b.text(NodeKey(3), "2026");
        b.close_box();
        b.text(NodeKey(4), "\u{5b57}");
    });
    let down = straight(0.0, 1.0);
    along(&mut cx, &mut layout, &[down], 0.0);
    let placed = glyphs_along(&layout, 0, &down, PastEnds::Hidden);
    let digits: Vec<_> = placed
        .iter()
        .filter(|(run, _, _)| run.key() == NodeKey(3))
        .collect();
    assert_eq!(digits.len(), 4);
    let scale = digits[0].0.combine_scale();
    assert!(scale < 1.0, "{scale}");
    assert!(digits.iter().all(|(_, _, place)| place.scale == scale));
    assert!(
        placed
            .iter()
            .filter(|(run, _, _)| run.key() != NodeKey(3))
            .all(|(_, _, place)| place.scale == 1.0)
    );
    // On a bend, the four digits share one rotation.
    let points = [point(0.0, 0.0), point(0.0, 30.0), point(40.0, 70.0)];
    let mut ends = [0.0; 3];
    let bend = Polyline::new(&points, &mut ends);
    let placed = glyphs_along(&layout, 0, &bend, PastEnds::Hidden);
    let turns: Vec<(f32, f32)> = placed
        .iter()
        .filter(|(run, _, _)| run.key() == NodeKey(3))
        .map(|(_, _, place)| turn(place))
        .collect();
    assert_eq!(turns.len(), 4);
    assert!(turns.iter().all(|&turned| turned == turns[0]), "{turns:?}");
}

/// A ruby annotation follows the path over its base: `XX` at 15 px over `X`
/// at 30 px sits 27 above the base's baseline.
#[test]
fn an_annotation_is_placed_over_its_base_along_a_path() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 30.0);
    let small = sized(&AHEM_FAMILY, 15.0);
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "X");
        b.open_annotation(NodeKey(3), &small, None);
        b.text(NodeKey(4), "XX");
        b.close_annotation();
        b.close_ruby();
    });
    let level = Straight {
        y: 100.0,
        ..straight(1.0, 0.0)
    };
    along(&mut cx, &mut layout, &[level], 0.0);
    let placed = glyphs_along(&layout, 0, &level, PastEnds::Hidden);
    assert_eq!(placed.len(), 3, "the base and its two-glyph reading");
    // The annotation paints after the base.
    let keys: Vec<NodeKey> = placed.iter().map(|(run, _, _)| run.key()).collect();
    assert_eq!(keys, [NodeKey(2), NodeKey(4), NodeKey(4)]);
    let base = placed[0].2;
    assert_eq!(base.y, 100.0, "the base on the path");
    for (_, _, reading) in &placed[1..] {
        assert!(near(reading.y, base.y - 27.0), "{reading:?}");
        assert_eq!(turn(reading), (1.0, 0.0));
    }
}

/// [`TextPath::point`] extends a path straight past its ends. A
/// [`ReversedPath`] starts at the path's end with its "up" pointing down the
/// page, so a line on it is turned half about. Glyphs past the end are
/// dropped under [`PastEnds::Hidden`] and extended under
/// [`PastEnds::Straight`].
#[test]
fn a_path_runs_on_past_its_ends_and_backward() {
    let path = Straight {
        length: 100.0,
        ..straight(1.0, 0.0)
    };
    assert_eq!(path.point(150.0, 10.0), point(150.0, -10.0));
    assert_eq!(path.point(-20.0, 0.0), point(-20.0, 0.0));
    let back = ReversedPath(path);
    assert_eq!(
        back.at(0.0),
        PathPoint {
            x: 100.0,
            y: 0.0,
            dx: -1.0,
            dy: -0.0
        }
    );
    assert_eq!(back.point(10.0, 5.0), point(90.0, 5.0));
    // Six 20 px squares on a path 100 long: the sixth's middle is past the
    // end.
    let mut cx = context();
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&nowrap()), |b| {
        b.text(NodeKey(1), "XXXXXX");
    });
    along(&mut cx, &mut layout, &[back], 0.0);
    let places = |ends| -> Vec<(f32, f32)> {
        glyphs_along(&layout, 0, &back, ends)
            .iter()
            .inspect(|(_, _, place)| assert_eq!(turn(place), (-1.0, 0.0)))
            .map(|(_, _, place)| (place.x, place.y))
            .collect()
    };
    let hidden = places(PastEnds::Hidden);
    assert_eq!(
        hidden,
        [
            (100.0, 0.0),
            (80.0, 0.0),
            (60.0, 0.0),
            (40.0, 0.0),
            (20.0, 0.0)
        ]
    );
    let straight_on = places(PastEnds::Straight);
    assert_eq!(straight_on[..5], hidden[..]);
    assert_eq!(straight_on.get(5), Some(&(0.0, 0.0)));
}

// Following the path -----------------------------------------------------

/// A line asked for again gets its own path again. Balancing relays the
/// second paragraph from its first line, so line 2's band is asked for after
/// line 3's, and each line still gets its own path's length.
#[test]
fn a_line_is_given_its_own_path_however_often_it_is_asked() {
    /// Exclusions that record the line indices asked for, in order.
    struct Asked<'a> {
        room: PathRoom<'a, Straight>,
        lines: RefCell<Vec<usize>>,
    }
    impl Exclusions for Asked<'_> {
        fn band(&self, line: usize, block: BlockExtents) -> InlineExtents {
            self.lines.borrow_mut().push(line);
            self.room.band(line, block)
        }
        fn below(&self, top: f32) -> Option<f32> {
            self.room.below(top)
        }
        fn place(&mut self, float: FloatRequest) -> PlacedFloat {
            self.room.place(float)
        }
        fn checkpoint(&self) -> ExclusionsCheckpoint {
            self.room.checkpoint()
        }
        fn rewind(&mut self, to: ExclusionsCheckpoint) {
            self.room.rewind(to);
        }
    }
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let block = ComputedBlockStyle {
        text_wrap_style: TextWrapStyle::Balance,
        ..ComputedBlockStyle::new(&style)
    };
    let mut layout = built(&mut cx, &block, |b| {
        b.text(NodeKey(1), "aaaa bbbb cccc dddd");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "aaaa bbbb cccc dddd");
    });
    let level = |length: f32| Straight {
        length,
        ..straight(1.0, 0.0)
    };
    let paths = [level(300.0), level(280.0), level(300.0), level(300.0)];
    let mut asked = Asked {
        room: PathRoom::new(&paths, 0.0),
        lines: RefCell::new(Vec::new()),
    };
    layout.break_lines(&mut cx, asked.room.area(), &mut asked);
    let lines = asked.lines.into_inner();
    let after_the_next = lines
        .iter()
        .skip_while(|&&line| line != 3)
        .any(|&line| line == 2);
    assert!(after_the_next, "balancing asks for line 2 again: {lines:?}");
    let text = |line: &Line<'_>| String::from(layout.text()[line.text_range()].trim_end());
    let broken: Vec<_> = layout.lines().map(|line| text(&line)).collect();
    assert_eq!(broken, ["aaaa bbbb", "cccc dddd", "aaaa bbbb", "cccc dddd"]);
    for line in layout.lines() {
        let length = paths.get(line.index()).map(|path| path.length);
        assert_eq!(
            Some(line.metrics().band.right),
            length,
            "line {}",
            line.index()
        );
    }
    // Breaking again along the same paths gives the same bands.
    let bands: Vec<_> = layout.lines().map(|line| line.metrics().band).collect();
    along(&mut cx, &mut layout, &paths, 0.0);
    let again: Vec<_> = layout.lines().map(|line| line.metrics().band).collect();
    assert_eq!(bands, again);
}

/// The ellipsis follows the path where it stands. Four 20 px squares and the
/// ellipsis fill a path 100 long.
#[test]
fn the_ellipsis_follows_the_path() {
    let mut cx = context();
    let style = nowrap();
    let block = ComputedBlockStyle {
        text_overflow: TextOverflow::Ellipsis,
        ..ComputedBlockStyle::new(&style)
    };
    let mut layout = built(&mut cx, &block, |b| {
        b.text(NodeKey(1), "XXXXXXXXXXXX");
    });
    let down = Straight {
        length: 100.0,
        ..straight(0.0, 1.0)
    };
    along(&mut cx, &mut layout, &[down], 0.0);
    let line = layout.line(0).expect("a line");
    assert!(line.has_ellipsis());
    let placed = glyphs_along(&layout, 0, &down, PastEnds::Hidden);
    let (ellipsis, glyph, place) = placed
        .iter()
        .find(|(run, _, _)| run.generated() == Some(Generated::Ellipsis))
        .expect("the ellipsis, placed");
    assert_eq!(placed.len(), 5, "four squares and the ellipsis");
    assert_eq!(ellipsis.inline().left, 80.0);
    assert!(
        near(place.y, glyph.x) && near(place.x, 16.0 - glyph.y),
        "{place:?}"
    );
    assert_eq!(turn(place), (0.0, 1.0));
}

/// A character's glyphs turn as one about the middle of their combined
/// advance, keeping their offsets, as SVG turns a typographic character.
///
/// `s` is two glyphs, 10 and 5 wide, after an `a` 10 wide, so its middle is
/// 17.5 along. The path turns down at 16, between the two glyphs' middles,
/// and both turn down. `r`, raised 4, stays 4 off the path.
#[test]
fn a_characters_glyphs_turn_as_one() {
    let mut cx = context();
    let style = sized(&SPLIT, 20.0);
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "asr");
    });
    let points = [point(0.0, 0.0), point(16.0, 0.0), point(16.0, 100.0)];
    let mut ends = [0.0; 3];
    let corner = Polyline::new(&points, &mut ends);
    along(&mut cx, &mut layout, &[corner], 0.0);
    let placed = glyphs_along(&layout, 0, &corner, PastEnds::Hidden);
    let glyphs: Vec<(f32, f32, (f32, f32))> = placed
        .iter()
        .map(|(_, _, place)| (thousandth(place.x), thousandth(place.y), turn(place)))
        .collect();
    assert_eq!(glyphs.len(), 4, "{glyphs:?}");
    assert_eq!(glyphs[0], (0.0, 0.0, (1.0, 0.0)), "`a`, level");
    assert_eq!(glyphs[1], (16.0, -6.0, (0.0, 1.0)), "`s`, turned down");
    assert_eq!(
        glyphs[2],
        (16.0, 4.0, (0.0, 1.0)),
        "`s`'s second glyph with it"
    );
    // `r`'s middle is 30 along, 14 down the second leg. Its origin is 5 back
    // up the path and 4 to the path's up side, the page's right.
    assert_eq!(
        glyphs[3],
        (20.0, 9.0, (0.0, 1.0)),
        "`r`, raised off the path"
    );
}

/// A character of many glyphs (an `s` drawn as two, with four accents) turns
/// as one, as do two such characters in a run. On a level path every glyph
/// lands where the straight line puts it.
#[test]
fn a_character_of_many_glyphs_turns_as_one() {
    let mut cx = context();
    let style = sized(&SPLIT, 20.0);
    let text = "as\u{301}\u{301}\u{301}\u{301}rs\u{301}\u{301}\u{301}\u{301}";
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), text);
    });
    let level = Straight {
        y: 100.0,
        ..straight(1.0, 0.0)
    };
    along(&mut cx, &mut layout, &[level], 0.0);
    let line = layout.line(0).expect("a line");
    let metrics = line.metrics();
    let on_the_line = glyphs_on_the_line(&line);
    let placed = glyphs_along(&layout, 0, &level, PastEnds::Hidden);
    assert_eq!(placed.len(), on_the_line.len());
    let offsets: Vec<usize> = placed
        .iter()
        .map(|(_, glyph, _)| glyph.text_offset)
        .collect();
    let longest = offsets
        .chunk_by(|a, b| a == b)
        .map(<[usize]>::len)
        .max()
        .unwrap_or_default();
    assert!(longest > 4, "a character of {longest} glyphs");
    for ((_, glyph, place), (_, x, y)) in placed.iter().zip(&on_the_line) {
        assert_eq!((glyph.x, glyph.y), (*x, *y));
        let (x, y) = (metrics.left + x, 100.0 - (metrics.ascent - y));
        assert!(
            near(place.x, x) && near(place.y, y),
            "{place:?} against {x}, {y}"
        );
    }
    // On a bend, each character's glyphs share one rotation.
    let points = [
        point(0.0, 0.0),
        point(15.0, 0.0),
        point(25.0, 10.0),
        point(25.0, 200.0),
    ];
    let mut ends = [0.0; 4];
    let bend = Polyline::new(&points, &mut ends);
    let placed = glyphs_along(&layout, 0, &bend, PastEnds::Hidden);
    assert_eq!(placed.len(), on_the_line.len());
    for character in placed.chunk_by(|a, b| a.1.text_offset == b.1.text_offset) {
        let turns: Vec<_> = character.iter().map(|(_, _, place)| turn(place)).collect();
        assert!(turns.iter().all(|&turned| turned == turns[0]), "{turns:?}");
    }
}

/// A cluster with an advance but no glyph still moves the characters after
/// it along the path. Summing glyph advances alone would set every later
/// middle short.
///
/// `fixx` uses a font whose `fi` ligature is one glyph, 0.75 em, justified
/// inter-character over 100. The ligature's second cluster draws nothing
/// and takes a third of the 65 spread, so the first `x` spans 58.3 to 90 and
/// the second 90 to 100. On a path 80 long, the first `x`'s middle (74.2)
/// is on it and the second's (95) is past it.
#[test]
fn a_cluster_that_draws_nothing_moves_what_follows() {
    const LIGATED: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Ligated"))];
    let mut font = TestFont::new("Test Ligated", &[(0x20, 0x7E)]);
    font.ligatures = vec![vec!['f', 'i']];
    let (ligature, x) = (font.ligature_glyph(0), font.glyph('x'));
    let mut cx = Context::new(collection(&[font], ahem_fallback()));
    let mut style = sized(&LIGATED, 20.0);
    style.text.justify = TextJustify::InterCharacter;
    let block = ComputedBlockStyle {
        text_align: TextAlign::Justify,
        text_align_last: TextAlignLast::Justify,
        ..ComputedBlockStyle::new(&style)
    };
    let mut layout = built(&mut cx, &block, |b| {
        b.text(NodeKey(1), "fixx");
    });
    let level = |length: f32| Straight {
        length,
        ..straight(1.0, 0.0)
    };
    along(&mut cx, &mut layout, &[level(100.0)], 0.0);
    let line = layout.line(0).expect("a line");
    let on_the_line: Vec<(u32, f32)> = line
        .items()
        .filter_map(|item| match item {
            Item::Text(run) => Some(run),
            _ => None,
        })
        .flat_map(|run| run.glyphs().map(|glyph| (glyph.id, thousandth(glyph.x))))
        .collect();
    assert_eq!(on_the_line, [(ligature, 0.0), (x, 58.333), (x, 90.0)]);
    let placed: Vec<(u32, f32)> = glyphs_along(&layout, 0, &level(80.0), PastEnds::Hidden)
        .iter()
        .map(|(_, glyph, place)| (glyph.id, thousandth(place.x)))
        .collect();
    assert_eq!(placed, [(ligature, 0.0), (x, 58.333)]);
}

/// Emphasis marks land where the straight line puts them, turned with the
/// marked character. Over upright text in a vertical line they stand too.
#[test]
fn emphasis_marks_follow_the_path() {
    let mut cx = context();
    let mut style = sized(&AHEM_FAMILY, 20.0);
    style.text.emphasis.marks = true;
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX");
    });
    let level = Straight {
        y: 100.0,
        ..straight(1.0, 0.0)
    };
    along(&mut cx, &mut layout, &[level], 0.0);
    let line = layout.line(0).expect("a line");
    let metrics = line.metrics();
    let marks: Vec<_> = paints(&line, &level, PastEnds::Hidden, |_| Decorates::None)
        .filter_map(|item| match item {
            PathPaint::Emphasis { mark, place } => Some((mark, place)),
            _ => None,
        })
        .collect();
    assert_eq!(marks.len(), 2);
    for (mark, place) in &marks {
        let y = 100.0 - (metrics.ascent - mark.baseline);
        assert!(
            near(place.x, metrics.left + mark.x) && near(place.y, y),
            "{place:?}"
        );
        assert_eq!(turn(place), (1.0, 0.0));
    }
    assert_eq!(marks[0].0.x, 10.0);
    // A vertical line down the page: marks stand over upright text.
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    let mut layout = built(&mut cx, &block, |b| {
        b.text(NodeKey(1), "\u{6f22}\u{5b57}");
    });
    let down = straight(0.0, 1.0);
    along(&mut cx, &mut layout, &[down], 0.0);
    let line = layout.line(0).expect("a line");
    let turns: Vec<(f32, f32)> = paints(&line, &down, PastEnds::Hidden, |_| Decorates::None)
        .filter_map(|item| match item {
            PathPaint::Emphasis { place, .. } => Some(turn(&place)),
            _ => None,
        })
        .collect();
    assert_eq!(turns, [(1.0, 0.0), (1.0, 0.0)]);
}

/// A decoration splits into one piece per character, as SVG and Chrome draw
/// it, each spanning its character on the decoration's baseline and turned
/// with it. On a level path the pieces meet end to end where the straight
/// bar was.
#[test]
fn decorations_are_cut_a_piece_a_character() {
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XXX");
    });
    let level = Straight {
        y: 100.0,
        ..straight(1.0, 0.0)
    };
    along(&mut cx, &mut layout, &[level], 0.0);
    let line = layout.line(0).expect("a line");
    let decorates = |key: NodeKey| {
        if key == NodeKey(0) {
            Decorates::Both
        } else {
            Decorates::None
        }
    };
    let mut under = Vec::new();
    let mut through = Vec::new();
    let mut order = Vec::new();
    for item in paints(&line, &level, PastEnds::Hidden, decorates) {
        match item {
            PathPaint::DecorationBeforeText(bar) => {
                assert_eq!(
                    bar.decoration().inline(),
                    InlineExtents {
                        left: 0.0,
                        right: 60.0
                    }
                );
                for PathPiece { inline, place } in bar.pieces() {
                    let inline = (inline.left, inline.right);
                    under.push((inline, place.x, place.y, turn(&place)));
                    order.push('u');
                }
            }
            PathPaint::DecorationAfterText(bar) => {
                for PathPiece { inline, place } in bar.pieces() {
                    through.push((inline, place.x, place.y));
                    order.push('t');
                }
            }
            PathPaint::Text(run) => order.extend(run.glyphs().map(|_| 'g')),
            _ => {}
        }
    }
    let expected = [
        ((0.0, 20.0), 0.0, 100.0, (1.0, 0.0)),
        ((20.0, 40.0), 20.0, 100.0, (1.0, 0.0)),
        ((40.0, 60.0), 40.0, 100.0, (1.0, 0.0)),
    ];
    assert_eq!(under, expected);
    assert_eq!(through.len(), 3);
    assert_eq!(order.iter().collect::<String>(), "uuugggttt");
    // Down the page, each piece turns with its character's glyph.
    let down = straight(0.0, 1.0);
    let mut pieces: Vec<(f32, f32, (f32, f32))> = Vec::new();
    for item in paints(&line, &down, PastEnds::Hidden, decorates) {
        if let PathPaint::DecorationBeforeText(under) = item {
            pieces.extend(
                under
                    .pieces()
                    .map(|PathPiece { place, .. }| (place.x, place.y, turn(&place))),
            );
        }
    }
    assert_eq!(
        pieces,
        [
            (0.0, 0.0, (0.0, 1.0)),
            (0.0, 20.0, (0.0, 1.0)),
            (0.0, 40.0, (0.0, 1.0))
        ]
    );
}

/// An atomic inline turns about its middle like a character. On a level
/// path its margin box's top-left corner lands where the straight line puts
/// it.
#[test]
fn an_atomic_inline_follows_the_path() {
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "X");
        b.atomic(
            NodeKey(2),
            &style,
            None,
            BoxSize {
                inline: 30.0,
                block: 10.0,
                baseline: None,
            },
        );
        b.text(NodeKey(3), "X");
    });
    let level = Straight {
        y: 100.0,
        ..straight(1.0, 0.0)
    };
    along(&mut cx, &mut layout, &[level], 0.0);
    let line = layout.line(0).expect("a line");
    let metrics = line.metrics();
    let atomics: Vec<_> = paints(&line, &level, PastEnds::Hidden, |_| Decorates::None)
        .filter_map(|item| match item {
            PathPaint::Atomic { atomic, place } => Some((atomic, place)),
            _ => None,
        })
        .collect();
    assert_eq!(atomics.len(), 1);
    let (item, place) = atomics[0];
    assert_eq!(item.key(), NodeKey(2));
    assert_eq!(
        item.inline(),
        InlineExtents {
            left: 20.0,
            right: 50.0
        }
    );
    let top = 100.0 - (metrics.ascent - item.block().over);
    assert!(near(place.x, 20.0) && near(place.y, top), "{place:?}");
    assert_eq!(turn(&place), (1.0, 0.0));
}

/// A polyline skips coincident and NaN points and has a rightward tangent
/// where it has no length; `from_measured` places as `new` did. A Bézier
/// path's straight segment is as long as its chord, and a quadratic raised
/// to a cubic follows the quadratic.
#[test]
fn polylines_and_bezier_paths_measure_and_place() {
    let points = [
        point(0.0, 0.0),
        point(0.0, 0.0),
        point(3.0, 4.0),
        point(f32::NAN, 1.0),
        point(3.0, 14.0),
    ];
    let mut ends = [0.0; 5];
    let path = Polyline::new(&points, &mut ends);
    assert_eq!(path.length(), 5.0, "the legs by the NaN point take nothing");
    let at = path.at(0.0);
    assert_eq!((at.x, at.y, at.dx, at.dy), (0.0, 0.0, 0.6, 0.8));
    let halfway = path.at(2.5);
    assert_eq!(ends, [0.0, 0.0, 5.0, 5.0, 5.0]);
    let kept = Polyline::from_measured(&points, &ends);
    assert_eq!(kept.at(2.5), halfway);
    // A short buffer measures the points it holds.
    let mut short = [0.0; 3];
    assert_eq!(Polyline::new(&points, &mut short).length(), 5.0);
    for points in [
        &[][..],
        &[point(7.0, 8.0)][..],
        &[point(1.0, 1.0), point(1.0, 1.0)][..],
    ] {
        let mut ends = [0.0; 2];
        let path = Polyline::new(points, &mut ends);
        assert_eq!(path.length(), 0.0);
        let at = path.at(0.0);
        assert_eq!((at.dx, at.dy), (1.0, 0.0), "{points:?}");
    }
    // A straight segment, then the quadratic (30, 40), (70, 40), (70, 80)
    // as a cubic.
    let (p0, c, p1) = (point(30.0, 40.0), point(70.0, 40.0), point(70.0, 80.0));
    let lift = |from: PagePoint| {
        point(
            from.x + (c.x - from.x) * 2.0 / 3.0,
            from.y + (c.y - from.y) * 2.0 / 3.0,
        )
    };
    let points = [
        point(0.0, 0.0),
        point(0.0, 0.0),
        point(30.0, 40.0),
        p0,
        lift(p0),
        lift(p1),
        p1,
    ];
    let mut table = [0.0; 65];
    let path = BezierPath::new(&points, &mut table);
    let at = path.at(25.0);
    assert!(near(at.x, 15.0) && near(at.y, 20.0) && near(at.dx, 0.6) && near(at.dy, 0.8));
    // The quadratic at t = ½ is (60, 50) with tangent (1, 1) / √2.
    let middle = (0..=1000u16)
        .map(|n| path.at(50.0 + f32::from(n) * 0.05))
        .min_by(|a, b| {
            let off = |at: &PathPoint| (at.x - 60.0).abs() + (at.y - 50.0).abs();
            off(a).total_cmp(&off(b))
        })
        .expect("points");
    assert!(
        (middle.x - 60.0).abs() < 0.05 && (middle.y - 50.0).abs() < 0.05,
        "{middle:?}"
    );
    assert!((middle.dx - middle.dy).abs() < 0.01, "{middle:?}");
    let length = path.length();
    let kept = BezierPath::from_measured(&points, &table);
    assert_eq!(kept.length(), length);
    // A table too short for one sample per segment measures chords.
    let mut tiny = [0.0; 2];
    assert_eq!(BezierPath::new(&points, &mut tiny).length(), 50.0);
    let mut none: [f32; 0] = [];
    assert_eq!(BezierPath::new(&points, &mut none).length(), 0.0);
}

/// No path, room or buffer panics: zero, NaN or infinite lengths, NaN
/// points, a path shorter than its line, no paths, and lines past the last
/// path. What cannot be placed is dropped, and every placement is finite.
#[test]
fn nothing_a_path_answers_panics() {
    /// A path that returns its value for every length and coordinate.
    struct Answers(f32);
    impl TextPath for Answers {
        fn length(&self) -> f32 {
            self.0
        }
        fn at(&self, _distance: f32) -> PathPoint {
            PathPoint {
                x: self.0,
                y: self.0,
                dx: self.0,
                dy: self.0,
            }
        }
    }
    let mut cx = context();
    let style = sized(&AHEM_FAMILY, 20.0);
    let mut layout = built(&mut cx, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "set along a path that is not one");
        b.atomic(
            NodeKey(2),
            &style,
            None,
            BoxSize {
                inline: 30.0,
                block: 10.0,
                baseline: None,
            },
        );
    });
    for value in [0.0, -5.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1e30] {
        let paths = [Answers(value), Answers(value)];
        for start in [0.0, -10.0, f32::NAN, 1e9] {
            along(&mut cx, &mut layout, &paths, start);
            for ends in [PastEnds::Hidden, PastEnds::Straight] {
                for line in 0..layout.lines().len() {
                    for place in placements(&layout, line, &paths[0], ends) {
                        assert!(place.transform().iter().all(|part| part.is_finite()));
                    }
                }
            }
            let _ = (paths[0].point(10.0, 10.0), ReversedPath(&paths[0]).at(3.0));
        }
    }
    // With no paths, no line has room.
    let none: [Straight; 0] = [];
    along(&mut cx, &mut layout, &none, 0.0);
    assert!(layout.lines().all(|line| line.metrics().band.size() == 0.0));
    // A path shorter than its line drops what is past its end.
    let short = Straight {
        length: 50.0,
        ..straight(1.0, 0.0)
    };
    layout.break_lines(&mut cx, Area::new(2000.0), &mut NoExclusions);
    let placed = glyphs_along(&layout, 0, &short, PastEnds::Hidden);
    assert!(!placed.is_empty() && placed.iter().all(|(_, _, place)| place.x <= 50.0));
    // Nonfinite points make a zero-length path.
    let points = [
        point(f32::NAN, 0.0),
        point(f32::INFINITY, 1.0),
        point(0.0, f32::NAN),
    ];
    let mut ends = [0.0; 3];
    let path = Polyline::new(&points, &mut ends);
    assert_eq!(path.length(), 0.0);
    let placed = glyphs_along(&layout, 0, &path, PastEnds::Straight);
    assert!(placed.is_empty(), "no point on it is a number");
}
