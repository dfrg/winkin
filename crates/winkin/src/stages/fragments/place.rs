//! Lays out one line at a time and writes its items once.
//!
//! `Placer::place_line` runs each line's phases in order:
//! - the pieces, in logical order (`pieces`);
//! - `line-padding`, justification (`justify`) and alignment;
//! - reordering by bidi level;
//! - box parts and the room their edges take (`boxes`);
//! - positions, left to right;
//! - truncation, on a line cut for an ellipsis (`cut`);
//! - the items, then the ruby annotation lines (`annotations`).
//!
//! **Alignment** is Chrome's `LineOffsetForTextAlign`. It sees the content,
//! and only the fitting part of preserved white space before a forced break
//! or the block's end. It moves the whole line, tabs included.
//!
//! **Bidi** follows Chrome.
//! - L1: the line's trailing preserved white space takes the paragraph's
//!   level. Under `break-spaces` it splits from the text before it, as
//!   Blink's `SplitTrailingBidiPreservedSpace` does, only where it ends
//!   its item or is all its item holds on the line. Inside one item's text
//!   at one level it keeps that level.
//! - L2: the pieces reorder by level with the resolver's `reorder`, as
//!   Blink's `BidiReorder` does. A piece with no level of its own takes the
//!   next piece's, or the paragraph's at the line's end.
//!
//! **Each phase has its own scratch:** [`LinePieces`], [`LineBoxes`],
//! [`LeafCounts`] and [`LineRubies`]. Each goes only to the phases that use
//! it, and is cleared, never dropped.

use super::{Gates, LineState, Piece, PieceKind, Placer};
use alloc::vec::Vec;
use core::iter;

use super::boxes::{BoxPartId, LineBoxId, LineBoxes};
use super::boxes::{LeafCounts, paint_order};
use super::cut::LineEllipsis;
use super::pieces::TabLine;
use super::ruby::LineRubies;
use super::{
    BlockResult, FragmentItem, FragmentItemFlags, FragmentItemId, Fragments, JustifiedLineId,
    PlaceInput,
};
use crate::data::{Id, Table, define_id, heap_bytes, u32_to_index};
use crate::stages::analysis::{BidiLevel, ClusterId, ParagraphFlags, ParagraphId};
use crate::stages::content::{BoxFacts, ContentFlags, ItemId, NodeId};
use crate::stages::lines::{LineBand, LineFlags, LineId, LineView};
use crate::stages::measure::{AutospaceRules, KeptBoxId, RubyColumnId};
use crate::stages::shape::ShapedRunId;
use crate::style::{FirstLineVariant, TextAlign, TextGroupAlign};
use crate::unicode::reorder_bidi;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

define_id! {
    /// Names a piece of the line being laid out by its logical place, in
    /// [`LinePieces`].
    pub(super) struct PieceId(u32);
}

define_id! {
    /// Names a place in the line's visual order, counted from the left.
    ///
    /// [`LinePieces`] says which piece stands there. [`LineBoxes`] holds the
    /// room box edges take beside it.
    pub(super) struct VisualId(u32);
}

/// The piece an ellipsis cut divides: the piece, the boundary, and the kept
/// part's advance.
///
/// It is written as two items, the kept part and the hidden part, as Chrome
/// hides its item and draws a truncated one in its place. A line has one at
/// most, so the pieces don't carry it.
#[derive(Copy, Clone, Debug)]
pub(super) struct PieceSplit {
    pub(super) piece: PieceId,
    pub(super) at: ClusterId,
    pub(super) kept: InlineLayoutUnit,
}

/// How a line is cut for an ellipsis: the ellipsis, and the piece the cut
/// divides, if any.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineCut {
    pub(super) ellipsis: LineEllipsis,
    pub(super) split: Option<PieceSplit>,
}

/// Which ends of the line a ruby column standing there sits flush with.
///
/// These are Chrome's `on_start_edge` and `on_end_edge`
/// (`LogicalLineBuilder::PlaceRubyColumn`). They apply on a justified line
/// in a block with no bidi. The start also applies on a paragraph's last
/// line when the block's `text-align` is `justify`, as in Chrome. The end
/// applies only when the line itself is justified.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct RubyEdges {
    pub(super) start: bool,
    pub(super) end: bool,
    /// The line's room, which a column holding the whole line spreads its
    /// base over.
    pub(super) room: LayoutUnit,
}

/// The pieces of a line being laid out and their visual order: the line's
/// own, or an annotation line's.
pub(super) struct LinePieces {
    /// Its pieces in logical order: its content, then what hangs.
    pub(super) logical: Table<PieceId, Piece>,
    /// Each piece's level as the reordering reads it.
    pub(super) levels: Table<PieceId, BidiLevel>,
    /// The pieces in visual order.
    pub(super) order: Table<VisualId, PieceId>,
    /// The resolver's `reorder` output as raw indexes, turned into `order`
    /// in [`reorder`](Self::reorder).
    resolved: Vec<u32>,
    /// The anchors at the line's end that go on to the next line's start,
    /// in order.
    pub(super) carried: Vec<Piece>,
    /// The advance of the hanging pieces, summed as they are pushed.
    hang: InlineLayoutUnit,
    /// The line's hyphen, once pushed. A line has one at most.
    hyphen: Option<PieceId>,
}

impl LinePieces {
    /// Makes an empty table, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            logical: Table::new(),
            levels: Table::new(),
            order: Table::new(),
            resolved: Vec::new(),
            carried: Vec::new(),
            hang: InlineLayoutUnit::ZERO,
            hyphen: None,
        }
    }

    /// Forgets the last line's pieces, keeping the anchors it carried on.
    pub(super) fn clear(&mut self) {
        self.logical.clear();
        self.hang = InlineLayoutUnit::ZERO;
        self.hyphen = None;
    }

    /// Adds the anchors the line before carried on, in the box `owner`
    /// open across the line's start.
    fn take_carried(&mut self, owner: Option<LineBoxId>) {
        for at in 0..self.carried.len() {
            if let Some(&piece) = self.carried.get(at) {
                self.push(Piece { owner, ..piece });
            }
        }
        self.carried.clear();
    }

    /// Returns the advance of the hanging pieces.
    ///
    /// Nothing changes a hanging piece's advance once it is pushed: it
    /// takes no justification room and no ruby room before alignment.
    fn hang(&self) -> InlineLayoutUnit {
        self.hang
    }

    /// Adds `piece` as a piece of its own.
    #[inline]
    pub(super) fn push(&mut self, piece: Piece) {
        if piece.flags.contains(FragmentItemFlags::HANGS) {
            self.hang += piece.advance;
        }
        self.logical
            .push_bounded(piece, "no more pieces on a line than a PieceId names");
    }

    /// Adds the line's hyphen, `piece`, and keeps where it is.
    pub(super) fn push_hyphen(&mut self, piece: Piece) {
        self.hyphen = Some(self.logical.next_id());
        self.push(piece);
    }

    /// Returns the line's hyphen, where it has one.
    fn hyphen(&self) -> Option<PieceId> {
        self.hyphen
    }

    /// Adds `piece` as [`push_or_join`](Self::push_or_join) does, with a gap
    /// for any room `before` and `after` it.
    ///
    /// A spread ruby base or annotation cuts its text at each opportunity
    /// this way.
    pub(super) fn push_with_room(
        &mut self,
        piece: Piece,
        (before, after): (InlineLayoutUnit, InlineLayoutUnit),
    ) {
        if before > InlineLayoutUnit::ZERO {
            self.push(piece.gap(piece.start, before));
        }
        self.push_or_join(piece);
        if after > InlineLayoutUnit::ZERO {
            self.push(piece.gap(piece.end, after));
        }
    }

    /// Inserts `piece` before the piece `at`, moving the rest one place on.
    fn insert(&mut self, at: PieceId, piece: Piece) {
        let end = PieceId::new(self.logical.next_id().get() + 1);
        self.push(piece);
        if let Some(moved) = self.logical.get_slice_mut(at..end) {
            moved.rotate_right(1);
        }
        if let Some(hyphen) = self.hyphen.as_mut()
            && *hyphen >= at
        {
            *hyphen = PieceId::new(hyphen.get() + 1);
        }
    }

    /// Adds `piece`, joining it to the last piece where they match.
    ///
    /// They match when kind, flags, node, level, owner, orientation and font
    /// agree and the last ends where `piece` starts. A tab never joins, so
    /// its item's size is its width.
    #[inline]
    pub(super) fn push_or_join(&mut self, piece: Piece) {
        work::step();
        if !piece.flags.contains(FragmentItemFlags::TAB)
            && let Some(last) = self.logical.last_mut()
            && last.kind == piece.kind
            && last.flags == piece.flags
            && last.node == piece.node
            && last.level == piece.level
            && last.owner == piece.owner
            && last.end == piece.start
            && last.orientation == piece.orientation
            && (piece.font.is_none() || last.font.is_none() || last.font == piece.font)
        {
            last.end = piece.end;
            last.advance += piece.advance;
            if piece.flags.contains(FragmentItemFlags::HANGS) {
                self.hang += piece.advance;
            }
            // The run of the first shaped cluster, or of the first.
            if last.run.is_none() || (last.font.is_none() && piece.font.is_some()) {
                last.run = piece.run;
            }
            last.font = last.font.or(piece.font);
            last.opportunities = last.opportunities.saturating_add(piece.opportunities);
            return;
        }
        self.push(piece);
    }

    /// Returns the piece at place `at` in visual order.
    #[inline]
    pub(super) fn visual(&self, at: VisualId) -> Option<&Piece> {
        self.order
            .get(at)
            .and_then(|&logical| self.logical.get(logical))
    }

    /// Puts the pieces in visual order by level with the resolver's
    /// `reorder`; `base` is the lowest level.
    ///
    /// A piece with no level of its own takes the next piece's, or `base` at
    /// the end, as Blink's opaque items do. `levels` keeps the result.
    ///
    /// Where every piece is at `base`, as on most lines, the order is
    /// straight through, or reversed for right-to-left, without the walk.
    pub(super) fn reorder(&mut self, base: BidiLevel) {
        self.levels.clear();
        let count = self.logical.len();
        if self.levels.extend(iter::repeat_n(base, count)).is_none() {
            debug_assert!(false, "no more pieces on a line than a PieceId names");
        }
        let uniform = self
            .logical
            .as_slice()
            .iter()
            .all(|piece| piece.kind.is_opaque() || piece.level == base);
        if !uniform {
            let mut next = base;
            for (at, piece) in self.logical.iter().rev() {
                if !piece.kind.is_opaque() {
                    next = piece.level;
                }
                if let Some(level) = self.levels.get_mut(at) {
                    *level = next;
                }
            }
        }
        self.order.clear();
        let pieces = self.logical.ids();
        let placed = if uniform && !base.is_rtl() {
            self.order.extend(pieces)
        } else if uniform {
            self.order.extend(pieces.rev())
        } else {
            self.resolved.clear();
            self.resolved.resize(self.logical.len(), 0);
            let levels = &self.levels;
            reorder_bidi(&mut self.resolved, |i| {
                levels.get(PieceId::new(i)).copied().unwrap_or(base).get()
            });
            let resolved = self.resolved.iter();
            self.order
                .extend(resolved.map(|&logical| PieceId::new(u32_to_index(logical))))
        };
        if placed.is_none() {
            debug_assert!(false, "no more pieces on a line than a VisualId names");
        }
    }

    /// Places the pieces left to right from `start`, adding the room `rooms`
    /// gives before and after each place.
    ///
    /// Where `counts` is given, it counts the leaves before each place as
    /// the walk passes them.
    pub(super) fn place(
        &mut self,
        start: InlineLayoutUnit,
        rooms: &Table<VisualId, (InlineLayoutUnit, InlineLayoutUnit)>,
        mut counts: Option<&mut LeafCounts>,
    ) {
        if let Some(counts) = counts.as_deref_mut() {
            counts.begin_count();
        }
        let mut pen = start;
        for (at, &logical) in self.order.iter() {
            work::step();
            let (before, after) = rooms.get(at).copied().unwrap_or_default();
            pen += before;
            if let Some(piece) = self.logical.get_mut(logical) {
                piece.inline = pen;
                pen += piece.advance;
                if let Some(counts) = counts.as_deref_mut() {
                    counts.count(piece.kind.is_leaf());
                }
            }
            pen += after;
        }
    }
}

/// Line layout's working memory, one part for each phase of a line.
///
/// Every vector is cleared, never dropped, so a warm relayout allocates
/// nothing.
pub(crate) struct PlaceScratch {
    /// The line's pieces and their visual order.
    pieces: LinePieces,
    /// Its kept boxes and their box parts.
    boxes: LineBoxes,
    /// Where its box parts' items go, and their paint order.
    counts: LeafCounts,
    /// Its ruby columns and their annotation lines.
    rubies: LineRubies,
}

impl PlaceScratch {
    /// Makes empty scratch, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            pieces: LinePieces::new(),
            boxes: LineBoxes::new(),
            counts: LeafCounts::new(),
            rubies: LineRubies::new(),
        }
    }

    /// Forgets the boxes and anchors the last line carried on, so none of
    /// another layout carries onto the first line.
    pub(super) fn begin(&mut self) {
        self.boxes.open.clear();
        self.pieces.carried.clear();
    }

    /// Empties the line's tables, keeping the boxes and anchors the line
    /// before carried on.
    fn begin_line(&mut self) {
        self.pieces.clear();
        self.boxes.begin_line();
        self.rubies.columns.clear();
    }
}

heap_bytes! {
    PlaceScratch { pieces, boxes, counts, rubies }
}

heap_bytes! {
    LinePieces { logical, levels, order, resolved, carried; hang, hyphen }
}

/// `text-group-align`: how far each line's band narrows, and how far its
/// left end moves in, to align the lines as one group.
///
/// Chrome doesn't parse it; this follows CSS. `least` is the least room any
/// line spares, found before any line is placed. If a line overflows, the
/// group spares nothing.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct Group {
    least: LayoutUnit,
    left: LayoutUnit,
}

impl Group {
    /// Finds the group for `input`'s lines, as the block's style asks.
    fn new(input: &PlaceInput<'_>) -> Self {
        let block = input.stages.content.block;
        let lines = input.lines.lines.as_slice();
        let Some(first) = lines.first() else {
            return Self::default();
        };
        if block.text_group_align == TextGroupAlign::None {
            return Self::default();
        }
        let least = lines
            .iter()
            .map(|line| line.band.width() - (line.indent + line.width))
            .min()
            .unwrap_or(LayoutUnit::ZERO);
        if least <= LayoutUnit::ZERO {
            return Self::default();
        }
        // A block whose paragraphs each pick their direction uses the first
        // line's.
        let rtl = BidiLevel::from_direction(block.direction)
            .unwrap_or_else(|| first.level(&input.stages.analysis.paragraphs))
            .is_rtl();
        let left = block.text_group_align.left_inset(least, rtl);
        Self { least, left }
    }

    /// Returns `band` narrowed for the group.
    fn narrow(self, band: LineBand) -> LineBand {
        if self.least == LayoutUnit::ZERO {
            return band;
        }
        band.inset(self.left, self.least - self.left)
    }
}

/// Which way a line's content goes in its band.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Side {
    Left,
    Right,
    Center,
}

impl Side {
    /// Returns the side a line set in `align` goes to.
    ///
    /// A `justify` line here is one that cannot be justified, so it is set
    /// at its start, as Chrome sets it.
    pub(super) fn from_align(align: TextAlign, rtl: bool) -> Self {
        match align {
            TextAlign::Start | TextAlign::Justify if rtl => Self::Right,
            TextAlign::Start | TextAlign::Justify => Self::Left,
            TextAlign::End if rtl => Self::Left,
            TextAlign::End => Self::Right,
            TextAlign::Left => Self::Left,
            TextAlign::Right => Self::Right,
            TextAlign::Center => Self::Center,
        }
    }

    /// Returns how far a line on this side moves in from its band's left,
    /// given `spare` room, as Chrome's `LineOffsetForTextAlign` does.
    ///
    /// A line that overflows spills out on its end side. A centered line
    /// moves by half the room, truncated on the grid.
    pub(super) fn left_offset(self, rtl: bool, spare: LayoutUnit) -> LayoutUnit {
        match self {
            Self::Left if rtl => spare.min(LayoutUnit::ZERO),
            Self::Left => LayoutUnit::ZERO,
            Self::Right if rtl => spare,
            Self::Right => spare.max(LayoutUnit::ZERO),
            Self::Center if rtl && spare < LayoutUnit::ZERO => spare,
            Self::Center => spare.half().max(LayoutUnit::ZERO),
        }
    }
}

/// Where a line is set in its band.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineOffset {
    /// Its line box's left edge, from the area's line-left.
    pub(super) left: LayoutUnit,
    /// Where its first piece in visual order starts, from the line box's
    /// left.
    ///
    /// Left to right, it is the indent. Right to left, it is where hanging
    /// content outside the line box's left starts, so the content ends at
    /// its width.
    start: InlineLayoutUnit,
}

impl Gates {
    /// Reads what every line of `input` reads the same.
    fn new(input: &PlaceInput<'_>) -> Self {
        let stages = input.stages.variant(FirstLineVariant::Standard);
        let (content, analysis, measured) = (stages.content, stages.analysis, stages.measured);
        let style = content.block;
        Self {
            text_align: style.text_align,
            text_align_last: style.text_align_last,
            has_box_edges: content.flags.contains(ContentFlags::BOXES_WITH_EDGES),
            has_mixed_levels: analysis.flags.contains(ParagraphFlags::MIXED_LEVELS),
            autospace: AutospaceRules::new(content),
            ends_on_a_line: analysis.paragraphs.ends_on_a_line(),
            has_tabs: analysis.flags.contains(ParagraphFlags::HAS_TABS),
            area: LineBand::from(input.area.inline),
            justification: input.tab_justification,
            ellipsis_space: input.ellipsis_space,
            has_line_padding: content.flags.contains(ContentFlags::LINE_PADDING),
            group: Group::new(input),
            ruby_in_bidi: !measured.ruby_columns().is_empty()
                && analysis.flags.contains(ParagraphFlags::RIGHT_TO_LEFT),
            has_decorated_boxes: content.flags.contains(ContentFlags::DECORATED_BOXES),
            has_upright: analysis.flags.contains(ParagraphFlags::HAS_UPRIGHT),
            has_anchors: !content.absolutes().is_empty(),
        }
    }
}

impl LineState<'_> {
    /// Makes the state before the first line, which holds no line's facts.
    fn new() -> Self {
        Self {
            paragraph: ParagraphId::new(0),
            level: BidiLevel::LTR,
            band: LineBand::default(),
            align: TextAlign::Start,
            ruby_edges: RubyEdges::default(),
            column_parts: &[],
            line_column: RubyColumnId::new(0),
            tab_line: TabLine::default(),
            spread: None,
            hyphen: None,
            open_column: None,
            boxed: false,
        }
    }
}

impl<'a> Placer<'a> {
    /// Makes the placer for `input`'s lines, with the block result `block`.
    ///
    /// It starts in the standard variant until [`enter`](Self::enter)
    /// switches it.
    pub(super) fn new(input: &'a PlaceInput<'a>, block: BlockResult) -> Self {
        let stages = input.stages.variant(FirstLineVariant::Standard);
        let (content, analysis, measured) = (stages.content, stages.analysis, stages.measured);
        Self {
            input,
            stages,
            clusters: &analysis.clusters,
            items: &content.items,
            item_clusters: &analysis.item_clusters,
            count: analysis.clusters.end_id(),
            kept_boxes: &measured.kept_boxes,
            lines: input.lines,
            block,
            gates: Gates::new(input),
            rubies: measured.ruby_columns(),
            item: ItemId::new(0),
            controls: analysis.control_levels(),
            run: stages.shaped.runs.cursor(ShapedRunId::new(0)),
            next_kept: KeptBoxId::new(0),
            shift_rows: None,
            column: RubyColumnId::new(0),
            open_boxes: 0,
            line_state: LineState::new(),
        }
    }

    /// Switches to the tables of `variant` and restarts the cursors.
    ///
    /// The block's first line may use the first-line variant. Every other
    /// line uses the standard one. Nothing happens if `variant` is current.
    fn enter(&mut self, variant: FirstLineVariant) {
        if variant == self.stages.variant {
            return;
        }
        self.stages = self.input.stages.variant(variant);
        self.kept_boxes = &self.stages.measured.kept_boxes;
        self.rubies = self.stages.measured.ruby_columns();
        self.run = self.stages.shaped.runs.cursor(ShapedRunId::new(0));
        self.next_kept = KeptBoxId::new(0);
        self.shift_rows = None;
        self.column = RubyColumnId::new(0);
    }

    /// Lays out every line in order and writes their items to `out`.
    pub(super) fn place_lines(&mut self, scratch: &mut PlaceScratch, out: &mut Fragments) {
        for ((id, line), &facts) in self
            .lines
            .lines
            .iter()
            .zip(self.input.placements.as_slice())
        {
            self.enter(line.variant());
            let line = LineView::new(id, line, facts);
            self.line_state = self.line_state(&line);
            // The ruby column cursor goes back to the first column continued
            // onto the line.
            if let Some(first) =
                (self.line_state.column_parts.iter()).min_by_key(|part| part.column)
            {
                self.column = self.column.min(first.column);
            }
            scratch.begin_line();
            self.place_line(scratch, out, id, &line);
        }
        if self.gates.has_anchors {
            self.place_unheld_anchors(out);
        }
    }

    /// Returns the state `line` starts with.
    ///
    /// Its alignment is decided first: a ruby column at the line's edges
    /// reads it while the pieces are made, and justification uses it after.
    /// The tab stops are measured from the line's start. The boxes open from
    /// the line before stay open.
    fn line_state(&self, line: &LineView<'_>) -> LineState<'a> {
        let gates = &self.gates;
        let paragraphs = &self.stages.analysis.paragraphs;
        let band = gates.group.narrow(line.band);
        let ends_paragraph = line.ends_paragraph(paragraphs);
        let align = if ends_paragraph {
            gates.text_align_last.resolve(gates.text_align)
        } else {
            gates.text_align
        };
        let ruby_edges = if self.rubies.is_empty() || gates.ruby_in_bidi {
            RubyEdges::default()
        } else {
            RubyEdges {
                start: align == TextAlign::Justify
                    || (ends_paragraph && gates.text_align == TextAlign::Justify),
                end: align == TextAlign::Justify,
                room: band.width() - line.indent,
            }
        };
        let tab_line = if gates.has_tabs {
            TabLine::new(line, paragraphs, gates.area, &self.stages.measured.prefix)
        } else {
            TabLine::default()
        };
        LineState {
            paragraph: line.paragraph,
            level: line.level(paragraphs),
            band,
            align,
            ruby_edges,
            column_parts: (self.lines.ruby.as_deref()).map_or(&[], |ruby| ruby.line_parts(line.id)),
            line_column: self.column,
            tab_line,
            spread: None,
            hyphen: None,
            open_column: None,
            boxed: self.open_boxes > 0,
        }
    }

    /// Lays out `line` and writes its items to `out`, each phase with its
    /// part of `scratch`.
    fn place_line(
        &mut self,
        scratch: &mut PlaceScratch,
        out: &mut Fragments,
        id: LineId,
        line: &LineView<'_>,
    ) {
        let PlaceScratch {
            pieces,
            boxes,
            counts,
            rubies,
        } = scratch;
        let spread = out.spread_mark();
        let (band, align) = (self.line_state.band, self.line_state.align);
        // Pieces: made in logical order, each tab sized where it lands.
        self.open_carried_boxes(pieces, boxes, line);
        pieces.take_carried(boxes.innermost());
        let shapes = self.lines.edges.line_edges(line);
        self.pieces(pieces, boxes, rubies, line, shapes);
        self.finish_columns(pieces, boxes, rubies, line);
        boxes.carry_open();
        if self.gates.has_line_padding {
            self.pad(pieces);
        }
        // Justification and alignment: justification spends the spare room
        // on the line's opportunities. Otherwise alignment moves the whole
        // line, tabs included, in the band `text-group-align` narrows.
        let (justified, justified_row) = self
            .justify_into(pieces, line, id, band, align, out)
            .unzip();
        let letter_indent = self.letter_indent(id);
        let offset = self.align(
            line,
            band,
            align,
            pieces.hang(),
            justified.is_some(),
            letter_indent,
        );
        // Reordering by level. A piece with no level of its own takes the
        // next piece's level, or the paragraph's at the line's end, as
        // Blink's opaque items do.
        pieces.reorder(self.line_state.level);
        // A spread ruby base's text takes in its room, which its boxes then
        // reach over.
        if !rubies.columns.is_empty() {
            Placer::absorb_base_spread(pieces, rubies);
        }
        // Box parts: found in visual order, with the room their edges
        // take.
        self.rooms(boxes, letter_indent);
        self.box_parts(pieces, boxes);
        // Positions: assigned left to right. Each box's edges sit at its
        // leftmost and rightmost parts.
        pieces.place(offset.start, &boxes.rooms, Some(counts));
        // Static positions: where the anchors stand, now that they are
        // placed.
        if self.gates.has_anchors {
            self.place_anchors(pieces, out, line, offset);
        }
        // Truncation, on a line cut for an ellipsis: what doesn't fit before
        // the ellipsis is hidden but keeps its place.
        let cut = if line.flags.contains(LineFlags::ELLIPSIS) {
            self.truncate(pieces, line, band, &offset, justified.as_ref())
        } else {
            None
        };
        let split = cut.and_then(|cut| cut.split).map(|split| split.piece);
        let hyphen = pieces.hyphen().zip(self.line_state.hyphen);
        counts.widen_leaves(pieces, split, hyphen.map(|(at, text)| (at, text.count())));
        // Items: written once, with the rows of the text a spread ruby base
        // took room into, then the annotation lines.
        self.emit(pieces, boxes, counts, out, line, offset, cut, justified_row);
        if !rubies.columns.is_empty() {
            self.annotations(pieces, rubies, out, line);
        }
        out.settle_spread(spread);
    }

    /// Gives each box open from the line before a placeholder at `line`'s
    /// start, outermost first.
    ///
    /// A box keeps its row, unless the line before was set in the other
    /// variant, whose rows are another table's. Then the kept-box cursor,
    /// restarted for the variant, walks to it: the boxes carried are in
    /// node order, outermost first.
    fn open_carried_boxes(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &mut LineBoxes,
        line: &LineView<'_>,
    ) {
        let start = line.clusters().start;
        for at in 0..boxes.open.len() {
            work::step();
            if let Some(&(node, kept)) = boxes.open.get(at) {
                let kept = match kept {
                    Some(kept) if self.kept_boxes.node(kept) == Some(node) => Some(kept),
                    _ => {
                        let at = self.kept_boxes.walk_to(self.next_kept, node);
                        (self.kept_boxes.node(at) == Some(node)).then_some(at)
                    }
                };
                let shifts = self.shift(line, node, boxes.innermost_shifts());
                boxes.open_box(pieces, (node, kept), start, false, shifts);
            }
        }
    }

    /// Returns `node`'s box facts in the current line's variant.
    #[inline]
    pub(super) fn box_facts(&self, node: NodeId) -> &'a BoxFacts {
        let content = self.stages.content;
        let id = content.nodes.box_facts(node, self.stages.variant);
        content.facts.box_facts(id)
    }

    /// Returns the part of line `id`'s indent inside the initial letter's
    /// box: the first line's indent, where that line opens one.
    ///
    /// Fitting counts that line's indent twice, as Chrome lays the box out
    /// as a block whose own first line takes the indent too
    /// (`CalculateInitialLetterBoxInlineSize`). The box's start edge takes
    /// the second.
    fn letter_indent(&self, id: LineId) -> LayoutUnit {
        let indent = self.stages.content.block.text_indent;
        if id != LineId::new(0) || indent.hanging || self.stages.measured.initial_letter().is_none()
        {
            return LayoutUnit::ZERO;
        }
        indent.length(self.gates.area.width())
    }

    /// Returns where `line` sits in `band` under `align`.
    ///
    /// `hang` is its hanging pieces' advance. `justified` says the
    /// opportunities already took the room. `letter_indent` is the part of
    /// its indent inside the initial letter's box, where the line starts.
    fn align(
        &self,
        line: &LineView<'_>,
        band: LineBand,
        align: TextAlign,
        hang: InlineLayoutUnit,
        justified: bool,
        letter_indent: LayoutUnit,
    ) -> LineOffset {
        let room = band.width();
        let content = line.indent + line.width;
        // Preserved white space before a forced break or at the block's end
        // hangs only where it overflows, so alignment sees the part that
        // fits. What hangs before it, an ideographic space say, hangs only
        // where none of it fits, as nothing after it then takes room.
        // Elsewhere hanging content is left out. A justified line leaves all
        // of it out, as Chrome hangs trailing spaces under `justify`
        // (`ShouldHangTrailingSpaces`).
        let counted = if line.flags.contains(LineFlags::CONDITIONAL_HANG) && !justified {
            let before = line.hang.unconditional;
            let fits = (line.hang.space - before)
                .min(room - content - before)
                .max(LayoutUnit::ZERO);
            if fits > LayoutUnit::ZERO {
                before + fits
            } else {
                LayoutUnit::ZERO
            }
        } else {
            LayoutUnit::ZERO
        };
        let rtl = self.line_state.level.is_rtl();
        // A justified line fills its band, since its opportunities took the
        // room. Otherwise alignment places the indent, the content, and the
        // fitting part of a conditional hang.
        let left = if justified {
            band.left
        } else {
            let side = Side::from_align(align, rtl);
            band.left + side.left_offset(rtl, room - (content + counted))
        };
        // Right to left, content hanging at the end sits at the line's left,
        // outside the line box except the part alignment counted. Left to
        // right, a mark hanging at the start sits outside it on the left.
        let hang_end = InlineLayoutUnit::from_layout(line.hang.end);
        let start = if rtl {
            -(hang + hang_end - InlineLayoutUnit::from_layout(counted)).max(InlineLayoutUnit::ZERO)
        } else {
            InlineLayoutUnit::from_layout(line.indent - letter_indent - line.hang.start)
        };
        LineOffset { left, start }
    }

    /// Adds `line-padding` pieces inside the innermost box at each end of the
    /// line's content, at the paragraph's level.
    ///
    /// Each end uses the padding of the text holding its first or last leaf
    /// before what hangs. That is what the breaker charged.
    fn pad(&self, pieces: &mut LinePieces) {
        let is_content = |piece: &Piece| {
            matches!(
                piece.kind,
                PieceKind::Text | PieceKind::Atomic | PieceKind::Hyphen
            ) && !piece.flags.contains(FragmentItemFlags::HANGS)
        };
        let slice = pieces.logical.as_slice();
        let (Some(first), Some(last)) = (
            slice.iter().position(is_content).map(PieceId::new),
            slice.iter().rposition(is_content).map(PieceId::new),
        ) else {
            return;
        };
        let facts = &self.stages.content.facts;
        let padding = |piece: &Piece| {
            let text = facts.text(self.stages.text_facts(piece.node));
            InlineLayoutUnit::from_layout(text.padding)
        };
        let (Some(head), Some(tail)) = (pieces.logical.get(first), pieces.logical.get(last)) else {
            return;
        };
        let pad = |of: &Piece, at: ClusterId, advance: InlineLayoutUnit| {
            Piece::new(
                PieceKind::Padding,
                of.node,
                self.line_state.level,
                at..at,
                advance,
                of.owner,
                of.shift,
            )
        };
        let end = pad(tail, tail.end, padding(tail));
        let start = pad(head, head.start, padding(head));
        if end.advance != InlineLayoutUnit::ZERO {
            pieces.insert(PieceId::new(last.get() + 1), end);
        }
        if start.advance != InlineLayoutUnit::ZERO {
            pieces.insert(first, start);
        }
    }

    /// Writes `line`'s items to `out`: its `Line` item, naming its
    /// justification row `justified`, then its pieces' items in visual order.
    ///
    /// Each box part's item comes before the items inside it. `counts`
    /// counts the leaf items, and the box items link in paint order.
    #[allow(clippy::too_many_arguments)]
    fn emit(
        &self,
        pieces: &LinePieces,
        boxes: &mut LineBoxes,
        counts: &mut LeafCounts,
        out: &mut Fragments,
        line: &LineView<'_>,
        offset: LineOffset,
        cut: Option<LineCut>,
        justified: Option<JustifiedLineId>,
    ) {
        let baseline = line.ascent();
        let head = out.items.next_id();
        let ellipsis = cut.map_or(0, |cut| cut.ellipsis.items());
        let total = counts.total() + boxes.parts.len() + ellipsis;
        let top = self.block.top(line.block_start);
        // The ellipsis belongs to the line, in no box. It is the first item
        // when drawn at the left (right to left) and the last when drawn at
        // the right, so items stay in visual order. The box items link past
        // it, the first from the line's item.
        let rtl = self.line_state.level.is_rtl();
        let leading = cut.filter(|_| rtl);
        let before = FragmentItemId::new(head.get() + if rtl { ellipsis } else { 0 });
        let first = paint_order(counts, boxes, before);
        let flags = if self.gates.has_decorated_boxes && self.line_state.boxed {
            FragmentItemFlags::BOXED
        } else {
            FragmentItemFlags::NONE
        };
        let item = FragmentItem::from_line(
            self.line_state.level,
            offset.left,
            top,
            total,
            first,
            justified,
            flags,
        );
        out.items
            .push_bounded(item, "no more items than a FragmentItemId names");
        out.line_heads
            .push_bounded(head, "no more lines than a LineId names");
        if let Some(cut) = leading {
            self.emit_ellipsis(out, cut.ellipsis, baseline);
        }
        let split = cut.and_then(|cut| cut.split);
        let mut part = BoxPartId::new(0);
        for (at, &logical) in pieces.order.iter() {
            work::step();
            while let Some(next) = boxes.parts.get(part).filter(|next| next.left == at) {
                self.emit_box_part(pieces, boxes, counts, out, part, next, baseline);
                part = BoxPartId::new(part.get() + 1);
            }
            let Some(piece) = pieces.logical.get(logical) else {
                continue;
            };
            let Some(kind) = piece.kind.item_kind() else {
                continue;
            };
            if piece.room != (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO) {
                out.push_spread(line.id, piece);
            }
            if let Some(split) = split.filter(|split| split.piece == logical) {
                self.emit_split(out, piece, kind, piece.flags, split, baseline);
            } else if piece.kind == PieceKind::Hyphen {
                self.emit_hyphen(out, piece, piece.flags, baseline);
            } else {
                out.items.push_bounded(
                    piece.fragment_item(kind, piece.flags, baseline),
                    "no more items than a FragmentItemId names",
                );
            }
        }
        if !rtl && let Some(cut) = cut {
            self.emit_ellipsis(out, cut.ellipsis, baseline);
        }
    }
}
