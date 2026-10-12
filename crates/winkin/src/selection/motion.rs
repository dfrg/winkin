//! Caret and selection motion, as Chrome's `SelectionModifier` moves, measured in Chrome 153.
//!
//! Everything here moves in text order. Left and right by character, by word
//! and to a line's ends go on the screen, in `visual`. Line, paragraph and
//! document motion have no screen side. There left and right go by the
//! focus paragraph's direction, as Chrome's `ModifyMovingRight` is
//! `ModifyMovingForward` in a left-to-right paragraph.
//!
//! - By character: a stop at each grapheme cluster boundary. A grapheme split
//!   by a style boundary is one, unless a box's edge parts it. A builder
//!   break opportunity's two sides are one stop, as Chrome skips a `<wbr>`.
//! - An offset drawn at two places is a stop at each, where Chrome stops
//!   once. At a wrap they are the line's end, upstream, and the next line's
//!   start, downstream, with any builder break opportunity there between
//!   them. Where the text changes direction they are the edges of the text
//!   on each side. Forward reaches the upstream place first and backward the
//!   downstream one. A box edge with room is one stop each way.
//! - A stop keeps the side it is reached from. Going forward the caret lands
//!   upstream, with the text it passed; going backward, downstream. A line's
//!   start after a forced break is downstream either way. So at a box edge
//!   with room the caret is drawn on the side it came from, and at any node
//!   boundary typed text goes into the node it came from. Chrome lands
//!   downstream everywhere.
//! - By word: forward to the next word's start past the spaces (Windows) or
//!   to its end (macOS and Linux), backward to a word's start everywhere, as
//!   Blink's `NextWordPositionForPlatform`. Words are UAX #29's over each
//!   whole paragraph. The motion steps character stops until one starts or
//!   ends a word, asking `words` in place, so it costs the distance it moves.
//!   A forced break is a stop of its own. A word starting a line after a
//!   wrap stops at that line's start, downstream.
//! - By line: to the position the next line hits at the starting column,
//!   which the selection keeps across short lines, as Blink's
//!   `x_pos_for_vertical_arrow_navigation`. Past the last line it goes to the
//!   text's end; before the first, to its start.
//! - To a line's ends, logically, as Chrome's Home and End go. Home goes to
//!   the line's first offset. At a wrap after white space, removed or
//!   hanging, End goes past all of it to the wrap, upstream, so text typed
//!   there follows the space. Chrome's End stops before removed white space
//!   and after the first preserved space hanging, as Blink's
//!   `AdjustForSoftLineWrap` puts it. Elsewhere End goes to the end of what
//!   the line placed, after the first preserved space hanging past it, as
//!   Chrome's does. End reaches into text an ellipsis hides, as in Chrome.

use super::place::ClusteredPosition;
use super::words::Words;
use super::{
    Affinity, Granularity, Motion, MotionDirection, Position, Selection, WordMotion, hit, place,
    visual,
};
use crate::data::Id;
use crate::layout::Layout;
use crate::layout::Line;
use crate::stages::analysis::{BidiLevel, ClusterClass, ClusterId, ScriptRunId};
use crate::stages::lines::LineId;
use crate::work;

/// Returns `selection` moved by `motion`, as [`Selection::modify`] describes.
pub(super) fn modify(layout: &Layout, selection: Selection, motion: Motion) -> Selection {
    let focus = place::snap(layout, ClusteredPosition::new(layout, selection.focus));
    let anchor = if selection.anchor == selection.focus {
        focus
    } else {
        place::snap(layout, ClusteredPosition::new(layout, selection.anchor))
    };
    let snapped = Selection {
        anchor: anchor.position,
        focus: focus.position,
        goal: selection.goal,
    };
    // Left and right on the screen, where a motion has a side there.
    let screen = match motion.direction {
        MotionDirection::Left => Some(false),
        MotionDirection::Right => Some(true),
        MotionDirection::Forward | MotionDirection::Backward => None,
    };
    // Right is forward in a left-to-right paragraph, and left in a
    // right-to-left one, as Chrome maps them.
    let rtl = screen.is_some() && paragraph_level(layout, focus).is_rtl();
    let forward = match motion.direction {
        MotionDirection::Forward => true,
        MotionDirection::Backward => false,
        MotionDirection::Left | MotionDirection::Right => {
            (motion.direction == MotionDirection::Right) != rtl
        }
    };
    // An arrow on a selection collapses it to the end it points at.
    if !motion.extend && motion.granularity == Granularity::Character && !snapped.is_collapsed() {
        let to = match screen {
            Some(right) => visual::further(layout, anchor, focus, right, rtl),
            None if forward => snapped.end(),
            None => snapped.start(),
        };
        return Selection::from(to);
    }
    let mut goal = None;
    let clusters = &layout.analysis().clusters;
    // Character and word motion land on caret stops, which snapping keeps
    // where they are. What the other motions find is snapped.
    let to = match (motion.granularity, screen) {
        (Granularity::Character, Some(right)) => visual::by_character(layout, focus, right, rtl),
        (Granularity::Character, None) => by_character(layout, focus, forward),
        (Granularity::Word, Some(right)) => {
            visual::by_word(layout, focus, right, rtl, motion.word_motion)
        }
        (Granularity::Word, None) => by_word(layout, focus, forward, motion.word_motion),
        (Granularity::Line, _) => {
            let (to, column) = by_line(layout, focus, forward, selection.goal);
            goal = column;
            place::snap(layout, to)
        }
        (Granularity::LineBoundary, Some(right)) => {
            place::snap(layout, visual::line_end(layout, focus, right))
        }
        (Granularity::LineBoundary, None) => {
            place::snap(layout, line_boundary(layout, focus, forward))
        }
        (Granularity::ParagraphBoundary, _) => {
            place::snap(layout, paragraph_boundary(layout, focus, forward))
        }
        (Granularity::DocumentBoundary, _) => {
            let (end, affinity) = if forward {
                (clusters.end_id(), Affinity::Upstream)
            } else {
                (ClusterId::new(0), Affinity::Downstream)
            };
            place::snap(
                layout,
                ClusteredPosition::from_cluster(clusters, end, affinity),
            )
        }
    }
    .position;
    Selection {
        anchor: if motion.extend { anchor.position } else { to },
        focus: to,
        goal,
    }
}

/// Returns the base level of the paragraph holding `located`.
pub(super) fn paragraph_level(layout: &Layout, located: ClusteredPosition) -> BidiLevel {
    let paragraphs = &layout.analysis().paragraphs;
    paragraphs
        .containing(located.cluster)
        .and_then(|id| paragraphs.get(id))
        .map_or(BidiLevel::LTR, |paragraph| paragraph.level)
}

/// Returns the next caret stop from `from`, a snapped position, forward or backward, on the side it is reached from.
///
/// Where `from`'s offset has a second place the motion reaches next
/// ([`other_place`]), that is the stop. Elsewhere it skips past a builder
/// break opportunity that `from` starts the near side of, and where there is
/// no stop, it returns the text's end. The affinity is the side the motion
/// comes from ([`arrived`]).
pub(super) fn by_character(
    layout: &Layout,
    from: ClusteredPosition,
    forward: bool,
) -> ClusteredPosition {
    if let Some(other) = other_place(layout, from, forward) {
        return other;
    }
    arrived(layout, next_stop(layout, from.cluster, forward), forward)
}

/// Returns the other place of `from`'s offset, where a motion forward or backward reaches it after `from`.
///
/// A wrap's offset is at the end of one line, upstream, and the start of the
/// next, downstream; a builder break opportunity there joins the two. One
/// where the text changes direction is drawn by the text on each side. A
/// motion forward reaches the upstream place first, and one backward the
/// downstream one. A box edge with room is one stop each way, so its other
/// side is no place of this kind. `None` where there is none, or where
/// `from` is drawn there already.
fn other_place(
    layout: &Layout,
    from: ClusteredPosition,
    forward: bool,
) -> Option<ClusteredPosition> {
    let then = if forward {
        Affinity::Downstream
    } else {
        Affinity::Upstream
    };
    let analysis = layout.analysis();
    let clusters = &analysis.clusters;
    // Across builder break opportunities, the two sides are one stop.
    let mut other = from.cluster;
    loop {
        work::step();
        let crossed = if forward {
            other
        } else {
            match other.get().checked_sub(1) {
                Some(before) => ClusterId::new(before),
                None => break,
            }
        };
        if clusters.class(crossed) != Some(ClusterClass::BreakOpportunity) {
            break;
        }
        other = if forward {
            ClusterId::new(other.get() + 1)
        } else {
            crossed
        };
    }
    let other = ClusteredPosition::from_cluster(clusters, other, then);
    if other.position == from.position {
        return None;
    }
    let down = if forward { other.cluster } else { from.cluster };
    if place::starts_line(layout, down) && !place::follows_break(layout, down) {
        // The upper line's end is drawn on it, unless only builder break
        // opportunities stand between it and what the line placed.
        let up = if forward { from.cluster } else { other.cluster };
        let lone = up.get().checked_sub(1).is_some_and(|before| {
            clusters.class(ClusterId::new(before)) == Some(ClusterClass::BreakOpportunity)
        });
        return (!lone).then_some(other);
    }
    // A change of direction starts a script run, whose level differs in
    // direction from the run before.
    let runs = &analysis.runs;
    let turns = runs.containing(from.cluster).is_some_and(|id| {
        let level = |id: ScriptRunId| runs.get(id).map(|run| run.level.is_rtl());
        let before = id.get().checked_sub(1).map(ScriptRunId::new);
        runs.start(id, clusters.end_id()) == from.cluster
            && before.is_some_and(|before| level(before) != level(id))
    });
    if !turns || other.cluster != from.cluster || place::is_sided(layout, from) {
        return None;
    }
    let here = place::caret(layout, from, None)?;
    let there = place::caret(layout, other, Some(LineId::new(here.line)))?;
    (here.line != there.line || here.inline != there.inline).then_some(other)
}

/// Returns the cluster starting at the next caret stop from the start of `from`, forward or backward.
///
/// It is [`by_character`]'s stop without its affinity.
pub(super) fn next_stop(layout: &Layout, from: ClusterId, forward: bool) -> ClusterId {
    let clusters = &layout.analysis().clusters;
    let end = clusters.end_id();
    let mut to = from;
    let mut only_generated = true;
    loop {
        work::step();
        // The cluster crossed.
        let crossed = if forward {
            if to >= end {
                break;
            }
            let crossed = to;
            to = ClusterId::new(to.get() + 1);
            crossed
        } else {
            let Some(before) = to.get().checked_sub(1) else {
                break;
            };
            to = ClusterId::new(before);
            to
        };
        only_generated &= clusters.class(crossed) == Some(ClusterClass::BreakOpportunity);
        if !only_generated && place::is_stop(layout, to) {
            break;
        }
    }
    to
}

/// Returns the position at `stop`'s start as a character motion forward or backward arrives at it.
///
/// Forward it is upstream, with the text before it, so a step forward onto a
/// wrap lands at the upper line's end. Backward it is downstream, with the
/// text after it. A line's start after a forced break has one place, and is
/// downstream either way.
pub(super) fn arrived(layout: &Layout, stop: ClusterId, forward: bool) -> ClusteredPosition {
    let affinity = if forward && !place::follows_break(layout, stop) {
        Affinity::Upstream
    } else {
        Affinity::Downstream
    };
    ClusteredPosition::from_cluster(&layout.analysis().clusters, stop, affinity)
}

/// Returns the position at `stop`'s start as a word motion forward or backward arrives at it.
///
/// It is where a character motion arrives, except that a word starting a
/// line, after a wrap, is that line's start, downstream.
fn word_arrived(layout: &Layout, stop: ClusterId, forward: bool) -> ClusteredPosition {
    let arrived = arrived(layout, stop, forward);
    if arrived.position.affinity == Affinity::Upstream && place::starts_line(layout, stop) {
        return ClusteredPosition::from_cluster(
            &layout.analysis().clusters,
            stop,
            Affinity::Downstream,
        );
    }
    arrived
}

/// Returns the next word stop from `from`, forward or backward, as `rule` says.
///
/// It is the first character stop where a word starts, or, forward under
/// [`WordMotion::StopAtWordEnd`], ends. Where there is none, it is the
/// text's end. At a builder break opportunity it stops on the side it reaches
/// first, as a character motion does.
pub(super) fn by_word(
    layout: &Layout,
    from: ClusteredPosition,
    forward: bool,
    rule: WordMotion,
) -> ClusteredPosition {
    let mut words = Words::new(layout);
    let mut from = from.cluster;
    loop {
        work::step();
        let to = next_stop(layout, from, forward);
        if to == from || is_word_stop(&mut words, to, forward, rule) {
            return word_arrived(layout, to, forward);
        }
        from = to;
    }
}

/// Returns whether a word motion stops at the caret stop at `stop`'s start, as `rule` says.
///
/// Going forward it stops where a word starts, or under `StopAtWordEnd` where
/// one ends. Going backward it stops where a word starts, on every platform.
pub(super) fn is_word_stop(
    words: &mut Words<'_>,
    stop: ClusterId,
    forward: bool,
    rule: WordMotion,
) -> bool {
    match (forward, rule) {
        (true, WordMotion::StopAtWordEnd) => words.ends_word(stop),
        _ => words.starts_word(stop),
    }
}

/// Returns the position the next line down or up hits at the column, and the column.
///
/// The column is `goal`, or `focus`'s caret where there is none, in the
/// area's coordinates. Past the last line it is the text's end, and before
/// the first its start.
fn by_line(
    layout: &Layout,
    focus: ClusteredPosition,
    forward: bool,
    goal: Option<f32>,
) -> (ClusteredPosition, Option<f32>) {
    let Some(caret) = place::caret(layout, focus, None) else {
        return (focus, goal);
    };
    let Some(line) = Line::new(layout, LineId::new(caret.line)) else {
        return (focus, goal);
    };
    let column = goal.unwrap_or(line.metrics().left + caret.inline.left);
    let target = if forward {
        caret.line.checked_add(1)
    } else {
        caret.line.checked_sub(1)
    };
    let clusters = &layout.analysis().clusters;
    match target.and_then(|target| Line::new(layout, LineId::new(target))) {
        Some(next) => (
            hit::hit_x(layout, &next, column - next.metrics().left),
            Some(column),
        ),
        None if forward => (
            ClusteredPosition::from_cluster(clusters, clusters.end_id(), Affinity::Upstream),
            Some(column),
        ),
        None => (
            ClusteredPosition::from_cluster(clusters, ClusterId::new(0), Affinity::Downstream),
            Some(column),
        ),
    }
}

/// Returns the logical start of `focus`'s line, or its end where `forward`: Home and End.
fn line_boundary(layout: &Layout, focus: ClusteredPosition, forward: bool) -> ClusteredPosition {
    match place::caret_line(layout, focus, None) {
        Some(line) => line_edge(layout, &line, forward),
        None => focus,
    }
}

/// Returns the logical start of `line`, or its end where `forward`.
pub(super) fn line_edge(layout: &Layout, line: &Line<'_>, forward: bool) -> ClusteredPosition {
    let clusters = line.record().clusters();
    if !forward {
        let start = Position::new(line.text_range().start, Affinity::Downstream);
        return ClusteredPosition::from_walk(layout, clusters.start, start);
    }
    // The end of the line's content that does not hang. Where preserved
    // white space hangs past it, the end is after the first hanging character.
    let mut content = None;
    let mut hanging = None;
    for leaf in place::leaves(*line) {
        work::step();
        let (start, end) = (leaf.start, leaf.end);
        match leaf.run() {
            Some(run) if run.is_hanging() => {
                hanging = Some(hanging.map_or(start, |at: usize| at.min(start)));
            }
            _ => content = Some(content.map_or(end, |at: usize| at.max(end))),
        }
    }
    // At a soft wrap after white space, hanging or removed, the end is the
    // wrap's, past all of it, so text typed there follows the space.
    if place::wraps(layout, line) && (hanging.is_some() || place::wrap_end(layout, line).is_some())
    {
        return ClusteredPosition::from_cluster(
            &layout.analysis().clusters,
            clusters.end,
            Affinity::Upstream,
        );
    }
    let end = match (content, hanging) {
        (Some(content), Some(hangs)) if content > hangs => content,
        // The boundary after the cluster starting where the hanging begins.
        (_, Some(hangs)) => {
            let hangs = ClusteredPosition::from_walk(layout, clusters.end, Position::from(hangs));
            let after = ClusterId::new(hangs.cluster.get() + 1);
            let after = after.min(layout.analysis().clusters.end_id());
            return ClusteredPosition::from_cluster(
                &layout.analysis().clusters,
                after,
                Affinity::Upstream,
            );
        }
        (Some(content), None) => content,
        (None, None) => line.text_range().start,
    };
    ClusteredPosition::from_walk(layout, clusters.end, Position::new(end, Affinity::Upstream))
}

/// Returns the start of the paragraph holding `focus`, or where `forward` its end.
///
/// The end is before the paragraph's separator.
fn paragraph_boundary(
    layout: &Layout,
    focus: ClusteredPosition,
    forward: bool,
) -> ClusteredPosition {
    let analysis = layout.analysis();
    let clusters = &analysis.clusters;
    let paragraphs = &analysis.paragraphs;
    let Some(id) = paragraphs.containing(focus.cluster) else {
        return focus;
    };
    let range = paragraphs.clusters(id);
    if !forward {
        return ClusteredPosition::from_cluster(clusters, range.start, Affinity::Downstream);
    }
    // Before the separator the paragraph ends with, where it has one.
    let last = range
        .end
        .get()
        .checked_sub(1)
        .map(ClusterId::new)
        .filter(|&last| last >= range.start);
    let separated = last.is_some_and(|last| {
        clusters
            .class(last)
            .is_some_and(ClusterClass::is_forced_break)
    });
    let end = match last {
        Some(last) if separated => last,
        _ => range.end,
    };
    ClusteredPosition::from_cluster(clusters, end, Affinity::Upstream)
}
