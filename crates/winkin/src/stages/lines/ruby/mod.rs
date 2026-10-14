//! The parts of ruby columns split across lines, with their annotation
//! levels. `split` splits a breakable column across lines.

mod split;
use super::{EdgeShapeId, LineId, Lines};
use crate::data::{Id, Table, define_id, heap_bytes};
use crate::stages::analysis::ClusterId;
use crate::stages::content::ItemId;
use crate::stages::measure::{RubyColumnId, RubyColumns, RubyLevelId};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;
use core::ops::Range;
pub(super) use split::Continued;

define_id! {
    /// Names a part of a split ruby column in [`RubyLines`].
    struct ColumnPartId(u32);
}

define_id! {
    /// Names one level of a part in [`RubyLines`].
    pub(crate) struct PartLevelId(u32);
}

/// The share of one annotation level that a column part sets on its line.
#[derive(Clone, Debug)]
pub(crate) struct PartLevel {
    /// The level in the measure stage's table.
    pub(super) level: RubyLevelId,
    /// Its clusters on this line, trailing collapsed white space included.
    pub(crate) clusters: Range<ClusterId>,
    /// Its width with reshaped edges.
    pub(crate) width: LayoutUnit,
    /// The end of its clusters without trailing collapsed white space.
    pub(crate) visible_end: ClusterId,
    /// Its reshaped annotation edges in the edge tables.
    pub(super) shapes: Range<EdgeShapeId>,
    /// The first item at its clusters' start.
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
    /// Its levels, in the column's level order.
    pub(crate) levels: Range<PartLevelId>,
    /// Its base width on this line, with reshaped edges and any hyphen.
    pub(crate) base_width: LayoutUnit,
    /// Its width: the base's or the widest level's.
    pub(crate) width: LayoutUnit,
    /// Its overhang over the text before and after it.
    pub(crate) overhang: (LayoutUnit, LayoutUnit),
}

/// A position in the ruby tables to rewind to.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct RubyMark {
    parts: ColumnPartId,
    levels: PartLevelId,
}

/// Every part of a split ruby column, in line order, and their levels.
///
/// [`Lines`] boxes it, made at the first split. A line has at most one part
/// of a column; a split column's part comes before a continued column's.
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

    /// Returns the tables' current position.
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

    /// Takes back the parts of `line`, the last line written, and any later
    /// line, walking back from the table's end.
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

    /// Adds `part`, set by the line being written.
    pub(super) fn push_part(&mut self, part: ColumnPart) {
        let _ = self.parts.push(part);
    }

    /// Returns the parts of `line`, in write order.
    pub(crate) fn line_parts(&self, line: LineId) -> &[ColumnPart] {
        let from = self.first_part_id(line);
        let rest = self
            .parts
            .get_slice(from..self.parts.next_id())
            .unwrap_or_default();
        let count = rest.iter().take_while(|part| part.line == line).count();
        rest.get(..count).unwrap_or_default()
    }

    /// Returns the parts of `line`, one of the last lines written, in write
    /// order.
    ///
    /// Walks back from the table's end, with no search; the breaker asks this of
    /// the line it fits and the line before.
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

    /// Returns the first annotation edge shape the line just fitted, `line`,
    /// keeps.
    ///
    /// Out of line, so that plain line records inline their construction.
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

    /// Returns the parts that can contain any of `clusters`, in write order.
    ///
    /// A part is on the line of its base's start, and its levels follow its
    /// base. The search runs from the line of the outermost column's base start
    /// to the line of the last cluster.
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

    /// Returns the line whose part sets annotation cluster `cluster`, or, with
    /// `upstream`, whose part's level ends at `cluster`.
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

/// One level's cut for the part being fitted, kept in the breaker's scratch
/// across a split's trials.
#[derive(Copy, Clone, Debug)]
pub(super) struct LevelCut {
    /// The level in the measure stage's table.
    pub(super) level: RubyLevelId,
    /// Where the level resumes on the line.
    pub(super) from: ClusterId,
    /// The first item at `from`.
    pub(super) item: ItemId,
    /// Its unshaped width from `from` to its end.
    pub(super) remaining: LayoutUnit,
    /// The target the last trial cut it for.
    pub(super) target: Option<LayoutUnit>,
    /// Where the last trial cut it; its end before any trial.
    pub(super) end: ClusterId,
    /// Its reshaped width from `from` to `end`, once a trial measured it.
    pub(super) width: Option<LayoutUnit>,
}

/// The shift a continued column's rest puts on prefix positions from the
/// column's end on.
///
/// The prefix counts the whole column; the line holds only its rest.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct ContinuationShift {
    /// The column's end, where the shift starts.
    pub(super) end: Option<ClusterId>,
    /// The rest's width less the prefix's width for it.
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
