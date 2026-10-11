//! The ruby columns and their levels, as the measure stage stores them.

use crate::config::RubyBreakWithin;
use crate::data::IdRange;
use crate::stages::content::{ItemFlags, MAX_RUBY_DEPTH};
use crate::stages::shape::{ClusterGlyphs, ShapedText};
use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

use crate::data::{Id, Table, define_id};
use crate::stages::analysis::{Analysis, ClusterId, Clusters};
use crate::stages::content::{Content, ItemId, ItemKind, NodeId};
use crate::style::{EmphasisSide, RubyPosition};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

use super::{Extent, RubyColumnId, RubyLevelId, ScanPrefix, TabReach};

define_id! { pub(super) struct ColumnRoomId(u32); }

define_id! {
    /// Names a level's cumulative width at one of its boundaries, and the
    /// floor there, in [`RubyColumns`]' tables of them.
    pub(super) struct StepId(u32);
}

/// A descendant's measured contribution at its closing boundary. An entirely
/// empty descendant contributes opening room instead of a glyph advance.
#[derive(Copy, Clone, Debug)]
pub(super) struct ColumnRoom {
    pub(super) at: ClusterId,
    pub(super) extra: LayoutUnit,
    pub(super) empty: bool,
}

/// The ruby columns, in text order, and their annotation levels, each
/// column's together: what the breaker makes room for, line
/// layout places, and justification reads of a base, each asking of one
/// cluster or one line.
///
/// Columns are in preorder with nondecreasing base starts. Parent bases
/// include descendants, so a seek finds the nearest column and follows its
/// parent chain to resolve ownership or the scope of an opportunity query.
/// Each column's own levels are contiguous, in closing order.
pub(crate) struct RubyColumns {
    pub(super) columns: Table<RubyColumnId, RubyColumn>,
    pub(super) levels: Table<RubyLevelId, RubyLevel>,
    /// Each level's cumulative width at each of its boundaries, from its
    /// start to its end, where its column may split.
    pub(super) steps: Table<StepId, InlineLayoutUnit>,
    /// The least of each level's cumulative widths from each of its
    /// boundaries to its end, beside `steps`: written whole as the level
    /// closes.
    pub(super) floors: Table<StepId, InlineLayoutUnit>,
    pub(super) rooms: Table<ColumnRoomId, ColumnRoom>,
    /// The tabs the columns' overhang reaches into, in text order.
    pub(super) tab_reaches: Vec<TabReach>,
}

impl RubyColumns {
    pub(super) const fn new() -> Self {
        Self {
            columns: Table::new(),
            levels: Table::new(),
            steps: Table::new(),
            floors: Table::new(),
            rooms: Table::new(),
            tab_reaches: Vec::new(),
        }
    }

    /// Empties every table, keeping their allocations.
    pub(super) fn clear(&mut self) {
        self.columns.clear();
        self.levels.clear();
        self.steps.clear();
        self.floors.clear();
        self.rooms.clear();
        self.tab_reaches.clear();
    }

    /// Returns how far a column's overhang reaches into the tab at `tab`, if
    /// it does.
    pub(crate) fn tab_reach(&self, tab: ClusterId) -> Option<TabReach> {
        if self.tab_reaches.is_empty() {
            return None;
        }
        work::seek();
        let found = self
            .tab_reaches
            .binary_search_by_key(&tab, |reach| reach.tab);
        found.ok().and_then(|at| self.tab_reaches.get(at)).copied()
    }

    /// Returns `table`'s entries from `level`'s first on: its entry at each
    /// of its boundaries, then the next levels'. Empty where its column
    /// may not split.
    fn level_steps<'t>(
        level: &RubyLevel,
        table: &'t Table<StepId, InlineLayoutUnit>,
    ) -> &'t [InlineLayoutUnit] {
        level
            .steps
            .and_then(|first| table.as_slice().get(first.get()..))
            .unwrap_or_default()
    }

    /// A level's exact cumulative width at a boundary, including its box edges.
    pub(crate) fn level_position(&self, level: &RubyLevel, at: ClusterId) -> InlineLayoutUnit {
        let offset = at.get().saturating_sub(level.clusters.start.get());
        let steps = Self::level_steps(level, &self.steps);
        steps.get(offset).copied().unwrap_or_default()
    }

    /// Returns the least of a level's cumulative widths at the boundaries
    /// from `from` to its end, both included.
    ///
    /// A cut at or past `from` is never narrower than this from any start.
    /// The floors never fall from one boundary to the next.
    pub(crate) fn level_floor(&self, level: &RubyLevel, from: ClusterId) -> InlineLayoutUnit {
        let offset = from.get().saturating_sub(level.clusters.start.get());
        if offset
            > level
                .clusters
                .end
                .get()
                .saturating_sub(level.clusters.start.get())
        {
            return InlineLayoutUnit::ZERO;
        }
        let floors = Self::level_steps(level, &self.floors);
        floors.get(offset).copied().unwrap_or_default()
    }

    /// Returns the first boundary of `level` from `from` on whose floor is
    /// over `limit`, by halving: no cut ending there or past it is within
    /// `limit`. The boundary past the level's end where none is.
    pub(crate) fn floor_above(
        &self,
        level: &RubyLevel,
        from: ClusterId,
        limit: InlineLayoutUnit,
    ) -> ClusterId {
        let past = ClusterId::new(level.clusters.end.get() + 1);
        let start = level.clusters.start.get();
        let offset = from.get().saturating_sub(start);
        let last = level.clusters.end.get().saturating_sub(start);
        let floors = Self::level_steps(level, &self.floors);
        let floors = floors.get(offset..=last).unwrap_or_default();
        if floors.is_empty() {
            return past;
        }
        work::seek();
        ClusterId::new(start + offset + floors.partition_point(|&floor| floor <= limit))
    }

    /// Writes the floors of the level whose steps end the table: from the
    /// end of the floors written so far.
    pub(super) fn close_floors(&mut self) {
        let from = self.floors.len();
        let steps = self.steps.as_slice().get(from..).unwrap_or_default();
        let _ = self.floors.extend_from_slice(steps);
        let mut least = None;
        for floor in (self.floors.as_mut_slice().get_mut(from..))
            .unwrap_or_default()
            .iter_mut()
            .rev()
        {
            let low = least.map_or(*floor, |least: InlineLayoutUnit| least.min(*floor));
            *floor = low;
            least = Some(low);
        }
    }

    /// The zero-room first, last and widest pieces for intrinsic sizing.
    /// Each annotation cursor advances once per piece. The measurement
    /// walk reuses the cursor scratch across columns.
    pub(super) fn min_pieces(
        &self,
        id: RubyColumnId,
        input: &super::MeasureInput<'_>,
        shaped: &ShapedText,
        prefix: ScanPrefix<'_>,
        starts: &mut Vec<ClusterId>,
    ) -> Option<(LayoutUnit, LayoutUnit, LayoutUnit)> {
        let column = self.get(id)?;
        if !column.may_break {
            return None;
        }
        let (content, analysis, mode) = (input.content, input.analysis, input.ruby_break_within);
        let clusters = &analysis.clusters;
        let levels = self.levels(column);
        starts.clear();
        starts.extend(levels.iter().map(|level| level.clusters.start));
        let next = |start: ClusterId, end: ClusterId, parent: Option<RubyColumnId>| {
            if start >= end {
                return end;
            }
            let limit = ClusterId::new(end.get().saturating_sub(1));
            if parent.is_none() {
                clusters
                    .first_opportunity(start..limit)
                    .or_else(|| clusters.first_emergency(start..limit))
                    .unwrap_or(end)
            } else {
                let near = Cell::new(id);
                self.opportunity(clusters, start..limit, parent, (false, false), &near)
                    .or_else(|| {
                        self.opportunity(clusters, start..limit, parent, (false, true), &near)
                    })
                    .unwrap_or(end)
            }
        };
        let level_width = |level: &RubyLevel, range: Range<ClusterId>| {
            let mut end = range.end;
            while end > range.start && clusters.is_breaking_space(ClusterId::new(end.get() - 1)) {
                end = ClusterId::new(end.get() - 1);
            }
            (self.level_position(level, end) - self.level_position(level, range.start)).to_layout()
        };
        let mut start = column.base.start;
        let mut ordinal = 0;
        let mut first = LayoutUnit::ZERO;
        let mut last;
        let mut widest = LayoutUnit::ZERO;
        loop {
            let blocked = mode == RubyBreakWithin::AllLevels
                && levels.iter().zip(starts.iter()).any(|(level, &from)| {
                    from < level.clusters.end
                        && next(from, level.clusters.end, None) == level.clusters.end
                });
            let monolithic = blocked
                || is_monolithic(
                    content,
                    analysis,
                    shaped,
                    (column.open, start..column.base.end),
                    (levels.iter().zip(starts.iter()))
                        .map(|(level, &from)| (level.open, from..level.clusters.end)),
                );
            let end = if monolithic {
                column.base.end
            } else {
                next(start, column.base.end, Some(id))
            };
            let mut visible = end;
            while visible > start && clusters.hangs(ClusterId::new(visible.get() - 1)) {
                visible = ClusterId::new(visible.get() - 1);
            }
            let base =
                prefix.get(visible).unwrap_or_default() - prefix.get(start).unwrap_or_default();
            let mut width = base.to_layout();
            for (level, from) in levels.iter().zip(starts.iter_mut()) {
                let past = if end == column.base.end {
                    level.clusters.end
                } else {
                    next(*from, level.clusters.end, None)
                };
                width = width.max(level_width(level, *from..past));
                *from = past;
            }
            if ordinal == 0 {
                first = width;
            }
            last = width;
            widest = widest.max(width);
            if end >= column.base.end {
                break;
            }
            start = end;
            ordinal += 1;
        }
        (ordinal > 0).then_some((first, last, widest))
    }

    /// Column `id`, or `None` past the last.
    #[inline]
    pub(crate) fn get(&self, id: RubyColumnId) -> Option<&RubyColumn> {
        self.columns.get(id)
    }

    /// Annotation level `id`, or `None` past the last.
    pub(crate) fn level(&self, id: RubyLevelId) -> Option<&RubyLevel> {
        self.levels.get(id)
    }

    /// The levels of `column`, in order: the innermost first on each side.
    pub(crate) fn levels(&self, column: &RubyColumn) -> &[RubyLevel] {
        self.levels
            .get_slice(column.levels.clone())
            .unwrap_or_default()
    }

    /// The columns `columns` names, in text order: empty where they are not
    /// the table's.
    pub(super) fn slice(&self, columns: Range<RubyColumnId>) -> &[RubyColumn] {
        self.columns.get_slice(columns).unwrap_or_default()
    }

    /// Returns where `column` ends: past its last level, or its base's end
    /// where it has no level.
    fn column_end(&self, column: &RubyColumn) -> ClusterId {
        self.levels(column)
            .last()
            .map_or(column.base.end, |level| level.clusters.end)
    }

    /// Whether the content has no column.
    pub(crate) fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    /// How many levels the columns have together.
    #[cfg(test)]
    pub(super) fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// Every column with its id, in text order, which the tests check.
    #[cfg(test)]
    pub(super) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (RubyColumnId, &RubyColumn)> + ExactSizeIterator {
        self.columns.iter()
    }

    /// The columns whose bases start among `clusters`, a line's: the columns
    /// of the line, walked to from column `near`.
    ///
    /// `near` is any column, or one past the last. A walk that holds a
    /// column near the line passes it, and pays a step a column between.
    pub(crate) fn line_columns(
        &self,
        near: RubyColumnId,
        clusters: Range<ClusterId>,
    ) -> Range<RubyColumnId> {
        let from = self.walk_to(near, |column| column.base.start < clusters.start);
        let to = self.walk_to(from, |column| column.base.start < clusters.end);
        from..to.max(from)
    }

    /// Returns the first column `before` does not hold of, walked to from
    /// column `near` a column at a time, either way.
    ///
    /// `before` must hold of every column up to some point and of none
    /// after, as a halving's predicate does.
    fn walk_to(
        &self,
        near: RubyColumnId,
        mut before: impl FnMut(&RubyColumn) -> bool,
    ) -> RubyColumnId {
        let columns = self.columns.as_slice();
        let mut at = near.get().min(columns.len());
        while let Some(back) = at.checked_sub(1)
            && columns.get(back).is_some_and(|column| !before(column))
        {
            work::step();
            at = back;
        }
        while columns.get(at).is_some_and(&mut before) {
            work::step();
            at += 1;
        }
        RubyColumnId::new(at)
    }

    /// The last column whose base starts at or before `cluster`, walked to
    /// from column `near`: what [`nearest`](Self::nearest) finds, with no
    /// search.
    fn nearest_from(&self, near: RubyColumnId, cluster: ClusterId) -> Option<RubyColumnId> {
        let past = self.walk_to(near, |column| column.base.start <= cluster);
        past.get().checked_sub(1).map(RubyColumnId::new)
    }

    /// The last column whose base starts at or before `cluster`, by halving:
    /// the nearest descendant candidate, whose parent chain resolves ownership.
    ///
    /// Content with no ruby has no column, and asks without a search.
    fn nearest(&self, cluster: ClusterId) -> Option<RubyColumnId> {
        if self.columns.is_empty() {
            return None;
        }
        self.columns
            .last_where(|column| column.base.start <= cluster)
    }

    /// Returns the base start of the outermost column around `cluster`, by
    /// halving: `cluster` itself where no column starts at or before it.
    ///
    /// Every column whose base or levels hold `cluster` is the nearest
    /// column or one of its ancestors, so none starts before this.
    pub(crate) fn outermost_start(&self, cluster: ClusterId) -> ClusterId {
        let mut id = self.nearest(cluster);
        let mut start = cluster;
        while let Some(column) = id.and_then(|id| self.get(id)) {
            start = start.min(column.base.start);
            id = column.parent;
        }
        start
    }

    /// The column whose base holds `cluster`, by halving, or `None` where no
    /// base does.
    pub(super) fn base_containing(&self, cluster: ClusterId) -> Option<RubyColumnId> {
        let mut id = self.nearest(cluster)?;
        loop {
            let column = self.get(id)?;
            if cluster < self.column_end(column) {
                return column.base.contains(&cluster).then_some(id);
            }
            id = column.parent?;
        }
    }

    /// Returns the span of the whole column that `boundary` is strictly
    /// inside.
    ///
    /// Column edges stay eligible, also where a query starts or ends inside
    /// a column. The column is walked to from `near`, which is left at the
    /// column found, for the next query to start from.
    pub(crate) fn interior(
        &self,
        boundary: ClusterId,
        near: &Cell<RubyColumnId>,
    ) -> Option<Range<ClusterId>> {
        self.interior_span(boundary, None, near)
    }

    /// Returns the span of the column whose parent is `parent` that
    /// `boundary` is strictly inside: the columns `parent`'s base holds, or
    /// the outermost where `parent` is `None`.
    ///
    /// The nearest column is walked to from `near`, and left there.
    fn interior_span(
        &self,
        boundary: ClusterId,
        parent: Option<RubyColumnId>,
        near: &Cell<RubyColumnId>,
    ) -> Option<Range<ClusterId>> {
        let found = self.nearest_from(near.get(), boundary);
        near.set(found.unwrap_or(RubyColumnId::new(0)));
        let mut column = self.get(found?)?;
        while column.parent != parent {
            column = self.get(column.parent?)?;
        }
        let end = self.column_end(column);
        (column.base.start < boundary && boundary < end).then_some(column.base.start..end)
    }

    /// Finds the first opportunity in `range`, or the last where `back`,
    /// stepping over the columns `parent`'s base holds as indivisible.
    ///
    /// It jumps across such a column in one step rather than visiting its
    /// inner opportunities. `back` searches backwards, and `emergency` the
    /// emergency opportunities. The columns are walked to from `near`, a
    /// column near `range`, which is left near the last boundary found.
    pub(crate) fn opportunity(
        &self,
        clusters: &Clusters,
        mut range: Range<ClusterId>,
        parent: Option<RubyColumnId>,
        (back, emergency): (bool, bool),
        near: &Cell<RubyColumnId>,
    ) -> Option<ClusterId> {
        // A column with no nested column has none to step over.
        let flat = parent.is_some_and(|parent| !self.has_nested(parent));
        while range.start < range.end {
            let boundary = match (back, emergency) {
                (false, false) => clusters.first_opportunity(range.clone()),
                (true, false) => clusters.last_opportunity(range.clone()),
                (false, true) => clusters.first_emergency(range.clone()),
                (true, true) => clusters.last_emergency(range.clone()),
            }?;
            if flat {
                return Some(boundary);
            }
            match self.interior_span(boundary, parent, near) {
                None => return Some(boundary),
                Some(column) if back => range.end = column.start,
                Some(column) => range.start = ClusterId::new(column.end.get() - 1),
            }
        }
        None
    }

    /// Returns whether column `id` has a column nested in its base: in
    /// preorder, its first child follows it.
    fn has_nested(&self, id: RubyColumnId) -> bool {
        self.get(RubyColumnId::new(id.get() + 1))
            .is_some_and(|next| next.parent == Some(id))
    }

    /// Returns the room that nested columns with no clusters take at `at`.
    ///
    /// It is opening room at this boundary, or closing room at the text's
    /// end. It is no part of the neighbouring glyph's advance.
    pub(super) fn empty_room(&self, at: ClusterId) -> LayoutUnit {
        if self.rooms.is_empty() {
            return LayoutUnit::ZERO;
        }
        work::seek();
        let rooms = self.rooms.as_slice();
        let from = rooms.partition_point(|room| room.at < at);
        rooms[from..]
            .iter()
            .take_while(|room| room.at == at)
            .filter(|room| room.empty)
            .fold(LayoutUnit::ZERO, |sum, room| sum + room.extra)
    }

    /// The annotation furthest out on this side over a base cluster,
    /// including annotations belonging to its enclosing columns.
    pub(crate) fn outermost_annotation(
        &self,
        cluster: ClusterId,
        side: RubySide,
    ) -> Option<RubyLevelId> {
        let mut id = self.base_containing(cluster)?;
        let mut found = None;
        loop {
            let column = self.get(id)?;
            if let Some((level, _)) = self.outermost(column, side) {
                found = Some(level);
            }
            match column.parent {
                Some(parent) => id = parent,
                None => return found,
            }
        }
    }

    /// The outermost level of `column` on `side`, and how many of its
    /// levels are on that side, the outermost the last of them; `None`
    /// where it has none there. What an emphasis mark over its base is set
    /// past, as Chrome sets it (`SetTextEmphasisAnnotationMetrics`).
    fn outermost(&self, column: &RubyColumn, side: RubySide) -> Option<(RubyLevelId, usize)> {
        let mut count = 0;
        let mut outermost = None;
        for (at, level) in self.levels(column).iter().enumerate() {
            work::step();
            if level.side == side {
                count += 1;
                outermost = Some(RubyLevelId::new(column.levels.start.get() + at));
            }
        }
        outermost.map(|id| (id, count))
    }
}

/// The most glyphs a ruby base, or the rest of one, holds and still stays
/// whole, as Chrome keeps a column with a short base from breaking.
const BASE_WHOLE_GLYPHS: usize = 4;

/// The most glyphs an annotation level, or the rest of one, holds and still
/// stays whole beside a base that does.
const ANNOTATION_WHOLE_GLYPHS: usize = 8;

/// Returns whether a ruby column, or its rest from a cut, stays whole on a
/// line: its base holds four glyphs or fewer, and each level eight or fewer.
///
/// `base` is the base's clusters, with an item at or before their start.
/// Each of `levels` is a level's clusters, with an item at or before their
/// start. The glyphs of `shaped` are counted, each part only as far as its
/// limit. Inner annotations' glyphs are no glyphs of an enclosing base.
pub(crate) fn is_monolithic(
    content: &Content,
    analysis: &Analysis,
    shaped: &ShapedText,
    (from, base): (ItemId, Range<ClusterId>),
    levels: impl IntoIterator<Item = (ItemId, Range<ClusterId>)>,
) -> bool {
    let count = |from, range, limit, annotation| {
        glyph_count(content, analysis, shaped, (from, range), limit, annotation)
    };
    count(from, base, BASE_WHOLE_GLYPHS + 1, false) <= BASE_WHOLE_GLYPHS
        && levels.into_iter().all(|(from, range)| {
            count(from, range, ANNOTATION_WHOLE_GLYPHS + 1, true) <= ANNOTATION_WHOLE_GLYPHS
        })
}

/// Counts the glyphs of `range` in annotations (`annotation`) or out of
/// them, stopping at `limit`.
///
/// The items are walked from `from`, an item at or before `range`'s start.
fn glyph_count(
    content: &Content,
    analysis: &Analysis,
    shaped: &ShapedText,
    (from, range): (ItemId, Range<ClusterId>),
    limit: usize,
    annotation: bool,
) -> usize {
    let items = &analysis.item_clusters;
    let mut count = 0;
    let mut cursor = items.cursor(items.walk_to(from, range.start));
    for at in range.ids() {
        work::step();
        while cursor.end() <= at {
            items.step(&mut cursor);
        }
        if (content.items.get(cursor.id()))
            .is_none_or(|item| item.flags.contains(ItemFlags::ANNOTATION) != annotation)
        {
            continue;
        }
        count += match shaped.glyphs.glyphs(at) {
            ClusterGlyphs::None => 0,
            ClusterGlyphs::One(_) => 1,
            ClusterGlyphs::Many(glyphs) => glyphs.len(),
        };
        if count >= limit {
            break;
        }
    }
    count
}

/// A walk's column context. One initial seek; later clusters move the
/// preorder cursor and rebuild ancestry only when crossing a column start.
#[derive(Clone)]
pub(crate) struct ColumnWalk<'a> {
    columns: &'a RubyColumns,
    near: Option<RubyColumnId>,
    stack: [RubyColumnId; MAX_RUBY_DEPTH],
    depth: usize,
}
impl<'a> ColumnWalk<'a> {
    /// Starts a walk at `at`, walking to its nearest column from column
    /// `near`, any column near it.
    pub(crate) fn from_column(columns: &'a RubyColumns, near: RubyColumnId, at: ClusterId) -> Self {
        let mut walk = Self {
            columns,
            near: columns.nearest_from(near, at),
            stack: [RubyColumnId::new(0); MAX_RUBY_DEPTH],
            depth: 0,
        };
        walk.rebuild();
        walk
    }
    /// Refills the ancestry stack from the nearest column, outermost first.
    fn rebuild(&mut self) {
        self.depth = 0;
        let mut id = self.near;
        while let Some(at) = id {
            if self.depth >= self.stack.len() {
                break;
            }
            self.stack[self.depth] = at;
            self.depth += 1;
            id = self.columns.get(at).and_then(|column| column.parent);
        }
        self.stack[..self.depth].reverse();
    }
    /// Steps the nearest column to `cluster`'s, one column at a time, and
    /// rebuilds the stack where it changes.
    fn move_to(&mut self, cluster: ClusterId) {
        let old = self.near;
        let near = self.near.unwrap_or(RubyColumnId::new(0));
        self.near = self.columns.nearest_from(near, cluster);
        if old != self.near {
            self.rebuild();
        }
    }
    /// Returns the columns on the stack whose span reaches past `cluster`,
    /// outermost first.
    fn active(&self, cluster: ClusterId) -> impl DoubleEndedIterator<Item = RubyColumnId> + '_ {
        self.stack[..self.depth].iter().copied().filter(move |&id| {
            (self.columns.get(id)).is_some_and(|column| cluster < self.columns.column_end(column))
        })
    }
    /// Returns the innermost column whose base holds `cluster`.
    pub(super) fn base(&mut self, cluster: ClusterId) -> Option<RubyColumnId> {
        self.move_to(cluster);
        let id = self.active(cluster).next_back()?;
        self.columns.get(id)?.base.contains(&cluster).then_some(id)
    }
    /// Returns the column around `cluster` that a justification walk in
    /// `parent`'s base takes as one unit: the child of `parent` around it,
    /// or at the top level, a base-shorter column or else the innermost.
    pub(super) fn scope_column(
        &mut self,
        cluster: ClusterId,
        parent: Option<RubyColumnId>,
    ) -> Option<&'a RubyColumn> {
        self.move_to(cluster);
        let id = self.active(cluster).find(|&id| {
            self.columns
                .get(id)
                .is_some_and(|column| column.parent == parent)
        })?;
        let column = self.columns.get(id)?;
        if parent.is_some() || column.is_base_shorter() {
            return Some(column);
        }
        self.columns.get(self.active(cluster).next_back()?)
    }
    /// Returns the outermost level on `side` of the columns around base
    /// cluster `cluster`, its enclosing columns' included.
    pub(crate) fn outermost(&mut self, cluster: ClusterId, side: RubySide) -> Option<RubyLevelId> {
        self.base(cluster)?;
        self.active(cluster).find_map(|id| {
            self.columns
                .get(id)
                .and_then(|column| self.columns.outermost(column, side))
                .map(|(id, _)| id)
        })
    }
}

/// A justification walk needs one column in its scope, rather than the
/// full ancestry stack kept by annotation walks. The cached result remains
/// valid between column boundaries, including backwards neighbour queries.
#[derive(Clone)]
pub(super) struct ColumnScope<'a> {
    pub(super) columns: &'a RubyColumns,
    near: Option<RubyColumnId>,
    span: Range<ClusterId>,
    parent: Option<RubyColumnId>,
    current: Option<&'a RubyColumn>,
}

impl<'a> ColumnScope<'a> {
    /// Starts a scope at `at`, seeking its nearest column once.
    pub(super) fn new(columns: &'a RubyColumns, at: ClusterId) -> Self {
        Self::with_near(columns, columns.nearest(at), at)
    }

    /// Starts a scope at `at`, walking to its nearest column from column
    /// `near`, any column near it.
    pub(super) fn from_column(columns: &'a RubyColumns, near: RubyColumnId, at: ClusterId) -> Self {
        Self::with_near(columns, columns.nearest_from(near, at), at)
    }

    /// Starts a scope at `at`, whose nearest column is `near`.
    fn with_near(columns: &'a RubyColumns, near: Option<RubyColumnId>, at: ClusterId) -> Self {
        Self {
            columns,
            near,
            span: at..at,
            parent: None,
            current: None,
        }
    }

    /// Returns the column [`ColumnWalk::scope_column`] finds, refreshing the
    /// cached answer only where `cluster` leaves its span or `parent`
    /// changes.
    pub(super) fn scope_column(
        &mut self,
        cluster: ClusterId,
        parent: Option<RubyColumnId>,
    ) -> Option<&'a RubyColumn> {
        if self.parent != parent || !self.span.contains(&cluster) {
            self.refresh(cluster, parent);
        }
        self.current
    }

    /// Finds the answer for `cluster` and the span of clusters it holds for:
    /// up to the nearest column boundary either side.
    fn refresh(&mut self, cluster: ClusterId, parent: Option<RubyColumnId>) {
        let mut walk = ColumnWalk {
            columns: self.columns,
            near: self.near,
            stack: [RubyColumnId::new(0); MAX_RUBY_DEPTH],
            depth: 0,
        };
        walk.rebuild();
        self.current = walk.scope_column(cluster, parent);
        self.near = walk.near;
        self.parent = parent;
        let mut start = ClusterId::new(0);
        let mut end = ClusterId::new(u32::MAX as usize);
        let next = walk
            .near
            .map_or(RubyColumnId::new(0), |id| RubyColumnId::new(id.get() + 1));
        if let Some(column) = self.columns.get(next) {
            end = column.base.start;
        }
        for id in walk.stack[..walk.depth].iter().copied() {
            let Some(column) = self.columns.get(id) else {
                continue;
            };
            let past = self.columns.column_end(column);
            for boundary in [column.base.start, column.base.end, past] {
                if boundary <= cluster {
                    start = start.max(boundary);
                } else {
                    end = end.min(boundary);
                }
            }
        }
        self.span = start..end;
    }
}

/// The side of its base a ruby annotation level is set on.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum RubySide {
    /// Over the base.
    Over,
    /// Under it.
    Under,
}

impl RubySide {
    /// The side the annotation at `level` of its column, counting from
    /// nought, is set on under `position`, the `ruby-position` of the ruby
    /// container it is in, as Chrome reads it off the container.
    /// `alternate`, which Chrome does not parse, starts on its side and
    /// alternates, as CSS Ruby says.
    pub(super) fn from_position(position: RubyPosition, level: usize) -> Self {
        let even = level.is_multiple_of(2);
        match position {
            RubyPosition::Over => Self::Over,
            RubyPosition::Under => Self::Under,
            RubyPosition::Alternate if even => Self::Over,
            RubyPosition::Alternate => Self::Under,
            RubyPosition::AlternateUnder if even => Self::Under,
            RubyPosition::AlternateUnder => Self::Over,
        }
    }
}

impl From<EmphasisSide> for RubySide {
    /// The side of its text an emphasis mark is set on, as the side of a
    /// base its annotations are: what a mark over text in a base is set
    /// past, an annotation on that side.
    fn from(side: EmphasisSide) -> Self {
        match side {
            EmphasisSide::Over => Self::Over,
            EmphasisSide::Under => Self::Under,
        }
    }
}

/// One annotation level of a ruby column: 36 bytes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct RubyLevel {
    /// Its clusters, which are 0 in the prefix: line layout sets them as a
    /// line of their own.
    pub(crate) clusters: Range<ClusterId>,
    /// Its annotation's opening item, whose node is its annotation's own
    /// element.
    pub(crate) open: ItemId,
    /// The side of its base it is set on.
    pub(crate) side: RubySide,
    /// Its line's width at its max-content, on the grid: its clusters'
    /// shaped advances and spacing, and the edges of the boxes in it.
    pub(crate) width: LayoutUnit,
    /// Its em box around its baseline, as Chrome's `ComputeEmHeight`
    /// unites its text's: what the levels are stacked by.
    pub(crate) extent: Extent,
    /// Its clusters' justification opportunities under its annotation's
    /// `text-justify`, which `ruby-align` spreads the room of a column
    /// wider than it over.
    pub(crate) opportunities: u32,
    /// Its band on this side, past the annotations nested in its base.
    pub(crate) depth: u32,
    /// Its first cumulative width in the stage's table of them, absent for
    /// an indivisible column.
    pub(super) steps: Option<StepId>,
}

/// A ruby column: a base and its annotations, sized once:
/// 68 bytes, one a column.
///
/// Its levels are a range of the stage's one table of them, not a vector of
/// its own, so a rebuild with many levels allocates nothing.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct RubyColumn {
    /// Its first item: the one after its container's opening item for the
    /// container's first column, and after the column before's last
    /// annotation for another.
    pub(crate) open: ItemId,
    /// Its base's end: its first annotation's opening item, or the
    /// container's closing item where it has none.
    pub(crate) base_end: ItemId,
    /// One past its last item: the next column's first, or the container's
    /// closing item.
    pub(crate) close: ItemId,
    /// Its base's clusters.
    pub(crate) base: Range<ClusterId>,
    /// The containing column, whose base holds this column as one unit.
    pub(crate) parent: Option<RubyColumnId>,
    /// Static eligibility for greedy fitting inside the base.
    pub(crate) may_break: bool,
    /// Its levels, in the stage's table.
    pub(crate) levels: Range<RubyLevelId>,
    /// The widest of the base and its annotations, on the grid.
    pub(crate) width: LayoutUnit,
    /// Its base's own width, on the grid: less than `width` where an
    /// annotation is wider, which a justified line reads.
    pub(crate) base_width: LayoutUnit,
    /// How `width` less the base's is placed by `ruby-align` (Chrome's
    /// `ApplyRubyAlign`): this much before the base's first cluster, at its
    /// logical start, from the column's own start; `room_inside` shared by
    /// the base's justification opportunities; and what is left after its
    /// last. Line layout places them.
    pub(crate) room_before: LayoutUnit,
    pub(crate) room_inside: LayoutUnit,
    /// How far it reaches over the text before it and after it, where that
    /// text is on its line: the prefix takes it off the column, and a line
    /// the column starts or ends pays it back.
    pub(crate) overhang: (LayoutUnit, LayoutUnit),
    /// Its base's em box around the container's baseline: what the first of
    /// its annotations on each side is set against, a line's columns
    /// together.
    pub(crate) base_em: Extent,
}

impl RubyColumn {
    /// Whether an annotation is wider than its base: a justified line takes
    /// the column as one object, as Chrome takes a base-shorter ruby
    /// (`kBaseShorterRubyMarker`), and spreads nothing inside it.
    pub(super) fn is_base_shorter(&self) -> bool {
        self.width > self.base_width
    }

    /// Where its room goes under `ruby-align: space-around` on a justified
    /// line whose start it stands at (`start`), whose end (`end`), or both,
    /// as Chrome's `ApplyRubyAlign` places it with `on_start_edge` and
    /// `on_end_edge`: its base flush with the edge, all its
    /// room at the other side but what the base's opportunities share; at
    /// both edges, all its room inside the base, as `space-between` puts it,
    /// or the base centred where it has no opportunity, and over the whole
    /// of `line`, the line's room, where it is wider than its base and so
    /// all the line holds, as Chrome spreads a base-shorter column alone on
    /// a line. At neither, the measure stage's placement.
    ///
    /// Returns the room before its base, the room its base's opportunities
    /// share, and its width on the line.
    pub(crate) fn room_on_edges(
        &self,
        start: bool,
        end: bool,
        line: LayoutUnit,
    ) -> (LayoutUnit, LayoutUnit, LayoutUnit) {
        let space = self.width - self.base_width;
        match (start, end) {
            (false, false) => (self.room_before, self.room_inside, self.width),
            (true, false) => (LayoutUnit::ZERO, self.room_inside, self.width),
            (false, true) => (space - self.room_inside, self.room_inside, self.width),
            (true, true) => {
                let width = if self.is_base_shorter() {
                    line.max(self.width)
                } else {
                    self.width
                };
                let space = width - self.base_width;
                // Room inside is what a base with an opportunity shares.
                if self.room_inside > LayoutUnit::ZERO {
                    (LayoutUnit::ZERO, space, width)
                } else {
                    (space.half(), LayoutUnit::ZERO, width)
                }
            }
        }
    }

    /// The ruby container it is in, of `content`, whose items it is among:
    /// its base's end's node where that is the container's closing item,
    /// the parent of its first annotation's node otherwise, and the block
    /// where the items do not say.
    pub(crate) fn container(&self, content: &Content) -> NodeId {
        match content.items.get(self.base_end) {
            Some(item) if item.kind == ItemKind::RubyClose => item.node,
            Some(item) => content.nodes.parent(item.node),
            None => NodeId::BLOCK,
        }
    }
}
