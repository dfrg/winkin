//! Choosing a line's end in its band, as more of [`Breaker`]'s `impl`.
//!
//! - **The grid.** A line from `s` to `e` in paragraph `p` is
//!   `⌈P[e] − P[p]⌉ − ⌈P[s] − P[p]⌉` wide, rounded up to 1/64. It fits a
//!   band `W` wide when that is at most `W + 1/64` (`AvailableWidthToFit`).
//!   Reshaped pieces add their exact difference.
//! - **Search.** The furthest boundary that fits is the last whose prefix
//!   sum is under a threshold, found by gallop and bisection in O(log n). A
//!   paragraph with tabs or a prefix that goes back is walked a cluster at
//!   a time.
//! - **Opportunity.** From there the fit steps forward over what hangs, then
//!   back through the opportunities, measuring each until one fits. Failing
//!   that, it takes an `overflow-wrap` break, then the first opportunity,
//!   overflowing.

use super::{
    Breaker, EdgeShapeId, Fitted, Fitting, LineBand, LineFlags, LineHang, LineHead, LineStart,
    Lines, ReshapedPieces,
};

use super::candidate::{end_cost, step_segment};
use super::fit::{Candidate, Hold, LineTrial};
use crate::data::{Id, IdRange, SortedCursor};
use crate::stages::analysis::ClusterId;
use crate::stages::content::ContentFlags;
use crate::stages::measure::{LineEdgeCost, LineEdgeFlags};
use crate::stages::shape::ShapedFlags;
use crate::unit::{InlineLayoutUnit, LayoutUnit};

impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Returns where `line` starts in `band`, and the paragraph's line-edge
    /// costs sought to its start.
    #[inline(always)]
    pub(super) fn line_start<'p>(
        &self,
        line: &LineTrial<'p>,
        band: LineBand,
    ) -> (LineStart<'p>, Option<SortedCursor<'a, LineEdgeCost>>) {
        let LineTrial {
            para,
            start,
            area,
            first,
            ..
        } = *line;
        let indent = self.indent(para, start, first);
        let mut costs = self.edge_costs(para, start);
        let (start_cost, hang_start) = self.start_cost(&mut costs, start, first);
        // A balanced paragraph's lines fit a narrowed room but keep their
        // full bands.
        let band_width = (band.width() - self.hold.narrowing()).max(LayoutUnit::ZERO);
        let fixed = LineStart {
            para,
            start,
            first_item: line.first_item,
            window: line.window,
            room: band_width + LayoutUnit::EPSILON - indent,
            start_cost,
            hang_start,
            fit_start: self.stages.measured.prefix.fit_position(para.start, start),
            indent,
            tab_origin: if para.tabs {
                band.start_from(area, para.level)
            } else {
                LayoutUnit::ZERO
            },
        };
        (fixed, costs)
    }

    /// Chooses the line `line` starts, fitted to `band`, leaving its reshaped
    /// pieces in the edge tables after `head`.
    #[inline(always)]
    pub(super) fn fit(
        &mut self,
        out: &mut Lines,
        line: &LineTrial<'_>,
        band: LineBand,
        head: &mut LineHead,
    ) -> Fitted {
        if !line.para.breakable_ruby {
            return self.fit_plain(out, line, band, head);
        }
        self.fit_ruby(out, line, band, head)
    }

    /// Fits a line of a paragraph with breakable ruby, as
    /// [`fit_plain`](Self::fit_plain) does, and keeps its column pieces.
    ///
    /// Takes back what an earlier fit of the line kept first. It is out of
    /// line so the dispatch in [`fit`](Self::fit) stays small.
    #[inline(never)]
    fn fit_ruby(
        &mut self,
        out: &mut Lines,
        line: &LineTrial<'_>,
        band: LineBand,
        head: &mut LineHead,
    ) -> Fitted {
        let (para, start) = (line.para, line.start);
        let next = out.lines.next_id();
        let shapes = out
            .lines
            .last()
            .map_or(EdgeShapeId::new(0), |line| line.shapes.end);
        out.truncate_edge_shapes(shapes);
        if let Some(ruby) = &mut out.ruby {
            ruby.truncate(next);
        }
        self.continued = self.continued_column(out, para, start);
        let mut fitted = self.fit_plain(out, line, band, head);
        self.finish_ruby(out, para, start, &mut fitted);
        fitted
    }

    /// Fits the line `line` starts, splitting a ruby column where one may
    /// break.
    #[inline(never)]
    fn fit_plain(
        &mut self,
        out: &mut Lines,
        line: &LineTrial<'_>,
        band: LineBand,
        head: &mut LineHead,
    ) -> Fitted {
        let (fixed, costs) = self.line_start(line, band);
        let para = fixed.para;
        let start = fixed.start;
        let shift = if para.breakable_ruby {
            self.continued
                .as_ref()
                .map_or_else(Default::default, |continued| continued.shift())
        } else {
            Default::default()
        };
        let mut fitting = Fitting {
            line: fixed,
            costs,
            head,
            shift,
        };
        let window = fixed.window;

        // A planned end is kept where the line fits there.
        if let Hold::End(end) = self.hold
            && end > start
            && end <= para.end
        {
            let planned = self.candidate(out, &mut fitting, end, false);
            if let Some(fitted) = settle(&fitting, &planned) {
                return fitted;
            }
        }

        // The rest of the paragraph, where it fits. A walked paragraph may
        // overflow on the way, so it skips this.
        if !para.walks() {
            if self.end_may_fit(&fitting) {
                let whole = self.candidate(out, &mut fitting, para.end, false);
                if let Some(fitted) = settle(&fitting, &whole) {
                    return fitted;
                }
            } else {
                out.rewind_edges(fitting.head.after());
            }
        }

        // The threshold is the line's room from its start, less what the
        // start's reshape adds.
        let delta = window.map_or(InlineLayoutUnit::ZERO, |window| window.delta);
        let threshold = fitting.line.fit_start
            + InlineLayoutUnit::from_layout(fitting.line.room - fitting.line.start_cost)
            + para.origin
            - delta;
        let (furthest, probes) = if para.walks() {
            self.walk(&fitting, para.end, threshold)
        } else {
            self.search(&fitting, start, para.end, threshold)
        };
        #[cfg(test)]
        {
            out.probes += probes;
        }
        #[cfg(not(test))]
        let _ = probes;

        if para.breakable_ruby
            && let Some(fitted) = self.split_ruby(out, &mut fitting, furthest)
        {
            return fitted;
        }
        let mut hung = furthest;
        while hung < para.end && self.clusters.hangs(hung) {
            hung = ClusterId::new(hung.get() + 1);
        }
        // A closing mark that does not fit whole may fit trimmed, where a
        // line may break after it.
        let trims = self
            .stages
            .content
            .flags
            .contains(ContentFlags::TRIMS_PUNCTUATION);
        if trims && hung == furthest && self.trims_end(para, furthest) {
            let end = ClusterId::new(furthest.get() + 1);
            let candidate = self.candidate(out, &mut fitting, end, true);
            if let Some(fitted) = settle(&fitting, &candidate) {
                return fitted;
            }
        }
        // A mark hanging past the end gives its advance back, so the next
        // opportunity may still fit.
        if para.costs || para.seams {
            let end = self
                .opportunity(hung..para.end, false, false)
                .unwrap_or(para.end);
            // So does a seam's room, as Blink's `AdjustOffsetForAutoSpacing`
            // counts it.
            let flex =
                para.costs && end_cost(para, &mut fitting.costs, end).flex > LayoutUnit::ZERO;
            let seam = para.seams && self.seam_before(para, end) > InlineLayoutUnit::ZERO;
            if end > hung && (flex || seam) {
                let candidate = self.candidate(out, &mut fitting, end, false);
                if let Some(fitted) = settle(&fitting, &candidate) {
                    return fitted;
                }
            }
        }
        let mut next = if hung == para.end && para.walks() {
            Some(para.end)
        } else {
            self.opportunity(start..hung, true, false)
        };
        while let Some(end) = next {
            let candidate = self.candidate(out, &mut fitting, end, false);
            if let Some(fitted) = settle(&fitting, &candidate) {
                return fitted;
            }
            next = self.opportunity(start..ClusterId::new(end.get() - 1), true, false);
        }

        // Nothing fits: take the last `overflow-wrap` break that fits before
        // the first opportunity, or else the earliest one, overflowing.
        let opportunity = self.opportunity(start..para.end, false, false);
        let first_break = opportunity.unwrap_or(para.end);
        let before = ClusterId::new(first_break.get() - 1);
        let mut next = self.opportunity(start..furthest.min(before), true, true);
        let mut earliest = None;
        while let Some(end) = next {
            let candidate = self.candidate(out, &mut fitting, end, false);
            if let Some(mut fitted) = settle(&fitting, &candidate) {
                fitted.flags.insert(LineFlags::EMERGENCY);
                return fitted;
            }
            earliest = Some(end);
            next = self.opportunity(start..ClusterId::new(end.get() - 1), true, true);
        }
        if let Some(end) = earliest.or_else(|| self.opportunity(start..before, false, true)) {
            let candidate = self.candidate(out, &mut fitting, end, false);
            let mut fitted = overflowing(&fitting, &candidate);
            fitted.flags.insert(LineFlags::EMERGENCY);
            return fitted;
        }

        if !para.monotone
            && !para.tabs
            && let Some(first) = opportunity
            && let Some(fitted) = self.later_fit(out, &mut fitting, first)
        {
            return fitted;
        }

        // Otherwise take the first opportunity, overflowing, with any spaces
        // that alone follow it. Its reshaped edges may still make it fit.
        let end = match opportunity {
            Some(end) if self.clusters.only_spaces(end..para.end) => para.end,
            Some(end) => end,
            None => para.end,
        };
        let candidate = self.candidate(out, &mut fitting, end, false);
        if end == para.end {
            return overflowing(&fitting, &candidate);
        }
        settle(&fitting, &candidate).unwrap_or_else(|| overflowing(&fitting, &candidate))
    }

    /// Returns the line `fitting` starts, ended at the last opportunity after
    /// `first` that fits, where nothing up to `first` fits.
    ///
    /// Only a prefix that goes back, as under a negative inline margin, can
    /// fit again past an overflow. The breaker goes on past such an
    /// overflow and ends the line before its next one (WPT
    /// `inline-negative-margin-001`). `None` where no later opportunity fits.
    #[cold]
    #[inline(never)]
    fn later_fit(
        &mut self,
        out: &mut Lines,
        fitting: &mut Fitting<'_, '_>,
        first: ClusterId,
    ) -> Option<Fitted> {
        let end = fitting.line.para.end;
        let mut fits = None;
        let mut at = first;
        while at < end {
            let next = ClusterId::new(at.get() + 1);
            let later = self.opportunity(next..end, false, false).unwrap_or(end);
            let candidate = self.candidate(out, fitting, later, false);
            match settle(fitting, &candidate) {
                Some(_) => fits = Some(later),
                None if fits.is_some() => break,
                None => {}
            }
            at = later;
        }
        // Each candidate replaces the last one's pieces, so the kept one is
        // measured again.
        let candidate = self.candidate(out, fitting, fits?, false);
        settle(fitting, &candidate)
    }

    /// Returns whether the line `fitting` starts may fit to its paragraph's
    /// end, weighed from the prefix without measuring a candidate.
    ///
    /// Returns `true` where the prefix alone cannot tell: a start window,
    /// seams, tabs, a start inside the hanging tail, or a reshaped end.
    #[inline]
    fn end_may_fit(&self, fitting: &Fitting<'_, '_>) -> bool {
        let line = &fitting.line;
        let para = line.para;
        let (content_end, tail) = para.end_tail;
        if line.window.is_some() || para.seams || para.tabs || line.start > content_end {
            return true;
        }
        if self.shaped.contains(ShapedFlags::HAS_UNSAFE)
            && self.removed_end(line, para.end).is_some()
        {
            return true;
        }
        let reach =
            self.stages.measured.prefix.get(para.end) - para.origin + fitting.shift.at(para.end);
        let content = ((reach - tail).ceil_to_grid() - line.fit_start).to_layout();
        let end = Candidate {
            end: para.end,
            content_end,
            content,
            full: content,
            conditional: None,
            cost: para.end_cost,
            pieces: ReshapedPieces::default(),
        };
        weigh(fitting, &end).0
    }

    /// Returns the last boundary from `start` to `end` whose prefix sum is at
    /// most `threshold`, and how many positions it compared.
    ///
    /// The prefix must not decrease. Returns `start` where nothing past it
    /// fits.
    fn search(
        &self,
        fitting: &Fitting<'_, '_>,
        start: ClusterId,
        end: ClusterId,
        threshold: InlineLayoutUnit,
    ) -> (ClusterId, usize) {
        let prefix = &self.stages.measured.prefix;
        let mut probes = 0;
        let furthest = gallop_last(start.get(), end.get(), |at| {
            probes += 1;
            let at = ClusterId::new(at);
            prefix.get(at) + fitting.shift.at(at) <= threshold
        });
        (ClusterId::new(furthest), probes)
    }

    /// Does what [`search`](Self::search) does, one boundary at a time,
    /// stopping at the first that does not fit.
    ///
    /// It serves a prefix that may go back and a paragraph with tabs. It
    /// records each tab's width in the scratch for the candidates.
    #[inline(never)]
    fn walk(
        &mut self,
        fitting: &Fitting<'_, '_>,
        end: ClusterId,
        threshold: InlineLayoutUnit,
    ) -> (ClusterId, usize) {
        let line = &fitting.line;
        let prefix = &self.stages.measured.prefix;
        let mut probes = 0;
        let mut at = line.start;
        let mut tabs = InlineLayoutUnit::ZERO;
        if line.para.tabs {
            self.scratch.tabs.begin(line);
        }
        let mut boundary = None;
        // The furthest boundary that fits, and where the walk stopped.
        let (furthest, stopped) = 'walk: {
            for step in self.line_segments(line, line.start..end) {
                let Some(segment) = step_segment(&mut boundary, step) else {
                    continue;
                };
                for cluster in (segment.start..segment.end).ids() {
                    probes += 1;
                    let next = ClusterId::new(cluster.get() + 1);
                    if line.para.tabs && self.clusters.is_tab(cluster) {
                        let width = self.tab_width(line, cluster, tabs, &segment, boundary);
                        self.scratch.tabs.push(cluster, width);
                        tabs += width;
                    }
                    if prefix.get(next) + fitting.shift.at(next) + tabs > threshold {
                        break 'walk (cluster, next);
                    }
                    at = next;
                }
            }
            (at, at)
        };
        if line.para.tabs {
            self.scratch.tabs.stop(stopped);
        }
        (furthest, probes)
    }
}

/// Returns the last position from `low` to `end` that `fits`, galloping
/// from `low` and then bisecting.
///
/// `fits` holds up to some position and for none after it. `low` is taken
/// to fit, and is returned where nothing past it does.
pub(super) fn gallop_last(
    mut low: usize,
    end: usize,
    mut fits: impl FnMut(usize) -> bool,
) -> usize {
    let mut step = 1usize;
    // Gallop: `low` fits, `high` does not.
    let mut high = loop {
        let probe = low.saturating_add(step).min(end);
        if !fits(probe) {
            break probe;
        }
        if probe == end {
            return end;
        }
        low = probe;
        step = step.saturating_mul(2);
    };
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if fits(middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

/// Returns whether a candidate fits with its costs, its width, and whether
/// it takes the end cost's flex back to fit.
fn weigh(fitting: &Fitting<'_, '_>, candidate: &Candidate) -> (bool, LayoutUnit, bool) {
    let cost = candidate.cost;
    let content = candidate.content + fitting.line.start_cost;
    let width = content + cost.end;
    let flex = cost.flex;
    // A line of one mark hanging at its start hangs it once.
    let hangs_twice = fitting.line.hang_start > LayoutUnit::ZERO
        && candidate.content_end.get() == fitting.line.start.get() + 1;
    let forced = cost.flags.contains(LineEdgeFlags::HANG_FORCED);
    if flex > LayoutUnit::ZERO && forced && !hangs_twice {
        let taken = content + cost.charged_end();
        return (taken <= fitting.line.room, taken, true);
    }
    if width <= fitting.line.room {
        return (true, width, false);
    }
    if flex > LayoutUnit::ZERO && !hangs_twice {
        let taken = width - flex;
        if taken <= fitting.line.room {
            return (true, taken, true);
        }
    }
    (false, width, false)
}

/// Returns the candidate as the line, where it fits.
#[inline]
pub(super) fn settle(fitting: &Fitting<'_, '_>, candidate: &Candidate) -> Option<Fitted> {
    let (fits, width, flex) = weigh(fitting, candidate);
    fits.then(|| Fitted::new(fitting, candidate, width, flex, false))
}

/// Returns the candidate as the line, whether it fits or not.
pub(super) fn overflowing(fitting: &Fitting<'_, '_>, candidate: &Candidate) -> Fitted {
    let (fits, width, flex) = weigh(fitting, candidate);
    Fitted::new(fitting, candidate, width, flex, !fits)
}

impl Fitted {
    /// Returns the line `candidate` makes, `width` wide with its costs.
    ///
    /// `flex` says the end cost's flex is taken back, and `overflows` that
    /// the line overflows its room.
    fn new(
        fitting: &Fitting<'_, '_>,
        candidate: &Candidate,
        width: LayoutUnit,
        flex: bool,
        overflows: bool,
    ) -> Self {
        let mut flags = LineFlags::NONE;
        if overflows {
            flags.insert(LineFlags::OVERFLOWS);
        }
        if candidate.cost.flags.contains(LineEdgeFlags::HYPHEN) {
            flags.insert(LineFlags::HYPHENATED);
        }
        let space = (candidate.full - candidate.content).max(LayoutUnit::ZERO);
        // Only a candidate at its paragraph's end has white space that hangs
        // only where it overflows.
        let unconditional = match candidate.conditional {
            Some(before) if space > LayoutUnit::ZERO => {
                flags.insert(LineFlags::CONDITIONAL_HANG);
                before.ceil_to_grid().to_layout().min(space)
            }
            _ => LayoutUnit::ZERO,
        };
        Fitted {
            end: candidate.end,
            content_end: candidate.content_end,
            indent: fitting.line.indent,
            width,
            hang: LineHang {
                space,
                unconditional,
                start: fitting.line.hang_start,
                end: if flex {
                    candidate.cost.flex
                } else {
                    LayoutUnit::ZERO
                },
            },
            flags,
            pieces: candidate.pieces,
        }
    }
}
