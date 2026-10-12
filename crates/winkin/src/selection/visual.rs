//! Left and right motion on the screen, by character, by word and to a line's ends.
//!
//! Within a line where text reads one way, screen order follows text order,
//! reversed in a right-to-left paragraph. "One way" means every cluster
//! crossed, and one either side, is at the paragraph's base level and none
//! is combined text.
//! So a motion is first found in text order, and only text that changes
//! direction, or crosses an RTL line edge, is walked on the screen.
//!
//! - By character, a step goes to the nearest caret stop on the line the way
//!   the arrow points. Stops are ordered by leaf as drawn, then by cluster
//!   within the leaf, so both sides of a zero-width character stay separate
//!   stops. Past the line's edge, right continues onto the next line and
//!   left onto the previous line.
//! - Character motion's rules hold. A builder break opportunity's two sides
//!   are one stop where they are drawn at one place. A stop keeps the side
//!   the step came from: the leaf it leaves, upstream at that leaf's logical
//!   end, or inside a leaf upstream going the way it reads. At a wrap the
//!   line's end, upstream, and the next line's start are two stops. Past
//!   white space the wrap removed, the line's end is a stop past that space,
//!   beyond the leaves in the paragraph's direction.
//! - A box edge that takes room is one stop each way, on the side the step
//!   came from: the edge of the leaf it leaves. The edge of the leaf it
//!   enters is no stop, so the next step crosses the room and the cluster
//!   past it together. A line's first stop is its own place.
//! - At a leaf's edge, a position is a stop only where its caret is drawn
//!   there. Where the text changes direction, an offset is drawn at the edge
//!   of the text on each side, and is a stop at each. Another position drawn
//!   where the caret already is, across a change of direction, is passed
//!   over; the arrow would not move the caret.
//! - By word, character steps repeat until one lands where a text-order word
//!   motion would stop, read the way the landing run reads. Going the way
//!   the run reads, [`WordMotion`] decides; against it, the stop is a word's
//!   start. So a right-to-left run swaps a word's halves, and a caret leaving
//!   Arabic for the Latin after it does not skip a word.
//! - Two positions drawn at one place across a change of direction are one
//!   word stop. A word motion stops there where either would. A word
//!   starting a line after a wrap stops at that line's start, not at the
//!   upper line's end. A motion that finds no stop ends at the last one it
//!   reached.
//! - Inside a leaf a step follows text order, so a word crossed costs its
//!   length. Each leaf edge costs a walk of the leaves the step reads, from
//!   the focus's on.
//! - To a line's end, the motion goes to the edge of the leaf drawn there.
//!   Where that edge is the line's logical end or start, it is End or Home in
//!   text order. Elsewhere it is the position a point past that end hits.
//!   White space a wrap removed stands at the line's end in the paragraph's
//!   direction, so on that side the motion is End, past it.
//! - An arrow collapses a selection to the end drawn further that way, where
//!   both ends are at two places on one line. Elsewhere it collapses to the
//!   end the paragraph's direction maps the arrow to, as Chrome does.

use core::cmp::Ordering;

use super::place::{self, ClusteredPosition, Leaf};
use super::words::Words;
use super::{Affinity, Caret, Position, Selection, WordMotion, hit, motion};
use crate::data::{Id, IdRange};
use crate::layout::{Layout, Line};
use crate::stages::analysis::{ClusterClass, ClusterId, RunOrientation};
use crate::stages::lines::{InlineExtents, LineId};
use crate::stages::{Segments, Step};
use crate::style::FirstLineVariant;
use crate::work;

/// Returns the next caret stop right or left of `focus` on the screen, or `focus` where there is none.
///
/// `rtl` says whether `focus`'s paragraph reads right to left.
pub(super) fn by_character(
    layout: &Layout,
    focus: ClusteredPosition,
    right: bool,
    rtl: bool,
) -> ClusteredPosition {
    let forward = right != rtl;
    // In right-to-left text an offset's other place, at a wrap or a change
    // of direction, is never a step the fast path takes; the walk on the
    // screen finds it.
    let logical = if rtl {
        motion::arrived(
            layout,
            motion::next_stop(layout, focus.cluster, forward),
            forward,
        )
    } else {
        motion::by_character(layout, focus, forward)
    };
    if reads_one_way(layout, focus.cluster, logical.cluster, rtl) {
        if !rtl || advances(layout, focus, logical, right) {
            return logical;
        }
        // Right of the last line's right end there is no stop.
        if right && starts_last_line(layout, focus) {
            return focus;
        }
    }
    character_on_screen(layout, focus, right)
}

/// Returns whether `focus` is downstream at the start of the layout's last line, before drawn text.
///
/// The caller has checked that the text there reads right to left at the
/// paragraph's level. The line's first cluster is then its rightmost, so
/// the caret is at the line's right end with no stop past it, and no line
/// follows for a step right to go on to.
fn starts_last_line(layout: &Layout, focus: ClusteredPosition) -> bool {
    let lines = layout.line_records();
    if focus.position.affinity != Affinity::Downstream || lines.ruby.is_some() {
        return false;
    }
    let drawn = matches!(
        layout.analysis().clusters.class(focus.cluster),
        Some(ClusterClass::Text | ClusterClass::Symbol | ClusterClass::Emoji)
    );
    drawn
        && lines
            .lines
            .last()
            .is_some_and(|line| line.clusters().start == focus.cluster)
}

/// Returns whether a logical step moves visibly the requested way on the same line.
///
/// The caller has checked that the text reads one way. There a step that
/// stays at its offset does not move. A caret is drawn on the line holding
/// its offset, unless it is upstream at the line's end, so two such
/// positions on different lines have carets on different lines. Where both
/// are on one line and the step crosses only drawn text, it moves the way
/// the paragraph maps it. Only other steps place both carets, each a walk of
/// the line.
fn advances(layout: &Layout, from: ClusteredPosition, to: ClusteredPosition, right: bool) -> bool {
    // A step from a line's end at a wrap leaves no leaf; the walk on the
    // screen finds the side it lands on.
    if from.offset() == to.offset()
        || (from.position.affinity == Affinity::Upstream
            && place::starts_line(layout, from.cluster))
    {
        return false;
    }
    let lines = layout.line_records();
    // Whether a position on `line` is upstream at its end, where its caret
    // may be on the next line, after a forced break.
    let ends = |at: ClusteredPosition, line: LineId| {
        at.position.affinity == Affinity::Upstream
            && lines
                .lines
                .get(line)
                .is_none_or(|record| record.clusters().end == at.cluster)
    };
    if lines.ruby.is_none()
        && let Some(line) = place::line(layout, from, None)
        && !ends(from, line)
    {
        if place::line(layout, to, Some(line)) != Some(line) {
            if to.position.affinity == Affinity::Downstream {
                return false;
            }
        } else if !ends(to, line) && crosses_drawn_text(layout, line, from.cluster, to.cluster) {
            return true;
        }
    }
    let Some(from) = place::caret(layout, from, None) else {
        return false;
    };
    let Some(to) = place::caret(layout, to, Some(LineId::new(from.line))) else {
        return false;
    };
    from.line == to.line
        && if right {
            to.inline.left > from.inline.right
        } else {
            to.inline.right < from.inline.left
        }
}

/// Returns whether every cluster between `from` and `to`, both on `line`, is drawn in a leaf of the line.
///
/// A letter, a symbol or an emoji is. A space is where such a cluster is on
/// the line either side of the step; at a line's start or end it may be
/// removed, or hang.
fn crosses_drawn_text(layout: &Layout, line: LineId, from: ClusterId, to: ClusterId) -> bool {
    let clusters = &layout.analysis().clusters;
    let is_drawn = |cluster: ClusterId| {
        matches!(
            clusters.class(cluster),
            Some(ClusterClass::Text | ClusterClass::Symbol | ClusterClass::Emoji)
        )
    };
    let (low, high) = (from.min(to), from.max(to));
    let line_start = layout
        .line_records()
        .lines
        .get(line)
        .map(|record| record.clusters().start);
    let inside = || {
        line_start.is_some_and(|start| start < low)
            && is_drawn(ClusterId::new(low.get() - 1))
            && is_drawn(high)
    };
    let mut spaces = false;
    (low..high).ids().all(|cluster| {
        work::step();
        is_drawn(cluster)
            || matches!(
                clusters.class(cluster),
                Some(ClusterClass::Space | ClusterClass::NoBreakSpace | ClusterClass::OtherSpace)
            ) && (spaces || {
                spaces = inside();
                spaces
            })
    })
}

/// Returns the next word stop right or left of `focus` on the screen.
///
/// `rtl` says whether `focus`'s paragraph reads right to left. `rule` says
/// where a step moving the way its run reads stops.
pub(super) fn by_word(
    layout: &Layout,
    focus: ClusteredPosition,
    right: bool,
    rtl: bool,
    rule: WordMotion,
) -> ClusteredPosition {
    let logical = motion::by_word(layout, focus, right != rtl, rule);
    if reads_one_way(layout, focus.cluster, logical.cluster, rtl)
        && (!rtl
            || (advances(layout, focus, logical, right)
                && motion::is_word_stop(
                    &mut Words::new(layout),
                    logical.cluster,
                    right != rtl,
                    rule,
                )))
    {
        return logical;
    }
    word_on_screen(layout, focus, right, rule)
}

/// Returns the right or left end on the screen of `focus`'s line.
///
/// It is End or Home where the leaf drawn there reaches that end of the
/// line's text. Elsewhere it is the position a point past that end hits.
pub(super) fn line_end(
    layout: &Layout,
    focus: ClusteredPosition,
    right: bool,
) -> ClusteredPosition {
    let Some(line) = place::caret_line(layout, focus, None) else {
        return focus;
    };
    // White space a wrap removed stands at the line's end in the paragraph's
    // direction, as if it hung, and the end past it is End. On the other
    // side, the leaf drawn there ends the line.
    let wraps = place::wrap_end(layout, &line).is_some();
    if wraps && right != line.level().is_rtl() {
        return motion::line_edge(layout, &line, true);
    }
    let mut end: Option<Leaf<'_>> = None;
    let (mut first, mut last) = (usize::MAX, 0);
    for leaf in place::leaves(line) {
        work::step();
        if right || end.is_none() {
            end = Some(leaf);
        }
        first = first.min(leaf.start);
        last = last.max(leaf.end);
    }
    let Some(end) = end else {
        // A line with nothing on it has one position, found in text order.
        return motion::line_edge(layout, &line, right != line.level().is_rtl());
    };
    // The leaf's edge that way is its logical end where it reads that way,
    // and its start where it reads the other way.
    if right != end.is_rtl() {
        if end.end == last && !wraps {
            return motion::line_edge(layout, &line, true);
        }
    } else if end.start == first {
        return motion::line_edge(layout, &line, false);
    }
    let x = if right { f32::MAX } else { f32::MIN };
    hit::hit_x(layout, &line, x)
}

/// Returns the end of the selection from `anchor` to `focus` a right or left arrow collapses it to.
///
/// It is the end drawn further that way, where both are at two places on one
/// line. Elsewhere it is the end the paragraph's direction maps the arrow to;
/// `rtl` says whether `focus`'s paragraph reads right to left.
pub(super) fn further(
    layout: &Layout,
    anchor: ClusteredPosition,
    focus: ClusteredPosition,
    right: bool,
    rtl: bool,
) -> Position {
    let selection = Selection::new(anchor.position, focus.position);
    let at_focus = place::caret(layout, focus, None);
    let near = at_focus.map(|caret| LineId::new(caret.line));
    if let (Some(at_anchor), Some(at_focus)) = (place::caret(layout, anchor, near), at_focus)
        && at_anchor.line == at_focus.line
        && let Some(order) = at_anchor.inline.left.partial_cmp(&at_focus.inline.left)
        && order != Ordering::Equal
    {
        return if (order == Ordering::Greater) == right {
            anchor.position
        } else {
            focus.position
        };
    }
    if right != rtl {
        selection.end()
    } else {
        selection.start()
    }
}

/// Returns [`by_character`]'s answer walked on the screen, whatever the text.
///
/// It is the stop a step lands on, or `focus` where there is none.
pub(super) fn character_on_screen(
    layout: &Layout,
    focus: ClusteredPosition,
    right: bool,
) -> ClusteredPosition {
    place::caret(layout, focus, None)
        .and_then(|caret| step(layout, focus, &caret, right))
        .map_or(focus, |landing| landing.located)
}

/// Returns [`by_word`]'s answer walked on the screen, whatever the text.
///
/// It steps until one lands where a text-order word motion stops, read the
/// way the landing run reads. Where none does, it is the last stop reached.
/// Landings and their twins each have their own `Words`, so each run is
/// segmented once however they alternate.
pub(super) fn word_on_screen(
    layout: &Layout,
    focus: ClusteredPosition,
    right: bool,
    rule: WordMotion,
) -> ClusteredPosition {
    let (mut landings, mut twins) = (Words::new(layout), Words::new(layout));
    let mut at = focus;
    // The caret of `at` where a step found it, and the line of the last
    // caret found, where the next is expected.
    let mut caret: Option<Caret> = None;
    let mut near: Option<LineId> = None;
    let mut inside: Option<Inside> = None;
    loop {
        work::step();
        let landing = match inside.and_then(|leaf| within(layout, at, leaf, right)) {
            Some(landing) => Some(landing),
            None => caret
                .or_else(|| place::caret(layout, at, near))
                .and_then(|caret| step(layout, at, &caret, right)),
        };
        let Some(landing) = landing else {
            return at;
        };
        // The run the step lands in. At a change of direction, the twin is
        // the other position drawn at the same place, one stop with it.
        caret = None;
        let (rtl, twin) = match landing.inside {
            Some(leaf) => (leaf.rtl, None),
            None => match landing
                .caret
                .or_else(|| place::caret(layout, landing.located, near))
            {
                Some(found) => {
                    caret = Some(found);
                    near = Some(LineId::new(found.line));
                    (found.rtl, twin(layout, landing.located, &found))
                }
                None => (false, None),
            },
        };
        // A word starting a line after a wrap stops at that line's start, as
        // in text order, and not at the upper line's end.
        let upper = landing.located.position.affinity == Affinity::Upstream
            && place::starts_line(layout, landing.located.cluster);
        if !upper {
            if motion::is_word_stop(&mut landings, landing.located.cluster, right != rtl, rule) {
                return landing.located;
            }
            if let Some((twin, rtl)) = twin
                && motion::is_word_stop(&mut twins, twin.cluster, right != rtl, rule)
            {
                return twin;
            }
        }
        at = landing.located;
        inside = landing.inside;
    }
}

/// Returns another position drawn at `located`'s `caret` across a change of direction.
///
/// It comes with whether its leaf reads right to left. It is one place with
/// `located`, so a step from either passes over it. `None` where there is none.
fn twin(
    layout: &Layout,
    located: ClusteredPosition,
    caret: &Caret,
) -> Option<(ClusteredPosition, bool)> {
    if caret.inline.left != caret.inline.right {
        return None;
    }
    let x = caret.inline.left;
    let line = Line::new(layout, LineId::new(caret.line))?;
    let mut found = None;
    each_stop(layout, &line, |stop| {
        let other =
            stop.place.x == x && stop.rtl != caret.rtl && stop.located.offset() != located.offset();
        if found.is_none()
            && other
            && place::is_stop(layout, stop.located.cluster)
            && is_drawn(layout, &line, stop.located, x)
        {
            found = Some((arrival(layout, stop.located, &line).0, stop.rtl));
        }
    });
    found
}

/// Returns whether the text from cluster `from` to cluster `to`, and a cluster either side, reads one way.
///
/// Every cluster must be at the base level of a paragraph that reads right
/// to left where `rtl`, else left to right, and none may be combined text.
/// There screen order meets the stops text order does. A walk that ends
/// short, with a row missing, does not read one way.
fn reads_one_way(layout: &Layout, from: ClusterId, to: ClusterId, rtl: bool) -> bool {
    let stages = layout.stages().variant(FirstLineVariant::Standard);
    let clusters = &stages.analysis.clusters;
    let (low, high) = (from.min(to), from.max(to));
    let first = ClusterId::new(low.get().saturating_sub(1));
    let last = ClusterId::new((high.get() + 1).min(clusters.end_id().get()));
    let mut reached = first;
    for step in Segments::new(&stages, first..last) {
        work::step();
        let Step::Segment(segment) = step else {
            continue;
        };
        let (Some(paragraph), Some(script)) =
            (segment.paragraph_row(&stages), segment.script_row(&stages))
        else {
            return false;
        };
        let base = paragraph.level;
        // A run is at its clusters' level. A combined run stands as one
        // character.
        if base.is_rtl() != rtl
            || script.level != base
            || script.orientation == RunOrientation::Combined
        {
            return false;
        }
        reached = segment.end;
    }
    reached == last
}

/// Where a step on the screen lands: the position, and the leaf strictly around it.
///
/// The next step of a word motion walks that leaf in text order. The caret
/// comes with it where the step found it.
#[derive(Copy, Clone, Debug)]
struct Landing {
    located: ClusteredPosition,
    inside: Option<Inside>,
    caret: Option<Caret>,
}

/// A leaf a step landed inside: its text range and whether it reads right to left.
#[derive(Copy, Clone, Debug)]
struct Inside {
    start: usize,
    end: usize,
    rtl: bool,
}

impl Inside {
    /// Returns `located`, a stop strictly inside the leaf, on the side a step right or left comes from.
    ///
    /// A step the way the leaf reads lands upstream, and one against it
    /// downstream, as a motion in text order does.
    fn arrived(self, located: ClusteredPosition, right: bool) -> ClusteredPosition {
        let affinity = if right != self.rtl {
            Affinity::Upstream
        } else {
            Affinity::Downstream
        };
        ClusteredPosition {
            position: Position::new(located.offset(), affinity),
            ..located
        }
    }
}

/// Where a caret stop is along its line, in the order a walk on the screen meets it.
///
/// It orders by x, then by leaf left to right, then by stop within the leaf
/// left to right. The last key orders the two sides of a zero-width
/// character.
#[derive(Copy, Clone, Debug)]
struct Place {
    x: f32,
    leaf: i64,
    rank: i64,
}

impl Place {
    /// Returns a place before every stop at `x` for a walk right, or after every one for a walk left.
    ///
    /// At an infinite `x`, it is where a walk onto a new line starts.
    const fn new(x: f32, right: bool) -> Self {
        let edge = if right { i64::MIN } else { i64::MAX };
        Self {
            x,
            leaf: edge,
            rank: edge,
        }
    }

    /// Returns its order against `other` along the line, left to right.
    fn order(&self, other: &Self) -> Ordering {
        self.x
            .total_cmp(&other.x)
            .then(self.leaf.cmp(&other.leaf))
            .then(self.rank.cmp(&other.rank))
    }

    /// Returns whether it is past `other` the way a walk goes, right or left.
    fn is_past(&self, other: &Self, right: bool) -> bool {
        self.order(other)
            == if right {
                Ordering::Greater
            } else {
                Ordering::Less
            }
    }
}

/// A candidate caret stop of a line, as a walk on the screen meets it.
#[derive(Copy, Clone, Debug)]
struct Stop {
    /// The position there, drawn in its leaf, upstream at the leaf's logical end.
    located: ClusteredPosition,
    place: Place,
    /// Whether it is at its leaf's edge, where another leaf may draw its caret.
    edge: bool,
    /// Whether, at its leaf's edge, it is the left one.
    left: bool,
    /// Whether its leaf reads right to left.
    rtl: bool,
    /// Its leaf, where it is strictly inside it.
    inside: Option<Inside>,
}

/// Returns one step right or left on the screen from `focus`, whose caret is `caret`.
///
/// It lands on the nearest caret stop past the caret on its line, or else
/// the nearest on the next line the way the walk goes. `None` where there
/// is none.
fn step(layout: &Layout, focus: ClusteredPosition, caret: &Caret, right: bool) -> Option<Landing> {
    let mut line = Line::new(layout, LineId::new(caret.line))?;
    let mut stops = stops(layout, line, right, Some((focus, caret)));
    let mut on_focus_line = true;
    loop {
        // Whether a stop of this line has been passed over already.
        let mut passed = on_focus_line;
        let landing = stops.find(|stop| {
            // A builder break opportunity's two sides are one stop, and a
            // position is one stop with itself, where they are drawn at one
            // place. At a wrap or a change of direction they are two.
            let here = on_focus_line
                && caret.inline.left <= stop.place.x
                && stop.place.x <= caret.inline.right;
            if here && only_generated_between(layout, focus.cluster, stop.located.cluster) {
                return false;
            }
            // A leaf entered across a box's room is not stopped at its edge:
            // the step crosses the room and the cluster past it together. A
            // line's first stop is its own place.
            let entering = stop.edge && stop.left == right && passed;
            passed = true;
            if entering && place::is_sided(layout, stop.located) {
                return false;
            }
            // Skip another position drawn where the caret is, across a change
            // of direction; the arrow would not move the caret.
            let in_place = on_focus_line
                && caret.inline.left == caret.inline.right
                && stop.place.x == caret.inline.left
                && stop.rtl != caret.rtl;
            // At a leaf's edge, a stop counts only where its caret is drawn
            // there.
            !in_place && (!stop.edge || is_drawn(layout, &line, stop.located, stop.place.x))
        });
        if let Some(stop) = landing {
            let (located, caret) = match stop.inside {
                Some(leaf) => (leaf.arrived(stop.located, right), None),
                None => arrival(layout, stop.located, &line),
            };
            return Some(Landing {
                located,
                inside: stop.inside,
                caret,
            });
        }
        // Screen order continues right onto the next line, left onto the previous.
        let next = if right {
            line.index().checked_add(1)
        } else {
            line.index().checked_sub(1)
        }?;
        line = Line::new(layout, LineId::new(next))?;
        stops = self::stops(layout, line, right, None);
        on_focus_line = false;
    }
}

/// Returns a step on the screen from `at` inside `leaf`, the leaf the last step landed in.
///
/// It is the next text-order stop the way the leaf reads, where that is still
/// inside the leaf. Its caret is then drawn in the leaf, past `at`'s.
fn within(layout: &Layout, at: ClusteredPosition, leaf: Inside, right: bool) -> Option<Landing> {
    let to = motion::next_stop(layout, at.cluster, right != leaf.rtl);
    let to = ClusteredPosition::from_cluster(&layout.analysis().clusters, to, Affinity::Downstream);
    (leaf.start < to.offset() && to.offset() < leaf.end).then_some(Landing {
        located: leaf.arrived(to, right),
        inside: Some(leaf),
        caret: None,
    })
}

/// A line's caret stops past a place, nearest first, the way a walk on the
/// screen goes: right, from left to right; left, from right to left.
///
/// Its leaves come in the order they are drawn, and each leaf's stops by
/// place. A stop that is no caret stop ([`place::is_stop`]) is passed over.
/// Each stop handed out costs a walk of the leaf it is in, so a step costs
/// the leaves it reads, not a walk of its line for each stop it passes over.
struct Stops<'a, I> {
    layout: &'a Layout,
    line: Line<'a>,
    right: bool,
    /// The leaves still to read, the next from the end the walk goes toward.
    leaves: I,
    /// The index, left to right, of the next leaf read.
    next: i64,
    /// The leaf being read, with its index left to right.
    leaf: Option<(i64, Leaf<'a>)>,
    /// The place of the stop handed out last, or where the walk starts.
    past: Place,
    /// Whether the line has no leaf and its one stop, its start, is still to
    /// hand out.
    bare: bool,
    /// Whether the line's end past white space its wrap removed is still to
    /// look for, and whether it comes before the leaves. It is found only
    /// where the walk reaches it, since finding it reads the whole line.
    wrap: Option<bool>,
}

/// Returns the stops of `line` past where a walk on the screen enters it.
///
/// Where `focus` gives a position on the line and its caret, the walk starts
/// at the first stop at its offset drawn where its caret is, among the
/// leaves holding the offset. Where there is none, it starts at the caret's
/// x, before every stop there for a walk right and after every one for a
/// walk left. Without `focus`, the walk enters the line at its edge: its left
/// for a walk right, its right for a walk left.
fn stops<'a>(
    layout: &'a Layout,
    line: Line<'a>,
    right: bool,
    focus: Option<(ClusteredPosition, &Caret)>,
) -> Stops<'a, impl DoubleEndedIterator<Item = Leaf<'a>> + Clone> {
    let mut leaves = place::leaves(line);
    let edge = if right {
        f32::NEG_INFINITY
    } else {
        f32::INFINITY
    };
    let mut stops = Stops {
        layout,
        line,
        right,
        leaves: leaves.clone(),
        next: 0,
        leaf: None,
        past: Place::new(edge, right),
        bare: false,
        wrap: place::wraps(layout, &line).then_some(false),
    };
    let Some((focus, caret)) = focus else {
        // Entering the line on the wrap's side, its end comes first.
        stops.wrap = stops.wrap.map(|_| right == line.level().is_rtl());
        let count = if right {
            usize::from(leaves.next().is_some())
        } else {
            leaves.count()
        };
        stops.next = if right { 0 } else { leaf_key(count) - 1 };
        stops.bare = count == 0;
        return stops;
    };
    let offset = focus.offset();
    let there = |stop: &Stop| {
        stop.located.offset() == offset
            && caret.inline.left <= stop.place.x
            && stop.place.x <= caret.inline.right
    };
    let mut found: Option<(i64, Leaf<'a>, Place)> = None;
    let mut count = 0;
    for (at, leaf) in leaves.clone().enumerate() {
        work::step();
        count = at + 1;
        if found.is_some() || offset < leaf.start || leaf.end < offset {
            continue;
        }
        let at = leaf_key(at);
        leaf_stops(layout, at, &leaf, |stop| {
            if found.is_none() && there(&stop) {
                found = Some((at, leaf, stop.place));
            }
        });
    }
    let x = if right {
        caret.inline.right
    } else {
        caret.inline.left
    };
    stops.past = Place::new(x, right);
    stops.next = if right { 0 } else { leaf_key(count) - 1 };
    stops.bare = count == 0;
    if stops.bare {
        // The line's one stop is where the focus is, or no stop is.
        let start = bare_stop(layout, &line);
        if there(&start) {
            stops.past = start.place;
        }
    } else if let Some((at, leaf, place)) = found {
        // The walk goes on from the focus's leaf, past the leaves behind it.
        let before = usize::try_from(at).unwrap_or_default();
        if right {
            let _ = leaves.nth(before);
        } else {
            let _ = leaves.nth_back(count - 1 - before);
        }
        stops.leaves = leaves;
        stops.next = if right { at + 1 } else { at - 1 };
        stops.leaf = Some((at, leaf));
        stops.past = place;
    }
    stops
}

impl<'a, I: DoubleEndedIterator<Item = Leaf<'a>>> Stops<'a, I> {
    /// Returns the stop of the leaf `leaf`, the `at`th left to right,
    /// nearest past the last handed out.
    fn nearest(&self, at: i64, leaf: &Leaf<'_>) -> Option<Stop> {
        let (layout, past, right) = (self.layout, self.past, self.right);
        let mut best: Option<Stop> = None;
        leaf_stops(layout, at, leaf, |stop| {
            let nearer = best.is_none_or(|best| best.place.is_past(&stop.place, right));
            if nearer
                && stop.place.is_past(&past, right)
                && place::is_stop(layout, stop.located.cluster)
            {
                best = Some(stop);
            }
        });
        best
    }
}

impl<'a, I: DoubleEndedIterator<Item = Leaf<'a>>> Iterator for Stops<'a, I> {
    type Item = Stop;

    fn next(&mut self) -> Option<Stop> {
        if self.wrap == Some(true) {
            self.wrap = None;
            if let Some(stop) = wrap_stop(self.layout, &self.line) {
                self.past = stop.place;
                return Some(stop);
            }
        }
        loop {
            work::step();
            if let Some((at, leaf)) = self.leaf
                && let Some(stop) = self.nearest(at, &leaf)
            {
                self.past = stop.place;
                return Some(stop);
            }
            let leaf = if self.right {
                self.leaves.next()
            } else {
                self.leaves.next_back()
            };
            let Some(leaf) = leaf else {
                // Past the leaves toward it, the wrap's end, where the walk
                // has not passed it.
                if self.wrap.take().is_some()
                    && let Some(stop) = wrap_stop(self.layout, &self.line)
                {
                    let past = if self.right {
                        stop.place.x > self.past.x
                    } else {
                        stop.place.x < self.past.x
                    };
                    if past {
                        self.past = stop.place;
                        return Some(stop);
                    }
                }
                // A line with no leaf has one stop, its start.
                if !self.bare {
                    return None;
                }
                self.bare = false;
                let start = bare_stop(self.layout, &self.line);
                let counts = start.place.is_past(&self.past, self.right)
                    && place::is_stop(self.layout, start.located.cluster);
                return counts.then_some(start);
            };
            // The leaves are drawn in order along the line, so every stop of
            // one is past every stop of the one read before. A box's negative
            // margin could overlap two; the tests check that none of theirs
            // do.
            #[cfg(test)]
            debug_assert!(self.leaf.is_none_or(|(at, before)| {
                let edge = |leaf: &Leaf<'_>, at: i64, far: bool| Place {
                    x: if far == self.right {
                        leaf.inline.right
                    } else {
                        leaf.inline.left
                    },
                    leaf: at,
                    rank: 0,
                };
                !edge(&before, at, true).is_past(&edge(&leaf, self.next, false), self.right)
            }));
            self.leaf = Some((self.next, leaf));
            self.next += if self.right { 1 } else { -1 };
        }
    }
}

/// Returns `at`, a leaf's index along its line, as a place's key.
fn leaf_key(at: usize) -> i64 {
    i64::try_from(at).unwrap_or(i64::MAX)
}

/// Calls `each` with every caret stop `line` draws, as a walk on the screen meets them.
///
/// The stops are each boundary of each leaf, drawn in that leaf. A combined
/// unit and an atomic inline have only their ends. A line with no leaf has
/// one stop, its start.
fn each_stop(layout: &Layout, line: &Line<'_>, mut each: impl FnMut(Stop)) {
    let mut any = false;
    for (at, leaf) in place::leaves(*line).enumerate() {
        work::step();
        any = true;
        leaf_stops(layout, leaf_key(at), &leaf, &mut each);
    }
    if !any {
        each(bare_stop(layout, line));
    }
}

/// Calls `each` with every caret stop of `leaf`, the `at`th of its line left
/// to right, in text order.
///
/// A text run's clusters come with their stops. The edges of a combined
/// unit and an atomic inline have their clusters found by search.
fn leaf_stops(layout: &Layout, at: i64, leaf: &Leaf<'_>, mut each: impl FnMut(Stop)) {
    let clusters = &layout.analysis().clusters;
    let rtl = leaf.is_rtl();
    let inside = Inside {
        start: leaf.start,
        end: leaf.end,
        rtl,
    };
    let mut stop = |rank: usize, located: ClusteredPosition, x: f32, edge: bool| {
        let rank = i64::try_from(rank).unwrap_or(i64::MAX);
        each(Stop {
            located,
            place: Place {
                x,
                leaf: at,
                rank: if rtl { -rank } else { rank },
            },
            edge,
            left: (rank == 0) != rtl,
            rtl,
            inside: (!edge).then_some(inside),
        });
    };
    let located = |offset: usize, cluster: Option<ClusterId>| {
        ClusteredPosition::with_cluster(layout, leaf_position(leaf, offset), cluster)
    };
    let InlineExtents {
        left: left_edge,
        right: right_edge,
    } = leaf.inline;
    let (start_x, end_x) = if rtl {
        (right_edge, left_edge)
    } else {
        (left_edge, right_edge)
    };
    // Combined text is set across the line, so a walk along it passes
    // over the unit from one end to the other.
    let combined = leaf.is_combined();
    let mut places = leaf
        .run()
        .filter(|_| !combined)
        .map(|run| run.places().peekable());
    let first = places
        .as_mut()
        .and_then(|places| places.peek())
        .map(|&(cluster, _, _)| cluster)
        .filter(|&cluster| clusters.start(cluster).get() == leaf.start);
    stop(0, located(leaf.start, first), start_x, true);
    match places {
        Some(places) => {
            for (rank, (cluster, from, to)) in places.enumerate() {
                work::step();
                let offset = clusters.range(cluster).end.get();
                let x = if rtl { from } else { to }.to_px();
                let after = ClusteredPosition {
                    position: leaf_position(leaf, offset),
                    cluster: ClusterId::new(cluster.get() + 1),
                };
                stop(rank + 1, after, x, offset == leaf.end);
            }
        }
        None => stop(1, located(leaf.end, None), end_x, true),
    }
}

/// Returns the stop upstream at `line`'s end, where it wraps past white space it removed.
///
/// It stands past that space, beyond every leaf in the paragraph's direction.
fn wrap_stop(layout: &Layout, line: &Line<'_>) -> Option<Stop> {
    let caret = place::wrap_end(layout, line)?;
    let clusters = &layout.analysis().clusters;
    let end = line.record().clusters().end;
    let rtl = line.level().is_rtl();
    Some(Stop {
        located: ClusteredPosition::from_cluster(clusters, end, Affinity::Upstream),
        place: Place {
            x: caret.inline.left,
            leaf: if rtl { i64::MIN } else { i64::MAX },
            rank: 0,
        },
        edge: false,
        left: false,
        rtl,
        inside: None,
    })
}

/// Returns the one stop of `line`, which has no leaf: its start.
fn bare_stop(layout: &Layout, line: &Line<'_>) -> Stop {
    let clusters = &layout.analysis().clusters;
    let start = line.record().clusters().start;
    let start = ClusteredPosition::from_cluster(clusters, start, Affinity::Downstream);
    let near = Some(LineId::new(line.index()));
    let x = place::caret(layout, place::snap(layout, start), near)
        .map_or(0.0, |caret| caret.inline.left);
    Stop {
        located: start,
        place: Place {
            x,
            leaf: 0,
            rank: 0,
        },
        edge: false,
        left: false,
        rtl: line.level().is_rtl(),
        inside: None,
    }
}

/// Returns whether the caret of a stop at `located` is drawn on `line` at `x` along it.
///
/// That is false where another leaf across a change of direction draws it.
fn is_drawn(layout: &Layout, line: &Line<'_>, located: ClusteredPosition, x: f32) -> bool {
    let near = Some(LineId::new(line.index()));
    place::caret(layout, located, near).is_some_and(|caret| {
        caret.line == line.index() && caret.inline.left <= x && x <= caret.inline.right
    })
}

/// Returns whether only builder break opportunities lie between clusters `a` and `b`.
///
/// Such boundaries are one stop. Equal clusters also qualify.
fn only_generated_between(layout: &Layout, a: ClusterId, b: ClusterId) -> bool {
    let clusters = &layout.analysis().clusters;
    (a.min(b)..a.max(b)).ids().all(|cluster| {
        work::step();
        clusters.class(cluster) == Some(ClusterClass::BreakOpportunity)
    })
}

/// Returns a stop at `located` on `line` as a character motion arrives at it.
///
/// It keeps the side of the leaf it is drawn in, upstream at the leaf's
/// logical end, so the caret stays with the text it came from, and at a wrap
/// on the upper line. Where upstream is drawn on another line, it is
/// downstream. The caret comes with it where it was found.
fn arrival(
    layout: &Layout,
    located: ClusteredPosition,
    line: &Line<'_>,
) -> (ClusteredPosition, Option<Caret>) {
    if located.position.affinity == Affinity::Downstream {
        return (located, None);
    }
    let down = ClusteredPosition {
        position: Position::new(located.offset(), Affinity::Downstream),
        cluster: located.cluster,
    };
    let near = Some(LineId::new(line.index()));
    match place::caret(layout, located, near) {
        Some(up) if up.line == line.index() => (located, Some(up)),
        _ => (down, place::caret(layout, down, near)),
    }
}

/// Returns offset `at` of `leaf` as a position drawn in it, upstream at its logical end.
fn leaf_position(leaf: &Leaf<'_>, at: usize) -> Position {
    if at == leaf.end && leaf.start < leaf.end {
        Position::new(at, Affinity::Upstream)
    } else {
        Position::new(at, Affinity::Downstream)
    }
}
