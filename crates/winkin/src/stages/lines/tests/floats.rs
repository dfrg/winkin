//! Float tests:
//! - a float a paragraph opens with is placed before its first line is
//!   fitted;
//! - a float reached inside a line goes beside it where it fits beside the
//!   text before its anchor, and below it where it does not or where
//!   clearance would push it down; every later float follows it below;
//! - a line with no room beside the floats moves down past them where the
//!   block wraps, but not under `nowrap` nor for an indent;
//! - a line tried and not kept takes its floats back and places them again;
//! - a float the line breaks before is left to the next line;
//! - floats no line reaches are placed where the text ends;
//! - initial letters, the intrinsic sizes and relayout;
//! - hosts whose answers are nonsense or never settle.

use alloc::vec::Vec;
use core::cell::Cell;

use super::*;
use crate::FloatPlacement;
use crate::build::FloatSide;
use crate::config::Config;
use crate::style::{InitialLetter, LineClamp, LineGroup, LineHeight, TextBoxTrim};

/// A float the host has placed: its key, its side and its margin box.
#[derive(Copy, Clone, PartialEq, Debug)]
struct Held {
    key: NodeKey,
    side: FloatSide,
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

/// A host that places floats simply.
///
/// - Each float goes as high as asked, never above an earlier float's top,
///   and below every float before it where it clears them.
/// - It goes at its side's edge of the band where it fits, or lower.
/// - Each band is what the floats leave of the area, and of a shelf that
///   narrows it from some height down, like a float from a block before.
///
/// It keeps every request, trials included.
struct Host {
    left: f32,
    right: f32,
    placed: Vec<Held>,
    /// The shelf: from where down the area's right end comes in, and to
    /// where.
    shelf: Option<(f32, f32)>,
    /// The floats that clear every float before them.
    clears: Vec<NodeKey>,
    asked: Vec<FloatRequest>,
    /// How many times it is asked for a checkpoint.
    checkpoints: Cell<usize>,
}

impl Host {
    /// Returns a host with no floats, over lines `width` long from zero.
    fn new(width: f32) -> Self {
        Self {
            left: 0.0,
            right: width,
            placed: Vec::new(),
            shelf: None,
            clears: Vec::new(),
            asked: Vec::new(),
            checkpoints: Cell::new(0),
        }
    }

    /// Returns the host with the area's right end at `right` from `top`
    /// down.
    fn shelved(mut self, top: f32, right: f32) -> Self {
        self.shelf = Some((top, right));
        self
    }

    /// Returns the host with a float a block before placed, reaching in.
    ///
    /// The block's own checkpoints are all after it, so no rewind takes it
    /// back.
    fn reaching(mut self, held: Held) -> Self {
        self.placed.push(held);
        self
    }

    /// Returns the host with `key` clearing the floats before it.
    fn clearing(mut self, key: u64) -> Self {
        self.clears.push(NodeKey(key));
        self
    }

    /// Returns what the floats and the shelf leave of the area from `start`
    /// to `end`.
    fn room(&self, start: f32, end: f32) -> (f32, f32) {
        let (mut left, mut right) = (self.left, self.right);
        if let Some((top, edge)) = self.shelf
            && end > top
        {
            right = right.min(edge);
        }
        for float in &self.placed {
            if float.top < end && start < float.bottom && float.right > float.left {
                match float.side {
                    FloatSide::Left => left = left.max(float.right),
                    FloatSide::Right => right = right.min(float.left),
                }
            }
        }
        (left, right.max(left))
    }
}

impl Exclusions for Host {
    fn band(&self, _line: usize, block: BlockExtents) -> InlineExtents {
        let (left, right) = self.room(block.start, block.end);
        InlineExtents { left, right }
    }

    fn below(&self, top: f32) -> Option<f32> {
        self.placed
            .iter()
            .map(|float| float.bottom)
            .filter(|&bottom| bottom > top)
            .reduce(f32::min)
    }

    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        self.asked.push(float);
        let mut top = float.block_start;
        if self.clears.contains(&float.key) {
            top = self.placed.iter().map(|f| f.bottom).fold(top, f32::max);
        }
        top = self.placed.iter().map(|f| f.top).fold(top, f32::max);
        loop {
            let (left, right) = self.room(top, top + float.block_size);
            if right - left >= float.inline_size {
                break;
            }
            match self.below(top) {
                Some(next) => top = next,
                None => break,
            }
        }
        let (left, right) = self.room(top, top + float.block_size);
        let left = match float.side {
            FloatSide::Left => left,
            FloatSide::Right => right - float.inline_size,
        };
        let held = Held {
            key: float.key,
            side: float.side,
            left,
            right: left + float.inline_size,
            top,
            bottom: top + float.block_size,
        };
        self.placed.push(held);
        PlacedFloat {
            inline: InlineExtents {
                left: held.left,
                right: held.right,
            },
            block: BlockExtents {
                start: held.top,
                end: held.bottom,
            },
        }
    }

    fn checkpoint(&self) -> ExclusionsCheckpoint {
        self.checkpoints.set(self.checkpoints.get() + 1);
        ExclusionsCheckpoint(self.placed.len() as u64)
    }

    fn rewind(&mut self, to: ExclusionsCheckpoint) {
        self.placed
            .truncate(usize::try_from(to.0).unwrap_or(usize::MAX));
    }
}

/// Returns where each line's box starts across the block.
fn tops(layout: &Layout) -> Vec<f32> {
    layout.lines().map(|line| line.metrics().top).collect()
}

/// Returns the host's requests, by key and placement start.
fn asked(host: &Host) -> Vec<(NodeKey, f32)> {
    host.asked
        .iter()
        .map(|asked| (asked.key, asked.block_start))
        .collect()
}

/// A float anchored where a paragraph starts is placed at its first line's
/// top before the line is fitted, as Blink's `PositionLeadingFloats` does.
///
/// In Ahem at 10 px, on lines 10 tall, a float 50 wide and 25 tall narrows
/// the first three lines of a 100-wide area to 50. The fourth, below it,
/// has the whole width again. The first line keeps the float where the
/// host put it. The host is asked to place its margin box.
#[test]
fn a_float_a_paragraph_opens_with_is_what_its_first_line_starts_beside() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    let text = "XX XX XX XX XX XX XX XX XX XX";
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(50.0, 25.0));
        b.text(NodeKey(1), text);
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(
        texts(&layout),
        ["XX XX ", "XX XX ", "XX XX ", "XX XX XX ", "XX"]
    );
    assert_eq!(
        bands(&layout),
        [
            (50.0, 50.0),
            (50.0, 50.0),
            (50.0, 50.0),
            (0.0, 100.0),
            (0.0, 100.0)
        ]
    );
    let first = layout.line(0).expect("a line");
    let placed: Vec<FloatPlacement> = first.floats().collect();
    assert_eq!(placed.len(), 1);
    assert_eq!(
        (placed[0].key, placed[0].side),
        (NodeKey(2), FloatSide::Left)
    );
    assert_eq!((placed[0].inline.left, placed[0].inline.right), (0.0, 50.0));
    assert_eq!((placed[0].block.start, placed[0].block.end), (0.0, 25.0));
    assert!(
        layout
            .lines()
            .skip(1)
            .all(|line| line.floats().count() == 0)
    );
    assert_eq!(layout.floats().count(), 1);

    let mut margined = style;
    margined.edges.margin = Sides::<f32> {
        top: 0.0,
        right: 10.0,
        bottom: 5.0,
        left: -20.0,
    }
    .into();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(
            NodeKey(2),
            &margined,
            FloatSide::Left,
            float_size(50.0, 25.0),
        );
        b.text(NodeKey(1), text);
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(
        (host.asked[0].inline_size, host.asked[0].block_size),
        (40.0, 30.0)
    );
    assert_eq!(bands(&layout)[0], (40.0, 60.0));

    // Its size and margins are each truncated onto the grid, as Chrome
    // holds a box's lengths: 50.012 is 50, 10.012 is 10, -10.012 is -10 and
    // 10.99 is 10.984375.
    margined.edges.margin = Sides::<f32> {
        top: 10.99,
        right: 10.012,
        bottom: 0.0,
        left: -10.012,
    }
    .into();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(
            NodeKey(2),
            &margined,
            FloatSide::Left,
            float_size(50.012, 25.0),
        );
        b.text(NodeKey(1), text);
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(
        (host.asked[0].inline_size, host.asked[0].block_size),
        (50.0, 35.984375)
    );
}

/// A float reached inside a line goes at the line's top where it fits
/// beside the text before its anchor, as Blink's `HandleFloat` and
/// `ShouldPushFloatAfterLine` decide.
///
/// - The line is then fitted again in what the float leaves. `XX ` reaches
///   30, and a float 30 wide fits beside it in 100, so the first line holds
///   `XX XX ` in the 70 left.
/// - Where it does not fit, it is placed from the line's bottom, and the
///   line keeps its band. `XX XX XX ` reaches 80 without its space, and a
///   float 40 wide does not fit.
/// - A float after one that went below the line goes below it too, however
///   small.
/// - A collapsible space before the anchor comes off where that makes it
///   fit.
#[test]
fn a_float_reached_inside_a_line_is_placed_beside_it_where_it_fits() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX ");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(30.0, 15.0));
        b.text(NodeKey(3), "XX XX XX XX XX XX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(texts(&layout), ["XX XX ", "XX XX ", "XX XX XX"]);
    assert_eq!(bands(&layout), [(30.0, 70.0), (30.0, 70.0), (0.0, 100.0)]);
    assert_eq!(asked(&host), [(NodeKey(2), 0.0)]);
    assert_eq!(layout.line(0).expect("a line").floats().count(), 1);

    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX XX XX ");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(40.0, 15.0));
        b.float(NodeKey(4), &style, FloatSide::Right, float_size(5.0, 5.0));
        b.text(NodeKey(3), "XX XX XX XX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(texts(&layout)[0], "XX XX XX ");
    assert_eq!(bands(&layout)[0], (0.0, 100.0));
    assert_eq!(asked(&host), [(NodeKey(2), 10.0), (NodeKey(4), 10.0)]);
    assert_eq!(bands(&layout)[1], (40.0, 55.0));
    let first: Vec<NodeKey> = layout
        .line(0)
        .expect("a line")
        .floats()
        .map(|float| float.key)
        .collect();
    assert_eq!(first, [NodeKey(2), NodeKey(4)], "the first line holds both");

    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX XX XX ");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(20.0, 15.0));
        b.text(NodeKey(3), "XX XX XX XX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(texts(&layout)[0], "XX XX XX ");
    assert_eq!(bands(&layout)[0], (20.0, 80.0));
    assert_eq!(asked(&host), [(NodeKey(2), 0.0)]);
}

/// A float the host would put lower than the line's top goes below the line
/// instead, as Blink pushes a cleared float after the line.
///
/// Here the float clears a float before it. The host is asked at the line's
/// top, the answer is taken back, and the host is asked again from the
/// line's bottom.
#[test]
fn a_float_that_clears_goes_below_the_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(20.0, 40.0));
        b.text(NodeKey(1), "XX ");
        b.float(NodeKey(4), &style, FloatSide::Right, float_size(20.0, 10.0));
        b.text(NodeKey(3), "XX XX XX XX XX");
    });
    let mut host = Host::new(100.0).clearing(4);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(
        asked(&host),
        [(NodeKey(2), 0.0), (NodeKey(4), 0.0), (NodeKey(4), 10.0)]
    );
    assert_eq!(host.placed.len(), 2, "the trial's answer is taken back");
    assert_eq!(host.placed[1].top, 40.0);
    assert_eq!(bands(&layout)[0], (20.0, 80.0), "beside the first alone");
}

/// A line the floats narrow until nothing fits moves down to where the
/// bands next change, where the block wraps.
///
/// A float 90 wide leaves 10, in which `XX` does not fit, so the line moves
/// to the float's bottom and has the whole width. Under `nowrap` it stays
/// beside the float, overflowing, as in Chrome. An indent that leaves no
/// room moves nothing, since going lower makes no room for it.
#[test]
fn a_line_with_no_room_beside_a_float_moves_below_it() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(90.0, 25.0));
        b.text(NodeKey(1), "XX XX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(texts(&layout), ["XX XX"]);
    assert_eq!(tops(&layout), [25.0]);
    assert_eq!(bands(&layout), [(0.0, 100.0)]);
    assert_eq!(layout.metrics().block_end, 35.0);

    let mut nowrap = style;
    nowrap.text.wrap_mode = TextWrapMode::NoWrap;
    fixture.build(&mut layout, &ComputedBlockStyle::new(&nowrap), |b| {
        b.float(NodeKey(2), &nowrap, FloatSide::Left, float_size(90.0, 25.0));
        b.text(NodeKey(1), "XX XX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(tops(&layout), [0.0]);
    assert!(records(&layout)[0].flags.contains(LineFlags::OVERFLOWS));

    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..indented(95.0, 0.0, false, false)
        },
        |b| b.text(NodeKey(1), "XX XX"),
    );
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(tops(&layout), [0.0, 10.0], "an indent moves nothing");
}

/// A block with no float of its own moves its lines below floats reaching
/// in from the blocks before.
///
/// The host's float, 90 wide and 25 tall, leaves 10, so the first line
/// moves to 25 and the lines after stack on it. Text with no float takes
/// the plain path, which hands the float driver only such a line.
#[test]
fn a_line_moves_below_a_float_reaching_in_from_before() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.text(&mut layout, &style, "XX XX XX XX XX XX XX XX XX XX XX XX");
    let reaching = Held {
        key: NodeKey(99),
        side: FloatSide::Left,
        left: 0.0,
        right: 90.0,
        top: 0.0,
        bottom: 25.0,
    };
    let mut host = Host::new(100.0).reaching(reaching);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(tops(&layout), [25.0, 35.0, 45.0, 55.0]);
    assert!(host.asked.is_empty(), "nothing of the block's to place");
    assert_eq!(host.placed, [reaching]);
    // Beside a float that leaves room, the lines stay and are narrowed.
    let narrower = Held {
        right: 40.0,
        ..reaching
    };
    let mut host = Host::new(100.0).reaching(narrower);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(tops(&layout)[..3], [0.0, 10.0, 20.0]);
    assert_eq!(bands(&layout)[..3], [(40.0, 60.0); 3]);
}

/// A line that moves down takes back the floats it placed while tried, and
/// places them again where it goes.
///
/// Chrome likewise restores its exclusion space for each layout
/// opportunity. The float after `X` fits beside it in the 20 a float 80
/// wide leaves, but its word does not. So the line moves below the wide
/// float, and the small float is placed again at the new top, kept once.
/// When the line asks where to go, the host has only the floats from before
/// it. The small float's own bottom would move the line past only itself.
#[test]
fn a_line_that_moves_places_its_floats_again() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(80.0, 25.0));
        b.text(NodeKey(1), "X");
        b.float(NodeKey(4), &style, FloatSide::Left, float_size(5.0, 5.0));
        b.text(NodeKey(3), "XXXXXX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(tops(&layout), [25.0]);
    assert_eq!(
        asked(&host),
        [(NodeKey(2), 0.0), (NodeKey(4), 0.0), (NodeKey(4), 25.0)]
    );
    assert_eq!(host.placed.len(), 2);
    assert_eq!(host.placed[1].top, 25.0);
    assert_eq!(bands(&layout), [(5.0, 95.0)]);
}

/// A float whose anchor the line no longer reaches once fitted beside it is
/// taken back, and the next line reaches it again.
///
/// `XXX` straddles the anchor of a float 30 wide, which fits beside
/// `XX XXX` in 100. Beside it only `XX ` fits, so the line ends there
/// without the float, in the band left once it is gone. The next line
/// places it.
#[test]
fn a_float_the_line_breaks_before_is_the_next_lines() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX XXX");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(30.0, 15.0));
        b.text(NodeKey(3), "XXX XX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(texts(&layout), ["XX ", "XXXXXX ", "XX"]);
    assert_eq!(bands(&layout)[0], (0.0, 100.0));
    assert_eq!(layout.line(0).expect("a line").floats().count(), 0);
    assert_eq!(layout.line(1).expect("a line").floats().count(), 1);
    assert_eq!(host.placed.len(), 1);
    assert_eq!(host.placed[0].top, 10.0);
}

/// Floats no line reaches are placed where the text ends, as Blink places
/// the floats of a line with nothing else on it.
///
/// They sit after a final forced break, or in a block with no text. A float
/// on a forced break's far side belongs to the next line, at its top.
#[test]
fn floats_no_line_reaches_are_placed_where_the_text_ends() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX");
        b.line_break(NodeKey(2));
        b.float(NodeKey(3), &style, FloatSide::Left, float_size(10.0, 10.0));
        b.text(NodeKey(4), "XX");
        b.line_break(NodeKey(5));
        b.float(NodeKey(6), &style, FloatSide::Left, float_size(10.0, 10.0));
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(layout.lines().len(), 2);
    assert_eq!(layout.line(0).expect("a line").floats().count(), 0);
    assert_eq!(layout.line(1).expect("a line").floats().count(), 1);
    let placed: Vec<(NodeKey, f32)> = layout
        .floats()
        .map(|float| (float.key, float.block.start))
        .collect();
    assert_eq!(placed, [(NodeKey(3), 10.0), (NodeKey(6), 20.0)]);

    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(NodeKey(3), &style, FloatSide::Left, float_size(10.0, 10.0));
    });
    let mut host = Host::new(100.0);
    let area = Area {
        block_start: 7.0,
        ..Area::new(100.0)
    };
    fixture.lay_out_with(&mut layout, area, &mut host);
    assert_eq!(layout.lines().len(), 0);
    assert_eq!(asked(&host), [(NodeKey(3), 7.0)]);
    assert_eq!(layout.floats().count(), 1);
}

/// Floats past a `line-clamp` are not placed, since their text is on no
/// line.
///
/// Clamped to one line, the float anchored on it is placed, below it where
/// it does not fit beside. The floats after the clamp, one inside the
/// second line and one after a forced break, are never asked for. The block
/// ends after its one line.
#[test]
fn floats_past_the_clamp_are_not_placed() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(1),
        ..ComputedBlockStyle::new(&style)
    };
    fixture.build(&mut layout, &clamped, |b| {
        b.text(NodeKey(1), "XXXX ");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(10.0, 10.0));
        b.text(NodeKey(3), "XXXX XXXX ");
        b.float(NodeKey(4), &style, FloatSide::Left, float_size(10.0, 10.0));
        b.text(NodeKey(5), "XXXX");
        b.line_break(NodeKey(6));
        b.float(NodeKey(7), &style, FloatSide::Left, float_size(10.0, 10.0));
        b.text(NodeKey(8), "XX");
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert_eq!(layout.lines().len(), 1);
    assert!(layout.line(0).expect("a line").has_ellipsis());
    let placed: Vec<NodeKey> = layout.floats().map(|float| float.key).collect();
    assert_eq!(placed, [NodeKey(2)]);
    assert!(asked(&host).iter().all(|&(key, _)| key == NodeKey(2)));
    assert_eq!(layout.metrics().block_end, 10.0);
}

/// Floats past a clamp by height are not placed either.
///
/// The lines are measured until one ends past the block's end. That line's
/// floats are taken back before the kept lines are laid out again. Here the
/// second line, which holds a float, ends at 20, past an end of 15. The
/// block keeps the first line alone, with its float.
#[test]
fn floats_past_a_clamp_by_height_are_not_placed() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Auto,
        ..ComputedBlockStyle::new(&style)
    };
    fixture.build(&mut layout, &clamped, |b| {
        b.text(NodeKey(1), "XXXX ");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(10.0, 10.0));
        b.text(NodeKey(3), "XXXX XXXX ");
        b.float(NodeKey(4), &style, FloatSide::Left, float_size(10.0, 10.0));
        b.text(NodeKey(5), "XXXX");
    });
    let mut host = Host::new(100.0);
    let area = Area {
        block_end: Some(15.0),
        ..Area::new(100.0)
    };
    fixture.lay_out_with(&mut layout, area, &mut host);
    assert_eq!(layout.lines().len(), 1);
    assert!(layout.line(0).expect("a line").has_ellipsis());
    let placed: Vec<NodeKey> = layout.floats().map(|float| float.key).collect();
    assert_eq!(placed, [NodeKey(2)]);
    let held: Vec<NodeKey> = host.placed.iter().map(|float| float.key).collect();
    assert_eq!(held, [NodeKey(2)], "the measure's floats taken back");
    assert_eq!(layout.metrics().block_end, 10.0);
}

/// A taller line fitted again in a narrower band takes back the floats it
/// placed at the strut's height and places them again.
///
/// A shelf narrows the area from 15 down. The strut's line, 10 tall, does
/// not reach it, but a line with a 40 px letter does. The line's float is
/// asked for twice and kept once.
#[test]
fn a_taller_line_places_its_floats_again_in_its_own_band() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    let large = ahem(40.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX ");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(10.0, 5.0));
        b.open_box(NodeKey(3), &large, None);
        b.text(NodeKey(4), "Y");
        b.close_box();
        b.text(NodeKey(5), " XX XX XX XX XX XX");
    });
    let mut host = Host::new(200.0).shelved(15.0, 150.0);
    fixture.lay_out_with(&mut layout, Area::new(200.0), &mut host);
    assert_eq!(bands(&layout)[0], (10.0, 140.0));
    assert_eq!(host.placed.len(), 1, "kept once");
    assert_eq!(asked(&host), [(NodeKey(2), 0.0), (NodeKey(2), 0.0)]);
}

/// Where the block trims its start, the host is asked for the later lines'
/// bands where the trim moved them, as Chrome lays them.
///
/// In Chrome 153, a float 35 px tall beside 40 px lines trimmed by 10
/// narrows the second line, which stands at 30. Untrimmed, the line clears
/// it. Here a shelf from 75 down narrows the second line of an untrimmed
/// block, which reaches 80, and not a trimmed one's, which reaches 70.
#[test]
fn the_lines_after_a_trimmed_first_are_asked_where_they_stand() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(40.0),
            ..LineGroup::INITIAL
        },
        ..ahem(20.0)
    };
    for trim in [TextBoxTrim::None, TextBoxTrim::TrimStart] {
        let block = ComputedBlockStyle {
            text_box_trim: trim,
            ..ComputedBlockStyle::new(&style)
        };
        fixture.build(&mut layout, &block, |b| {
            b.text(NodeKey(1), "XXXX XXXX XXXX XXXX XXXX");
        });
        let mut host = Host::new(200.0).shelved(75.0, 100.0);
        fixture.lay_out_with(&mut layout, Area::new(200.0), &mut host);
        let tops: Vec<f32> = layout.lines().map(|line| line.metrics().top).collect();
        if trim == TextBoxTrim::None {
            assert_eq!(texts(&layout), ["XXXX XXXX ", "XXXX ", "XXXX ", "XXXX"]);
            assert_eq!(tops, [0.0, 40.0, 80.0, 120.0]);
        } else {
            assert_eq!(texts(&layout), ["XXXX XXXX ", "XXXX XXXX ", "XXXX"]);
            assert_eq!(tops, [-10.0, 30.0, 70.0]);
        }
    }
}

/// A block's initial letter is reported to the host once its first line is
/// kept, as Chrome's `CreateExclusionSpaceForInitialLetterBox` makes it.
///
/// - Its margin box runs from the first line's top to the bottom of the ink
///   its box is fitted to, on the paragraph's start side.
/// - The lines beside it have what it leaves, and so does the line under
///   its sink where its glyph descends into it.
/// - In Ahem at 10 px on 10 px lines, a letter 3 lines tall is set at
///   35 px, its capitals 28 high, from the first to the third baseline. Its
///   `X` reaches 7 below.
/// - A raised letter moves the first line's text down by the lines it
///   stands above. The block's own style sets no letter.
#[test]
fn an_initial_letter_takes_room_from_the_lines_beside_it() {
    let mut fixture = fixture();
    fixture.cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let style = ahem(10.0);
    let mut letter = style;
    letter.line.initial_letter = InitialLetter {
        size: 3.0,
        sink: 3,
        ..InitialLetter::NONE
    };
    let text = " XX XX XX XX XX XX XX XX XX XX XX XX";
    for (direction, side) in [
        (BaseDirection::Ltr, FloatSide::Left),
        (BaseDirection::Rtl, FloatSide::Right),
    ] {
        let block = ComputedBlockStyle {
            direction,
            ..ComputedBlockStyle::new(&style)
        };
        fixture.build(&mut layout, &block, |b| {
            b.open_box(NodeKey(7), &letter, None);
            b.text(NodeKey(8), "X");
            b.close_box();
            b.text(NodeKey(1), text);
        });
        let mut host = Host::new(100.0);
        fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
        assert_eq!(host.asked.len(), 1);
        let asked = host.asked[0];
        assert_eq!((asked.key, asked.side), (NodeKey(7), side));
        assert_eq!(
            (asked.inline_size, asked.block_size, asked.block_start),
            (35.0, 35.0, 0.0)
        );
        let lengths: Vec<f32> = bands(&layout).iter().map(|band| band.1).collect();
        assert_eq!(lengths[..5], [100.0, 65.0, 65.0, 65.0, 100.0]);
        assert_eq!(tops(&layout)[..3], [0.0, 10.0, 20.0]);
        assert_eq!(layout.floats().count(), 0, "the letter is no float");
    }
    let mut raised = letter;
    raised.line.initial_letter.sink = 1;
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.open_box(NodeKey(7), &raised, None);
        b.text(NodeKey(8), "X");
        b.close_box();
        b.text(NodeKey(1), text);
    });
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    // The text moves down two lines; the letter stays, its baseline on the
    // first line's, and its glyph's foot reaches into the second.
    assert_eq!(tops(&layout)[..3], [0.0, 30.0, 40.0]);
    assert_eq!(asked(&host), [(NodeKey(7), 0.0)]);
    let lengths: Vec<f32> = bands(&layout).iter().map(|band| band.1).collect();
    assert_eq!(lengths[..3], [100.0, 65.0, 100.0]);
    fixture.text(&mut layout, &letter, text);
    let mut host = Host::new(100.0);
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut host);
    assert!(
        host.asked.is_empty(),
        "the block's own style sets no letter"
    );
}

/// An initial letter alone breaks without the float driver's trials.
///
/// The letter is one exclusion, known before the first line and placed once
/// that line is kept. So the host is never asked for a checkpoint, even
/// where lines move below the letter. The lines still match the driver's:
/// - the first line's band is asked at the letter's height, where a shelf
///   15 px down from a block before narrows it;
/// - later lines sit beside the letter where they fit, and below it where
///   the 3 px beside it hold nothing.
///
/// The text is Ahem at 10 px, with a letter 3 lines tall and 35 px wide.
#[test]
fn an_initial_letter_alone_is_placed_without_trials() {
    let mut fixture = fixture();
    fixture.cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let style = ahem(10.0);
    let mut letter = style;
    letter.line.initial_letter = InitialLetter {
        size: 3.0,
        sink: 3,
        ..InitialLetter::NONE
    };
    for (width, text, lines, top, bands_seen) in [
        (
            40.0,
            " XX XX XX XX",
            vec!["X ", "XX ", "XX ", "XX ", "XX"],
            vec![0.0, 35.0, 45.0, 55.0, 65.0],
            vec![(0.0, 38.0); 5],
        ),
        (
            60.0,
            " XX XX XX XX XX XX",
            vec!["X ", "XX ", "XX ", "XX ", "XX XX ", "XX"],
            vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0],
            vec![
                (0.0, 58.0),
                (35.0, 23.0),
                (35.0, 23.0),
                (35.0, 23.0),
                (0.0, 58.0),
                (0.0, 58.0),
            ],
        ),
    ] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
            b.open_box(NodeKey(7), &letter, None);
            b.text(NodeKey(8), "X");
            b.close_box();
            b.text(NodeKey(1), text);
        });
        let mut host = Host::new(width).shelved(15.0, width - 2.0);
        fixture.lay_out_with(&mut layout, Area::new(width), &mut host);
        assert_eq!(texts(&layout), lines, "in {width}");
        assert_eq!(tops(&layout), top, "in {width}");
        assert_eq!(bands(&layout), bands_seen, "in {width}");
        assert_eq!(asked(&host), [(NodeKey(7), 0.0)]);
        assert_eq!(host.checkpoints.get(), 0, "trials in {width}");
    }
}

/// The intrinsic sizes count floats.
///
/// A max-content line sets every float its paragraph holds beside it. A
/// float after a forced break belongs to the next paragraph. No line is
/// narrower than the widest float. Laid out at max-content, nothing wraps.
#[test]
fn floats_are_counted_in_the_intrinsic_sizes() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(30.0, 10.0));
        b.text(NodeKey(1), "XX XX");
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (30.0, 80.0));
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX XX XX");
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(5.0, 10.0));
        b.line_break(NodeKey(3));
        b.float(NodeKey(4), &style, FloatSide::Left, float_size(45.0, 10.0));
        b.text(NodeKey(5), "XX XX");
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (45.0, 95.0));
    let mut host = Host::new(sizes.max_content);
    fixture.lay_out_with(&mut layout, Area::new(sizes.max_content), &mut host);
    assert_eq!(texts(&layout), ["XX XX XX\n", "XX XX"]);
}

/// A relayout around floats equals a fresh layout, record for record and
/// float for float.
///
/// The widths put floats beside and below lines, and lines below floats.
/// The block opens with an initial letter whose room its host keeps.
#[test]
fn a_relayout_around_floats_equals_a_fresh_layout() {
    let mut fixture = fixture();
    let style = ahem(10.0);
    let mut letter = style;
    letter.line.initial_letter = InitialLetter {
        size: 2.0,
        sink: 2,
        ..InitialLetter::NONE
    };
    let build = |fixture: &mut Fixture, layout: &mut Layout| {
        fixture.build(layout, &ComputedBlockStyle::new(&style), |b| {
            b.open_box(NodeKey(7), &letter, None);
            b.text(NodeKey(8), "X");
            b.close_box();
            b.float(NodeKey(2), &style, FloatSide::Left, float_size(30.0, 25.0));
            b.text(NodeKey(1), "XX XX ");
            b.float(NodeKey(3), &style, FloatSide::Right, float_size(20.0, 15.0));
            b.text(NodeKey(4), "XXXX XX XX ");
            b.float(NodeKey(5), &style, FloatSide::Left, float_size(60.0, 10.0));
            b.text(NodeKey(6), "XX XX XX XX XX XX");
        });
    };
    let mut layout = Layout::new();
    build(&mut fixture, &mut layout);
    for width in [100.0, 45.0, 70.0, 300.0, 10.0, 100.0] {
        fixture.lay_out_with(&mut layout, Area::new(width), &mut Host::new(width));
        let mut fresh = Layout::new();
        build(&mut fixture, &mut fresh);
        fixture.lay_out_with(&mut fresh, Area::new(width), &mut Host::new(width));
        let (a, b) = (layout.line_records(), fresh.line_records());
        assert_eq!(a.lines.as_slice(), b.lines.as_slice(), "at {width}");
        assert_eq!(a.floats.as_slice(), b.floats.as_slice(), "at {width}");
        assert_eq!(a.block, b.block);
    }
}

/// A host whose answers are nonsense still gives valid lines, and one whose
/// answers never settle cannot keep a line moving.
///
/// - Bands are not numbers or run backwards.
/// - `below` is not a number, infinite, not below, or a hair below forever.
/// - Places are not numbers.
/// - Float sizes are not numbers, or past the grid.
#[test]
fn any_host_gives_valid_lines() {
    struct Hostile(usize);
    impl Exclusions for Hostile {
        fn band(&self, _line: usize, _block: BlockExtents) -> InlineExtents {
            match self.0 {
                0 => InlineExtents {
                    left: f32::NAN,
                    right: f32::NAN,
                },
                1 => InlineExtents {
                    left: 80.0,
                    right: -30.0,
                },
                _ => InlineExtents {
                    left: 95.0,
                    right: f32::INFINITY,
                },
            }
        }
        fn below(&self, top: f32) -> Option<f32> {
            match self.0 {
                0 => Some(f32::NAN),
                1 => Some(f32::INFINITY),
                2 => Some(top),
                3 => Some(top - 5.0),
                _ => Some(top + 1e-6),
            }
        }
        fn place(&mut self, _float: FloatRequest) -> PlacedFloat {
            PlacedFloat {
                inline: InlineExtents {
                    left: f32::NAN,
                    right: f32::NEG_INFINITY,
                },
                block: BlockExtents {
                    start: f32::NAN,
                    end: f32::NAN,
                },
            }
        }
        fn checkpoint(&self) -> ExclusionsCheckpoint {
            ExclusionsCheckpoint(u64::MAX)
        }
        fn rewind(&mut self, _to: ExclusionsCheckpoint) {}
    }
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.float(NodeKey(2), &style, FloatSide::Left, float_size(1e30, -3.0));
        b.text(NodeKey(1), "XX XX ");
        b.float(
            NodeKey(3),
            &style,
            FloatSide::Right,
            float_size(f32::NAN, 5.0),
        );
        b.text(NodeKey(4), "XX XX XX");
    });
    for which in 0..5 {
        for block_start in [0.0, -1e30, f32::NAN] {
            let area = Area {
                block_start,
                ..Area::new(100.0)
            };
            fixture.lay_out_with(&mut layout, area, &mut Hostile(which));
        }
    }
    // A hair below forever moves the first line the most times a line may
    // move, a 1/64 at a time. Then it stays and overflows.
    fixture.lay_out_with(&mut layout, Area::new(100.0), &mut Hostile(4));
    let line = layout.line(0).expect("a line");
    assert_eq!(
        line.metrics().top,
        super::super::fit_floats::MAX_MOVES_DOWN as f32 / 64.0
    );
    assert!(records(&layout)[0].flags.contains(LineFlags::OVERFLOWS));
}
