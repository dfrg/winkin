//! The parts of ruby columns split across lines, and their annotation
//! levels, which advance independently of each other.
use super::{EdgeShapeId, LineId, Lines};
use crate::data::{Id, Table, define_id, heap_bytes};
use crate::stages::analysis::ClusterId;
use crate::stages::content::ItemId;
use crate::stages::measure::{RubyColumnId, RubyColumns, RubyLevelId};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;
use core::ops::Range;

define_id! {
    /// Names a part of a split ruby column in [`RubyLines`]' table of them.
    struct ColumnPartId(u32);
}

define_id! {
    /// Names one level of a part in [`RubyLines`]' table of them.
    pub(crate) struct PartLevelId(u32);
}

/// The share of one annotation level that a column part sets on its line.
#[derive(Clone, Debug)]
pub(crate) struct PartLevel {
    /// The level in the measure stage's table.
    pub(super) level: RubyLevelId,
    /// Its clusters on this line, collapsed white space at the end included.
    pub(crate) clusters: Range<ClusterId>,
    /// Its width with its edges reshaped.
    pub(crate) width: LayoutUnit,
    /// Where its clusters end once trailing collapsed white space is left out.
    pub(crate) visible_end: ClusterId,
    /// The reshaped annotation edges it keeps in the edge tables.
    pub(super) shapes: Range<EdgeShapeId>,
    /// The first item at its clusters' start, where a walk over them starts.
    pub(crate) item: ItemId,
}

/// The part of a split ruby column that one line sets.
#[derive(Clone, Debug)]
pub(crate) struct ColumnPart {
    /// The line that sets it.
    pub(crate) line: LineId,
    /// The column in the measure stage's table.
    pub(crate) column: RubyColumnId,
    /// Its base clusters on this line.
    pub(crate) base: Range<ClusterId>,
    /// Its levels, one row each, in the column's level order.
    pub(crate) levels: Range<PartLevelId>,
    /// Its base's width on this line, with reshaped edges and any hyphen.
    pub(crate) base_width: LayoutUnit,
    /// The width the part takes: its base's or its widest level's.
    pub(crate) width: LayoutUnit,
    /// How far it overhangs the text before it and after it.
    pub(crate) overhang: (LayoutUnit, LayoutUnit),
}

/// Where the ruby tables stand, which a rewind takes them back to.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct RubyMark {
    parts: ColumnPartId,
    levels: PartLevelId,
}

/// Every part of a split ruby column, in line order, and their levels.
///
/// The breaker writes each line's parts as it fits the line. A line holds
/// at most one part of a column. Parts of one line may come in any column
/// order: a split column's part comes before a continued column's.
#[derive(Default)]
pub(crate) struct RubyLines {
    parts: Table<ColumnPartId, ColumnPart>,
    pub(crate) levels: Table<PartLevelId, PartLevel>,
}

impl RubyLines {
    /// Empties both tables, keeping their allocations.
    pub(super) fn clear(&mut self) {
        self.parts.clear();
        self.levels.clear();
    }

    /// Returns where the tables stand, to rewind to later.
    pub(super) fn mark(&self) -> RubyMark {
        RubyMark {
            parts: self.parts.next_id(),
            levels: self.levels.next_id(),
        }
    }

    /// Takes back every part and level written since `mark`.
    pub(super) fn rewind(&mut self, mark: RubyMark) {
        self.parts.truncate(mark.parts);
        self.levels.truncate(mark.levels);
    }

    /// Takes back the parts of `line`, the last line written, and of any
    /// line after it.
    ///
    /// They are the table's tail, walked back from its end.
    pub(super) fn truncate(&mut self, line: LineId) {
        let parts = self.parts.as_slice();
        let mut count = parts.len();
        while let Some(last) = count.checked_sub(1)
            && parts.get(last).is_some_and(|part| part.line >= line)
        {
            work::step();
            count = last;
        }
        let count = ColumnPartId::new(count);
        let levels = self
            .parts
            .get(count)
            .map_or(self.levels.next_id(), |part| part.levels.start);
        self.parts.truncate(count);
        self.levels.truncate(levels);
    }

    /// Returns the first part on `line` or after it, by binary search.
    fn first_part_id(&self, line: LineId) -> ColumnPartId {
        work::seek();
        let parts = self.parts.as_slice();
        ColumnPartId::new(parts.partition_point(|part| part.line < line))
    }

    /// Returns every part, in line order.
    #[cfg(test)]
    pub(crate) fn parts(&self) -> &[ColumnPart] {
        self.parts.as_slice()
    }

    /// Adds `part`, which the line being written sets.
    pub(super) fn push_part(&mut self, part: ColumnPart) {
        let _ = self.parts.push(part);
    }

    /// Returns the parts of one line, in write order.
    ///
    /// It seeks the line's first part, and walks its few parts from there.
    pub(crate) fn line_parts(&self, line: LineId) -> &[ColumnPart] {
        let from = self.first_part_id(line);
        let rest = self
            .parts
            .get_slice(from..self.parts.next_id())
            .unwrap_or_default();
        let count = rest.iter().take_while(|part| part.line == line).count();
        rest.get(..count).unwrap_or_default()
    }

    /// Returns the parts of `line`, one of the last lines written, in
    /// write order.
    ///
    /// They are walked back from the table's end, past the parts of any
    /// line after it, with no search. The breaker asks this of the line
    /// it fits and the line before.
    pub(super) fn recent_line_parts(&self, line: LineId) -> &[ColumnPart] {
        let parts = self.parts.as_slice();
        let mut end = parts.len();
        while let Some(last) = end.checked_sub(1)
            && parts.get(last).is_some_and(|part| part.line > line)
        {
            work::step();
            end = last;
        }
        let mut start = end;
        while let Some(last) = start.checked_sub(1)
            && parts.get(last).is_some_and(|part| part.line == line)
        {
            work::step();
            start = last;
        }
        parts.get(start..end).unwrap_or_default()
    }

    /// Returns the first annotation edge shape that `line`, the line just
    /// fitted, keeps.
    ///
    /// It stays out of line so that plain line records inline their
    /// construction.
    #[inline(never)]
    pub(super) fn line_first_shape(&self, line: LineId) -> Option<EdgeShapeId> {
        self.recent_line_parts(line)
            .iter()
            .flat_map(|part| {
                self.levels
                    .get_slice(part.levels.clone())
                    .unwrap_or_default()
            })
            .map(|level| level.shapes.start)
            .min()
    }

    /// Returns the parts that can hold any of `clusters`, in write order.
    ///
    /// A part is on the line holding its base's start, and its levels
    /// follow its base. So the lines searched run from the one holding the
    /// outermost column's base start to the one holding the last cluster.
    pub(crate) fn around(
        &self,
        lines: &Lines,
        columns: &RubyColumns,
        clusters: Range<ClusterId>,
    ) -> &[ColumnPart] {
        let Some(last) = clusters.end.get().checked_sub(1) else {
            return &[];
        };
        let first = columns.outermost_start(clusters.start);
        let to = lines.at(ClusterId::new(last));
        let from = self.first_part_id(lines.at(first));
        let to = self.first_part_id(LineId::new(to.get().saturating_add(1)));
        self.parts.get_slice(from..to.max(from)).unwrap_or_default()
    }

    /// Returns the line whose part sets annotation cluster `cluster`.
    ///
    /// With `upstream`, returns the line whose part's level ends at
    /// `cluster`.
    pub(crate) fn annotation_line(
        &self,
        lines: &Lines,
        columns: &RubyColumns,
        cluster: ClusterId,
        upstream: bool,
    ) -> Option<LineId> {
        let held = if upstream {
            ClusterId::new(cluster.get().checked_sub(1)?)
        } else {
            cluster
        };
        let around = self.around(lines, columns, held..ClusterId::new(held.get() + 1));
        around.iter().find_map(|part| {
            self.levels
                .get_slice(part.levels.clone())
                .unwrap_or_default()
                .iter()
                .any(|level| {
                    if upstream {
                        level.clusters.start < cluster && cluster <= level.clusters.end
                    } else {
                        level.clusters.contains(&cluster)
                    }
                })
                .then_some(part.line)
        })
    }

    /// Returns `part`'s row for level `id`.
    pub(crate) fn level(&self, part: &ColumnPart, id: RubyLevelId) -> Option<&PartLevel> {
        self.levels
            .get_slice(part.levels.clone())?
            .iter()
            .find(|level| level.level == id)
    }
}

heap_bytes! { RubyLines { parts, levels } }

/// One level's cut for the part being fitted, which the breaker keeps in
/// its scratch across a split's trials.
#[derive(Copy, Clone, Debug)]
pub(super) struct LevelCut {
    /// The level in the measure stage's table.
    pub(super) level: RubyLevelId,
    /// Where the level resumes on the line.
    pub(super) from: ClusterId,
    /// The first item at `from`, where a walk over the level's part starts.
    pub(super) item: ItemId,
    /// Its unshaped width from `from` to its end.
    pub(super) remaining: LayoutUnit,
    /// The target the last trial cut it for.
    pub(super) target: Option<LayoutUnit>,
    /// Where the last trial cut it: its end before any trial.
    pub(super) end: ClusterId,
    /// Its reshaped width from `from` to `end`, once a trial measured it.
    pub(super) width: Option<LayoutUnit>,
}

/// How the rest of a continued column shifts prefix positions at and past
/// the column's end.
///
/// The prefix counts the column whole. The line holds only its rest, whose
/// width differs.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct ContinuationShift {
    /// The column's end, from which the shift applies.
    pub(super) end: Option<ClusterId>,
    /// The shift: the rest's width less what the prefix gives it.
    pub(super) delta: InlineLayoutUnit,
}

impl ContinuationShift {
    /// Returns the shift at boundary `end`.
    #[inline]
    pub(super) fn at(self, end: ClusterId) -> InlineLayoutUnit {
        if self.end.is_some_and(|boundary| end >= boundary) {
            self.delta
        } else {
            InlineLayoutUnit::ZERO
        }
    }
}
