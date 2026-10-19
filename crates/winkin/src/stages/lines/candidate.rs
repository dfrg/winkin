//! Measuring one candidate end of a line, as more of [`Breaker`]'s `impl`.
//!
//! - **Width.** A line to `e` reaches its prefix sum there, plus its reshaped
//!   pieces' deltas, its tabs and a continued ruby column's shift, rounded up
//!   to Chrome's grid. What hangs at its end comes off it.
//! - **Tabs.** A tab takes nothing in the prefix. Its width depends on where
//!   it lands; line layout sizes it with the same function.
//! - **Costs.** A line pays the line-edge cost at each end, and takes
//!   `text-indent` where it applies.

use alloc::vec::Vec;
use core::ops::Range;

use super::reshape::EdgeEnd;
use super::{Breaker, BreakerParagraph, Fitting, LineStart, Lines, ReshapedPieces};
use crate::data::{Id, IdRange, SortedCursor, heap_bytes};
use crate::stages::analysis::ClusterId;
use crate::stages::content::{ContentFlags, ItemFlags, ItemId, TextFlags};
use crate::stages::measure::{LineEdgeCost, LineEdgeFlags, tab_advance_reached};
use crate::stages::shape::{ShapedFlags, ShapingEdges};
use crate::stages::{Segment, Segments, Step};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

use super::fit::Candidate;

/// The tab widths a line's walk sized, which its candidates read back.
///
/// Readers size only the tabs past where the walk stopped.
pub(super) struct SizedTabs {
    /// The line's start, tab origin and indent, which every width depends
    /// on; `None` before a walk.
    line: Option<(ClusterId, LayoutUnit, LayoutUnit)>,
    /// Each tab the walk passed, with its width, in order.
    tabs: Vec<(ClusterId, InlineLayoutUnit)>,
    /// Where the walk stopped: every tab before it is recorded.
    walked: ClusterId,
}

impl SizedTabs {
    /// Returns an empty record, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            line: None,
            tabs: Vec::new(),
            walked: ClusterId::new(0),
        }
    }

    /// Forgets the tabs sized, at the start of a layout of the lines.
    pub(super) fn clear(&mut self) {
        self.line = None;
        self.tabs.clear();
    }

    /// Starts the record over for a walk over `line`.
    pub(super) fn begin(&mut self, line: &LineStart<'_>) {
        self.line = Some((line.start, line.tab_origin, line.indent));
        self.tabs.clear();
        self.walked = line.start;
    }

    /// Records `tab`, which the walk sized `width` wide.
    pub(super) fn push(&mut self, tab: ClusterId, width: InlineLayoutUnit) {
        self.tabs.push((tab, width));
    }

    /// Records that the walk stopped at boundary `at`.
    pub(super) fn stop(&mut self, at: ClusterId) {
        self.walked = at;
    }

    /// Returns the tabs sized on `line` and where their walk stopped; none
    /// and the line's start where the record is another line's.
    fn line_tabs(&self, line: &LineStart<'_>) -> (&[(ClusterId, InlineLayoutUnit)], ClusterId) {
        if self.line == Some((line.start, line.tab_origin, line.indent)) {
            (&self.tabs, self.walked)
        } else {
            (&[], line.start)
        }
    }
}

heap_bytes! {
    SizedTabs { tabs; line, walked }
}

impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Measures the line from the fitting's start to `end`, reshaping its
    /// edges where the font shaped across them.
    ///
    /// Takes back the last candidate's pieces first. With `trims_end`, the
    /// line ends at a closing mark it trims and is always reshaped.
    pub(super) fn candidate(
        &mut self,
        out: &mut Lines,
        fitting: &mut Fitting<'_, '_>,
        end: ClusterId,
        trims_end: bool,
    ) -> Candidate {
        // The start's window piece, shared by every candidate, stays.
        out.rewind_edges(fitting.head.after());
        let para = fitting.line.para;
        let start = fitting.line.start;
        let window = fitting.line.window;
        let mut pieces = ReshapedPieces::default();
        // The room of a seam the line ends at, which it gives back.
        let seam = if para.seams && end > start {
            self.seam_before(para, end)
        } else {
            InlineLayoutUnit::ZERO
        };
        let gives_back = seam > InlineLayoutUnit::ZERO;
        let reshapes = self.shaped.contains(ShapedFlags::HAS_UNSAFE);
        if reshapes || window.is_some() || trims_end || gives_back {
            // Where the line needs its exact end, the spaces it ends with are
            // left out of its reshape, so its last letter is not kerned
            // against them.
            let removed = if trims_end || gives_back {
                None
            } else {
                self.removed_end(&fitting.line, end)
            };
            let shaped_end = removed.unwrap_or(end);
            let reshaped = trims_end
                || removed.is_some()
                || (end < para.end
                    && self.stages.shaped.glyphs.word(end).is_unsafe_to_break()
                    && (!self.clusters.space_or_tab_before(end)
                        || self.space_break_reshaped(&fitting.line, end)));
            let whole = ShapingEdges {
                line_start: start > para.start,
                trim_start: window.is_some_and(|window| window.trims),
                trim_end: trims_end,
                ..ShapingEdges::default()
            };
            let ends = EdgeEnd {
                whole,
                reshaped,
                trims: trims_end,
                seam,
                whole_at_window_end: trims_end || gives_back,
            };
            let line = start..shaped_end;
            pieces = self.edge_pieces(out, para, line, window, fitting.head, ends);
        }
        // At the paragraph's end, part of what hangs may hang only where it
        // overflows.
        let (content_end, mut tail) =
            self.hanging_tail(start, end, |cluster| pieces.advance(&*out, cluster));
        let conditional = (end == para.end)
            .then(|| self.conditional_start(content_end..end))
            .flatten()
            .map(|from| {
                let mut near = None;
                (content_end..from)
                    .ids()
                    .fold(InlineLayoutUnit::ZERO, |sum, cluster| {
                        sum + pieces
                            .advance(&*out, cluster)
                            .unwrap_or_else(|| self.stages.cluster_advance_near(cluster, &mut near))
                    })
            });
        let mut reach = self.stages.measured.prefix.get(end) - para.origin
            + pieces.delta()
            + fitting.shift.at(end);
        if para.tabs {
            // The prefix gives tabs nothing. Hanging tabs come off with the
            // rest of what hangs.
            let (tabs, hanging_tabs) = self.line_tabs(&fitting.line, end, content_end);
            reach += tabs;
            tail += hanging_tabs;
        }
        Candidate {
            end,
            content_end,
            content: ((reach - tail).ceil_to_grid() - fitting.line.fit_start).to_layout(),
            full: (reach.ceil_to_grid() - fitting.line.fit_start).to_layout(),
            conditional,
            cost: end_cost(para, &mut fitting.costs, end),
            pieces,
        }
    }

    /// Returns where a line ending at `end` ends without what hangs, and how
    /// far what hangs reaches.
    ///
    /// Steps back no further than `floor`. Each cluster is as wide as
    /// `reshaped` says where it has a reshaped advance, and otherwise its own
    /// advance. A box's edge among the hanging clusters stays content.
    pub(super) fn hanging_tail(
        &self,
        floor: ClusterId,
        end: ClusterId,
        reshaped: impl Fn(ClusterId) -> Option<InlineLayoutUnit>,
    ) -> (ClusterId, InlineLayoutUnit) {
        let mut content_end = end;
        let mut tail = InlineLayoutUnit::ZERO;
        let mut near = None;
        while content_end > floor {
            work::step();
            let cluster = ClusterId::new(content_end.get() - 1);
            if !self.clusters.hangs(cluster) {
                break;
            }
            tail += reshaped(cluster)
                .unwrap_or_else(|| self.stages.cluster_advance_near(cluster, &mut near));
            content_end = cluster;
        }
        (content_end, tail)
    }

    /// Returns where the white space that hangs only where it overflows
    /// starts in `tail`, a paragraph's hanging end.
    ///
    /// That white space is the trailing run of `pre-wrap` texts. Where white
    /// space collapses, space separators hang whole, as Blink's
    /// `ComputeTrailingSpaceWidth` hangs them. `None` where the last text
    /// with a breaking space collapses, or `tail` has none.
    fn conditional_start(&self, tail: Range<ClusterId>) -> Option<ClusterId> {
        let clusters = self.clusters;
        let spaces = |range: Range<ClusterId>| {
            range
                .ids()
                .any(|cluster| clusters.is_breaking_space(cluster))
        };
        if !spaces(tail.clone()) {
            return None;
        }
        let facts = &self.stages.content.facts;
        let mut start = None;
        for step in Segments::new(&self.stages, tail) {
            if let Step::Segment(segment) = step
                && spaces(segment.start..segment.end)
            {
                let conditional = segment
                    .text_facts(&self.stages)
                    .is_some_and(|text| facts.text(text).has(TextFlags::HANGS_CONDITIONALLY));
                start = match (conditional, start) {
                    (true, None) => Some(segment.start),
                    (true, held) => held,
                    (false, _) => None,
                };
            }
        }
        start
    }

    /// Returns `tab`'s width on the line being fitted, given the width
    /// `before` of the tabs before it.
    ///
    /// `boundary` gives the first item at its boundary. A tab in ruby
    /// annotation text takes no room on its base's line.
    #[inline(never)]
    pub(super) fn tab_width(
        &self,
        line: &LineStart<'_>,
        tab: ClusterId,
        before: InlineLayoutUnit,
        segment: &Segment,
        boundary: Option<(ClusterId, ItemId)>,
    ) -> InlineLayoutUnit {
        let stages = &self.stages;
        let Some(stops) = segment
            .item_row(stages)
            .filter(|item| !item.flags.contains(ItemFlags::ANNOTATION))
            .and_then(|item| self.measured.text_metrics(stages.text_facts(item.node)))
            .map(|metrics| metrics.tab)
        else {
            return InlineLayoutUnit::ZERO;
        };
        let from = boundary
            .filter(|&(at, _)| at == tab)
            .map_or(segment.item, |(_, first)| first);
        let pen = stages.pen(tab, from);
        // A tab past the start's window moves by what its reshape adds.
        let moved = line
            .window
            .filter(|window| window.end <= tab)
            .map_or(InlineLayoutUnit::ZERO, |window| window.delta);
        let position = InlineLayoutUnit::from_layout(line.indent)
            + (pen - self.stages.measured.prefix.get(line.start))
            + moved
            + before;
        let reach = self.stages.measured.ruby_columns().tab_reach(tab);
        InlineLayoutUnit::from_layout(tab_advance_reached(line.tab_origin, position, stops, reach))
    }

    /// Returns the width of `line`'s tabs before `end`, and of those from
    /// `content_end` on, which hang.
    ///
    /// It is out of line so text without tabs pays nothing.
    #[inline(never)]
    pub(super) fn line_tabs(
        &self,
        line: &LineStart<'_>,
        end: ClusterId,
        content_end: ClusterId,
    ) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let (mut all, mut hanging) = (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        let (sized, walked) = self.scratch.tabs.line_tabs(line);
        for &(tab, width) in sized {
            if tab >= end {
                break;
            }
            all += width;
            if tab >= content_end {
                hanging += width;
            }
        }
        if end <= walked {
            return (all, hanging);
        }
        let mut boundary = None;
        for step in self.line_segments(line, walked..end) {
            let Some(segment) = step_segment(&mut boundary, step) else {
                continue;
            };
            for cluster in (segment.start..segment.end).ids() {
                if self.clusters.is_tab(cluster) {
                    let width = self.tab_width(line, cluster, all, &segment, boundary);
                    all += width;
                    if cluster >= content_end {
                        hanging += width;
                    }
                }
            }
        }
        (all, hanging)
    }

    /// Returns the walk over `clusters` of the line being fitted.
    ///
    /// The walk starts at the line's first item, with no search, and skips
    /// the steps before `clusters`.
    pub(super) fn line_segments(
        &self,
        line: &LineStart<'_>,
        clusters: Range<ClusterId>,
    ) -> impl Iterator<Item = Step> + use<'a> {
        let (para, from) = (line.para.id, clusters.start);
        let walked = line.start..clusters.end;
        Segments::from_item(&self.stages, para, line.first_item, walked).filter_map(move |step| {
            match step {
                Step::Item { at, .. } if at < from => None,
                Step::Segment(segment) if segment.end <= from => None,
                Step::Segment(mut segment) => {
                    segment.start = segment.start.max(from);
                    Some(Step::Segment(segment))
                }
                step @ Step::Item { .. } => Some(step),
            }
        })
    }

    /// Returns a cursor into `para`'s line-edge costs sought to `at`, or
    /// `None` where `para` has no cost.
    pub(super) fn edge_costs(
        &self,
        para: &BreakerParagraph,
        at: ClusterId,
    ) -> Option<SortedCursor<'a, LineEdgeCost>> {
        let costs = self.stages.measured.edge_costs();
        para.costs.then(|| costs.cursor(at))
    }

    /// Returns what a line starting at `start` pays there, and how far a
    /// mark hangs past its start, moving `costs` to `start`.
    ///
    /// Only the block's first line (`first`) hangs its first cluster under
    /// `hanging-punctuation: first`. Out of line, a relayout of content with
    /// no line-edge cost runs about 1% more instructions.
    #[inline(always)]
    pub(super) fn start_cost(
        &self,
        costs: &mut Option<SortedCursor<'_, LineEdgeCost>>,
        start: ClusterId,
        first: bool,
    ) -> (LayoutUnit, LayoutUnit) {
        let Some(cost) = costs.as_mut().and_then(|costs| costs.get(start)) else {
            return (LayoutUnit::ZERO, LayoutUnit::ZERO);
        };
        if !cost.flags.contains(LineEdgeFlags::FIRST_LINE_ONLY) {
            return (cost.start, LayoutUnit::ZERO);
        }
        let hang = self.stages.cluster_advance(start).to_layout();
        if first {
            (cost.start, hang)
        } else {
            // Another line starting here pays the rest of the cost.
            (cost.start + hang, LayoutUnit::ZERO)
        }
    }

    /// Returns the `text-indent` a line starting at `start` takes, `first`
    /// marking the block's first line.
    ///
    /// - The first line takes it, and under `each-line` every line after a
    ///   forced break. `hanging` inverts which lines do.
    /// - A percentage is of the area's width, not the band's, truncated as
    ///   Chrome's `MinimumValueForLength` does. Nothing clamps it.
    /// - A first line that opens an initial letter takes it twice, once
    ///   before the letter's box and once inside it
    ///   (`CalculateInitialLetterBoxInlineSize`).
    pub(super) fn indent(
        &self,
        para: &BreakerParagraph,
        start: ClusterId,
        first: bool,
    ) -> LayoutUnit {
        let indent = self.text_indent;
        let after_break = start == para.start && !first;
        if (first || (indent.each_line && after_break)) == indent.hanging {
            return LayoutUnit::ZERO;
        }
        let length = self.indent;
        let letter = self
            .stages
            .content
            .flags
            .contains(ContentFlags::INITIAL_LETTER);
        if first && letter && self.stages.measured.initial_letter().is_some() {
            return length + length;
        }
        length
    }
}

/// Returns what a line of `para` ending at `end` pays there, moving `costs`
/// to `end` short of the paragraph's end.
pub(super) fn end_cost(
    para: &BreakerParagraph,
    costs: &mut Option<SortedCursor<'_, LineEdgeCost>>,
    end: ClusterId,
) -> LineEdgeCost {
    if end == para.end {
        return para.end_cost;
    }
    costs
        .as_mut()
        .and_then(|costs| costs.get(end))
        .copied()
        .unwrap_or_default()
}

/// Returns the segment `step` is, or `None` for an item, recording in
/// `boundary` the first item at each boundary.
pub(super) fn step_segment(
    boundary: &mut Option<(ClusterId, ItemId)>,
    step: Step,
) -> Option<Segment> {
    match step {
        Step::Segment(segment) => Some(segment),
        Step::Item { at, id } => {
            if boundary.is_none_or(|(first, _)| first != at) {
                *boundary = Some((at, id));
            }
            None
        }
    }
}
