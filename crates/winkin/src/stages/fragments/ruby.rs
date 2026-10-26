//! Places a line's ruby columns.
//!
//! A column's base sits on the line among the rest, as Blink's
//! `PlaceRubyColumn` places it. The column's spare room goes into gaps
//! before and after the base and at its justification opportunities, as
//! `ruby-align` says. Gaps write no items and take the level of the text
//! beside them, so reordering keeps them with it. An overhang onto text
//! beside the column shortens the gap on that side.

use alloc::vec::Vec;
use core::ops::Range;

use super::annotations::BoxesInAnnotation;
use super::boxes::LineBoxes;
use super::justify::{Justified, LineJustification};
use super::place::{LinePieces, PieceId};
use super::{FragmentItemFlags, Piece, PieceKind, Placer, spreads_inside};
use crate::data::IdRange;
use crate::data::{Id, heap_bytes};
use crate::stages::analysis::{BidiLevel, ClusterClass, ClusterId};
use crate::stages::content::{ItemFlags, ItemId, NodeId};
use crate::stages::lines::{ColumnPart, LevelBands, LineView};
use crate::stages::measure;
use crate::stages::measure::{JustifyOpportunities, RubyColumn, RubyColumnId, RubySpread};
use crate::stages::shape::ShapedRunId;
use crate::style::RubyAlign;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// A ruby column on the line being laid out.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineColumn {
    /// Its id in measurement's ruby columns.
    pub(super) column: RubyColumnId,
    /// Its index among the line's columns.
    index: usize,
    /// Its base and sizes on this line.
    pub(super) sizes: ColumnSizes,
    /// Its closing item: the item after its last.
    pub(super) close: ItemId,
    /// Where it ends on this line: its part's base end where the line
    /// splits its base, else its closing item's boundary.
    pub(super) end: ClusterId,
    /// Its ruby container.
    pub(super) container: NodeId,
    /// Whether its base starts on a line before this one.
    pub(super) continued: bool,
    /// The index of the column it is nested in, if any.
    parent: Option<usize>,
    /// How its base's spread room is shared, saved for nested columns.
    spread: Option<LineJustification>,
    /// Its gap before its base, among the line's pieces, and the one after
    /// it, once its end is met.
    pub(super) leading: PieceId,
    pub(super) trailing: Option<PieceId>,
    /// Its room before its base and among its base's opportunities, and its
    /// width on this line.
    ///
    /// These are measurement's values, except at a justified line's edge
    /// (see [`RubyEdges`](super::place::RubyEdges)).
    room_before: InlineLayoutUnit,
    room_inside: InlineLayoutUnit,
    width: InlineLayoutUnit,
    /// The room `ruby-align` leaves after its base: what its width leaves
    /// past its base and the room before and inside it.
    room_after: InlineLayoutUnit,
    /// Its overhang onto the text before and after it on this line, which
    /// shortens the gaps there.
    pub(super) overhang: (InlineLayoutUnit, InlineLayoutUnit),
    /// Where its base starts in the prefix, past its container's opening
    /// edge.
    start: InlineLayoutUnit,
    /// How far its base's baseline is raised from the line's.
    pub(super) shift: LayoutUnit,
    /// The room its parent's spread base gives it before and after, which a
    /// nested column's gaps take on.
    pub(super) parent_room: (InlineLayoutUnit, InlineLayoutUnit),
    /// The shaping run its base starts in on this line.
    first_run: ShapedRunId,
    /// The advance of the collapsible spaces ending its base at the line's
    /// end, which line layout removes.
    removed: InlineLayoutUnit,
}

/// A ruby column's base and sizes as the line being placed sets them: its
/// part's where the column is split across lines, else its own.
#[derive(Copy, Clone, Debug)]
pub(super) struct ColumnSizes {
    /// Its base's clusters on the line, start and end.
    base: (ClusterId, ClusterId),
    /// Its base's width, and the widest of its base and annotations.
    base_width: LayoutUnit,
    pub(super) width: LayoutUnit,
    /// How far it overhangs the text before and after it.
    overhang: (LayoutUnit, LayoutUnit),
}

impl ColumnSizes {
    /// Returns `column`'s sizes, or those of `part`, the line's part of it.
    fn new(column: &RubyColumn, part: Option<&ColumnPart>) -> Self {
        match part {
            Some(part) => Self {
                base: (part.base.start, part.base.end),
                base_width: part.base_width,
                width: part.width,
                overhang: part.overhang,
            },
            None => Self {
                base: (column.base.start, column.base.end),
                base_width: column.base_width,
                width: column.width,
                overhang: column.overhang,
            },
        }
    }

    /// Returns its base's clusters on the line.
    fn base(&self) -> Range<ClusterId> {
        self.base.0..self.base.1
    }

    /// Returns the room before its base, the room its base's opportunities
    /// share, and its width on the line, for a column at a justified line's
    /// `start` edge, `end` edge, or both.
    ///
    /// This is where `ruby-align: space-around` puts the room, as Blink's
    /// `ApplyRubyAlign` does with `on_start_edge` and `on_end_edge`:
    /// - At one edge, the base sits flush with it, and all the room the
    ///   base's opportunities don't share goes on the other side.
    /// - At both edges, all the room goes inside the base, as
    ///   `space-between` puts it, or the base is centred where it has no
    ///   opportunity. A column wider than its base is then all the line
    ///   holds, and spreads over `line`, the line's room, as a base-shorter
    ///   column alone on a line spreads.
    /// - At neither, the room is `before` and `inside`.
    fn room_on_edges(
        &self,
        (before, inside): (LayoutUnit, LayoutUnit),
        (start, end): (bool, bool),
        line: LayoutUnit,
    ) -> (LayoutUnit, LayoutUnit, LayoutUnit) {
        let space = self.width - self.base_width;
        match (start, end) {
            (false, false) => (before, inside, self.width),
            (true, false) => (LayoutUnit::ZERO, inside, self.width),
            (false, true) => (space - inside, inside, self.width),
            (true, true) => {
                let width = if self.width > self.base_width {
                    line.max(self.width)
                } else {
                    self.width
                };
                let space = width - self.base_width;
                // Room inside is what a base with an opportunity shares.
                if inside > LayoutUnit::ZERO {
                    (LayoutUnit::ZERO, space, width)
                } else {
                    (space.half(), LayoutUnit::ZERO, width)
                }
            }
        }
    }
}

impl<'a> Placer<'a> {
    /// Returns the part of column `id` that the line being placed sets,
    /// where the column is split.
    pub(super) fn line_part(&self, id: RubyColumnId) -> Option<&'a ColumnPart> {
        self.line_ruby
            .column_parts
            .iter()
            .rev()
            .find(|part| part.column == id)
    }

    /// Returns where `column` ends on the line: the base end of `part`, its
    /// part on the line, where the line splits its base, else where its
    /// closing item stands.
    fn column_end(&self, column: &RubyColumn, part: Option<&ColumnPart>) -> ClusterId {
        match part {
            Some(part) if part.base.end < column.base.end => part.base.end,
            _ => self.item_clusters.start(column.close),
        }
    }

    /// Closes every column still open at the end of `line`, innermost
    /// first.
    #[cold]
    #[inline(never)]
    pub(super) fn finish_columns(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
    ) {
        while let Some(open) = self
            .line_ruby
            .open_column
            .and_then(|index| rubies.columns.get(index))
            .copied()
        {
            self.close_column(pieces, boxes, rubies, line, open);
        }
    }

    /// Closes and opens the ruby columns at the walk's item, whose first
    /// cluster is `at`.
    ///
    /// An open column closes at the item after its last. A column opens at
    /// its first item. `rest` is the room the items before take at this
    /// boundary, past the prefix sums.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    pub(super) fn update_columns(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
        at: ClusterId,
        rest: InlineLayoutUnit,
    ) {
        while let Some(open) = self
            .line_ruby
            .open_column
            .and_then(|index| rubies.columns.get(index))
            .copied()
            .filter(|open| open.close == self.item)
        {
            self.close_column(pieces, boxes, rubies, line, open);
        }
        // Most items open no column, which the column's first item says
        // before its part on the line is looked for.
        while let Some(column) = self.rubies.get(self.column) {
            work::step();
            if column.open > self.item {
                break;
            }
            let part = self.line_part(self.column);
            let sizes = ColumnSizes::new(column, part);
            // A column whose base starts at the line's end belongs to the
            // next line. The item at the end boundary only closes the column
            // before it.
            if sizes.base.0 < sizes.base.1 && sizes.base.0 >= line.clusters().end {
                break;
            }
            let id = self.column;
            self.column = RubyColumnId::new(id.get() + 1);
            // A column that starts before this line, left behind by a forced
            // break inside it, belongs to the line before. Its text here
            // takes no room.
            if part.is_none() && (column.open < self.item || sizes.base.0 < line.clusters().start) {
                // The columns between the parts continued onto the line and
                // where the cursor stood as it started are all left behind:
                // the walk goes on at the next of either.
                if id < self.line_ruby.line_column {
                    let next = self
                        .line_ruby
                        .column_parts
                        .iter()
                        .map(|part| part.column)
                        .filter(|&column| column > id)
                        .fold(self.line_ruby.line_column, RubyColumnId::min);
                    self.column = self.column.max(next);
                }
                continue;
            }
            let start = at.max(line.clusters().start);
            self.open_column(pieces, boxes, rubies, line, (id, column, part), start, rest);
            if column.close == self.item
                && let Some(open) = rubies.columns.last().copied()
            {
                self.close_column(pieces, boxes, rubies, line, open);
            }
        }
    }

    /// Opens ruby column `id`, `column`, with `part`, its part on `line`
    /// where it is split, at boundary `at`.
    ///
    /// It pushes the gap before the base, less any overhang onto text before
    /// it. It sets up the room the base's opportunities share, given out as
    /// its pieces are made. It records the column in `rubies`, with what
    /// closing it and setting its annotations read.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn open_column(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
        (id, column, part): (RubyColumnId, &'a RubyColumn, Option<&'a ColumnPart>),
        at: ClusterId,
        rest: InlineLayoutUnit,
    ) {
        let mut sizes = ColumnSizes::new(column, part);
        let base = sizes.base();
        // Collapsible spaces ending the base at the line's end go, and the
        // column narrows to what is left.
        let removed = if part.is_none() && !base.is_empty() {
            self.base_end_spaces(line, column)
        } else {
            None
        };
        let mut spread_base = base.clone();
        if let Some((from, advance)) = removed {
            self.line_ruby.base_spaces = Some(from);
            sizes.base_width = sizes.base_width - advance.to_layout();
            let widest = (self.rubies.levels(column).iter())
                .fold(sizes.base_width, |widest, level| widest.max(level.width));
            sizes.width = widest;
            spread_base = base.start..from;
        }
        let content = self.stages.content;
        let container = column.container(content);
        let align = self.box_facts(container).ruby.align;
        let text = content.facts.text(self.stages.text_facts(container));
        let scope = (id, id.max(self.line_ruby.line_column));
        let (analysis, ruby_columns) = (self.stages.analysis, self.rubies);
        let opportunities = || {
            let (justify, base) = (text.justify, spread_base.clone());
            JustifyOpportunities::from_base(
                content,
                analysis,
                base,
                justify,
                ruby_columns,
                scope,
                None,
            )
        };
        let (mut room_before, mut room_inside) = (column.room_before, column.room_inside);
        if part.is_some() || removed.is_some() {
            let space = sizes.width - sizes.base_width;
            // Only room spread inside the base reads its opportunities.
            let count = if space > LayoutUnit::ZERO && spreads_inside(align) {
                opportunities().map_or(0, |found| found.count_range(spread_base.clone()))
            } else {
                0
            };
            let spread = RubySpread::from_base(align, space, count);
            (room_before, room_inside) = (spread.start, spread.inside);
        }
        let level = if base.is_empty() {
            self.line_state.level
        } else {
            let run = self.advance_run(base.start);
            self.run_level(run, self.line_state.level)
        };
        let first_run = self.run.id();
        // Where the column starts the line, no text is beside it, and the
        // breaker paid its overhang back.
        let before = if base.start > line.clusters().start {
            InlineLayoutUnit::from_layout(sizes.overhang.0)
        } else {
            InlineLayoutUnit::ZERO
        };
        let end = self.column_end(column, part);
        // At a justified line's edge, the base sits flush with it.
        let (starts, ends) = if column.parent.is_none() && align == RubyAlign::SpaceAround {
            (
                self.line_ruby.ruby_edges.start
                    && !base.is_empty()
                    && base.start == line.clusters().start,
                self.line_ruby.ruby_edges.end && end == self.inflow_end(line),
            )
        } else {
            (false, false)
        };
        let (room_before, room_inside, width) = sizes.room_on_edges(
            (room_before, room_inside),
            (starts, ends),
            self.line_ruby.ruby_edges.room,
        );
        let room_after =
            (width - sizes.base_width - room_before - room_inside).max(LayoutUnit::ZERO);
        let [room_before, room_inside, width, room_after] =
            [room_before, room_inside, width, room_after].map(InlineLayoutUnit::from_layout);
        let (shift, _) = boxes.innermost_shifts();
        let parent_room = match &self.line_ruby.spread {
            Some(spread) if column.parent.is_some() && !base.is_empty() => (
                spread.room(base.start).0,
                spread.room(ClusterId::new(base.end.get() - 1)).1,
            ),
            _ => (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO),
        };
        let leading = pieces.logical.next_id();
        pieces.push(Piece::new(
            PieceKind::Gap,
            container,
            level,
            at..at,
            room_before - before + parent_room.0,
            boxes.innermost(),
            shift,
        ));
        self.line_ruby.spread = Justified::new(opportunities, spread_base.clone(), room_inside);
        let index = rubies.columns.len();
        let parent = self.line_ruby.open_column;
        self.line_ruby.open_column = Some(index);
        rubies.columns.push(LineColumn {
            column: id,
            index,
            sizes,
            close: column.close,
            end,
            container,
            continued: column.base.start < line.clusters().start,
            parent,
            spread: self
                .line_ruby
                .spread
                .as_ref()
                .map(|spread| spread.justification),
            leading,
            trailing: None,
            room_before,
            room_inside,
            width,
            room_after,
            overhang: (before, InlineLayoutUnit::ZERO),
            start: self.stages.measured.prefix.get(at) + rest,
            shift,
            parent_room,
            first_run,
            removed: removed.map_or(InlineLayoutUnit::ZERO, |(_, advance)| advance),
        });
    }

    /// Closes the ruby column `open` on `line`, pushing the gap after its
    /// base.
    ///
    /// The gap takes the column's width less its base, the gap before, the
    /// shared room, and any overhang onto text after it. So its pieces add up
    /// to what the prefix sums gave it.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    fn close_column(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
        open: LineColumn,
    ) {
        self.line_ruby.spread = None;
        let sizes = open.sizes;
        let base = sizes.base();
        let base_width = if base.is_empty() {
            InlineLayoutUnit::ZERO
        } else {
            self.stages.measured.prefix.get(base.end)
                - open.start
                - self.closes_past_base(&open)
                - open.removed
        };
        let after = if open.end < line.clusters().end {
            InlineLayoutUnit::from_layout(sizes.overhang.1)
        } else {
            InlineLayoutUnit::ZERO
        };
        let room = open.width - base_width - open.room_before - open.room_inside - after;
        let level = if base.is_empty() || !self.gates.has_mixed_levels {
            self.line_state.level
        } else {
            // Both gaps take the lowest level in the base, its boxes' bidi
            // controls included, so reordering keeps them outside every
            // part of it: a column is placed as one unit.
            let lowest = self.base_level(pieces, &open);
            if let Some(leading) = pieces.logical.get_mut(open.leading) {
                leading.level = lowest;
            }
            lowest
        };
        let (shift, _) = boxes.innermost_shifts();
        let trailing = pieces.logical.next_id();
        pieces.push(Piece::new(
            PieceKind::Gap,
            open.container,
            level,
            open.end..open.end,
            room + open.parent_room.1,
            boxes.innermost(),
            shift,
        ));
        if let Some(last) = rubies.columns.get_mut(open.index) {
            last.trailing = Some(trailing);
            last.overhang.1 = after;
        }
        self.line_ruby.open_column = open.parent;
        // The parent's base spreads its room again, by the amounts saved as
        // it opened, so its base is not walked again.
        if let Some(parent) = open.parent.and_then(|index| rubies.columns.get(index))
            && let Some(saved) = parent.spread
        {
            self.line_ruby.spread = JustifyOpportunities::from_base(
                self.stages.content,
                self.stages.analysis,
                parent.sizes.base(),
                self.stages
                    .content
                    .facts
                    .text(self.stages.text_facts(parent.container))
                    .justify,
                self.rubies,
                (parent.column, parent.column.max(self.line_ruby.line_column)),
                Some(saved.summary),
            )
            .map(|opportunities| Justified {
                justification: saved,
                opportunities,
            });
        }
    }

    /// Returns the lowest bidi level in the base of the open column `open`:
    /// its shaping runs' and its boxes' bidi controls'.
    ///
    /// The runs are walked from the one the base starts in, up to its end.
    /// They count the runs whose clusters make no piece, such as removed
    /// spaces, so the levels come from the runs and not from the pieces.
    /// The controls are the pieces pushed since the column opened.
    fn base_level(&self, pieces: &LinePieces, open: &LineColumn) -> BidiLevel {
        let runs = &self.stages.shaped.runs;
        let end = open.sizes.base.1;
        let mut cursor = runs.cursor(open.first_run);
        let mut lowest = self.run_level(runs.get(open.first_run), self.line_state.level);
        while cursor.end() < end {
            work::step();
            let id = cursor.id();
            runs.step(&mut cursor);
            if cursor.id() == id {
                break;
            }
            lowest = lowest.min(self.run_level(runs.get(cursor.id()), self.line_state.level));
        }
        let since = open.leading..pieces.logical.next_id();
        (pieces.logical.get_slice(since).unwrap_or_default().iter())
            .filter(|piece| piece.kind == PieceKind::Control)
            .fold(lowest, |lowest, piece| lowest.min(piece.level))
    }

    /// Returns what the prefix charges at the end of the base of `open` that
    /// is not the base's, where the column ends there too.
    ///
    /// A column with no annotation text ends where its base does. The
    /// closing edges after its base, its container's among them, lead the
    /// items at that boundary, so the prefix there holds them.
    fn closes_past_base(&self, open: &LineColumn) -> InlineLayoutUnit {
        let sizes = open.sizes;
        let at = sizes.base.1;
        if self.item_clusters.start(open.close) != at {
            return InlineLayoutUnit::ZERO;
        }
        let Some(column) = self.rubies.get(open.column) else {
            return InlineLayoutUnit::ZERO;
        };
        let mut sum = InlineLayoutUnit::from_layout(
            sizes.width - sizes.overhang.0 - sizes.overhang.1 - sizes.base_width,
        );
        for id in (column.base_end..self.items.next_id()).ids() {
            work::step();
            let Some(item) = self.items.get(id) else {
                break;
            };
            let clusters = self.item_clusters.range(id);
            if clusters.start != at || !clusters.is_empty() {
                break;
            }
            if item.flags.contains(ItemFlags::ANNOTATION) {
                continue;
            }
            if item.kind.is_open() {
                break;
            }
            if item.kind.is_close() {
                sum += measure::edge_room(self.stages.content, None, item, false);
            }
        }
        sum
    }

    /// Returns where the collapsible spaces ending `column`'s base start,
    /// and their advance, where the column ends `line`.
    ///
    /// Blink's `RemoveTrailingCollapsibleSpace` removes them from the
    /// column's base, and the column takes the width of what is left.
    #[cold]
    #[inline(never)]
    fn base_end_spaces(
        &self,
        line: &LineView<'_>,
        column: &RubyColumn,
    ) -> Option<(ClusterId, InlineLayoutUnit)> {
        if self.item_clusters.start(column.close) != self.clusters.drawn_end(line.clusters()) {
            return None;
        }
        let base = column.base.clone();
        let start = base.start.max(line.clusters().start);
        let mut from = base.end;
        while from > start && self.is(ClusterId::new(from.get() - 1), ClusterClass::Space) {
            from = ClusterId::new(from.get() - 1);
        }
        if from == base.end {
            return None;
        }
        let last = ClusterId::new(base.end.get() - 1);
        let item = self.item_clusters.walk_to_cluster(column.open, last);
        let node = self.items.get(item)?.node;
        let facts = self.stages.content.facts.text(self.stages.text_facts(node));
        if !facts.collapse.collapses_spaces() {
            return None;
        }
        let prefix = &self.stages.measured.prefix;
        Some((from, prefix.get(base.end) - prefix.get(from)))
    }

    /// Returns where `line`'s in-flow content ends, leaving out a forced
    /// break but keeping trailing white space.
    ///
    /// This is Blink's `InflowEndOffsetWithoutForcedBreak`.
    fn inflow_end(&self, line: &LineView<'_>) -> ClusterId {
        let end = line.clusters().end;
        match end.get().checked_sub(1).map(ClusterId::new) {
            Some(last) if self.is(last, ClusterClass::Separator) => last,
            _ => end,
        }
    }

    /// Adds `piece`, text of a spread ruby base, one cluster at a time with
    /// a gap at each opportunity that takes room.
    ///
    /// `advance` measures each cluster. Clusters join where no gap parts
    /// them. Kept out of line so plain text never pays for it.
    #[inline(never)]
    pub(super) fn spread_text(
        &self,
        pieces: &mut LinePieces,
        piece: Piece,
        advance: &dyn Fn(&Self, ClusterId, ClusterId) -> InlineLayoutUnit,
    ) {
        let Some(spread) = self.line_ruby.spread.as_ref() else {
            pieces.push_or_join(piece);
            return;
        };
        for cluster in (piece.start..piece.end).ids() {
            work::step();
            let next = ClusterId::new(cluster.get() + 1);
            let one = Piece {
                start: cluster,
                end: next,
                advance: advance(self, cluster, next),
                ..piece
            };
            pieces.push_with_room(one, spread.room(cluster));
        }
    }

    /// Moves the room `ruby-align` spread in the line's ruby bases into
    /// their text, as justification room is part of the glyphs.
    ///
    /// Each base's text takes the room at its opportunities, and its
    /// outermost text the room its column leaves before and after it, out
    /// of the gaps that held them. The text of a column nested in the base
    /// takes none of its parent's room. The pieces are in visual order, so the
    /// gap visually first is on the base's left. Nothing moves along the
    /// line.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    pub(super) fn absorb_base_spread(pieces: &mut LinePieces, rubies: &LineRubies) {
        pieces.absorb_spread();
        for open in &rubies.columns {
            work::step();
            let Some(trailing) = open.trailing else {
                continue;
            };
            let base = PieceId::new(open.leading.get() + 1)..trailing;
            let base_pieces = pieces.logical.get_slice(base.clone()).unwrap_or_default();
            // A tab's width is where it lands, which the column's width
            // does not count, so a base holding one keeps its room in its
            // gaps.
            if base_pieces
                .iter()
                .any(|piece| piece.flags.contains(FragmentItemFlags::TAB))
            {
                continue;
            }
            let after = open.room_after;
            if open.room_before <= InlineLayoutUnit::ZERO && after <= InlineLayoutUnit::ZERO {
                continue;
            }
            let lead_first = pieces
                .order
                .as_slice()
                .iter()
                .inspect(|_| work::step())
                .find(|&&at| at == open.leading || at == trailing)
                .is_none_or(|&at| at == open.leading);
            let (left, right) = if lead_first {
                ((open.leading, open.room_before), (trailing, after))
            } else {
                ((trailing, after), (open.leading, open.room_before))
            };
            // The text of the columns nested in the base is theirs.
            let nested = |at: PieceId| {
                rubies.columns.iter().any(|inner| {
                    inner.parent == Some(open.index)
                        && inner.leading <= at
                        && inner.trailing.is_none_or(|trailing| at <= trailing)
                })
            };
            for ((gap, room), at_left) in [(left, true), (right, false)] {
                let taken = pieces.absorb_end(base.clone(), (room, at_left), nested);
                if taken > InlineLayoutUnit::ZERO
                    && let Some(gap) = pieces.logical.get_mut(gap)
                {
                    gap.advance = gap.advance - taken;
                }
            }
        }
    }
}

/// The ruby columns on the line being laid out, and the annotation lines
/// they set.
pub(super) struct LineRubies {
    /// The ruby columns on the line.
    pub(super) columns: Vec<LineColumn>,
    /// Where its annotation levels stand.
    pub(super) bands: LevelBands,
    /// The pieces of the annotation line being set.
    pub(super) annotation: LinePieces,
    /// The boxes inside the annotation line being set.
    pub(super) boxes: BoxesInAnnotation,
}

impl LineRubies {
    /// Makes an empty table, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            columns: Vec::new(),
            bands: LevelBands::new(),
            annotation: LinePieces::new(),
            boxes: BoxesInAnnotation::new(),
        }
    }
}

heap_bytes! {
    LineRubies { columns, bands, annotation, boxes }
}

impl LinePieces {
    /// Moves the room each spread gap holds into the text of its node beside
    /// it, as the glyph before an opportunity expands: into the text before
    /// it, or the text after it where none is before.
    ///
    /// The gap keeps its place with no room, and the text takes the room on
    /// the gap's side. Nothing moves along the line.
    pub(super) fn absorb_spread(&mut self) {
        let count = self.logical.next_id().get();
        for at in 0..count {
            let Some(gap) = self.logical.get(PieceId::new(at)).copied() else {
                continue;
            };
            if gap.kind != PieceKind::Spread || gap.advance == InlineLayoutUnit::ZERO {
                continue;
            }
            work::step();
            let spread = |piece: &Piece| piece.kind == PieceKind::Spread && piece.node == gap.node;
            // A tab is as wide as the distance to its stop, and takes no room.
            let text = |piece: &Piece| {
                piece.kind == PieceKind::Text
                    && piece.node == gap.node
                    && !piece.flags.contains(FragmentItemFlags::TAB)
            };
            // The text before, past the gaps it already took in.
            let mut back = at;
            while let Some(previous) = back.checked_sub(1)
                && self.logical.get(PieceId::new(previous)).is_some_and(spread)
            {
                back = previous;
            }
            let before = back
                .checked_sub(1)
                .filter(|&previous| self.logical.get(PieceId::new(previous)).is_some_and(text));
            let mut on = at + 1;
            while self.logical.get(PieceId::new(on)).is_some_and(spread) {
                on += 1;
            }
            let after =
                Some(on).filter(|&next| self.logical.get(PieceId::new(next)).is_some_and(text));
            let Some((owner, gap_after)) =
                before.map(|at| (at, true)).or(after.map(|at| (at, false)))
            else {
                continue;
            };
            let owner = PieceId::new(owner);
            let Some(text) = self.logical.get(owner) else {
                continue;
            };
            // A gap after the text in logical order is on its right, or its
            // left where it runs right to left.
            if gap_after != text.level.is_rtl() {
                self.add_room(owner, InlineLayoutUnit::ZERO, gap.advance);
            } else {
                self.add_room(owner, gap.advance, InlineLayoutUnit::ZERO);
            }
            if let Some(gap) = self.logical.get_mut(PieceId::new(at)) {
                gap.advance = InlineLayoutUnit::ZERO;
            }
        }
    }

    /// Moves `room` into the leftmost text piece of `range`, in visual
    /// order, or the rightmost where `!left`, and returns how much it moved:
    /// none where `range` holds no text. Pieces `nested` names are passed
    /// over.
    ///
    /// The text takes the room on that side, expanding the glyph at the edge
    /// of a spread base or annotation.
    pub(super) fn absorb_end(
        &mut self,
        range: Range<PieceId>,
        (room, left): (InlineLayoutUnit, bool),
        nested: impl Fn(PieceId) -> bool,
    ) -> InlineLayoutUnit {
        if room <= InlineLayoutUnit::ZERO {
            return InlineLayoutUnit::ZERO;
        }
        let logical = &self.logical;
        let holds = |at: &PieceId| {
            range.contains(at)
                && !nested(*at)
                && logical.get(*at).is_some_and(|piece| {
                    piece.kind == PieceKind::Text && !piece.flags.contains(FragmentItemFlags::HANGS)
                })
        };
        let order = self.order.as_slice().iter().copied();
        let found = if left {
            order.inspect(|_| work::step()).find(holds)
        } else {
            order.rev().inspect(|_| work::step()).find(holds)
        };
        let Some(text) = found.filter(|&at| self.logical.get(at).is_some()) else {
            return InlineLayoutUnit::ZERO;
        };
        if left {
            self.add_room(text, room, InlineLayoutUnit::ZERO);
        } else {
            self.add_room(text, InlineLayoutUnit::ZERO, room);
        }
        room
    }
}
