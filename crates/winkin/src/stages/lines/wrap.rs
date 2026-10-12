//! `text-wrap-style: balance` and `pretty`: a driver over the greedy fit, a
//! paragraph at a time.
//!
//! **Greedy first.** Each paragraph is laid out greedily, with floats, the
//! initial letter, the first line's variant and a clamp. Those lines stay
//! unless something better is found, so a fine paragraph costs one layout.
//! Otherwise the lines are taken back, with the host's floats, and laid out
//! again. A paragraph is the lines between forced breaks, balanced
//! separately as CSS Text 4 says. Chrome balances only a block's first
//! paragraph, which is not copied.
//!
//! **`balance`**, as Chrome does it:
//! - Where the greedy lines are six or fewer and their rooms don't depend on
//!   where they break, the scorer evens their slack over the same count.
//!   That needs no float anchored in them, no first-line variant, and a
//!   searched prefix.
//! - Otherwise, or where the scorer finds nothing, the room is halved down
//!   to the narrowest that holds the paragraph in as many lines. It starts
//!   from four fifths of the lines' average and stops at a pixel, as in
//!   Chrome's `ParagraphLineBreaker`.
//! - Beyond Chrome, halving has no line limit and runs beside floats. Every
//!   band is narrowed alike, so each line leaves even room, as CSS Text 4's
//!   "remaining space" says. The initial letter's lines are scored too.
//! - Each trial is a whole greedy layout, taken back. A pixel's precision
//!   takes at most 25 trials.
//!
//! **`pretty`**: the scorer weighs a paragraph's last lines where the last
//! one ends short. Chrome's rule weighs four lines. It calls a line short
//! when it is one word under a third of its room, or the two lines before it
//! are hyphenated. `Pretty::Even` weighs six, and also counts any last line
//! under a fifth. The count of lines stays as the greedy fit made it.
//!
//! **A plan is followed while it fits.** A line the scorer ended is fitted
//! to that end and measured like any candidate. If it no longer fits, the
//! rest of the paragraph is fitted greedily. That happens when a float lands
//! elsewhere, a taller line meets another band, or a reshaped edge is wider.

use core::ops::Range;

use super::annotate::Carry;
use super::candidate::end_cost;
use super::fit::{ParagraphLines, ParagraphPlan, Resume};
use super::score::{BALANCE_LINES, BreakPoint, LineChoice, PlannedEnds, ScoreRule, WrapLineId};
use super::{
    Breaker, BreakerParagraph, Cursor, Exclusions, ExclusionsCheckpoint, LineFlags, LineId,
    LineRecord, Lines, LinesMark,
};
use crate::config::Pretty;
use crate::data::{Id, SortedCursor};
use crate::stages::analysis::ClusterId;
use crate::stages::measure::{LineEdgeCost, LineEdgeFlags};
use crate::style::TextAlign;
use crate::unit::LayoutUnit;
use crate::work;

/// Where a line of a paragraph started, which a layout from that line
/// rewinds to.
///
/// It records the host's floats, the lines' tables, the cursor and the
/// carry.
#[derive(Copy, Clone, Debug)]
pub(super) struct LayoutMark {
    from: Resume,
    cursor: Cursor,
    stage: LinesMark,
    placing: ExclusionsCheckpoint,
    carry: Carry,
}

impl LayoutMark {
    /// Returns where the lines stand now, before the paragraph's line
    /// `from`.
    pub(super) fn new(
        from: Resume,
        carry: Carry,
        cursor: &Cursor,
        out: &Lines,
        exclusions: &dyn Exclusions,
    ) -> Self {
        Self {
            from,
            cursor: *cursor,
            stage: out.mark(),
            placing: exclusions.checkpoint(),
            carry,
        }
    }

    /// Takes back every line laid out since, with everything it placed,
    /// reshaped and settled, as Chrome's `RewindFloats` does for floats.
    fn rewind(&self, cursor: &mut Cursor, out: &mut Lines, exclusions: &mut dyn Exclusions) {
        exclusions.rewind(self.placing);
        out.rewind(self.stage);
        *cursor = self.cursor;
    }
}

/// How many of a paragraph's last lines [`RecentMarks`] keeps: the most the
/// scorer weighs.
const RECENT: usize = BALANCE_LINES;

/// The marks of a paragraph's last [`RECENT`] greedy lines, on the stack.
///
/// A replan rewinds to the first changed line rather than the paragraph's
/// start. So a long paragraph whose last lines `pretty` changes is laid out
/// again only from them.
pub(super) struct RecentMarks {
    marks: [Option<LayoutMark>; RECENT],
}

impl RecentMarks {
    /// Returns no marks.
    fn new() -> Self {
        Self {
            marks: [None; RECENT],
        }
    }

    /// Keeps `mark`, in place of the mark [`RECENT`] lines before it.
    pub(super) fn record(&mut self, mark: LayoutMark) {
        if let Some(slot) = self.marks.get_mut(mark.from.line.get() % RECENT) {
            *slot = Some(mark);
        }
    }

    /// Returns the mark of the paragraph's `line`, where it is kept.
    fn get(&self, line: WrapLineId) -> Option<&LayoutMark> {
        self.marks
            .get(line.get() % RECENT)?
            .as_ref()
            .filter(|mark| mark.from.line == line)
    }
}

/// What the driver does with a paragraph once its greedy lines are laid
/// out.
#[derive(Copy, Clone, Debug)]
enum WrapDecision {
    /// Keep them.
    Keep,
    /// Lay them out again to the scorer's ends, `plan`, from `from`, the
    /// first line that differs.
    Ends { from: WrapLineId, plan: PlannedEnds },
    /// Keep them, since they end where the scorer's ends, `plan`, do.
    Scored(PlannedEnds),
    /// Balance them by halving the room.
    Halve(Halving),
}

/// What halving holds a paragraph to: as many lines as the greedy ones,
/// reaching as far, in a room of at least `lower`.
#[derive(Copy, Clone, Debug)]
struct Halving {
    lower: LayoutUnit,
    lines: usize,
    end: ClusterId,
}

/// Returns what to do with the greedy `lines` under `plan`: lay them out
/// again from the first line it ends elsewhere, or keep them.
fn ends_differ(lines: &[LineRecord], plan: PlannedEnds) -> WrapDecision {
    let differs = plan.iter().find(|&(line, end)| {
        lines
            .get(line.get())
            .is_none_or(|greedy| greedy.clusters().end != end)
    });
    match differs {
        Some((from, _)) => WrapDecision::Ends { from, plan },
        None => WrapDecision::Scored(plan),
    }
}

impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Lays out a paragraph's lines balanced or scored, as
    /// `text-wrap-style` asks, from where `cursor` stands.
    pub(super) fn wrapped_paragraph<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        lines: &ParagraphLines<'_>,
        cursor: &mut Cursor,
    ) {
        let from = Resume::new(lines.para.start, WrapLineId::new(0));
        let mark = LayoutMark::new(from, self.carry, cursor, out, self.exclusions);
        let mut recent = RecentMarks::new();
        let recording = Some(&mut recent);
        self.paragraph_lines::<FLOATS>(out, lines, ParagraphPlan::Greedy, from, cursor, recording);
        let greedy = mark.stage.lines..out.lines.next_id();
        let first = greedy.start;
        let decision = match self.choice {
            LineChoice::Greedy => WrapDecision::Keep,
            LineChoice::Balance => self.plan_balance(out, lines, greedy),
            LineChoice::Pretty(rule) => self.plan_pretty(out, lines.para, greedy, rule),
        };
        match decision {
            WrapDecision::Keep => {}
            WrapDecision::Scored(plan) => {
                // The lines end where the scorer put their ends, as if laid
                // out to them.
                for (line, _) in plan.iter() {
                    let id = LineId::new(first.get() + line.get());
                    if let Some(record) = out.lines.get_mut(id) {
                        record.flags.insert(LineFlags::SCORED);
                    }
                }
            }
            WrapDecision::Ends { from, plan } => {
                // Back to the first line the plan changes, where its mark is
                // kept, or else to the paragraph's start.
                let mark = *recent.get(from).unwrap_or(&mark);
                self.take_back(out, &mark, cursor);
                let (planned, from) = (ParagraphPlan::Planned(plan), mark.from);
                self.paragraph_lines::<FLOATS>(out, lines, planned, from, cursor, None);
            }
            WrapDecision::Halve(halving) => {
                self.halve::<FLOATS>(out, lines, &mark, halving, cursor)
            }
        }
    }

    /// Takes back what was laid out since `mark`, and the carry with it.
    fn take_back(&mut self, out: &mut Lines, mark: &LayoutMark, cursor: &mut Cursor) {
        mark.rewind(cursor, out, self.exclusions);
        self.scratch.placements.truncate(out.lines.next_id());
        self.carry = mark.carry;
    }

    /// Returns how to balance a paragraph whose greedy lines are `greedy`.
    fn plan_balance(
        &mut self,
        out: &mut Lines,
        lines: &ParagraphLines<'_>,
        greedy: Range<LineId>,
    ) -> WrapDecision {
        let greedy_lines = out.lines.slice(greedy.clone());
        let (Some(first), Some(last)) = (greedy_lines.first(), greedy_lines.last()) else {
            return WrapDecision::Keep;
        };
        // One line has nothing to even, and an overflowing line leaves
        // nothing to even against. Chrome declines both, and a line ending at
        // an emergency break, which its breaker takes only after overflowing.
        if greedy_lines.len() <= 1
            || greedy_lines.iter().any(|line| line.flags.overflowed())
            || lines.area.width() <= LayoutUnit::ZERO
        {
            return WrapDecision::Keep;
        }
        let scorable = greedy_lines.len() <= BALANCE_LINES
            && !lines.para.walks()
            && !first.flags.contains(LineFlags::FIRST_LINE)
            && !self.holds_floats(out, greedy_lines, lines.para.start);
        let rule = scorable.then(|| self.score_rule(LineChoice::Balance, last));
        // Chrome's bisection starts from four fifths of the lines' average,
        // their indents included (`LineWidthSum`).
        let sum = greedy_lines
            .iter()
            .fold(LayoutUnit::ZERO, |sum, line| sum + line.width + line.indent);
        let average = sum.divided(i32::try_from(greedy_lines.len()).unwrap_or(i32::MAX));
        let halving = Halving {
            lower: LayoutUnit::from_px(average.to_px() * 0.8),
            lines: greedy_lines.len(),
            end: last.clusters().end,
        };
        if let Some(rule) = rule
            && let Some(plan) = self.score(out, lines.para, greedy.clone(), greedy.start, &rule)
        {
            return ends_differ(out.lines.slice(greedy), plan);
        }
        WrapDecision::Halve(halving)
    }

    /// Returns how to lay out `para`, whose greedy lines are `greedy`, under
    /// `pretty` by `rule`.
    fn plan_pretty(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        greedy: Range<LineId>,
        rule: Pretty,
    ) -> WrapDecision {
        let count = greedy.end.get().saturating_sub(greedy.start.get());
        if count <= 1 || para.walks() {
            return WrapDecision::Keep;
        }
        // The scored lines are the last ones. A first line in its own variant
        // stays as it is, since the standard prefix does not measure it.
        let choice = LineChoice::Pretty(rule);
        let mut from = LineId::new(greedy.end.get() - count.min(choice.scored()));
        if out
            .lines
            .get(from)
            .is_some_and(|line| line.flags.contains(LineFlags::FIRST_LINE))
        {
            from = LineId::new(from.get() + 1);
        }
        let scored = out.lines.slice(from..greedy.end);
        let (Some(first), Some(last)) = (scored.first(), scored.last()) else {
            return WrapDecision::Keep;
        };
        let Some(facts) = self
            .scratch
            .placements
            .get(LineId::new(greedy.end.get() - 1))
        else {
            return WrapDecision::Keep;
        };
        if scored.len() <= 1
            || scored.iter().any(|line| line.flags.overflowed())
            || !self.ends_short(scored, facts.content_end, rule)
            || self.holds_floats(out, scored, first.clusters().start)
        {
            return WrapDecision::Keep;
        }
        let score_rule = self.score_rule(choice, last);
        self.score(out, para, greedy.clone(), from, &score_rule)
            .map_or(WrapDecision::Keep, |plan| {
                ends_differ(out.lines.slice(greedy), plan)
            })
    }

    /// Returns whether the last of `scored` ends short by `rule`.
    ///
    /// Chrome's `ShouldOptimize` calls it short when it is one word under a
    /// third of its room, or the two lines before it are hyphenated.
    /// `Pretty::Even` also counts any last line under a fifth of its room.
    fn ends_short(&self, scored: &[LineRecord], content_end: ClusterId, rule: Pretty) -> bool {
        let Some(last) = scored.last() else {
            return false;
        };
        let width = last.width + last.indent;
        let room = last.band.width();
        if width < room.divided(3) && !self.breaks_inside(last, content_end) {
            return true;
        }
        let hyphenated = |back: usize| {
            scored
                .len()
                .checked_sub(back)
                .and_then(|at| scored.get(at))
                .is_some_and(|line| line.flags.contains(LineFlags::HYPHENATED))
        };
        if scored.len() >= 3 && hyphenated(2) && hyphenated(3) {
            return true;
        }
        rule == Pretty::Even && width < room.divided(5)
    }

    /// Returns whether `line` may break inside its content, as Chrome's
    /// `CanBreakInside`.
    fn breaks_inside(&self, line: &LineRecord, content_end: ClusterId) -> bool {
        let start = line.clusters().start;
        let last = ClusterId::new(content_end.get().saturating_sub(1));
        start < last && self.opportunity(start..last, false, false).is_some()
    }

    /// Returns whether any of `lines` holds a float anchored after `from`.
    ///
    /// Such a float is placed where the lines break, so its band depends on
    /// them. Floats at the paragraph's start are placed before its first
    /// line.
    fn holds_floats(&self, out: &Lines, lines: &[LineRecord], from: ClusterId) -> bool {
        let stages = &self.stages;
        let (floats, item_clusters) = (stages.content.floats(), &stages.analysis.item_clusters);
        lines.iter().any(|line| {
            out.floats.slice(line.floats.clone()).iter().any(|placed| {
                work::step();
                floats
                    .get(placed.float)
                    .and_then(|float| item_clusters.get(float.item))
                    .is_some_and(|at| at > from)
            })
        })
    }

    /// Returns what the scorer weighs lines by under `choice`.
    ///
    /// The last greedy line's band is the room a line has, as Chrome's
    /// `LineWidths::Default`, at the block's own font size.
    fn score_rule(&self, choice: LineChoice, last: &LineRecord) -> ScoreRule {
        let justified = self.text_align == TextAlign::Justify;
        let size = self.stages.block_size();
        ScoreRule::new(choice, justified, last.band.width(), size)
    }

    /// Weighs `para`'s lines from `from` to the end of `greedy` by `rule`,
    /// and plans their ends.
    ///
    /// Returns `None` where the scorer finds nothing to choose.
    fn score(
        &mut self,
        out: &Lines,
        para: &BreakerParagraph,
        greedy: Range<LineId>,
        from: LineId,
        rule: &ScoreRule,
    ) -> Option<PlannedEnds> {
        let scored = out.lines.slice(from..greedy.end);
        let (first, last) = (scored.first()?, scored.last()?);
        let (start, end) = (first.clusters().start, last.clusters().end);
        // The costs move forward with the break points, from the scored
        // lines' start.
        let mut costs = self.edge_costs(para, start);
        let scoring = &mut self.scratch.scoring;
        scoring.begin(WrapLineId::new(from.get() - greedy.start.get()));
        for line in scored {
            work::step();
            if !scoring.push_room(line.band.width() - line.indent) {
                return None;
            }
        }
        // The scored lines' start, every opportunity inside them, and their
        // end. A `hanging-punctuation: first` mark hangs only from the
        // block's first line.
        let opening = BreakPoint::new(
            start,
            self.start_position(para, &mut costs, start, from == LineId::new(0)),
            LayoutUnit::ZERO,
            false,
            rule.hyphen,
        );
        if !self.scratch.scoring.push(opening) {
            return None;
        }
        let before_end = ClusterId::new(end.get().saturating_sub(1));
        let mut next = start;
        while let Some(opportunity) = self.opportunity(next..before_end, false, false) {
            work::step();
            // Hanging white space after an opportunity belongs to the line
            // before, as in the greedy fit and Blink's `AppendTrailingSpaces`.
            // Only an atomic inline's opportunity has any. Measured from the
            // space, the next line would score fuller than it is.
            let mut at = opportunity;
            while at < end && self.clusters.hangs(at) {
                work::step();
                at = ClusterId::new(at.get() + 1);
            }
            if at >= end {
                break;
            }
            let (reach, hyphenated) = self.line_end(para, &mut costs, start, at);
            let point = BreakPoint::new(
                at,
                self.start_position(para, &mut costs, at, false),
                reach,
                hyphenated,
                rule.hyphen,
            );
            if !self.scratch.scoring.push(point) {
                return None;
            }
            next = at;
        }
        let (reach, hyphenated) = self.line_end(para, &mut costs, start, end);
        let closing = BreakPoint::new(end, LayoutUnit::ZERO, reach, hyphenated, rule.hyphen);
        if !self.scratch.scoring.push(closing) {
            return None;
        }
        self.scratch.scoring.best_ends(rule)
    }

    /// Returns where a line starting at `start` starts on the grid, from the
    /// paragraph's start, less its start cost.
    ///
    /// `first` marks the block's first line. `costs` moves to `start`.
    fn start_position(
        &self,
        para: &BreakerParagraph,
        costs: &mut Option<SortedCursor<'_, LineEdgeCost>>,
        start: ClusterId,
        first: bool,
    ) -> LayoutUnit {
        let position = self.stages.measured.prefix.fit_position(para.start, start);
        position.to_layout() - self.start_cost(costs, start, first).0
    }

    /// Returns where a line ending at `end` ends on the grid, from the
    /// paragraph's start, and whether it shows a hyphen.
    ///
    /// It measures by the prefix as the fit does, back to `floor` at most.
    /// It takes off what hangs and a seam's room, and adds the end cost less
    /// a `force-end` hang. Reshaped edges are measured only when the line is
    /// fitted. `costs` moves to `end`.
    fn line_end(
        &self,
        para: &BreakerParagraph,
        costs: &mut Option<SortedCursor<'_, LineEdgeCost>>,
        floor: ClusterId,
        end: ClusterId,
    ) -> (LayoutUnit, bool) {
        let (_, tail) = self.hanging_tail(floor, end, |_| None);
        let reach =
            self.stages.measured.prefix.get(end) - para.origin - self.seam_before(para, end) - tail;
        let cost = end_cost(para, costs, end);
        (
            reach.ceil_to_grid().to_layout() + cost.charged_end(),
            cost.flags.contains(LineEdgeFlags::HYPHEN),
        )
    }

    /// Balances a paragraph by halving its room to the narrowest that holds
    /// it as `halving` says, to a pixel.
    ///
    /// Each trial rewinds to `mark`, as in Chrome's
    /// `ParagraphLineBreaker::BisectAvailableWidth`. The final layout narrows
    /// each line's own band by as much.
    fn halve<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        lines: &ParagraphLines<'_>,
        mark: &LayoutMark,
        halving: Halving,
        cursor: &mut Cursor,
    ) {
        let full = lines.area.width();
        // Halving past a pixel moves nothing worth the trial.
        let pixel = LayoutUnit::from_px(1.0);
        let (mut lower, mut upper) = (halving.lower, full);
        // Whether the last lines laid out are the found room's: the greedy
        // ones at first, then a trial that held.
        let mut found = true;
        while lower + pixel < upper {
            work::step();
            let middle = (upper + lower).half();
            self.take_back(out, mark, cursor);
            let plan = ParagraphPlan::Narrowed {
                by: full - middle,
                most: Some(halving.lines),
            };
            let tally = self.paragraph_lines::<FLOATS>(out, lines, plan, mark.from, cursor, None);
            found = tally.holds(halving.lines, halving.end);
            if found {
                upper = middle;
            } else {
                lower = middle;
            }
        }
        // A trial that held laid out every line with none overflowing, so its
        // lines are kept.
        if found {
            return;
        }
        self.take_back(out, mark, cursor);
        let plan = if upper < full {
            ParagraphPlan::Narrowed {
                by: full - upper,
                most: None,
            }
        } else {
            ParagraphPlan::Greedy
        };
        self.paragraph_lines::<FLOATS>(out, lines, plan, mark.from, cursor, None);
    }
}
