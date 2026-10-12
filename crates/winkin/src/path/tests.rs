//! Text on path tests. They pin:
//! - a straight path placing every glyph where the straight line does;
//! - a path turning each character the way it runs at the character's
//!   middle, its glyphs as one, and leaving out what is past its ends;
//! - each line as long as its own path, however often the breaker asks;
//! - upright text across a path, sideways text along it, and combined text
//!   narrowed along it;
//! - annotations, marks, decorations, atomic inlines and the ellipsis
//!   following the text;
//! - a path running on past its ends and backward;
//! - nothing a path or a buffer answers panicking.
//!
//! The figures are in Ahem: every glyph an em square, 0.8 of it over the
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

/// The point `x` across the page and `y` down it.
fn point(x: f32, y: f32) -> PagePoint {
    PagePoint::new(x, y)
}

const SPLIT: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed("Test Split"))];

/// A context over Ahem, and a font whose `s` is drawn as two glyphs, its
/// own half an em wide and a quarter of an em after it, and whose `r` is
/// drawn a fifth of an em up.
fn context() -> Context {
    let mut split = TestFont::new("Test Split", &[(0x20, 0x7E)]);
    split.splits = vec!['s'];
    split.raised = vec![('r', 200)];
    Context::new(collection(&[split], ahem_fallback()))
}

/// 20 px Ahem, on one line however long.
fn nowrap() -> ComputedStyle<'static> {
    let mut style = sized(&AHEM_FAMILY, 20.0);
    style.text.wrap_mode = TextWrapMode::NoWrap;
    style
}

/// A layout of what `calls` builds in `block`.
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

/// `layout` broken along `paths`, each line `start` along its own.
fn along<P: TextPath>(cx: &mut Context, layout: &mut Layout, paths: &[P], start: f32) {
    let mut room = PathRoom::new(paths, start);
    layout.break_lines(cx, room.area(), &mut room);
}

/// A straight path at a slope, for placing lines along: from `(x, y)`,
/// running `(dx, dy)`, `length` long.
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

/// A straight path from the origin, running `(dx, dy)`, 1000 long.
fn straight(dx: f32, dy: f32) -> Straight {
    Straight {
        x: 0.0,
        y: 0.0,
        dx,
        dy,
        length: 1000.0,
    }
}

/// The glyphs line `line` of `layout` places along `path`, what hides past
/// its ends as `ends` says: each with its run and its placement.
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

/// Every placement line `line` of `layout` hands out along `path`, what
/// hides past its ends as `ends` says, decorations and all.
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

/// Each glyph of line `line`'s runs, in painting order, as the straight line
/// places it: its run and its place from the line box's left and top.
fn glyphs_on_the_line(line: &Line<'_>) -> Vec<(NodeKey, f32, f32)> {
    line.items()
        .filter_map(|item| match item {
            Item::Text(run) | Item::Generated(run) => Some(run),
            _ => None,
        })
        .flat_map(|run| run.glyphs().map(move |glyph| (run.key(), glyph.x, glyph.y)))
        .collect()
}

/// Whether `a` and `b` are the same to a thousandth.
fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// `x` rounded to a thousandth, halves away from zero, so that what a path
/// works out in floats compares.
fn thousandth(x: f32) -> f32 {
    (x * 1000.0 + 0.5 * x.signum()) as i32 as f32 / 1000.0
}

/// The way a placement turns its glyph, to a thousandth.
fn turn(place: &Placement) -> (f32, f32) {
    (thousandth(place.cos), thousandth(place.sin))
}

// Placing along a path ---------------------------------------------------

/// Along a straight path level with the baseline, every glyph goes where the
/// line would have put it, unturned: the line's baseline on the path, 100
/// down the page, and each glyph along it from where the line box starts.
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

/// Along a path running down the page each glyph is turned a quarter to face
/// it, its baseline along the path, and whatever runs past its end is left
/// off: of `downward`'s eight 20 px squares, whose middles are 10, 30, 50
/// and on along the line, a path 30 long holds the first two.
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
    // Down the page, each glyph's origin at the top of its square, on the
    // path: its baseline runs down it.
    let ys: Vec<f32> = all.iter().map(|place| place.y).collect();
    assert_eq!(ys, [0.0, 20.0, 40.0, 60.0, 80.0, 100.0, 120.0, 140.0]);
    assert!(all.iter().all(|place| place.x == 50.0));
    let some = placed(&mut cx, down(30.0));
    assert_eq!(some.len(), 2, "{some:?}");
    assert_eq!(some, all[..2]);
}

/// Each line is as long as the path it goes on, less the start, and the
/// lines past the last path take its length: the first line has 100 of its
/// 120, room for `one` in 20 px squares, and the second 300 of its 320,
/// room for `two three four`.
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

/// In a vertical line, an ideograph stands upright on the path whichever
/// way the path runs, its top toward the start of the path; text on its
/// side lies along it. Down the page an upright glyph is not turned at all,
/// and every glyph goes where the vertical line puts it: along the line
/// down the page, and across it from the central baseline, 10 in from the
/// line's right in 20 px Ahem.
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
        // Where the straight vertical line puts it.
        assert!(
            near(place.x, 10.0 - glyph.y) && near(place.y, glyph.x),
            "{place:?}"
        );
    }
    assert!(placed.windows(2).all(|pair| pair[1].2.y > pair[0].2.y));
    // Across the page, the ideographs lie with their tops to the left.
    let across = straight(1.0, 0.0);
    let placed = glyphs_along(&layout, 0, &across, PastEnds::Hidden);
    assert_eq!(turn(&placed[0].2), (0.0, -1.0));
    assert_eq!(turn(&placed[3].2), (1.0, 0.0));
}

/// Combined text squeezed into its em is narrowed along the path too, by the
/// run's own factor, and turns as one character: four Ahem digits, each an
/// em wide, in one em. Every other glyph is drawn as it is.
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
    // Along a curve, the four turn about their em's middle as one.
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

/// A ruby annotation comes with the line it is set over, over the base
/// along the path: `X` at 30 px under `XX` at half that, whose em box stands
/// on the base's, 27 over its baseline, and the line grows to hold it.
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
    // The reading is painted before the text it is over.
    let keys: Vec<NodeKey> = placed.iter().map(|(run, _, _)| run.key()).collect();
    assert_eq!(keys, [NodeKey(4), NodeKey(4), NodeKey(2)]);
    let base = placed[2].2;
    assert_eq!(base.y, 100.0, "the base on the path");
    for (_, _, reading) in &placed[..2] {
        assert!(near(reading.y, base.y - 27.0), "{reading:?}");
        assert_eq!(turn(reading), (1.0, 0.0));
    }
}

/// A path's points run on straight past its ends, and a path run backward
/// starts at its end, running the other way, its up down the page. Along a
/// path run backward a line is turned half about and set from the path's
/// end; what is past the path's end is left out, or set on straight past
/// it where the path is taken to run on.
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
    // Six 20 px squares, 120 along a path 100 long: the sixth's middle is
    // past its end.
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

/// A line asked about again is given its own path again: balancing lays a
/// paragraph out anew from its first line, asking its lines' bands a second
/// time, and each line keeps its own path's length. The second paragraph
/// here, `aaaa bbbb cccc` and `dddd` greedily on paths 300 long, is
/// balanced into two lines of two words, asked for after its second line
/// was. A room that took a line whose top is above the last one asked for
/// as the first line would give it the first path's room.
#[test]
fn a_line_is_given_its_own_path_however_often_it_is_asked() {
    /// A host that remembers which lines it was asked about, in order.
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
    // And breaking again from the start, at the same paths, asks nothing
    // it remembers.
    let bands: Vec<_> = layout.lines().map(|line| line.metrics().band).collect();
    along(&mut cx, &mut layout, &paths, 0.0);
    let again: Vec<_> = layout.lines().map(|line| line.metrics().band).collect();
    assert_eq!(bands, again);
}

/// The ellipsis a line is cut for is drawn along the path where it stands,
/// with the line's text. Four 20 px squares and
/// the ellipsis after them fill a path 100 long.
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

/// A character's glyphs turn as one about the middle of their joint
/// advance, their offsets kept, as SVG turns a typographic character: `s`
/// is drawn as two glyphs, 10 and 5 wide, its middle 17.5 along after an
/// `a` 10 wide; the path turns down the page at 16, between the two
/// glyphs' own middles, and both are turned down with the character. `r`,
/// drawn 4 up, stays 4 off the path.
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
    // `r`'s middle is 30 along, 14 down the second leg; its origin 5
    // back up the path, and 4 off it to the path's up, the page's right.
    assert_eq!(
        glyphs[3],
        (20.0, 9.0, (0.0, 1.0)),
        "`r`, raised off the path"
    );
}

/// A character of many glyphs -- an `s` drawn as two, under four accents
/// -- turns as one all the same, and so do two of them in a run. Along a
/// level path every glyph goes where the straight line puts it.
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
    // Round a bend, each character's glyphs turned alike.
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

/// A cluster that takes room and draws no glyph moves the characters after
/// it along the path as it moves them along the line, each character
/// turning about its own cluster's middle. A running pen that summed the
/// glyphs' advances would leave such a cluster out, and set every middle
/// after it short by its room. `fixx`
/// in a font whose `fi` is one glyph, three quarters of an em, justified
/// between every two letters over 100: the ligature's second cluster
/// draws nothing and takes a third of the 65 spread, so the first `x`
/// spans 58.3 to 90 and the second 90 to 100. Along a path 80 long, the
/// first `x`'s middle, 74.2, is on it and the second's, 95, past it.
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

/// Emphasis marks go where the marks of the straight line go, each turned
/// with the character it marks; over upright text in a vertical line they
/// stand with it.
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
    // Down a path in a vertical line, standing over upright text.
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

/// A decoration is drawn on a path as SVG and Chrome draw it: a piece a
/// character, from the character's start to its end on the decoration's
/// baseline, turned with it. Along a level path the pieces meet end to end
/// where the straight bar was; down a path each is turned as its glyph is.
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
    // Down the page, every piece turned as its character's glyph is.
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

/// An atomic inline is placed as a character is, about its middle: its
/// margin box's corner at its left and top goes where the straight line
/// puts it, along a level path.
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

/// A polyline measures its legs, passes over points that coincide and
/// points that are no number, and runs to the right where it has no length;
/// kept and handed back, its measure places as it did. A Bézier path's
/// straight segment is as long as its chord, and its quadratic one, raised
/// to a cubic, runs where the quadratic does.
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
    // A buffer too short measures what it holds.
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
    // A straight segment, and a quadratic one from (30, 40) through
    // (70, 40) to (70, 80), as a cubic.
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
    // The quadratic's middle, t = ½: (60, 50), running (1, 1) over √2.
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
    // A table too short for a sample a segment measures the chords it can.
    let mut tiny = [0.0; 2];
    assert_eq!(BezierPath::new(&points, &mut tiny).length(), 50.0);
    let mut none: [f32; 0] = [];
    assert_eq!(BezierPath::new(&points, &mut none).length(), 0.0);
}

/// Nothing a path, a room or a buffer answers panics: a path of no length,
/// of NaN or infinite length, whose points are no number, a path shorter
/// than a line, no paths at all, and a line past the last path. What cannot
/// be placed is left out; nothing placed is not a number.
#[test]
fn nothing_a_path_answers_panics() {
    /// A path whose every answer is `value`.
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
    // No path at all: no line has room.
    let none: [Straight; 0] = [];
    along(&mut cx, &mut layout, &none, 0.0);
    assert!(layout.lines().all(|line| line.metrics().band.size() == 0.0));
    // A path shorter than its line: what is past it is left out.
    let short = Straight {
        length: 50.0,
        ..straight(1.0, 0.0)
    };
    layout.break_lines(&mut cx, Area::new(2000.0), &mut NoExclusions);
    let placed = glyphs_along(&layout, 0, &short, PastEnds::Hidden);
    assert!(!placed.is_empty() && placed.iter().all(|(_, _, place)| place.x <= 50.0));
    // Points that are no number make a path of no length.
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
