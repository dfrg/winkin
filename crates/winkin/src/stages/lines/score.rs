//! The scorer of `text-wrap-style: balance` and `pretty`, as Blink's
//! `ScoreLineBreaker` after Minikin's `OptimalLineBreaker`.
//!
//! It chooses the ends of a paragraph's scored lines. Each line scores
//! the room it leaves squared, plus what its end costs.
//!
//! Every place a line may end, a [`BreakPoint`], is measured once from
//! the paragraph's prefix. A line between two points is then one
//! subtraction. The best way to each point is the best way to an
//! earlier one plus the line between. The scored lines are few: six for
//! `balance` and four for `pretty`, as in Chrome, and six for `Pretty::Even`
//! (beyond Chrome). So the pass costs what Chrome's does.
//!
//! The rules differ in what the last line costs:
//! - Under `balance` its slack counts like any line's, so the lines come out
//!   as even as the count allows.
//! - Under `pretty` its slack counts nothing. Only a line that leaves the
//!   last word alone pays, Blink's orphan penalty.
//! - `Pretty::Even` (beyond Chrome) also makes a last line under a fifth of
//!   its room cost more than any other choice. The lines before then give it
//!   the words it needs.

use crate::config::Pretty;
use crate::data::IdRange;
use crate::data::{Id, Table, define_id, heap_bytes};
use crate::stages::analysis::ClusterId;
use crate::style::TextWrapStyle;
use crate::unit::LayoutUnit;
use crate::work;

define_id! {
    /// Names a [`BreakPoint`] in the scorer's table.
    ///
    /// The scored lines' start is first and their end last.
    struct BreakPointId(u32);
}

define_id! {
    /// Names a line of the paragraph being scored, counting from its first.
    pub(super) struct WrapLineId(u32);
}

/// How many lines `balance` scores, as Chrome's `kMaxLinesForBalance`.
///
/// A longer paragraph is balanced by bisecting the room instead.
pub(super) const BALANCE_LINES: usize = 6;

/// The score of a line that does not fit, past any fitting line's, as
/// Blink's `kScoreOverfull`.
const OVERFULL: f32 = 1e12;

/// The cost of a line that leaves the last word alone, as Blink's
/// `kOrphansPenalty`, in square pixels.
const ORPHAN: f32 = 10000.0;

/// The multiplier on the penalty of the line before the last under
/// `pretty`, as Blink's `kLastLinePenaltyMultiplier`.
///
/// A hyphen on the line before the last costs four times another line's.
const LAST_LINE: f32 = 4.0;

/// How a block's paragraphs choose their lines, from `text-wrap-style` and
/// the config's `pretty` rule.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum LineChoice {
    /// The greedy fit alone, for `auto`, `stable` and blocks that do not
    /// wrap.
    Greedy,
    /// `balance`: every line's slack as even as the count allows.
    Balance,
    /// `pretty`, by the config's rule.
    Pretty(Pretty),
}

impl LineChoice {
    /// Returns the choice for `style`, with `pretty` as the config's rule.
    ///
    /// A block that does not wrap has no choice to make, per CSS Text 4.
    pub(super) fn new(style: TextWrapStyle, pretty: Pretty, wraps: bool) -> Self {
        match style {
            _ if !wraps => Self::Greedy,
            TextWrapStyle::Auto | TextWrapStyle::Stable => Self::Greedy,
            TextWrapStyle::Balance => Self::Balance,
            TextWrapStyle::Pretty => Self::Pretty(pretty),
        }
    }

    /// Returns how many of a paragraph's last lines `pretty` scores.
    ///
    /// Chrome scores four, `kMaxLinesForOptimal`. `Pretty::Even` (beyond
    /// Chrome) scores six, so a short last line takes its words from more
    /// lines. `balance` scores a whole paragraph of up to [`BALANCE_LINES`].
    pub(super) fn scored(self) -> usize {
        match self {
            Self::Pretty(Pretty::Limited) => 4,
            Self::Greedy | Self::Balance | Self::Pretty(Pretty::Even) => BALANCE_LINES,
        }
    }
}

/// A place a line may end, as Blink's `LineBreakCandidate`.
///
/// Positions are from the paragraph's start, as the fit measures a line. A
/// line from `a` to `b` is `b.end − a.start` wide, edge costs included.
#[derive(Copy, Clone, Debug)]
pub(super) struct BreakPoint {
    /// The boundary where one line ends, hanging content included, and the
    /// next starts.
    at: ClusterId,
    /// Where a line starting here starts, less what the start costs, as
    /// Blink's `pos_no_break`.
    start: LayoutUnit,
    /// Where a line ending here ends, plus what the end costs, as Blink's
    /// `pos_if_break`.
    end: LayoutUnit,
    /// What ending a line here costs besides its slack: a hyphen and the
    /// orphan penalty.
    penalty: f32,
    /// Whether a line ending here shows a hyphen.
    hyphenated: bool,
}

impl BreakPoint {
    /// Returns the point at `at`; a shown hyphen costs `hyphen`.
    pub(super) fn new(
        at: ClusterId,
        start: LayoutUnit,
        end: LayoutUnit,
        hyphenated: bool,
        hyphen: f32,
    ) -> Self {
        Self {
            at,
            start,
            end,
            penalty: if hyphenated { hyphen } else { 0.0 },
            hyphenated,
        }
    }
}

/// The best way found to a point.
///
/// `prev` is where the line ending here starts. `line` is which paragraph
/// line starts here.
#[derive(Copy, Clone, Debug)]
struct BreakScore {
    score: f32,
    prev: BreakPointId,
    line: WrapLineId,
}

/// The weights the scorer uses, as Blink's `SetupParameters` after
/// Minikin's `computePenalties`.
#[derive(Copy, Clone, Debug)]
pub(super) struct ScoreRule {
    /// Whether the last line's slack counts, as under `balance`.
    ///
    /// Under `pretty` it counts nothing, and the last line pays four times
    /// the penalty of the line before.
    balanced: bool,
    /// Whether the text is justified.
    ///
    /// Then a line may be overfull only where it is the last, a hyphen
    /// costs less, and a line costs nothing.
    justified: bool,
    /// What a hyphen costs: twice the room times the font size, or half the
    /// product where the text is justified.
    pub(super) hyphen: f32,
    /// What every line costs: twice a hyphen, or nothing where justified.
    ///
    /// The line count is held to the greedy fit's anyway.
    line: f32,
    /// Whether a last line under a fifth of its room pays extra, as under
    /// `Pretty::Even`.
    short_last: bool,
}

impl ScoreRule {
    /// Returns the weights for `choice`, with lines `room` long and the
    /// block's font size `font_size` in pixels.
    pub(super) fn new(
        choice: LineChoice,
        justified: bool,
        room: LayoutUnit,
        font_size: f32,
    ) -> Self {
        let room = room.max(LayoutUnit::ZERO).to_px();
        let (hyphen, line) = if justified {
            (room * font_size / 2.0, 0.0)
        } else {
            let hyphen = room * font_size * 2.0;
            (hyphen, hyphen * 2.0)
        };
        Self {
            balanced: choice == LineChoice::Balance,
            justified,
            hyphen,
            line,
            short_last: choice == LineChoice::Pretty(Pretty::Even),
        }
    }
}

/// The scorer's working memory: the scored lines' rooms, their break
/// points and the best way to each.
///
/// The breaker keeps it as scratch. It is cleared and never freed, so a
/// warm relayout allocates nothing.
pub(super) struct Scoring {
    /// The first scored line, among the paragraph's.
    first: WrapLineId,
    /// Each scored line's room: its band less its indent, as its greedy
    /// line had it.
    ///
    /// A line past them has the last's room.
    rooms: [LayoutUnit; BALANCE_LINES],
    /// How many lines are scored.
    lines: usize,
    points: Table<BreakPointId, BreakPoint>,
    scores: Table<BreakPointId, BreakScore>,
}

impl Scoring {
    /// Returns empty scratch, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            first: WrapLineId(0),
            rooms: [LayoutUnit::ZERO; BALANCE_LINES],
            lines: 0,
            points: Table::new(),
            scores: Table::new(),
        }
    }

    /// Forgets the last scored lines and begins scoring from the
    /// paragraph's line `first`.
    pub(super) fn begin(&mut self, first: WrapLineId) {
        self.first = first;
        self.lines = 0;
        self.points.clear();
        self.scores.clear();
    }

    /// Takes the room of the next scored line.
    ///
    /// Returns `false` past the most lines the scorer weighs.
    pub(super) fn push_room(&mut self, room: LayoutUnit) -> bool {
        let Some(slot) = self.rooms.get_mut(self.lines) else {
            return false;
        };
        *slot = room;
        self.lines += 1;
        true
    }

    /// Takes the next break point, from the scored lines' start to their
    /// end.
    ///
    /// Returns `false` where the table is full.
    pub(super) fn push(&mut self, point: BreakPoint) -> bool {
        self.points.push(point).is_some()
    }

    /// Returns the room of the paragraph's `line`, never negative.
    ///
    /// A line past the scored ones has the last one's room, as Chrome's
    /// `AvailableWidth` reads its `LineWidths`.
    fn room(&self, line: WrapLineId) -> LayoutUnit {
        let at = line.get().saturating_sub(self.first.get());
        let rooms = self.rooms.get(..self.lines).unwrap_or_default();
        rooms
            .get(at)
            .or(rooms.last())
            .copied()
            .unwrap_or(LayoutUnit::ZERO)
            .max(LayoutUnit::ZERO)
    }

    /// Returns the best ends of the scored lines under `rule`, as Blink's
    /// `Optimize`.
    ///
    /// Returns `None` where the scored lines have fewer than three
    /// opportunities.
    /// Also returns `None` where the best has another line count than the
    /// greedy fit, which Chrome falls back to the greedy fit from.
    pub(super) fn best_ends(&mut self, rule: &ScoreRule) -> Option<PlannedEnds> {
        // The scored lines' start and end, and at least three opportunities, as
        // Blink's `kMinCandidates`.
        let count = self.points.len();
        if count < 5 {
            return None;
        }
        // A line ending at the last opportunity leaves the last word alone
        // and pays for it. Where that is a hyphen inside the last word, the
        // ones before it back to the word's start pay too.
        for point in self.points.as_mut_slice().iter_mut().take(count - 1).rev() {
            point.penalty += ORPHAN;
            if !point.hyphenated {
                break;
            }
        }
        self.scores.clear();
        let _ = self.scores.push(BreakScore {
            score: 0.0,
            prev: BreakPointId::new(0),
            line: self.first,
        });
        let short = self.short_penalty(rule);
        let mut active = BreakPointId::new(0);
        for end in (BreakPointId::new(1)..self.points.next_id()).ids() {
            let score = self.best_to(rule, end, &mut active, short);
            self.scores.push(score)?;
        }
        self.trace()
    }

    /// Returns what a short last line costs under `Pretty::Even`, and zero
    /// under the other rules.
    ///
    /// It exceeds all the slack the scored lines could leave squared. So a
    /// choice with a long enough last line always wins where one exists.
    fn short_penalty(&self, rule: &ScoreRule) -> f32 {
        if !rule.short_last {
            return 0.0;
        }
        let widest = self
            .rooms
            .iter()
            .take(self.lines)
            .max()
            .map_or(LayoutUnit::ZERO, |&room| room.max(LayoutUnit::ZERO))
            .to_px();
        let count = u16::try_from(self.lines.saturating_add(1)).unwrap_or(u16::MAX);
        f32::from(count) * widest * widest
    }

    /// Returns the best way to a line ending at point `end`, as the
    /// inner loop of Blink's `ComputeScores`.
    ///
    /// It tries starts from `active`, the first whose line here is not
    /// overfull. `active` moves on as earlier starts stop fitting. It prunes
    /// as Blink does, so ties go the same way.
    fn best_to(
        &self,
        rule: &ScoreRule,
        end: BreakPointId,
        active: &mut BreakPointId,
        short: f32,
    ) -> BreakScore {
        let fallback = BreakScore {
            score: f32::MAX,
            prev: BreakPointId::new(0),
            line: WrapLineId::new(0),
        };
        let (Some(end_point), Some(from)) = (self.points.get(end), self.scores.get(*active)) else {
            return fallback;
        };
        let last = end.get() + 1 == self.points.len();
        let mut best = f32::MAX;
        let mut best_prev = BreakPointId::new(0);
        let mut line = from.line;
        let mut room = self.room(line);
        let mut available = room + LayoutUnit::EPSILON;
        let mut edge = end_point.end - available;
        let mut hope = 0.0;
        for start in (*active..end).ids() {
            work::step();
            let (Some(start_score), Some(start_point)) =
                (self.scores.get(start), self.points.get(start))
            else {
                break;
            };
            if start_score.line != line {
                line = start_score.line;
                let next = self.room(line) + LayoutUnit::EPSILON;
                if next != available {
                    room = self.room(line);
                    available = next;
                    edge = end_point.end - available;
                    hope = 0.0;
                }
            }
            if start_score.score + hope >= best {
                continue;
            }
            let delta = (start_point.start - edge).to_px();
            let mut width = 0.0;
            let mut extra = 0.0;
            if (last || !rule.justified) && delta < 0.0 {
                width = OVERFULL;
            } else if last && !rule.balanced {
                extra = LAST_LINE * start_point.penalty;
                // Under `Pretty::Even`, a last line under a fifth of its
                // room pays extra.
                if short > 0.0 && end_point.end - start_point.start < room.divided(5) {
                    extra += short;
                }
            } else if delta < 0.0 {
                width = OVERFULL;
            } else {
                width = delta * delta;
            }
            if delta < 0.0 {
                *active = BreakPointId::new(start.get() + 1);
            } else {
                hope = width;
            }
            let score = start_score.score + width + extra;
            if score <= best {
                best = score;
                best_prev = start;
            }
        }
        let line = self
            .scores
            .get(best_prev)
            .map_or(fallback.line, |prev| WrapLineId::new(prev.line.get() + 1));
        BreakScore {
            score: best + end_point.penalty + rule.line,
            prev: best_prev,
            line,
        }
    }

    /// Follows the best way back from the scored lines' end into a plan, as
    /// Blink's `ComputeBreakPoints`.
    ///
    /// Returns `None` where the plan has another line count than the scored
    /// lines.
    fn trace(&self) -> Option<PlannedEnds> {
        let last = self.scores.last_id()?;
        let made = self
            .scores
            .get(last)
            .map_or(0, |score| score.line.get().saturating_sub(self.first.get()));
        if made != self.lines {
            return None;
        }
        // Ends are pushed from the last back, then reversed.
        let mut plan = PlannedEnds::new(self.first);
        let mut at = last;
        while at.get() > 0 {
            work::step();
            let (Some(score), Some(point)) = (self.scores.get(at), self.points.get(at)) else {
                break;
            };
            if !plan.push(point.at) || score.prev >= at {
                break;
            }
            at = score.prev;
        }
        if plan.len != self.lines {
            return None;
        }
        plan.ends.get_mut(..plan.len)?.reverse();
        Some(plan)
    }
}

heap_bytes! {
    Scoring { points, scores; first, rooms, lines }
}

/// The line ends the scorer chose for a paragraph's scored lines.
///
/// Lines it did not score are fitted as the greedy fit fitted them. The
/// few ends live on the stack.
#[derive(Copy, Clone, Debug)]
pub(super) struct PlannedEnds {
    /// The first scored line, among the paragraph's.
    first: WrapLineId,
    /// Each scored line's end, from the first.
    ends: [ClusterId; BALANCE_LINES],
    /// How many of `ends` it has.
    len: usize,
}

impl PlannedEnds {
    /// Returns a plan with no ends, for lines scored from the paragraph's
    /// line `first`.
    fn new(first: WrapLineId) -> Self {
        Self {
            first,
            ends: [ClusterId::new(0); BALANCE_LINES],
            len: 0,
        }
    }

    /// Takes the next line's end.
    ///
    /// Returns `false` past the most lines the scorer weighs.
    fn push(&mut self, end: ClusterId) -> bool {
        let Some(slot) = self.ends.get_mut(self.len) else {
            return false;
        };
        *slot = end;
        self.len += 1;
        true
    }

    /// Returns where the plan ends the paragraph's `line`, if it is
    /// scored.
    pub(super) fn get(&self, line: WrapLineId) -> Option<ClusterId> {
        let at = line.get().checked_sub(self.first.get())?;
        self.ends.get(..self.len)?.get(at).copied()
    }

    /// Returns each scored line with its end, in order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (WrapLineId, ClusterId)> + '_ {
        let first = self.first.get();
        self.ends
            .iter()
            .take(self.len)
            .enumerate()
            .map(move |(at, &end)| (WrapLineId::new(first + at), end))
    }
}
