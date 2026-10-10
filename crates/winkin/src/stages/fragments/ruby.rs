//! Places a line's ruby columns and writes their annotation lines.
//!
//! A column's base sits on the line among the rest, as Chrome's
//! `PlaceRubyColumn` places it. The column's spare room goes into gaps
//! before and after the base and at its justification opportunities, as
//! `ruby-align` says. Gaps write no items and take the level of the text
//! beside them, so reordering keeps them with it. An overhang onto text
//! beside the column shortens the gap on that side.
//!
//! Once the line is placed, each annotation is set as a line of its own
//! over the column's box. `ruby-align` spreads it, as Chrome's
//! `PlaceRubyAnnotation` does. Its baseline comes from `LevelBands::stack`,
//! the function the breaker sized the line with. The annotation lines'
//! items follow the line's own.

use alloc::vec::Vec;
use core::iter::from_fn;
use core::ops::Range;

use super::place::{LineBoxes, LinePieces, LineRubies, PieceId};
use super::read::AnnotationText;
use super::{
    FragmentItem, FragmentItemFlags, FragmentItemKind, Fragments, Justified, Piece, PieceKind,
    Placer,
};
use crate::data::IdRange;
use crate::data::{Id, Table, define_id, heap_bytes, stable_sort_by_key};
use crate::stages::analysis::{BidiLevel, ClusterClass, ClusterId};
use crate::stages::content::{ItemFlags, ItemId, ItemKind, NodeId, NodeKind};
use crate::stages::lines::{LineView, RubyPiece};
use crate::stages::measure::{
    JustifyOpportunities, RubyColumn, RubyColumnId, RubyLevel, RubyLevelId, RubySpread,
};
use crate::stages::shape::ShapedRunId;
use crate::stages::{Segments, Step, measure};
use crate::style::RubyAlign;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// A ruby column on the line being laid out.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineColumn {
    /// Its id in measurement's ruby columns.
    column: RubyColumnId,
    /// Its index among the line's columns.
    index: usize,
    /// Its base's clusters on this line, start and end.
    base: (ClusterId, ClusterId),
    /// The index of the column it is nested in, if any.
    parent: Option<usize>,
    /// How its base's spread room is shared, saved for nested columns.
    spread: Option<super::LineJustification>,
    /// Its gap before its base, among the line's pieces, and the one after
    /// it, once its end is met.
    leading: PieceId,
    trailing: Option<PieceId>,
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
    overhang: (InlineLayoutUnit, InlineLayoutUnit),
    /// Where its base starts in the prefix, past its container's opening
    /// edge.
    start: InlineLayoutUnit,
    /// How far its base's baseline is raised from the line's.
    shift: LayoutUnit,
    /// The room its parent's spread base gives it before and after, which a
    /// nested column's gaps take on.
    parent_room: (InlineLayoutUnit, InlineLayoutUnit),
    /// The shaping run its base starts in on this line.
    first_run: ShapedRunId,
}

define_id! {
    /// Names a box inside the annotation line being set, in
    /// [`BoxesInAnnotation`].
    struct AnnotationBoxId(u32);
}

/// A box inside the annotation line being set: its node, its range of the
/// annotation's pieces in logical order, and their reach once placed.
#[derive(Copy, Clone, Debug)]
struct BoxInAnnotation {
    node: NodeId,
    start: PieceId,
    end: PieceId,
    reach: Option<(InlineLayoutUnit, InlineLayoutUnit)>,
}

impl BoxInAnnotation {
    /// Widens its reach to take in `left` to `right` too.
    fn widen(&mut self, left: InlineLayoutUnit, right: InlineLayoutUnit) {
        self.reach = Some(match self.reach {
            None => (left, right),
            Some((from, to)) => (from.min(left), to.max(right)),
        });
    }
}

/// The boxes inside the annotation line being set, in tree order, and the
/// stack of those open during the walk.
///
/// It is line layout scratch: each annotation line clears it, and it is
/// never dropped.
pub(super) struct BoxesInAnnotation {
    boxes: Table<AnnotationBoxId, BoxInAnnotation>,
    open: Vec<AnnotationBoxId>,
}

impl BoxesInAnnotation {
    /// Makes an empty table, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            boxes: Table::new(),
            open: Vec::new(),
        }
    }

    /// Forgets the last annotation line's boxes.
    fn clear(&mut self) {
        self.boxes.clear();
        self.open.clear();
    }

    /// Opens the box `node` before piece `at`.
    fn open(&mut self, node: NodeId, at: PieceId) {
        let opened = self.boxes.push(BoxInAnnotation {
            node,
            start: at,
            end: at,
            reach: None,
        });
        match opened {
            Some(id) => self.open.push(id),
            None => debug_assert!(false, "no more boxes than an AnnotationBoxId names"),
        }
    }

    /// Opens the boxes `nodes` names, innermost first, before piece `at`,
    /// so that the outermost is opened first.
    fn open_around(&mut self, nodes: impl Iterator<Item = NodeId>, at: PieceId) {
        let from = self.boxes.next_id();
        for node in nodes {
            self.open(node, at);
        }
        // The open stack names them by id, outermost at the bottom once the
        // rows are reversed.
        if let Some(opened) = self.boxes.get_slice_mut(from..self.boxes.next_id()) {
            opened.reverse();
        }
    }

    /// Closes the innermost open box before piece `at`.
    fn close(&mut self, at: PieceId) {
        if let Some(open) = self.open.pop()
            && let Some(found) = self.boxes.get_mut(open)
        {
            found.end = at;
        }
    }

    /// Finds each box's reach along the line from the placed `pieces`, its
    /// edges included.
    ///
    /// One walk widens the innermost open box by each piece. A closing box
    /// widens the box around it. So the walk is linear however deep the
    /// boxes nest.
    fn reach(&mut self, pieces: &LinePieces) {
        self.open.clear();
        let mut next = AnnotationBoxId::new(0);
        for (at, piece) in pieces.logical.iter() {
            work::step();
            while let Some(found) = self.boxes.get(next)
                && found.start <= at
            {
                self.open.push(next);
                next = AnnotationBoxId::new(next.get() + 1);
            }
            if let Some(&top) = self.open.last()
                && let Some(found) = self.boxes.get_mut(top)
            {
                found.widen(piece.inline, piece.inline + piece.advance);
            }
            // The boxes closing after it widen the box around them.
            let after = PieceId::new(at.get() + 1);
            while let Some(&top) = self.open.last()
                && self.boxes.get(top).is_some_and(|b| b.end <= after)
            {
                self.open.pop();
                let Some(&closed) = self.boxes.get(top) else {
                    break;
                };
                if let Some(&outer) = self.open.last()
                    && let Some(around) = self.boxes.get_mut(outer)
                    && let Some((left, right)) = closed.reach
                {
                    around.widen(left, right);
                }
            }
        }
    }
}

heap_bytes! {
    BoxesInAnnotation { boxes, open }
}

/// Where an annotation line is set: its level, its column's box along the
/// line, its baseline from the line box's top, and its ruby container.
#[derive(Copy, Clone, Debug)]
struct AnnotationPlace {
    id: RubyLevelId,
    left: InlineLayoutUnit,
    width: InlineLayoutUnit,
    baseline: LayoutUnit,
    container: NodeId,
    /// The first item at its text's start: the level's opening item, or the
    /// first item of a part continued from the line before.
    first: ItemId,
}

/// Whether `ruby-align` places room among a base's or an annotation's
/// justification opportunities, so that their count matters.
fn spreads_inside(align: RubyAlign) -> bool {
    matches!(align, RubyAlign::SpaceAround | RubyAlign::SpaceBetween)
}

impl<'a> Placer<'a> {
    /// Returns the piece of column `id` that the line being placed sets,
    /// where the column is split.
    fn line_piece(&self, id: RubyColumnId) -> Option<&'a RubyPiece> {
        self.ruby_pieces
            .iter()
            .rev()
            .find(|piece| piece.column == id)
    }

    /// Returns column `id` as the line being placed sets it, with its
    /// piece where the column is split: the piece's base, widths and
    /// overhang.
    fn column_on_line(&self, id: RubyColumnId) -> Option<(RubyColumn, Option<&'a RubyPiece>)> {
        let mut column = self.rubies.get(id)?.clone();
        let piece = self.line_piece(id);
        if let Some(piece) = piece {
            column.base = piece.base.clone();
            column.base_width = piece.base_width;
            column.width = piece.width;
            column.overhang = piece.overhang;
        }
        Some((column, piece))
    }

    /// Closes every column still open at the end of `line`, innermost
    /// first.
    pub(super) fn finish_columns(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
    ) {
        while let Some(open) = self
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
        loop {
            let open = self
                .open_column
                .and_then(|index| rubies.columns.get(index))
                .copied();
            let Some(open) = open else {
                break;
            };
            if self
                .rubies
                .get(open.column)
                .is_none_or(|column| column.close != self.item)
            {
                break;
            }
            self.close_column(pieces, boxes, rubies, line, open);
        }
        while let Some((column, piece)) = self.column_on_line(self.column) {
            work::step();
            let open = column.open;
            if open > self.item {
                break;
            }
            // A column whose base starts at the line's end belongs to the
            // next line. The item at the end boundary only closes the column
            // before it.
            if !column.base.is_empty() && column.base.start >= line.clusters().end {
                break;
            }
            let id = self.column;
            self.column = RubyColumnId::new(id.get() + 1);
            // A column that starts before this line, left behind by a forced
            // break inside it, belongs to the line before. Its text here
            // takes no room.
            if piece.is_none() && (open < self.item || column.base.start < line.clusters().start) {
                // The columns between the pieces continued onto the line and
                // where the cursor stood as it started are all left behind:
                // the walk goes on at the next of either.
                if id < self.line_column {
                    let next = self
                        .ruby_pieces
                        .iter()
                        .map(|piece| piece.column)
                        .filter(|&column| column > id)
                        .fold(self.line_column, RubyColumnId::min);
                    self.column = self.column.max(next);
                }
                continue;
            }
            self.open_column(
                pieces,
                boxes,
                rubies,
                line,
                id,
                at.max(line.clusters().start),
                rest,
            );
            if column.close == self.item
                && let Some(open) = rubies.columns.last().copied()
            {
                self.close_column(pieces, boxes, rubies, line, open);
            }
        }
    }

    /// Opens ruby column `id` on `line` at boundary `at`.
    ///
    /// It pushes the gap before the base, less any overhang onto text before
    /// it. It sets up the room the base's opportunities share, given out as
    /// its pieces are made. It records the column in `rubies`.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn open_column(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
        id: RubyColumnId,
        at: ClusterId,
        rest: InlineLayoutUnit,
    ) {
        let Some((mut column, piece)) = self.column_on_line(id) else {
            return;
        };
        if piece.is_some() {
            let container = column.container(self.stages.content);
            let align = self.box_facts(container).ruby.align;
            let space = column.width - column.base_width;
            // Only room spread inside the base reads its opportunities.
            let opportunities = if space > LayoutUnit::ZERO && spreads_inside(align) {
                let text = self
                    .stages
                    .content
                    .facts
                    .text(self.stages.text_facts(container));
                JustifyOpportunities::from_base(
                    self.stages.content,
                    self.stages.analysis,
                    column.base.clone(),
                    text.justify,
                    self.rubies,
                    (id, id.max(self.line_column)),
                )
                .map_or(0, |found| found.count_range(column.base.clone()))
            } else {
                0
            };
            let spread = RubySpread::from_base(align, space, opportunities);
            column.room_before = spread.start;
            column.room_inside = spread.inside;
        }
        let base = column.base.clone();
        let level = if base.is_empty() {
            self.level
        } else {
            let run = self.advance_run(base.start);
            self.run_level(run, self.level)
        };
        let first_run = self.run.id();
        // Where the column starts the line, no text is beside it, and the
        // breaker paid its overhang back.
        let before = if base.start > line.clusters().start {
            InlineLayoutUnit::from_layout(column.overhang.0)
        } else {
            InlineLayoutUnit::ZERO
        };
        let container = column.container(self.stages.content);
        let content = self.stages.content;
        let text = content.facts.text(self.stages.text_facts(container));
        // At a justified line's edge, the base sits flush with it.
        let (starts, ends) = if column.parent.is_none()
            && self.box_facts(container).ruby.align == RubyAlign::SpaceAround
        {
            let end = piece.map_or_else(
                || self.item_clusters.start(column.close),
                |piece| {
                    if piece.base.end
                        < self
                            .rubies
                            .get(id)
                            .map_or(piece.base.end, |column| column.base.end)
                    {
                        piece.base.end
                    } else {
                        self.item_clusters.start(column.close)
                    }
                },
            );
            (
                self.ruby_edges.start && !base.is_empty() && base.start == line.clusters().start,
                self.ruby_edges.end && end == self.inflow_end(line),
            )
        } else {
            (false, false)
        };
        let (room_before, room_inside, width) =
            column.room_on_edges(starts, ends, self.ruby_edges.room);
        let room_after =
            (width - column.base_width - room_before - room_inside).max(LayoutUnit::ZERO);
        let (room_before, room_inside, width, room_after) = (
            InlineLayoutUnit::from_layout(room_before),
            InlineLayoutUnit::from_layout(room_inside),
            InlineLayoutUnit::from_layout(width),
            InlineLayoutUnit::from_layout(room_after),
        );
        let (shift, _) = boxes.innermost_shifts();
        let parent_room = if column.parent.is_some() && !column.base.is_empty() {
            self.spread.as_ref().map_or(
                (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO),
                |spread| {
                    (
                        spread.room(column.base.start).0,
                        spread.room(ClusterId::new(column.base.end.get() - 1)).1,
                    )
                },
            )
        } else {
            (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO)
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
        self.spread = Justified::from_base(
            content,
            self.stages.analysis,
            base,
            text.justify,
            room_inside,
            self.rubies,
            (id, id.max(self.line_column)),
        );
        let index = rubies.columns.len();
        let parent = self.open_column;
        self.open_column = Some(index);
        rubies.columns.push(LineColumn {
            column: id,
            index,
            base: (column.base.start, column.base.end),
            parent,
            spread: self.spread.as_ref().map(|spread| spread.justification),
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
        self.spread = None;
        let Some((column, piece)) = self.column_on_line(open.column) else {
            return;
        };
        let id = open.column;
        let base = column.base.clone();
        let base_width = if base.is_empty() {
            InlineLayoutUnit::ZERO
        } else {
            self.stages.measured.prefix.get(base.end) - open.start - self.closes_past_base(&column)
        };
        let end = piece.map_or_else(
            || self.item_clusters.start(column.close),
            |piece| {
                if piece.base.end
                    < self
                        .rubies
                        .get(id)
                        .map_or(piece.base.end, |column| column.base.end)
                {
                    piece.base.end
                } else {
                    self.item_clusters.start(column.close)
                }
            },
        );
        let after = if end < line.clusters().end {
            InlineLayoutUnit::from_layout(column.overhang.1)
        } else {
            InlineLayoutUnit::ZERO
        };
        let room = open.width - base_width - open.room_before - open.room_inside - after;
        let level = if base.is_empty() {
            self.level
        } else if self.has_mixed_levels {
            // Both gaps take the lowest level in the base, its boxes' bidi
            // controls included, so reordering keeps them outside every
            // part of it, as Chrome places a column as one unit.
            let controls = pieces
                .logical
                .get_slice(open.leading..pieces.logical.next_id())
                .unwrap_or_default()
                .iter()
                .filter(|piece| piece.kind == PieceKind::Control)
                .map(|piece| piece.level)
                .min();
            let runs = self.lowest_level(open.first_run, base.end);
            let lowest = controls.map_or(runs, |controls| controls.min(runs));
            if let Some(leading) = pieces.logical.get_mut(open.leading) {
                leading.level = lowest;
            }
            lowest
        } else {
            let last = (column.open..column.base_end)
                .ids()
                .rev()
                .find_map(|id| {
                    let item = self.items.get(id)?;
                    let range = self.item_clusters.range(id);
                    (!item.flags.contains(ItemFlags::ANNOTATION) && !range.is_empty())
                        .then(|| ClusterId::new(range.end.get() - 1))
                })
                .unwrap_or(base.start);
            let run = self.advance_run(last);
            self.run_level(run, self.level)
        };
        let (shift, _) = boxes.innermost_shifts();
        let trailing = pieces.logical.next_id();
        let container = column.container(self.stages.content);
        pieces.push(Piece::new(
            PieceKind::Gap,
            container,
            level,
            end..end,
            room + open.parent_room.1,
            boxes.innermost(),
            shift,
        ));
        if let Some(last) = rubies.columns.get_mut(open.index) {
            last.trailing = Some(trailing);
            last.overhang.1 = after;
        }
        self.open_column = open.parent;
        if let Some(parent) = open.parent.and_then(|index| rubies.columns.get(index))
            && let Some(saved) = parent.spread
            && let Some(column) = self.rubies.get(parent.column)
        {
            let container = column.container(self.stages.content);
            self.spread = JustifyOpportunities::from_base_summary(
                self.stages.content,
                self.stages.analysis,
                parent.base.0..parent.base.1,
                self.stages
                    .content
                    .facts
                    .text(self.stages.text_facts(container))
                    .justify,
                self.rubies,
                (parent.column, parent.column.max(self.line_column)),
                saved.summary,
            )
            .map(|opportunities| Justified {
                justification: saved,
                opportunities,
            });
        }
    }

    /// Returns the lowest bidi level of the shaping runs from `first` up to
    /// cluster `end`.
    fn lowest_level(&self, first: ShapedRunId, end: ClusterId) -> BidiLevel {
        let runs = &self.stages.shaped.runs;
        let mut cursor = runs.cursor(first);
        let mut lowest = self.run_level(runs.get(first), self.level);
        while cursor.end() < end {
            work::step();
            let id = cursor.id();
            runs.step(&mut cursor);
            if cursor.id() == id {
                break;
            }
            lowest = lowest.min(self.run_level(runs.get(cursor.id()), self.level));
        }
        lowest
    }

    /// Returns what the prefix charges at the end of `column`'s base that is
    /// not the base's, where the column ends there too.
    ///
    /// A column with no annotation text ends where its base does. The
    /// closing edges after its base, its container's among them, lead the
    /// items at that boundary, so the prefix there holds them.
    fn closes_past_base(&self, column: &RubyColumn) -> InlineLayoutUnit {
        let at = column.base.end;
        if self.item_clusters.start(column.close) != at {
            return InlineLayoutUnit::ZERO;
        }
        let mut sum = InlineLayoutUnit::from_layout(
            column.width - column.overhang.0 - column.overhang.1 - column.base_width,
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

    /// Returns where `line`'s in-flow content ends, leaving out a forced
    /// break but keeping trailing white space.
    ///
    /// This is Chrome's `InflowEndOffsetWithoutForcedBreak`.
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
        let Some(spread) = self.spread.as_ref() else {
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
    /// their text, as Chrome's expansion is part of the glyphs, and returns
    /// whether any text took room in.
    ///
    /// Each base's text takes the room at its opportunities, and its
    /// outermost text the room its column leaves before and after it, out
    /// of the gaps that held them. The text of a column nested in the base
    /// takes none of its parent's room. The pieces are in visual order, so the
    /// gap visually first is on the base's left. Nothing moves along the
    /// line.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    pub(super) fn absorb_base_spread(pieces: &mut LinePieces, rubies: &LineRubies) -> bool {
        let mut moved = pieces.absorb_spread();
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
                if taken > InlineLayoutUnit::ZERO {
                    moved = true;
                    if let Some(gap) = pieces.logical.get_mut(gap) {
                        gap.advance = gap.advance - taken;
                    }
                }
            }
        }
        moved
    }

    /// Writes the annotation lines of `line`'s ruby columns to `out`, after
    /// the line's own items.
    ///
    /// Each level sits over its column's box, which the column's gaps in
    /// `pieces` bound. Its baseline is where the breaker stacked its level.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    pub(super) fn annotations(
        &mut self,
        pieces: &LinePieces,
        rubies: &mut LineRubies,
        out: &mut Fragments,
        line: &LineView<'_>,
    ) {
        let LineRubies {
            columns,
            bands,
            annotation,
            boxes,
        } = rubies;
        let Some(last) = columns.last() else {
            return;
        };
        // The columns open across the line's start come first, then those
        // whose bases start on it.
        let line_start = line.clusters().start;
        let split = columns
            .iter()
            .position(|open| {
                self.rubies
                    .get(open.column)
                    .is_none_or(|column| column.base.start >= line_start)
            })
            .unwrap_or(columns.len());
        let on_line = columns.get(split).map_or(Range::default(), |starts| {
            starts.column..RubyColumnId::new(last.column.get() + 1)
        });
        let open = columns.get(..split).unwrap_or_default().iter();
        bands.stack(
            self.rubies,
            open.map(|open| open.column)
                .chain((on_line.start.get()..on_line.end.get()).map(RubyColumnId::new)),
            line.extent,
            self.lines
                .ruby
                .as_deref()
                .map(|ruby| (ruby, self.ruby_pieces)),
        );
        // Closing-item order is postorder: child annotations precede their
        // parent's, while siblings retain text order.
        if columns.iter().any(|open| {
            self.rubies
                .get(open.column)
                .is_some_and(|column| column.parent.is_some())
        }) {
            stable_sort_by_key(&mut columns[..], |open| {
                self.rubies.get(open.column).map(|column| column.close)
            });
        }
        let baseline = line.ascent();
        for index in 0..columns.len() {
            let Some(&open) = columns.get(index) else {
                continue;
            };
            let (Some((column, piece)), Some(trailing)) =
                (self.column_on_line(open.column), open.trailing)
            else {
                continue;
            };
            let (Some(lead), Some(trail)) = (
                pieces.logical.get(open.leading),
                pieces.logical.get(trailing),
            ) else {
                continue;
            };
            // The column's box spans from its start gap to its end gap,
            // with the overhangs. Its start is on the left for a
            // left-to-right base. It is at least the column's width, wider
            // where justification spread the base, as Chrome widens it
            // (`JustifyResults`).
            let (left, right) = if lead.inline <= trail.inline {
                (
                    lead.inline - open.overhang.0 + open.parent_room.0,
                    trail.inline + trail.advance + open.overhang.1 - open.parent_room.1,
                )
            } else {
                (
                    trail.inline - open.overhang.1 + open.parent_room.1,
                    lead.inline + lead.advance + open.overhang.0 - open.parent_room.0,
                )
            };
            let width = (right - left).max(InlineLayoutUnit::from_layout(column.width));
            let container = column.container(self.stages.content);
            let levels = column.levels.clone();
            for id in levels.ids() {
                work::step();
                let Some(level) = self.rubies.level(id) else {
                    continue;
                };
                let mut level = level.clone();
                let mut first = level.open;
                if let Some(piece) = piece {
                    let Some(part) = self
                        .lines
                        .ruby
                        .as_ref()
                        .and_then(|ruby| ruby.level(piece, id))
                    else {
                        continue;
                    };
                    if part.clusters.is_empty() {
                        continue;
                    }
                    if part.clusters.start > level.clusters.start {
                        first = part.item;
                    }
                    level.clusters = part.clusters.clone();
                    level.clusters.end = part.visible_end;
                    if level.clusters.is_empty() {
                        continue;
                    }
                    level.width = part.width;
                    // Only room spread inside the annotation reads its
                    // opportunities.
                    level.opportunities = if width.to_layout() - level.width > LayoutUnit::ZERO
                        && spreads_inside(self.box_facts(container).ruby.align)
                    {
                        let node = self
                            .items
                            .get(level.open)
                            .map_or(container, |item| item.node);
                        let justify = self
                            .stages
                            .content
                            .facts
                            .text(self.stages.text_facts(node))
                            .justify;
                        JustifyOpportunities::new(
                            self.stages.content,
                            self.stages.analysis,
                            level.clusters.clone(),
                            justify,
                        )
                        .map_or(0, |found| found.count_range(level.clusters.clone()))
                    } else {
                        0
                    };
                }
                // The level's depth on its side, counting out from the base,
                // picks its band on the line.
                let side = level.side;
                let raise = bands
                    .raise(side, level.depth as usize)
                    .unwrap_or(LayoutUnit::ZERO);
                let place = AnnotationPlace {
                    id,
                    left,
                    width,
                    baseline: baseline - open.shift - raise,
                    container,
                    first,
                };
                self.emit_annotation(annotation, boxes, out, line, &level, place);
            }
        }
    }

    /// Writes one annotation line, `level` of a column, at `place`.
    ///
    /// Its text is spread by `ruby-align`, as Chrome's `ApplyRubyAlign` does,
    /// reordered as a line of its own, and set from the column box's left.
    /// Then its items are written.
    // Out of line, so that a line with no ruby pays nothing for it.
    #[inline(never)]
    fn emit_annotation(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &mut BoxesInAnnotation,
        out: &mut Fragments,
        line: &LineView<'_>,
        level: &RubyLevel,
        place: AnnotationPlace,
    ) {
        let clusters = level.clusters.clone();
        let content = self.stages.content;
        // The annotation's own element: its opening item's node.
        let node = self
            .items
            .get(level.open)
            .map_or(place.container, |item| item.node);
        let text = content.facts.text(self.stages.text_facts(node));
        let request = content.facts.text_request(self.stages.text_facts(node));
        let size = self
            .stages
            .fonts
            .resolution(request)
            .map_or(0.0, |resolution| resolution.computed);
        // Its text is walked from its first cluster, found through its
        // level, never through the line holding a cluster. Its direction is
        // its first segment's.
        let walk = Segments::from_item(
            &self.stages,
            line.paragraph,
            place.first,
            clusters.start..self.count,
        );
        let first = self.first_level(walk, clusters.end);
        let rtl = self.ruby_in_bidi && !clusters.is_empty() && first.is_some_and(BidiLevel::is_rtl);
        let spread = RubySpread::from_annotation(
            self.box_facts(place.container).ruby.align,
            self.text_align,
            place.width.to_layout() - level.width,
            level.opportunities,
            size,
            rtl,
        );
        let spreading = Justified::new(
            content,
            self.stages.analysis,
            clusters,
            text.justify,
            InlineLayoutUnit::from_layout(spread.inside),
        );
        let parts = (node, place.first);
        self.annotation_pieces(pieces, boxes, line, level, parts, walk, spreading.as_ref());
        // Reorder by level as a line of its own, and set from the box's left.
        // Right to left, the logical start is on the right.
        let placed = pieces.logical.as_slice().iter();
        let base = placed.clone().map(|piece| piece.level).min();
        let total = placed.fold(InlineLayoutUnit::ZERO, |sum, piece| sum + piece.advance);
        pieces.reorder(base.unwrap_or(self.level));
        let start = InlineLayoutUnit::from_layout(spread.start);
        let lead = if rtl {
            place.width - start - total
        } else {
            start
        };
        // The text takes in the room spread at its opportunities, and the
        // room on its left and right, as Chrome's expansion is part of its
        // glyphs.
        let moved = pieces.absorb_spread();
        let all = PieceId::new(0)..pieces.logical.next_id();
        let trail = (place.width - lead - total).max(InlineLayoutUnit::ZERO);
        let taken = pieces.absorb_end(all.clone(), (lead, true), |_| false);
        let given = pieces.absorb_end(all, (trail, false), |_| false);
        pieces.place(place.left + lead - taken, &Table::new());
        boxes.reach(pieces);
        self.annotation_items(pieces, boxes, out, node, place);
        if moved || taken > InlineLayoutUnit::ZERO || given > InlineLayoutUnit::ZERO {
            out.push_spread(line.id, pieces.logical.as_slice());
        }
    }

    /// Returns the boxes inside annotation `annotation` open across the
    /// boundary whose first item is `first`, innermost first.
    ///
    /// A box closing at the boundary is open across it, and one opening
    /// there is not.
    fn boxes_around(&self, first: ItemId, annotation: NodeId) -> impl Iterator<Item = NodeId> {
        let nodes = &self.stages.content.nodes;
        let start = self.items.get(first).map(|item| {
            if item.kind == ItemKind::Close {
                item.node
            } else {
                nodes.parent(item.node)
            }
        });
        let mut next = start.filter(|&node| node != annotation && node != NodeId::BLOCK);
        from_fn(move || {
            loop {
                work::step();
                let node = next?;
                let parent = nodes.parent(node);
                next = (parent != node && parent != annotation && parent != NodeId::BLOCK)
                    .then_some(parent);
                if nodes.kind(node) == Some(NodeKind::Box) {
                    return Some(node);
                }
            }
        })
    }

    /// Makes the pieces of annotation line `level`, whose element is `node`,
    /// and records the boxes in it.
    ///
    /// - Text: a piece per cluster, in its shaping run's font and level,
    ///   stepping as the text does in the line's variant, with gaps where
    ///   `spreading` gives room.
    /// - Box edges: gaps taking the room the level's width counted, as in
    ///   Chrome, at the level of the text beside them.
    ///
    /// The walk runs from `first`, the annotation's opening item or the
    /// first item of a continued part, to its closing, going past its last
    /// cluster to meet it. A continued part opens the boxes around its start
    /// first, found up its first item's ancestors.
    #[allow(clippy::too_many_arguments)]
    fn annotation_pieces(
        &self,
        pieces: &mut LinePieces,
        boxes: &mut BoxesInAnnotation,
        line: &LineView<'_>,
        level: &RubyLevel,
        (node, first): (NodeId, ItemId),
        mut walk: Segments<'a>,
        spreading: Option<&Justified<'_>>,
    ) {
        pieces.logical.clear();
        boxes.clear();
        if first != level.open {
            boxes.open_around(self.boxes_around(first, node), pieces.logical.next_id());
        }
        let clusters = level.clusters.clone();
        let words = self.input.stages.measured.word_spacing_rule;
        // The line's reshaped edges that hold some of the level.
        let shapes = self.lines.edges.line_edges_touching(line, &clusters);
        // The bidi level of the last text segment, which a closing edge
        // after it takes, as does an opening edge after the last.
        let mut before = None;
        loop {
            work::step();
            let ahead = walk;
            let segment = match walk.next() {
                // Its own opening edge, where its line starts with its text.
                Some(Step::Item { at, id }) if id == level.open => {
                    if let Some(item) = self.items.get(id)
                        && self.item_clusters.start(id) >= clusters.start
                    {
                        let room = measure::edge_room(self.stages.content, None, item, true);
                        let beside = self.first_level(ahead, clusters.end);
                        self.annotation_edge(pieces, item.node, beside, at, room);
                    }
                    continue;
                }
                // Items before its own opening belong to the base or to
                // earlier levels.
                Some(Step::Item { id, .. }) if id < level.open => continue,
                // A box's edge, until its own closing.
                Some(Step::Item { at, id }) if at <= clusters.end => {
                    let Some(item) = self.items.get(id) else {
                        continue;
                    };
                    if item.kind == ItemKind::AnnotationClose && item.node == node {
                        // Its own closing edge.
                        let room = measure::edge_room(self.stages.content, None, item, false);
                        let beside = before.or_else(|| self.first_level(ahead, clusters.end));
                        self.annotation_edge(pieces, item.node, beside, at, room);
                        break;
                    }
                    let opens = item.kind == ItemKind::Open;
                    let actual = self.item_clusters.start(id);
                    if actual < clusters.start {
                        if opens {
                            boxes.open(item.node, pieces.logical.next_id());
                        } else if item.kind == ItemKind::Close {
                            boxes.close(pieces.logical.next_id());
                        }
                        continue;
                    }
                    if actual == clusters.end && opens {
                        break;
                    }
                    if !item.flags.contains(ItemFlags::ANNOTATION)
                        || !(opens || item.kind == ItemKind::Close)
                    {
                        continue;
                    }
                    let after = || self.first_level(ahead, clusters.end);
                    let beside = if opens {
                        after().or(before)
                    } else {
                        before.or_else(after)
                    };
                    if opens {
                        boxes.open(item.node, pieces.logical.next_id());
                    }
                    pieces.push(Piece::new(
                        PieceKind::Gap,
                        item.node,
                        beside.unwrap_or(self.level),
                        at..at,
                        measure::edge_room(self.stages.content, None, item, opens),
                        None,
                        LayoutUnit::ZERO,
                    ));
                    if !opens {
                        boxes.close(pieces.logical.next_id());
                    }
                    continue;
                }
                Some(Step::Segment(segment)) if segment.start < clusters.end => segment,
                _ => break,
            };
            let Some(item) = self.items.get(segment.item) else {
                continue;
            };
            if !item.flags.contains(ItemFlags::ANNOTATION) {
                continue;
            }
            let run = self.stages.shaped.runs.get(segment.run);
            let bidi = self.run_level(run, self.level);
            before = Some(bidi);
            if !matches!(item.kind, ItemKind::Text | ItemKind::Atomic) {
                continue;
            }
            // An atomic inline steps its margin box and its spacing, as on a
            // line.
            if item.kind == ItemKind::Atomic {
                let text = run.map(|run| AnnotationText::new(&self.stages, words, run.shaping));
                let width = self
                    .stages
                    .content
                    .item_atomic(segment.item)
                    .map_or(InlineLayoutUnit::ZERO, |atomic| {
                        InlineLayoutUnit::from_layout(atomic.margin_inline)
                    });
                for cluster in (segment.start..segment.end.min(clusters.end)).ids() {
                    work::step();
                    let next = ClusterId::new(cluster.get() + 1);
                    let spacing = text
                        .as_ref()
                        .map_or(InlineLayoutUnit::ZERO, |text| text.spacing_after(cluster));
                    let piece = Piece::new(
                        PieceKind::Atomic,
                        item.node,
                        bidi,
                        cluster..next,
                        width + spacing,
                        None,
                        LayoutUnit::ZERO,
                    );
                    let room = spreading.map(|spread| spread.room(cluster));
                    pieces.push_with_room(piece, room.unwrap_or_default());
                }
                continue;
            }
            let Some(run) = run else {
                continue;
            };
            let (font, orientation) = (Some(run.font), self.orientation(Some(run)));
            // Its clusters step as its text does in the line's variant,
            // spaced as its shaping run's facts say.
            let text = AnnotationText::new(&self.stages, words, run.shaping);
            for cluster in (segment.start..segment.end.min(clusters.end)).ids() {
                work::step();
                let next = ClusterId::new(cluster.get() + 1);
                let advance = shapes
                    .iter()
                    .find_map(|shape| shape.entry(cluster))
                    .and_then(|entry| self.lines.edges.advances.get(entry))
                    .map_or_else(
                        || text.step(cluster),
                        |&advance| InlineLayoutUnit::from_text(advance),
                    );
                let piece = Piece {
                    font,
                    orientation,
                    ..Piece::new(
                        PieceKind::Text,
                        item.node,
                        bidi,
                        cluster..next,
                        advance,
                        None,
                        LayoutUnit::ZERO,
                    )
                };
                let room = spreading.map(|spread| spread.room(cluster));
                pieces.push_with_room(piece, room.unwrap_or_default());
            }
        }
        while !boxes.open.is_empty() {
            boxes.close(pieces.logical.next_id());
        }
    }

    /// Adds a gap for an annotation's own edge at boundary `at`, taking
    /// `room`, at the level of the text beside it, `beside`.
    ///
    /// The level's width counts the edges, so its pieces add up to it.
    fn annotation_edge(
        &self,
        pieces: &mut LinePieces,
        node: NodeId,
        beside: Option<BidiLevel>,
        at: ClusterId,
        room: InlineLayoutUnit,
    ) {
        if room == InlineLayoutUnit::ZERO {
            return;
        }
        pieces.push(Piece::new(
            PieceKind::Gap,
            node,
            beside.unwrap_or(self.level),
            at..at,
            room,
            None,
            LayoutUnit::ZERO,
        ));
    }

    /// Writes the items of the annotation line for `node` at `place`: its
    /// head, then its kept boxes, then its text and atomic inlines.
    fn annotation_items(
        &self,
        pieces: &LinePieces,
        boxes: &BoxesInAnnotation,
        out: &mut Fragments,
        node: NodeId,
        place: AnnotationPlace,
    ) {
        // Kept boxes go after the head and before the text, outermost first,
        // in paint order. Each spans its border box over all its pieces, as
        // Chrome gives a box in an annotation its own fragment. Where
        // reordering splits a box's pieces, one item covers them all.
        let kept = boxes
            .boxes
            .as_slice()
            .iter()
            .filter(|found| found.reach.is_some() && self.boxes.get(found.node).is_some())
            .count();
        let leaves = pieces
            .logical
            .as_slice()
            .iter()
            .filter(|piece| matches!(piece.kind, PieceKind::Text | PieceKind::Atomic))
            .count();
        let head = FragmentItem::from_annotation(
            node,
            self.level,
            place.id,
            place.left,
            place.width.to_layout(),
            place.baseline,
            kept + leaves,
        );
        if out.items.push(head).is_none() {
            debug_assert!(false, "no more items than a FragmentItemId names");
            return;
        }
        for found in boxes.boxes.as_slice() {
            let Some((reach_left, reach_right)) = found.reach else {
                continue;
            };
            if self.boxes.get(found.node).is_none() {
                continue;
            }
            let (left, right) = self.box_facts(found.node).margin_line;
            let from = reach_left + InlineLayoutUnit::from_layout(left);
            let to = (reach_right - InlineLayoutUnit::from_layout(right)).max(from);
            let level = pieces
                .logical
                .get(found.start)
                .map_or(self.level, |piece| piece.level);
            let item = FragmentItem::from_box_part(
                found.node,
                level,
                from,
                (to - from).to_layout(),
                place.baseline,
                FragmentItemFlags::NONE,
                0,
                None,
            );
            if out.items.push(item).is_none() {
                debug_assert!(false, "no more items than a FragmentItemId names");
                return;
            }
        }
        for &logical in pieces.order.as_slice() {
            let Some(piece) = pieces.logical.get(logical) else {
                continue;
            };
            let (kind, flags) = match piece.kind {
                PieceKind::Text => (FragmentItemKind::Text, self.emphasis(piece.node)),
                PieceKind::Atomic => (FragmentItemKind::Atomic, FragmentItemFlags::NONE),
                _ => continue,
            };
            let flags = piece
                .flags
                .union(FragmentItemFlags::ANNOTATION)
                .union(flags);
            let item = piece.item(kind, flags, place.baseline);
            if out.items.push(item).is_none() {
                debug_assert!(false, "no more items than a FragmentItemId names");
                return;
            }
        }
    }

    /// Returns the bidi level of the first segment `walk` meets before
    /// `end`, if any.
    ///
    /// An annotation takes its direction from it, and an edge takes the
    /// level of the text after it.
    fn first_level(&self, mut walk: Segments<'_>, end: ClusterId) -> Option<BidiLevel> {
        let segment = walk
            .find_map(|step| match step {
                Step::Segment(segment) => Some(segment),
                Step::Item { .. } => None,
            })
            .filter(|segment| segment.start < end)?;
        Some(self.run_level(self.stages.shaped.runs.get(segment.run), self.level))
    }
}
