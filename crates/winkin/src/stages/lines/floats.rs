//! Float placement, as more of [`Breaker`]'s `impl`.
//!
//! Fitting is a pure function of a band; `Breaker::line` in `fit` places the
//! floats around it, as Blink's breaker and `InlineLayoutAlgorithm` do:
//! - Floats anchored at a paragraph's start go at its first line's top
//!   before it is fitted (`PositionLeadingFloats`).
//! - A float anchored on the line goes at its top where it fits beside the
//!   text before its anchor; the line is then refitted.
//! - Otherwise it and every later float on the line go below the line
//!   (`ShouldPushFloatAfterLine`, `PlaceFloatingObjects`).
//! - A line taller than its band's query height is refitted at its height.
//! - In a wrapping block, a line that floats squeeze into overflow moves down
//!   to where the bands next change.
//!
//! A rejected trial rewinds through the host's checkpoint (Blink's
//! `RewindFloats`). Without `FLOATS`, set where the content has floats, the
//! driver still avoids the host's floats but takes no checkpoint and places
//! nothing. An initial letter is not a content float.

use super::fit::{LineTrial, MAX_MOVES_DOWN, moves_to};
use super::reshape::EndSpaces;
use super::{
    Breaker, BreakerParagraph, Cursor, Exclusions, ExclusionsCheckpoint, Fitted, FloatRequest,
    LineBand, LineFloatId, LineHead, LineStart, Lines, PlacedFloat,
};
use crate::build::{Clear, FloatSide};
use crate::data::Id;
use crate::stages::analysis::{ClusterClass, ClusterId};
use crate::stages::content::{BreakClearanceId, FloatId, NodeKey};
use crate::stages::measure::InitialLetter;
use crate::unit::{InlineLayoutUnit, LayoutUnit};

/// Returns whether the line fitted as `fitted` from `start` reaches the float
/// anchored at `at`.
///
/// An anchor at the line's end counts, as Blink treats the float as a
/// trailing item, unless a forced break ends the paragraph there.
fn holds(para: &BreakerParagraph, start: ClusterId, fitted: &Fitted, at: ClusterId) -> bool {
    at >= start && (at < fitted.end || (at == fitted.end && !(at == para.end && para.forced)))
}

/// The float state a rejected trial rewinds to.
#[derive(Copy, Clone, Debug)]
pub(super) struct Placing {
    checkpoint: ExclusionsCheckpoint,
    kept: LineFloatId,
    next: FloatId,
}

impl Placing {
    /// Returns the current state, `next` being the first float not yet placed.
    pub(super) fn new(exclusions: &dyn Exclusions, out: &Lines, next: FloatId) -> Self {
        Self {
            checkpoint: exclusions.checkpoint(),
            kept: out.float_end(),
            next,
        }
    }

    /// Returns whether a float has been recorded since this state.
    pub(super) fn placed_since(&self, out: &Lines) -> bool {
        out.float_end() != self.kept
    }

    /// Takes back every float placed since this state (Blink's `RewindFloats`).
    pub(super) fn rewind(
        self,
        exclusions: &mut dyn Exclusions,
        out: &mut Lines,
        next: &mut FloatId,
    ) {
        exclusions.rewind(self.checkpoint);
        out.truncate_floats(self.kept);
        *next = self.next;
    }
}

/// A content float with its anchor and margin-box sizes.
#[derive(Copy, Clone, Debug)]
struct Anchor {
    id: FloatId,
    /// The cluster boundary of the anchor.
    at: ClusterId,
    key: NodeKey,
    side: FloatSide,
    inline: LayoutUnit,
    block: LayoutUnit,
}

impl Anchor {
    /// Returns the request that places the float from `block_start`.
    fn request(&self, block_start: LayoutUnit) -> FloatRequest {
        FloatRequest {
            key: self.key,
            side: self.side,
            inline_size: self.inline.to_px(),
            block_size: self.block.to_px(),
            block_start: block_start.to_px(),
        }
    }
}

impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Moves `cursor` below the floats cleared by a forced break that ends the
    /// line at `end`.
    ///
    /// The next line, or the block's end, starts where the band in `area` no
    /// longer narrows on a cleared side: a `<br>`'s clearance applies after
    /// its line. Clearance runs from the line box's bottom; annotations
    /// reaching further carry over only their reach past it
    /// (`AddAnyClearanceAfterLine`). Inlined, so content without clearing breaks
    /// makes no call.
    #[inline]
    pub(super) fn clear_floats(
        &mut self,
        out: &Lines,
        end: ClusterId,
        area: LineBand,
        cursor: &mut Cursor,
    ) {
        if !self.stages.content.clearances().is_empty() {
            self.clear_past_break(out, end, area, cursor);
        }
    }

    /// Does the work of [`clear_floats`](Self::clear_floats).
    #[inline(never)]
    fn clear_past_break(
        &mut self,
        out: &Lines,
        end: ClusterId,
        area: LineBand,
        cursor: &mut Cursor,
    ) {
        let clearances = self.stages.content.clearances();
        let Some(last) = end.get().checked_sub(1).map(ClusterId::new) else {
            return;
        };
        let items = &self.stages.analysis.item_clusters;
        let mut found = None;
        while let Some(clearance) = clearances.get(cursor.clearances) {
            let at = items.start(clearance.item);
            if at > last {
                break;
            }
            cursor.clearances = BreakClearanceId::new(cursor.clearances.get() + 1);
            if at == last {
                found = Some(clearance.clear);
            }
        }
        let Some(clear) = found else {
            return;
        };
        // A negative carry is the annotations' reach past the line box, which the
        // cursor already stands below.
        let reach = (-self.carry).max(LayoutUnit::ZERO);
        let line_end = cursor.top - reach;
        let top = self.clear_band(out, clear, area, line_end - cursor.trimmed);
        let clearance = top + cursor.trimmed - line_end;
        if clearance <= LayoutUnit::ZERO {
            self.clear_past_trimmed_end(out, clear, area, line_end - cursor.trimmed, cursor);
            return;
        }
        cursor.top = cursor.top.max(top + cursor.trimmed);
        cursor.cleared = Some(top);
        if reach > LayoutUnit::ZERO {
            self.carry = -(reach - clearance).max(LayoutUnit::ZERO);
        }
    }

    /// Keeps the block's end below the floats `clear` clears where
    /// `text-box-trim` pulls it up into them, `line_end` being the line's end.
    ///
    /// The clearance applies after the trim: a 100 px float beside a 100 px
    /// line trimmed to 80 at `alphabetic` leaves the block 100 px tall.
    #[cold]
    fn clear_past_trimmed_end(
        &mut self,
        out: &Lines,
        clear: Clear,
        area: LineBand,
        line_end: LayoutUnit,
        cursor: &mut Cursor,
    ) {
        let Some(trims) = self.trims.filter(|trims| trims.ends) else {
            return;
        };
        let Some((id, line)) = out.lines.last_id().zip(out.lines.last()) else {
            return;
        };
        let trimmed = trims.end(line.extent, out.scale(id), line.variant());
        if trimmed <= LayoutUnit::ZERO {
            return;
        }
        let start = line_end - trimmed;
        let top = self.clear_band(out, clear, area, start);
        if top > start {
            cursor.cleared = Some(top.min(line_end));
        }
    }

    /// Returns the first position from `top` down where the band in `area` no
    /// longer narrows on a side `clear` clears, within [`MAX_MOVES_DOWN`] moves.
    fn clear_band(
        &self,
        out: &Lines,
        clear: Clear,
        area: LineBand,
        mut top: LayoutUnit,
    ) -> LayoutUnit {
        for _ in 0..MAX_MOVES_DOWN {
            let band = self.band(out, top, LayoutUnit::EPSILON);
            let narrowed = (clear.clears(FloatSide::Left) && band.left > area.left)
                || (clear.clears(FloatSide::Right) && band.right < area.right);
            if !narrowed {
                break;
            }
            let Some(next) = moves_to(&*self.exclusions, top) else {
                break;
            };
            top = next;
        }
        top
    }

    /// Returns the content's float `id`, or `None` past the last.
    fn anchor(&self, id: FloatId) -> Option<Anchor> {
        let content = self.stages.content;
        let float = content.floats().get(id)?;
        let at = self.stages.analysis.item_clusters.get(float.item)?;
        let (nodes, node) = (&content.nodes, content.item_node(float.item));
        let (inline, block) = float.margin_box;
        Some(Anchor {
            id,
            at,
            key: nodes.key(node),
            side: float.side,
            inline,
            block,
        })
    }

    /// Places `anchor` from `block_start` and records where the host puts it.
    fn place(&mut self, out: &mut Lines, anchor: &Anchor, block_start: LayoutUnit) {
        let placed = self.exclusions.place(anchor.request(block_start));
        self.record(out, anchor.id, placed);
    }

    /// Records where the host put `float`, in placement order.
    fn record(&mut self, out: &mut Lines, float: FloatId, placed: PlacedFloat) {
        let left = LayoutUnit::from_px(placed.inline.left);
        out.push_float(float, left, LayoutUnit::from_px(placed.block.start));
    }

    /// Places the floats anchored at or before `start` at the line's `top`, fit
    /// or not, as Blink's `PositionLeadingFloats` does.
    ///
    /// Only a paragraph's first line has any; a float at a soft break belongs to
    /// the line before. They stay put when the line moves down.
    pub(super) fn place_leading(
        &mut self,
        out: &mut Lines,
        start: ClusterId,
        top: LayoutUnit,
        floats: &mut FloatId,
    ) {
        self.place_reached(out, top, floats, |anchor| anchor.at <= start);
    }

    /// Places the fitted line's floats beside it where each fits, refitting the
    /// line in the band each leaves.
    ///
    /// As in Blink's `HandleFloat` and `ShouldPushFloatAfterLine`, a float goes
    /// at the line's top where its margin box fits beside the text before its
    /// anchor (less a trailing collapsible space), and the host puts it no lower.
    /// The first that fails, and every later one, go below the line. A float the
    /// refitted line no longer reaches is taken back.
    pub(super) fn place_on_line(
        &mut self,
        out: &mut Lines,
        head: &mut LineHead,
        line: &LineTrial<'_>,
        floats: &mut FloatId,
        band: &mut LineBand,
        fitted: &mut Fitted,
    ) {
        while let Some(anchor) = self.anchor(*floats)
            && holds(line.para, line.start, fitted, anchor.at)
        {
            let (fixed, _) = self.line_start(line, *band);
            let (reach, space) = self.anchor_reach(&fixed, anchor.at);
            let size = anchor.inline;
            let room = band.width() + LayoutUnit::EPSILON;
            if reach + size > room && reach - space + size > room {
                return;
            }
            let before = Placing::new(self.exclusions, out, *floats);
            let placed = self.exclusions.place(anchor.request(line.top));
            if LayoutUnit::from_px(placed.block.start) > line.top {
                before.rewind(self.exclusions, out, floats);
                return;
            }
            self.record(out, anchor.id, placed);
            *band = line.area.meet(self.band(out, line.top, line.reserve));
            out.rewind_edges(head.after());
            *fitted = self.fit(out, line, *band, head);
            if !holds(line.para, line.start, fitted, anchor.at) {
                // The break fits the narrower band, so it fits once the float is gone.
                before.rewind(self.exclusions, out, floats);
                *band = line.area.meet(self.band(out, line.top, line.reserve));
                return;
            }
            *floats = FloatId::new(floats.get() + 1);
        }
    }

    /// Places the kept line's remaining floats from its line box's `bottom`, as
    /// Blink's `PlaceFloatingObjects` does.
    pub(super) fn place_after(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        fitted: &Fitted,
        bottom: LayoutUnit,
        floats: &mut FloatId,
    ) {
        self.place_reached(out, bottom, floats, |anchor| {
            holds(para, start, fitted, anchor.at)
        });
    }

    /// Places the floats no line reaches at `top`, after every line's floats.
    ///
    /// Their anchors follow a final forced break or lie in text with no line.
    pub(super) fn place_unreached(
        &mut self,
        out: &mut Lines,
        top: LayoutUnit,
        floats: &mut FloatId,
    ) {
        self.place_reached(out, top, floats, |_| true);
    }

    /// Places floats from `floats` on at `block_start` while `reached` accepts
    /// them, and moves `floats` past them.
    fn place_reached(
        &mut self,
        out: &mut Lines,
        block_start: LayoutUnit,
        floats: &mut FloatId,
        reached: impl Fn(&Anchor) -> bool,
    ) {
        while let Some(anchor) = self.anchor(*floats)
            && reached(&anchor)
        {
            self.place(out, &anchor, block_start);
            *floats = FloatId::new(floats.get() + 1);
        }
    }

    /// Returns how far the text before the anchor at `at` reaches on the line,
    /// and the width of a collapsible space just before it.
    ///
    /// The reach includes the indent and start cost, as Blink's `position_`
    /// does. Blink drops the space where the float does not fit with it
    /// (`TrailingCollapsibleSpaceWidth`).
    fn anchor_reach(&self, line: &LineStart<'_>, at: ClusterId) -> (LayoutUnit, LayoutUnit) {
        let (para, start) = (line.para, line.start);
        // A float ends a shaping run, so its anchor is never inside the window.
        let moved = line
            .window
            .filter(|window| window.end <= at)
            .map_or(InlineLayoutUnit::ZERO, |window| window.delta);
        let mut reach = self.stages.measured.prefix.get(at) - para.origin + moved;
        if para.tabs {
            reach += self.line_tabs(line, at, at).0;
        }
        let reach =
            (reach.ceil_to_grid() - line.fit_start).to_layout() + line.start_cost + line.indent;
        let space = at
            .get()
            .checked_sub(1)
            .map(ClusterId::new)
            .filter(|&last| {
                last >= start
                    && self.clusters.class(last) == Some(ClusterClass::Space)
                    && self.end_spaces(last..at) == EndSpaces::Collapse
            })
            .map_or(LayoutUnit::ZERO, |last| {
                self.stages.cluster_advance(last).to_layout()
            });
        (reach, space)
    }

    /// Places the initial letter's exclusion with the host once the first line
    /// is kept, the host's positions standing `trimmed` above the records.
    ///
    /// The margin box follows Blink's `CreateExclusionSpaceForInitialLetterBox`:
    /// down to the letter's ink, on the paragraph's start side, widened by a
    /// positive first-line `text-indent`. No line records it.
    pub(super) fn sink_initial_letter(
        &mut self,
        out: &mut Lines,
        letter: &InitialLetter,
        trimmed: LayoutUnit,
    ) {
        let Some(line) = out.lines.as_slice().first() else {
            return;
        };
        let Some(facts) = self.scratch.placements.as_slice().first() else {
            return;
        };
        // The first line's annotations shift the letter, and the exclusion's end,
        // down.
        let shift = out.letter_shift();
        let top = (letter.exclusion_top + shift).min(LayoutUnit::ZERO);
        let height = letter.exclusion_top + letter.exclusion_height + shift - top;
        let width = letter.exclusion_width + line.indent.max(LayoutUnit::ZERO);
        if height <= LayoutUnit::ZERO || width <= LayoutUnit::ZERO {
            return;
        }
        let request = FloatRequest {
            key: self.stages.content.nodes.key(letter.node),
            side: FloatSide::from_level(line.level(&self.stages.analysis.paragraphs)),
            inline_size: width.to_px(),
            block_size: height.to_px(),
            block_start: (facts.block_start - trimmed + top).to_px(),
        };
        self.exclusions.place(request);
    }
}
