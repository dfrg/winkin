//! Splits breakable ruby columns across lines.
//!
//! A line is fitted as plain text first. Where its furthest boundary falls
//! inside a column that may break, [`split_ruby`](Breaker::split_ruby)
//! splits the column there:
//! - `base_cut` finds the last base opportunity whose width fits the base's
//!   share of the room;
//! - `level_cut` cuts each annotation level at the last opportunity that
//!   fits the level's share;
//! - `split_width` reshapes each level's edges and takes the widest of the
//!   base and its levels;
//! - `record_ruby` keeps the piece and its levels for the line.
//!
//! A line that starts inside a split column continues it.
//! [`continued_column`](Breaker::continued_column) sizes the column's rest
//! before the line is fitted. The rest's width shifts prefix positions past
//! the column's end. [`finish_ruby`](Breaker::finish_ruby) records the rest
//! where the line holds all of it.
//!
//! It follows Chrome's rules:
//! - The room is shared in proportion: the base and each level get the
//!   share their width has of the column's.
//! - A rest of four base glyphs or fewer, with eight or fewer glyphs in
//!   each level, stays whole.
//! - A base cut whose combined line does not fit is retried at the cut
//!   before it, for the same target.
//! - Under [`RubyBreakWithin::BaseOpportunities`] the base breaks on its own, and a
//!   level with no opportunity goes whole. Under [`RubyBreakWithin::AllLevels`]
//!   a column breaks only where each level left has an opportunity too.
use super::{ColumnPart, ContinuationShift, LevelCut, PartLevel, PartLevelId, RubyLines};
use crate::config::RubyBreakWithin;
use crate::data::{Id, IdRange, RunCursor};
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::ItemId;
use crate::stages::lines::reshape::EdgeEnd;
use crate::stages::lines::search::{gallop_last, overflowing, settle};
use crate::stages::lines::{
    Breaker, BreakerParagraph, Fitted, Fitting, LineHead, LineId, Lines, ReshapedPieces,
};
use crate::stages::measure::{
    LineEdgeFlags, RubyColumn, RubyColumnId, RubyLevel, RubyLevelId, is_monolithic,
};
use crate::stages::shape::ShapingEdges;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::Cell;
use core::iter::successors;
use core::mem;
use core::ops::Range;

/// A piece proposed for the line: one trial of a split, or a column's rest.
struct ProposedPiece {
    column: RubyColumnId,
    /// Its base clusters on the line.
    base: Range<ClusterId>,
    /// The width it takes: its base's or its widest level's.
    width: LayoutUnit,
    /// Its base's width, with reshaped edges and any hyphen.
    base_width: LayoutUnit,
    /// What it overhangs the text before it.
    before: LayoutUnit,
}

/// A base cut, with what the trial reads of it.
struct BaseCut {
    at: ClusterId,
    /// The base's width from the piece's start to the cut, less what hangs.
    width: LayoutUnit,
    /// The hyphen a soft hyphen before the cut shows.
    hyphen: LayoutUnit,
}

/// The column the line before split, which the line being fitted
/// continues.
#[derive(Clone, Debug)]
pub(crate) struct Continued {
    column: RubyColumnId,
    /// The line before's levels of the column, where each level resumes.
    levels: Range<PartLevelId>,
    /// The rest, where the line starts inside the column's base.
    rest: Option<Rest>,
}

/// The rest of a continued column from the line's start.
#[derive(Copy, Clone, Debug)]
struct Rest {
    /// Its width, its levels' edges reshaped.
    width: LayoutUnit,
    /// Its base's width, less what hangs at its end.
    base: LayoutUnit,
    shift: ContinuationShift,
}

impl Continued {
    /// Returns how the rest shifts prefix positions past the column's end.
    pub(crate) fn shift(&self) -> ContinuationShift {
        self.rest
            .map_or_else(ContinuationShift::default, |rest| rest.shift)
    }
}

impl<'a: 'c, 'c, 'm, 'provider> Breaker<'a, 'c, 'm, 'provider> {
    /// Returns the last piece the line before set that ends at `start`,
    /// of `column` where one is named.
    fn previous_piece(
        out: &Lines,
        column: Option<RubyColumnId>,
        start: ClusterId,
    ) -> Option<&ColumnPart> {
        let line = LineId::new(out.lines.next_id().get().checked_sub(1)?);
        out.ruby
            .as_ref()?
            .recent_line_parts(line)
            .iter()
            .rev()
            .find(|piece| piece.base.end == start && column.is_none_or(|id| piece.column == id))
    }

    /// Moves `cursor` to the item holding `at`, either way, empty edge
    /// items included.
    fn advance_item(&self, at: ClusterId, cursor: &mut RunCursor<ItemId, ClusterId>) {
        let items = &self.stages.analysis.item_clusters;
        while at < items.start(cursor.id()) && cursor.id().get() > 0 {
            *cursor = items.cursor(ItemId::new(cursor.id().get() - 1));
        }
        while cursor.end() <= at && cursor.id().get() < self.stages.content.items.len() {
            items.step(cursor);
        }
    }

    /// Returns whether `at` is a space that collapses, its item found by
    /// `cursor`.
    fn collapsible(&self, at: ClusterId, cursor: &mut RunCursor<ItemId, ClusterId>) -> bool {
        if !self.clusters.is_breaking_space(at) {
            return false;
        }
        self.advance_item(at, cursor);
        self.stages
            .content
            .items
            .get(cursor.id())
            .is_some_and(|item| {
                self.stages
                    .content
                    .facts
                    .text(self.stages.text_facts(item.node))
                    .collapse
                    .collapses_spaces()
            })
    }

    /// Returns where `level` resumes on this line, and the first item
    /// there: after the part `resume`, the line before's levels, set, and
    /// past collapsed spaces.
    ///
    /// The items are walked from the part's first item, or else the
    /// level's opening item.
    fn level_start(
        &self,
        out: &Lines,
        resume: Option<&Range<PartLevelId>>,
        level: &RubyLevel,
        id: RubyLevelId,
    ) -> (ClusterId, ItemId) {
        let (mut at, near) = resume
            .and_then(|range| {
                let levels = out.ruby.as_ref()?.levels.get_slice(range.clone())?;
                levels.iter().find(|level| level.level == id)
            })
            .map_or((level.clusters.start, level.open), |part| {
                (part.clusters.end, part.item)
            });
        let items = &self.stages.analysis.item_clusters;
        let near = items.walk_to(near, at);
        // Collapsed leading spaces have no place on a resumed annotation line.
        if at >= level.clusters.end || !self.clusters.is_breaking_space(at) {
            return (at, near);
        }
        let mut cursor = items.cursor(near);
        while at < level.clusters.end && self.collapsible(at, &mut cursor) {
            at = ClusterId::new(at.get() + 1);
        }
        (at, items.walk_to(cursor.id(), at))
    }

    /// Fills `cuts` with where each level of `column` resumes on this line,
    /// and its unshaped width from there to its end.
    fn start_levels(
        &self,
        out: &Lines,
        column: &RubyColumn,
        resume: Option<&Range<PartLevelId>>,
        cuts: &mut Vec<LevelCut>,
    ) {
        let rubies = self.stages.measured.ruby_columns();
        cuts.clear();
        for id in column.levels.clone().ids() {
            if let Some(level) = rubies.level(id) {
                let (from, item) = self.level_start(out, resume, level, id);
                cuts.push(LevelCut {
                    level: id,
                    from,
                    item,
                    remaining: self.visible_width(
                        level,
                        self.visible_annotation(level, from..level.clusters.end, item),
                    ),
                    target: None,
                    end: level.clusters.end,
                    width: None,
                });
            }
        }
    }

    /// Returns `range` of `level` less the collapsed spaces it ends with.
    ///
    /// Their items are walked to from `near`, an item at the range's start,
    /// or from the annotation's closing item where the range ends the level.
    fn visible_annotation(
        &self,
        level: &RubyLevel,
        range: Range<ClusterId>,
        near: ItemId,
    ) -> Range<ClusterId> {
        if range.is_empty()
            || !self
                .clusters
                .is_breaking_space(ClusterId::new(range.end.get() - 1))
        {
            return range;
        }
        let content = self.stages.content;
        let near = if range.end >= level.clusters.end {
            let node = content.items.get(level.open).map(|item| item.node);
            node.map_or(near, |node| {
                let end = content.nodes.items(node).end;
                ItemId::new(end.get().saturating_sub(1)).max(near)
            })
        } else {
            near
        };
        let items = &self.stages.analysis.item_clusters;
        let last = ClusterId::new(range.end.get().saturating_sub(1));
        let mut cursor = items.cursor(items.walk_to(near, last));
        self.visible_from(range, &mut cursor)
    }

    /// Returns `range` less the collapsed spaces it ends with, finding
    /// their items by `cursor`, which a walk carries.
    fn visible_from(
        &self,
        range: Range<ClusterId>,
        cursor: &mut RunCursor<ItemId, ClusterId>,
    ) -> Range<ClusterId> {
        let mut visible = range;
        while visible.end > visible.start
            && self.collapsible(ClusterId::new(visible.end.get() - 1), cursor)
        {
            visible.end = ClusterId::new(visible.end.get() - 1);
        }
        visible
    }

    /// Returns the unshaped width of `visible` of `level`, a range with no
    /// collapsed spaces at its end.
    fn visible_width(&self, level: &RubyLevel, visible: Range<ClusterId>) -> LayoutUnit {
        if visible.is_empty() {
            return LayoutUnit::ZERO;
        }
        let rubies = self.stages.measured.ruby_columns();
        (rubies.level_position(level, visible.end) - rubies.level_position(level, visible.start))
            .to_layout()
    }

    /// Returns the width of base clusters `base`, less what hangs at its end.
    fn base_width(&self, base: Range<ClusterId>) -> LayoutUnit {
        let prefix = &self.stages.measured.prefix;
        let (_, tail) = self.hanging_tail(base.start, base.end, |_| None);
        (prefix.get(base.end) - prefix.get(base.start) - tail).to_layout()
    }

    /// Returns the boundary after `column`'s last annotation.
    fn column_end(&self, column: &RubyColumn) -> ClusterId {
        self.stages.analysis.item_clusters.start(column.close)
    }

    /// Returns the width of the rest of a column from `start`, its base's
    /// end, with each level from where `cuts` resume it to its end, and its
    /// levels' edges reshaped; and its base's width, less what hangs.
    fn rest_width(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        base: Range<ClusterId>,
        cuts: &[LevelCut],
    ) -> (LayoutUnit, LayoutUnit) {
        let rubies = self.stages.measured.ruby_columns();
        let base = self.base_width(base);
        let mut width = base;
        for cut in cuts {
            if let Some(level) = rubies.level(cut.level) {
                let end = level.clusters.end;
                width = width.max(
                    self.reshaped_level(out, para, (level, cut), end, false)
                        .width,
                );
            }
        }
        (width, base)
    }

    /// Finds the column the line from `start` continues, and sizes its
    /// rest.
    ///
    /// Out of line, so that plain fitting stays small.
    #[inline(never)]
    pub(crate) fn continued_column(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
    ) -> Option<Continued> {
        let piece = Self::previous_piece(out, None, start)?;
        let mut continued = Continued {
            column: piece.column,
            levels: piece.levels.clone(),
            rest: None,
        };
        let measured = self.stages.measured;
        let Some(column) = measured.ruby_columns().get(continued.column) else {
            return Some(continued);
        };
        if start >= column.base.end {
            return Some(continued);
        }
        let mut cuts = mem::take(&mut self.scratch.level_cuts);
        self.start_levels(out, column, Some(&continued.levels), &mut cuts);
        let (width, base) = self.rest_width(out, para, start..column.base.end, &cuts);
        self.scratch.level_cuts = cuts;
        let extra = column.width - column.base_width - column.overhang.0 - column.overhang.1;
        continued.rest = Some(Rest {
            width,
            base,
            shift: ContinuationShift {
                end: Some(self.column_end(column)),
                delta: InlineLayoutUnit::from_layout(width - base - extra),
            },
        });
        Some(continued)
    }

    /// Returns whether the rest of `column` from `start` stays whole: four
    /// base glyphs or fewer, and eight or fewer in each level `cuts` start.
    ///
    /// The base's items are walked from `near`, an item at or before
    /// `start`'s.
    fn monolithic_rest(
        &self,
        column: &RubyColumn,
        (start, near): (ClusterId, ItemId),
        cuts: &[LevelCut],
    ) -> bool {
        if start == column.base.start {
            return false;
        }
        let rubies = self.stages.measured.ruby_columns();
        is_monolithic(
            self.stages.content,
            self.stages.analysis,
            self.stages.shaped,
            (near, start..column.base.end),
            (cuts.iter()).filter_map(|cut| {
                let level = rubies.level(cut.level)?;
                Some((cut.item, cut.from..level.clusters.end))
            }),
        )
    }

    /// Returns the width of the hyphen a soft hyphen before `end` shows.
    fn hyphen(&self, end: ClusterId, cursor: &mut RunCursor<ItemId, ClusterId>) -> LayoutUnit {
        let Some(last) = end.get().checked_sub(1).map(ClusterId::new) else {
            return LayoutUnit::ZERO;
        };
        if self.clusters.class(last) != Some(ClusterClass::SoftHyphen) {
            return LayoutUnit::ZERO;
        }
        self.advance_item(last, cursor);
        self.stages
            .content
            .items
            .get(cursor.id())
            .and_then(|item| {
                self.measured
                    .generated()
                    .hyphen(self.stages.text_facts(item.node))
            })
            .map_or(LayoutUnit::ZERO, |hyphen| {
                self.measured.generated().snapped(hyphen)
            })
    }

    /// Returns what reshaping the candidate's pieces in `range` adds to its
    /// width.
    fn base_delta(&self, fitted: &ReshapedPieces, range: Range<ClusterId>) -> LayoutUnit {
        fitted
            .iter()
            .filter(|piece| {
                let clusters = piece.shape.clusters();
                clusters.start >= range.start && clusters.end <= range.end
            })
            .fold(LayoutUnit::ZERO, |sum, piece| sum + piece.delta.to_layout())
    }

    /// Returns the furthest boundary of `base` whose width from its start
    /// fits `target`, by galloping and halving.
    ///
    /// Where the paragraph's prefix never goes back, a cut's width grows
    /// with the cut, so no cut past this boundary fits. Elsewhere it is the
    /// base's end, and every cut is tried.
    fn base_furthest(
        &self,
        para: &BreakerParagraph,
        base: Range<ClusterId>,
        target: LayoutUnit,
    ) -> ClusterId {
        if !para.monotone || base.is_empty() {
            return base.end;
        }
        work::seek();
        let fits = |at: usize| self.base_width(base.start..ClusterId::new(at)) <= target;
        ClusterId::new(gallop_last(base.start.get(), base.end.get(), fits))
    }

    /// Finds the last cut of `base` before `ceiling` whose width and hyphen
    /// fit `target`.
    ///
    /// It visits the cuts backwards from `ceiling`, or from `furthest` where
    /// that is nearer: no cut past `furthest` is within the target. A rejected
    /// trial resumes before its cut. Once the normal opportunities run out
    /// it keeps to the emergency ones: the target stays fixed and earlier
    /// cuts only disappear. Where no cut fits, it overflows at the earliest
    /// normal opportunity, or else the earliest emergency one.
    fn base_cut(
        &self,
        id: RubyColumnId,
        base: Range<ClusterId>,
        (target, furthest): (LayoutUnit, ClusterId),
        ceiling: ClusterId,
        emergency: &mut bool,
        cursor: &mut RunCursor<ItemId, ClusterId>,
    ) -> Option<BaseCut> {
        let rubies = self.stages.measured.ruby_columns();
        let end = ClusterId::new(base.end.get().saturating_sub(1)).min(ceiling);
        // The columns nested in the base follow it, and the line's own
        // columns follow those before the line.
        let near = Cell::new(id.max(self.column.get()));
        let opportunity = |range: Range<ClusterId>, kind: (bool, bool)| {
            rubies.opportunity(self.clusters, range, Some(id), kind, &near)
        };
        // The cut at `at`, the item of a hyphen before it found by `cursor`.
        let mut cut = |at: ClusterId| BaseCut {
            at,
            width: self.base_width(base.start..at),
            hyphen: self.hyphen(at, cursor),
        };
        loop {
            let mut past = end.min(furthest);
            while let Some(at) = opportunity(base.start..past, (true, *emergency)) {
                work::step();
                let cut = cut(at);
                if cut.width + cut.hyphen <= target {
                    return Some(cut);
                }
                past = ClusterId::new(at.get().saturating_sub(1));
            }
            if *emergency {
                let at = opportunity(base.start..end, (false, false))
                    .or_else(|| opportunity(base.start..end, (false, true)))?;
                return Some(cut(at));
            }
            *emergency = true;
        }
    }

    /// Cuts `level` from where `rest` resumes it, at the last opportunity
    /// whose width fits `target`: a normal one, else an emergency one, else
    /// the first of either. Returns the level's end where all of it fits.
    ///
    /// The walk stops once no later cut can fit: once the least of the
    /// level's cumulative widths past the cut is beyond the target. The
    /// emergency walk, which steps a cluster at a time, stops before the
    /// first boundary where that least is beyond it, past any spaces there.
    fn level_cut(&self, level: &RubyLevel, rest: &LevelCut, target: LayoutUnit) -> ClusterId {
        let start = rest.from;
        if start >= level.clusters.end || rest.remaining <= target {
            return level.clusters.end;
        }
        let rubies = self.stages.measured.ruby_columns();
        let origin = rubies.level_position(level, start);
        let end = ClusterId::new(level.clusters.end.get().saturating_sub(1));
        // No cut whose visible end is at `above` or past it fits, and a cut
        // past `ceiling` has its visible end there.
        let mut ceiling = rubies
            .floor_above(level, start, origin + InlineLayoutUnit::from_layout(target))
            .min(end);
        while ceiling < end && self.clusters.is_breaking_space(ceiling) {
            work::step();
            ceiling = ClusterId::new(ceiling.get() + 1);
        }
        // The cuts come in order: one item cursor serves them all, from the
        // first item at the start.
        let mut item = self.stages.analysis.item_clusters.cursor(rest.item);
        let mut first = None;
        for emergency in [false, true] {
            let mut from = start;
            let mut last = None;
            // Whether the floor is still worth checking: it stops being so
            // once one check fails.
            let mut guard = true;
            loop {
                work::step();
                let cut = if emergency {
                    self.clusters.first_emergency(from..ceiling)
                } else {
                    self.clusters.first_opportunity(from..end)
                };
                let Some(cut) = cut else {
                    break;
                };
                first.get_or_insert(cut);
                let visible = self.visible_from(start..cut, &mut item).end;
                let width = if visible == start {
                    LayoutUnit::ZERO
                } else {
                    (rubies.level_position(level, visible) - origin).to_layout()
                };
                if width <= target {
                    last = Some(cut);
                } else if guard {
                    if (rubies.level_floor(level, visible) - origin).to_layout() > target {
                        break;
                    }
                    guard = false;
                }
                from = cut;
            }
            if let Some(last) = last {
                return last;
            }
        }
        first
            .or_else(|| self.clusters.first_emergency(ceiling..end))
            .unwrap_or(level.clusters.end)
    }

    /// Returns `part`'s share of `room`, as `part` is of `whole`.
    fn proportion(room: LayoutUnit, part: LayoutUnit, whole: LayoutUnit) -> LayoutUnit {
        if whole <= LayoutUnit::ZERO {
            LayoutUnit::ZERO
        } else {
            LayoutUnit::from_px_truncated(room.to_px() * part.to_px() / whole.to_px())
        }
    }

    /// Cuts each level in `cuts` for its share of `room` and returns the
    /// piece's width: the widest of `base_width` and the levels, reshaped.
    ///
    /// A level whose target, or whose cut, is the last trial's keeps the
    /// cut, or the width, the last trial found.
    fn split_width(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        cuts: &mut [LevelCut],
        room: LayoutUnit,
        whole: LayoutUnit,
        base_width: LayoutUnit,
    ) -> LayoutUnit {
        let rubies = self.stages.measured.ruby_columns();
        let mut width = base_width;
        for cut in cuts {
            let Some(level) = rubies.level(cut.level) else {
                continue;
            };
            let target = Self::proportion(room, cut.remaining, whole);
            if cut.target != Some(target) {
                let end = self.level_cut(level, cut, target);
                if end != cut.end {
                    cut.width = None;
                }
                cut.target = Some(target);
                cut.end = end;
            }
            let level_width = match cut.width {
                Some(level_width) => level_width,
                None => *cut
                    .width
                    .insert((self.reshaped_level(out, para, (level, cut), cut.end, false)).width),
            };
            width = width.max(level_width);
        }
        width
    }

    /// Reshapes the unsafe edges of `range` of `level`, an annotation line's
    /// start and end, as a line's edges are.
    fn annotation_edges(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        level: &RubyLevel,
        range: Range<ClusterId>,
    ) -> ReshapedPieces {
        if range.is_empty() {
            return ReshapedPieces::default();
        }
        let glyphs = &self.stages.shaped.glyphs;
        let start_unsafe =
            range.start > level.clusters.start && glyphs.word(range.start).is_unsafe_to_break();
        let end_unsafe =
            range.end < level.clusters.end && glyphs.word(range.end).is_unsafe_to_break();
        let mark = out.edges.mark();
        let (window, head) = if start_unsafe {
            self.start_window(out, para, range.start, false).unzip()
        } else {
            (None, None)
        };
        let mut head = LineHead { mark, shaped: head };
        let ends = EdgeEnd {
            whole: ShapingEdges {
                line_start: start_unsafe,
                trim_start: false,
                trim_end: false,
            },
            reshaped: end_unsafe,
            trims: false,
            seam: InlineLayoutUnit::ZERO,
            whole_at_window_end: true,
        };
        self.edge_pieces(out, para, range, window, &mut head, ends)
    }

    /// Measures the part of `level` from where `cut` resumes it to `end`,
    /// with its edges reshaped, keeping the edge shapes where `retain`.
    /// Without `retain` the edge tables are taken back.
    fn reshaped_level(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        (level, cut): (&RubyLevel, &LevelCut),
        end: ClusterId,
        retain: bool,
    ) -> PartLevel {
        let rubies = self.stages.measured.ruby_columns();
        let clusters = cut.from..end;
        let visible = self.visible_annotation(level, clusters.clone(), cut.item);
        let visible_end = visible.end;
        let mark = out.edges.mark();
        let mut width = self.visible_width(level, visible.clone());
        let from = out.edges.shapes.next_id();
        let pieces = self.annotation_edges(out, para, level, visible);
        for piece in pieces.iter() {
            let range = piece.shape.clusters();
            let actual = out.edges.advance_sum(piece.shape.entries(range.clone()));
            let original =
                rubies.level_position(level, range.end) - rubies.level_position(level, range.start);
            width = width + (actual - original).to_layout();
            if retain {
                out.edges.shapes.push_bounded(
                    piece.shape,
                    "annotation edge shapes fit the cluster id space",
                );
            }
        }
        if !retain {
            out.edges.rewind(mark);
        }
        PartLevel {
            level: cut.level,
            clusters,
            width,
            visible_end,
            shapes: from..out.edges.shapes.next_id(),
            item: cut.item,
        }
    }

    /// Keeps `piece` for the line being fitted, with each level as `cuts`
    /// cut it, its edges reshaped and kept.
    fn record_ruby(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        piece: ProposedPiece,
        cuts: &[LevelCut],
    ) {
        let rubies = self.stages.measured.ruby_columns();
        let line = out.lines.next_id();
        // The tables are taken out while the levels' edges are reshaped,
        // which reads none of them.
        let mut ruby = out
            .ruby
            .take()
            .unwrap_or_else(|| Box::new(RubyLines::default()));
        let levels_start = ruby.levels.next_id();
        for cut in cuts {
            if let Some(level) = rubies.level(cut.level) {
                let _ =
                    (ruby.levels).push(self.reshaped_level(out, para, (level, cut), cut.end, true));
            }
        }
        ruby.push_part(ColumnPart {
            line,
            column: piece.column,
            base: piece.base,
            levels: levels_start..ruby.levels.next_id(),
            base_width: piece.base_width,
            width: piece.width,
            overhang: (piece.before, LayoutUnit::ZERO),
        });
        out.ruby = Some(ruby);
    }

    /// Splits the last breakable column on the line whose furthest boundary
    /// that fits is `furthest`, and returns the line that ends at the split.
    ///
    /// Columns are tried from the last that starts on the line back to the
    /// one the line continues. A column the line holds whole, followed by
    /// an opportunity, is passed over. Out of line, so that plain fitting
    /// stays small.
    #[inline(never)]
    pub(crate) fn split_ruby(
        &mut self,
        out: &mut Lines,
        fitting: &mut Fitting<'_, '_>,
        furthest: ClusterId,
    ) -> Option<Fitted> {
        if !fitting.line.para.breakable_ruby {
            return None;
        }
        let rubies = self.stages.measured.ruby_columns();
        let range =
            self.line_columns(fitting.line.start..ClusterId::new(furthest.get().saturating_add(1)));
        // A continuation's base starts before the line; include that root.
        let low = self
            .continued
            .as_ref()
            .map_or(range.start.get(), |continued| {
                range.start.get().min(continued.column.get())
            });
        // Of the columns that start before the line, only those open across
        // its start can split on it: the last of them and its ancestors.
        let open = successors(
            range.start.get().checked_sub(1).map(RubyColumnId::new),
            |id| rubies.get(*id).and_then(|column| column.parent),
        )
        .take_while(|id| id.get() >= low);
        let line = (range.start.get()..range.end.get())
            .rev()
            .map(RubyColumnId::new);
        let mut cuts = mem::take(&mut self.scratch.level_cuts);
        let mut fitted = None;
        for id in line.chain(open) {
            work::step();
            let Some(column) = rubies.get(id).filter(|column| column.may_break) else {
                continue;
            };
            fitted = self.split_column(out, fitting, furthest, id, column, &mut cuts);
            if fitted.is_some() {
                break;
            }
        }
        self.scratch.level_cuts = cuts;
        fitted
    }

    /// Splits column `id` where the line, whose furthest boundary that fits
    /// is `furthest`, ends inside it, and returns that line. `None` where the
    /// column stays whole.
    fn split_column(
        &mut self,
        out: &mut Lines,
        fitting: &mut Fitting<'_, '_>,
        furthest: ClusterId,
        id: RubyColumnId,
        column: &RubyColumn,
        cuts: &mut Vec<LevelCut>,
    ) -> Option<Fitted> {
        let rubies = self.stages.measured.ruby_columns();
        let start = column.base.start.max(fitting.line.start);
        if start >= column.base.end {
            return None;
        }
        let resume = Self::previous_piece(out, Some(id), start).map(|piece| piece.levels.clone());
        self.start_levels(out, column, resume.as_ref(), cuts);
        // A line that starts inside the column knows its first item.
        let near = if start == fitting.line.start {
            fitting.line.first_item
        } else {
            column.open
        };
        if self.monolithic_rest(column, (start, near), cuts) {
            return None;
        }
        if self.ruby_break_within == RubyBreakWithin::AllLevels
            && cuts.iter().any(|cut| {
                rubies.level(cut.level).is_some_and(|level| {
                    cut.from < level.clusters.end
                        && self.level_cut(level, cut, LayoutUnit::ZERO) == level.clusters.end
                })
            })
        {
            return None;
        }
        let prefix = &self.stages.measured.prefix;
        let used = (prefix.get(start) - prefix.get(fitting.line.start) + fitting.shift.at(start))
            .to_layout()
            + fitting.line.start_cost;
        let available = (fitting.line.room - used).max(LayoutUnit::ZERO);
        // The rest the line continues was sized before it was fitted.
        let continued = self
            .continued
            .as_ref()
            .filter(|continued| continued.column == id && start == fitting.line.start)
            .and_then(|continued| continued.rest);
        let (whole, base) = match continued {
            Some(rest) => (rest.width, rest.base),
            None => self.rest_width(out, fitting.line.para, start..column.base.end, cuts),
        };
        let before = if start > fitting.line.start {
            column.overhang.0.min((whole - base).max(LayoutUnit::ZERO))
        } else {
            LayoutUnit::ZERO
        };
        let column_end = self.column_end(column);
        if whole <= available + before
            && (column_end == fitting.line.para.end
                || self
                    .clusters
                    .attrs(ClusterId::new(column_end.get() - 1))
                    .is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER))
                || self
                    .opportunity(column_end..furthest, false, false)
                    .is_some())
        {
            return None;
        }
        let target = Self::proportion(available + before, base, whole);
        let furthest = self.base_furthest(fitting.line.para, start..column.base.end, target);
        let mut ceiling = column.base.end;
        let mut emergency = false;
        let mut item = self.stages.analysis.item_clusters.cursor(column.open);
        while let Some(cut) = self.base_cut(
            id,
            start..column.base.end,
            (target, furthest),
            ceiling,
            &mut emergency,
            &mut item,
        ) {
            if cut.at <= start {
                break;
            }
            let room = if whole <= available + before {
                Self::proportion(cut.width, whole, base)
            } else {
                available + before
            };
            let mut candidate = self.candidate(out, fitting, cut.at, false);
            if cut.hyphen > LayoutUnit::ZERO {
                candidate.cost.end = candidate.cost.end + cut.hyphen;
                candidate.cost.flags.insert(LineEdgeFlags::HYPHEN);
            }
            let base_width =
                cut.width + self.base_delta(&candidate.pieces, start..cut.at) + cut.hyphen;
            let width = self.split_width(out, fitting.line.para, cuts, room, whole, base_width);
            let extra = width - base_width - before;
            candidate.content = candidate.content + extra;
            candidate.full = candidate.full + extra;
            let piece = ProposedPiece {
                column: id,
                base: start..cut.at,
                width,
                base_width,
                before,
            };
            if let Some(fitted) = settle(fitting, &candidate) {
                self.record_ruby(out, fitting.line.para, piece, cuts);
                return Some(fitted);
            }
            if start == fitting.line.start {
                self.record_ruby(out, fitting.line.para, piece, cuts);
                return Some(overflowing(fitting, &candidate));
            }
            // The combined line does not fit: retry at a shorter cut.
            ceiling = ClusterId::new(cut.at.get().saturating_sub(1));
        }
        None
    }

    /// Records the rest of the column the line continues, where the line
    /// holds all of it and the fit split it no further.
    ///
    /// Out of line, so that plain fitting stays small.
    #[inline(never)]
    pub(crate) fn finish_ruby(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        fitted: &mut Fitted,
    ) {
        let Some(continued) = self.continued.take() else {
            return;
        };
        // The line kept a piece of the column where the fit split it.
        if out.ruby.as_ref().is_some_and(|ruby| {
            ruby.recent_line_parts(out.lines.next_id())
                .iter()
                .any(|piece| piece.column == continued.column)
        }) {
            return;
        }
        let measured = self.stages.measured;
        let Some(column) = measured.ruby_columns().get(continued.column) else {
            return;
        };
        let Some(rest) = continued.rest else {
            return;
        };
        if fitted.end < self.column_end(column) {
            return;
        }
        let mut cuts = mem::take(&mut self.scratch.level_cuts);
        self.start_levels(out, column, Some(&continued.levels), &mut cuts);
        let base_width = rest.base + self.base_delta(&fitted.pieces, start..column.base.end);
        let piece = ProposedPiece {
            column: continued.column,
            base: start..column.base.end,
            width: rest.width,
            base_width,
            before: LayoutUnit::ZERO,
        };
        self.record_ruby(out, para, piece, &cuts);
        self.scratch.level_cuts = cuts;
    }
}
