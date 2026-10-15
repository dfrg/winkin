//! Makes a line's pieces: its items walked in logical order, and the kept
//! boxes and ruby columns met on the way.
//!
//! A forward-only cursor walks the line's items, as the breaker does. It
//! makes:
//! - text, split where the used font or bidi level changes and around each
//!   tab;
//! - atomic inlines;
//! - preserved white space hanging past the line's end;
//! - a placeholder for each kept inline box where it opens, or at the line's
//!   start if it continues, as Blink's `AddBoxFragmentPlaceholder` does, a
//!   ruby container's among them;
//! - the edges of a ruby container that keeps no box, which take room;
//! - the anchor of each absolutely positioned box, which takes none.
//!
//! The breaker decides what a line holds at a boundary. The items at the
//! line's end before the break there (`BoundarySplit`) are this line's: the
//! closes of earlier boxes and the boxes that open and close there. The rest
//! are the next line's.
//!
//! A trailing collapsible space is removed, as Blink's breaker does, and a
//! forced break's separator draws nothing. Their clusters stay on the line
//! for carets and copying.
//!
//! Pieces follow nodes. A grapheme split by a style boundary is a piece in
//! each part's item, even when both parts share a shaping run, as Chrome
//! gives a span's text its own item.
//!
//! Advances are exact. A text piece is as wide as the prefix sums say,
//! from its first cluster's pen to its last's end (`MeasuredText::pen`,
//! `pen_end`). Where reshaped line edges hold some of its clusters, the
//! difference is added exactly. A box's edges take `measure::edge_room`, as
//! the prefix charged; a cloned box's take the breaker's edge cost. So the
//! pieces add up to the line's exact reach, and only the line's origin
//! rounds.
//!
//! A box covers what hangs inside it, as in Chrome, where
//! `ComputeInlinePositions` places hanging results inside their box. A box
//! closing after the last word, before a space outside it, still ends at
//! the word. Width and alignment leave hanging content out, so a box edge
//! after it stands past it. A collapsible space is removed, and no box
//! covers it.
//!
//! Each tab is a piece of its own, an item flagged `TAB` whose
//! size is its width to its stop. The width uses the same tab function the
//! breaker fitted with, so the pieces add up to the measured width. A
//! hanging tab is also flagged `HANGS`.

use core::ops::Range;

use super::boxes::{LineBoxId, LineBoxes};
use super::place::LinePieces;
use super::ruby::LineRubies;
use super::{FragmentItem, FragmentItemFlags, Justified, Piece, PieceKind, Placer};
use crate::data::Id;
use crate::data::IdRange;
use crate::stages::analysis::{
    BidiLevel, ClusterClass, ClusterId, ControlLevel, ParagraphFlags, Paragraphs, RunOrientation,
};
use crate::stages::content::{Item, ItemFlags, ItemId, ItemKind, NodeId};
use crate::stages::lines::{EdgeShape, LineBand, LineFlags, LineView};
use crate::stages::measure;
use crate::stages::measure::{BoundarySplit, PrefixAdvances, tab_advance_reached};
use crate::stages::shape::{ShapedRun, ShapedRunId};
use crate::style::VerticalAlign;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// Where the line's tabs count from, and how far the tabs placed so far
/// reach.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct TabLine {
    /// Whether its paragraph has a tab.
    any: bool,
    /// The line's start from the block's content edge, on its paragraph's
    /// start side.
    origin: LayoutUnit,
    /// The prefix sum at the line's start, less its indent.
    ///
    /// A tab lands at its pen less this, plus what earlier reshapes moved and
    /// earlier tabs reach.
    start: InlineLayoutUnit,
    /// How far the tabs placed so far reach.
    before: InlineLayoutUnit,
}

impl TabLine {
    /// Returns the start of `line`'s tabs in `area`, with none placed yet.
    #[inline(never)]
    pub(super) fn new(
        line: &LineView<'_>,
        paragraphs: &Paragraphs,
        area: LineBand,
        prefix: &PrefixAdvances,
    ) -> Self {
        Self {
            any: paragraphs
                .flags(line.paragraph)
                .contains(ParagraphFlags::HAS_TABS),
            origin: line.band.start_from(area, line.level(paragraphs)),
            start: prefix.get(line.clusters().start) - InlineLayoutUnit::from_layout(line.indent),
            before: InlineLayoutUnit::ZERO,
        }
    }
}

/// The boundary the item walk is at, among items that hold no cluster.
struct Boundary {
    /// The boundary, where the walk is at one.
    at: Option<ClusterId>,
    /// Where a break there falls among its items.
    split: BoundarySplit,
    /// The room of the items after the break. The pen for the following text
    /// stands past it.
    rest: InlineLayoutUnit,
    /// The first item at the line's end that starts the next line.
    next_line: ItemId,
    /// The line ends the text, and the last paragraph holds clusters, so
    /// every item at its end is the line's.
    everything_at_end: bool,
}

impl<'a> Placer<'a> {
    /// Walks the items on `line` from the cursor into `pieces`.
    ///
    /// Opens and closes kept boxes in `boxes` and ruby columns in `rubies`
    /// as it meets them.
    pub(super) fn pieces(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &mut LineBoxes,
        rubies: &mut LineRubies,
        line: &LineView<'_>,
        shapes: &[EdgeShape],
    ) {
        let range = line.clusters();
        let end = range.end;
        // At the text's end, every item is the last line's when the last
        // paragraph holds clusters. Otherwise what follows the leading
        // closes belongs to the next line, or to none.
        let everything_at_end =
            end == self.count && (self.gates.ends_on_a_line || range.start == end);
        let mut boundary = Boundary {
            at: None,
            split: BoundarySplit::default(),
            rest: InlineLayoutUnit::ZERO,
            next_line: ItemId::new(0),
            everything_at_end,
        };
        // Where the line's trailing preserved white space starts. It takes
        // the paragraph's level (L1), which matters only with mixed levels.
        let trailing = if self.gates.has_mixed_levels {
            self.trailing_space(line)
        } else {
            end
        };
        let mut removable = self.removable_spaces(line);
        while let Some(item) = self.items.get(self.item) {
            work::step();
            let Range {
                start: first,
                end: next,
            } = self.item_clusters.range(self.item);
            // The room of the items before this one at its first boundary.
            let rest_here = if boundary.at == Some(first) {
                boundary.rest
            } else {
                InlineLayoutUnit::ZERO
            };
            // A ruby column ending before this item, or starting with it.
            if !self.rubies.is_empty() {
                self.update_columns(pieces, boxes, rubies, line, first, rest_here);
                if let Some(from) = self.line_ruby.base_spaces {
                    removable = removable.min(from);
                }
            }
            if first < next {
                // An item holding clusters belongs to the line where it holds
                // some of the line's.
                if first >= end {
                    break;
                }
                let x = first.max(range.start);
                let y = next.min(end);
                if x < y && !item.flags.contains(ItemFlags::ANNOTATION) {
                    let id = self.item;
                    match item.kind {
                        ItemKind::Text => self.text(
                            pieces,
                            boxes,
                            line,
                            shapes,
                            id,
                            item,
                            first..next,
                            x..y,
                            rest_here,
                            trailing,
                            removable,
                        ),
                        ItemKind::Atomic => {
                            let clusters = (first..next, x..y);
                            self.atomic(pieces, boxes, line, (id, item), clusters, rest_here);
                        }
                        // A forced break's separator draws nothing. The
                        // caret after a line's last character is a
                        // position, not a piece.
                        _ => {}
                    }
                }
                boundary.at = None;
                if next > end {
                    // It goes on to the next line, which takes it from here.
                    break;
                }
                self.item = ItemId::new(self.item.get() + 1);
                continue;
            }
            // An item at boundary `first`, holding no cluster.
            if first > end || !self.boundary_item(pieces, boxes, line, item, first, &mut boundary) {
                break;
            }
            self.item = ItemId::new(self.item.get() + 1);
        }
    }

    /// Adds the piece of atomic inline `item`, content item `id`, holding
    /// `bounds`, for its clusters `x..y` on `line`, in the innermost open box.
    /// `rest` is the room of the items before it at its first boundary.
    ///
    /// Out of line, so that a line of text holds none of its code.
    #[inline(never)]
    fn atomic(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        line: &LineView<'_>,
        (id, item): (ItemId, &Item),
        (bounds, Range { start: x, end: y }): (Range<ClusterId>, Range<ClusterId>),
        rest: InlineLayoutUnit,
    ) {
        let closes = self.closes_after(id, bounds.end, y);
        let advance = self.exact_advance(bounds, x, y, rest, closes);
        let (shift, _) = self.shift(line, item.node, boxes.innermost_shifts());
        let run = self.advance_run(x);
        pieces.push(Piece::new(
            PieceKind::Atomic,
            item.node,
            self.run_level(run, self.line_state.level),
            x..y,
            advance,
            boxes.innermost(),
            shift,
        ));
    }

    /// Adds the pieces of the walk's item `item`, which holds no cluster, at
    /// boundary `b` on `line`: a box's placeholder, edge and controls, or an
    /// anchor. Returns `false` where the item starts the next line.
    ///
    /// Out of line, so that a line of plain text holds none of its code.
    #[inline(never)]
    fn boundary_item(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &mut LineBoxes,
        line: &LineView<'_>,
        item: &Item,
        b: ClusterId,
        boundary: &mut Boundary,
    ) -> bool {
        let end = line.clusters().end;
        let at_break = b == end && !boundary.everything_at_end;
        if boundary.at != Some(b) {
            boundary.at = Some(b);
            boundary.split = BoundarySplit::new(self.stages.analysis, b);
            boundary.rest = InlineLayoutUnit::ZERO;
            if at_break {
                boundary.next_line = measure::items_after_break(
                    self.stages.content,
                    self.stages.analysis,
                    b,
                    self.item,
                );
            }
        }
        let annotation = item.flags.contains(ItemFlags::ANNOTATION);
        let opener = !annotation && item.kind.is_open();
        let closer = !annotation && item.kind.is_close();
        if at_break && self.item >= boundary.next_line {
            // What follows the break at the line's end starts the next line.
            return false;
        }
        // Each inline box's open and close items pass once, in order, so
        // this counts the boxes open across a line's start.
        if matches!(item.kind, ItemKind::Open | ItemKind::Close) && !annotation {
            if opener {
                self.open_boxes = self.open_boxes.saturating_add(1);
                self.line_state.boxed = true;
            } else {
                self.open_boxes = self.open_boxes.saturating_sub(1);
            }
        }
        if opener || closer {
            let room = if self.gates.has_box_edges {
                let letter = self.stages.measured.initial_letter();
                measure::edge_room(self.stages.content, letter, item, opener)
            } else {
                InlineLayoutUnit::ZERO
            };
            // A box's controls come before its opening and after its
            // closing, as Blink's items have them.
            if opener {
                self.control(pieces, boxes, item, b);
            }
            self.edge(pieces, boxes, line, item, b, opener, room);
            if closer {
                self.control(pieces, boxes, item, b);
            }
            let mut before = InlineLayoutUnit::ZERO;
            (boundary.split).take(item.kind, room, &mut before, &mut boundary.rest);
        }
        if item.kind == ItemKind::Absolute {
            self.anchor(pieces, boxes, line, item, b, at_break);
            let mut before = InlineLayoutUnit::ZERO;
            let rest = &mut boundary.rest;
            (boundary.split).take(item.kind, InlineLayoutUnit::ZERO, &mut before, rest);
        }
        true
    }

    /// Adds the piece of the anchor `item` at boundary `at`, at its own
    /// level.
    ///
    /// An anchor at a soft break ends its line only where Chrome's breaker
    /// breaks after it, which it does only where the line's white space
    /// before it fits (`HandleOverflow`) and no scorer chose the break
    /// (`break_at_`). Elsewhere it goes to the next line's start, where
    /// `carried` keeps it.
    fn anchor(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        line: &LineView<'_>,
        item: &Item,
        at: ClusterId,
        at_break: bool,
    ) {
        let level =
            (self.item_controls().last()).map_or(self.line_state.level, |control| control.level);
        let piece = Piece::new(
            PieceKind::Absolute,
            item.node,
            level,
            at..at,
            InlineLayoutUnit::ZERO,
            boxes.innermost(),
            LayoutUnit::ZERO,
        );
        // Where the line's reach stands at the anchor: its white space
        // counted, and the edges after the anchor that end the line not.
        let overflows = || {
            let after = self.closes_after(self.item, at, at);
            let reach = InlineLayoutUnit::from_layout(line.indent + line.width + line.hang.space);
            reach - after > InlineLayoutUnit::from_layout(line.band.width())
        };
        if at_break && (line.flags.contains(LineFlags::SCORED) || overflows()) {
            pieces.carried.push(piece);
        } else {
            pieces.push(piece);
        }
    }

    /// Adds a piece at the level of the current item's bidi controls at
    /// boundary `at`, where analysis kept one.
    ///
    /// The piece belongs to the box around the item's box.
    fn control(&mut self, pieces: &mut LinePieces, boxes: &LineBoxes, item: &Item, at: ClusterId) {
        for control in self.item_controls() {
            pieces.push(Piece::new(
                PieceKind::Control,
                item.node,
                control.level,
                at..at,
                InlineLayoutUnit::ZERO,
                boxes.innermost(),
                LayoutUnit::ZERO,
            ));
        }
    }

    /// Returns the levels of the current item's bidi controls, moving past
    /// them and every earlier item's.
    ///
    /// The controls are in item order, so each is passed once.
    fn item_controls(&mut self) -> &'a [ControlLevel] {
        let item = self.item;
        let passed = (self.controls.iter())
            .take_while(|control| control.item < item)
            .count();
        let from = self.controls.get(passed..).unwrap_or_default();
        let here = from
            .iter()
            .take_while(|control| control.item == item)
            .count();
        let (controls, rest) = from.split_at_checked(here).unwrap_or((from, &[]));
        self.controls = rest;
        controls
    }

    /// Returns where `line`'s trailing preserved white space starts within
    /// its content.
    ///
    /// It covers the spaces, tabs and other separators just before the
    /// content's end; they are content only under `break-spaces`. Blink's
    /// `SplitTrailingBidiPreservedSpace` splits the same off to set it at
    /// the paragraph's level, except inside one item's text
    /// (`trailing_level_split`).
    fn trailing_space(&self, line: &LineView<'_>) -> ClusterId {
        let start = line.clusters().start;
        let mut at = line.content_end;
        while at > start {
            let before = ClusterId::new(at.get() - 1);
            if !self.clusters.is_breaking_space(before) {
                break;
            }
            at = before;
        }
        at
    }

    /// Returns where trailing white space `trailing..end` takes the
    /// paragraph's level, or `end` where it keeps its own.
    ///
    /// The caller has text of the same item before `trailing` on the line,
    /// and the item going on past `end`. Chrome splits trailing white space
    /// off only where it ends an item or is all its item holds on the line.
    /// Chrome's items also end at a level change and around each tab, so the
    /// white space keeps its level only where `trailing - 1` to `end`, both
    /// included, hold no tab and are at one level.
    ///
    /// Walks a copy of the shaping run cursor forward, leaving the walk's
    /// own cursor where it is.
    #[cold]
    #[inline(never)]
    fn trailing_level_split(
        &self,
        trailing: ClusterId,
        end: ClusterId,
        paragraph: BidiLevel,
    ) -> ClusterId {
        let before = ClusterId::new(trailing.get() - 1);
        if (trailing..ClusterId::new(end.get() + 1))
            .ids()
            .any(|cluster| self.is(cluster, ClusterClass::Tab))
        {
            return trailing;
        }
        let runs = &self.stages.shaped.runs;
        let mut cursor = self.run;
        while cursor.end() <= before && cursor.id().get() + 1 < runs.len() {
            runs.step(&mut cursor);
        }
        let level = self.run_level(runs.get(cursor.id()), paragraph);
        while cursor.end() <= end && cursor.id().get() + 1 < runs.len() {
            work::step();
            runs.step(&mut cursor);
            if self.run_level(runs.get(cursor.id()), paragraph) != level {
                return trailing;
            }
        }
        end
    }

    /// Returns where the spaces a collapsing text removes at `line`'s end
    /// may start.
    ///
    /// Only the spaces after the last hanging character that is not a space
    /// go, as CSS Text 3, section 4.1.3, removes "a sequence of collapsible
    /// spaces at the end of a line". A space before an ideographic space
    /// hangs with it, as in Chrome.
    fn removable_spaces(&self, line: &LineView<'_>) -> ClusterId {
        let start = line.content_end;
        let mut at = line.clusters().end;
        while at > start {
            let before = ClusterId::new(at.get() - 1);
            let gone = matches!(
                self.clusters.class(before),
                Some(
                    ClusterClass::Space | ClusterClass::Separator | ClusterClass::BreakOpportunity
                )
            );
            if !gone {
                break;
            }
            at = before;
        }
        at
    }

    /// Returns the room of the edges after item `id` at boundary `at` that
    /// end the line before a break there ([`BoundarySplit`]), which the
    /// prefix counts before `at`'s pen.
    ///
    /// Zero unless the text's piece ends at `at` (`y`). At the text's end,
    /// when the last paragraph holds clusters, every item there counts.
    fn closes_after(&self, id: ItemId, at: ClusterId, y: ClusterId) -> InlineLayoutUnit {
        if !self.gates.has_box_edges || y != at {
            return InlineLayoutUnit::ZERO;
        }
        let everything = at == self.count && self.gates.ends_on_a_line;
        let letter = self.stages.measured.initial_letter();
        let mut split = BoundarySplit::new(self.stages.analysis, at);
        let (mut before, mut after) = (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        let mut index = ItemId::new(id.get() + 1);
        while let Some(item) = self.items.get(index) {
            work::step();
            let clusters = self.item_clusters.range(index);
            if clusters.start != at || !clusters.is_empty() {
                break;
            }
            let kind = item.kind;
            if item.flags.contains(ItemFlags::ANNOTATION) {
                // An annotation's items take no room on the base's line.
            } else if kind.is_open() || kind.is_close() {
                let room = measure::edge_room(self.stages.content, letter, item, kind.is_open());
                split.take(kind, room, &mut before, &mut after);
            } else if kind == ItemKind::Absolute {
                split.take(kind, InlineLayoutUnit::ZERO, &mut before, &mut after);
            }
            index = ItemId::new(index.get() + 1);
        }
        if everything { before + after } else { before }
    }

    /// Returns the exact advance of clusters `u..v` of the item holding
    /// `bounds`, as the prefix sums measured them.
    ///
    /// Leaves out the room before the item's first cluster (`rest`), the
    /// closing edges after its last (`closes`), and an autospace seam's room
    /// at `v` that the text after draws ([`seam_before`](Self::seam_before)).
    /// No item takes room inside an item's clusters.
    #[inline(always)]
    fn exact_advance(
        &self,
        bounds: Range<ClusterId>,
        u: ClusterId,
        v: ClusterId,
        rest: InlineLayoutUnit,
        closes: InlineLayoutUnit,
    ) -> InlineLayoutUnit {
        self.seam_before(v)
            .map_or(InlineLayoutUnit::ZERO, |room| -room)
            + self.stages.span_advance(
                u..v,
                if u == bounds.start {
                    rest
                } else {
                    InlineLayoutUnit::ZERO
                },
                if v == bounds.end {
                    closes
                } else {
                    InlineLayoutUnit::ZERO
                },
            )
    }

    /// Returns the room of an autospace seam at boundary `at` that the text
    /// after it draws, before its first glyph, or `None` where there is
    /// none.
    ///
    /// The side before reads right to left and the side after left to
    /// right, so Chrome's `TextAutoSpace` writes the room into the later
    /// item. It is looked for only where some style sets `text-autospace`
    /// and some paragraph has mixed levels and reads right to left. `at` is
    /// inside the line, so the seam is in the line's paragraph.
    #[inline]
    fn seam_before(&self, at: ClusterId) -> Option<InlineLayoutUnit> {
        if !self.gates.has_seams_before {
            return None;
        }
        self.mixed_seam_before(at)
    }

    /// Returns [`seam_before`](Self::seam_before)'s room where some seam may
    /// have it, out of line.
    #[cold]
    #[inline(never)]
    fn mixed_seam_before(&self, at: ClusterId) -> Option<InlineLayoutUnit> {
        let paragraph = self
            .stages
            .analysis
            .paragraphs
            .get(self.line_state.paragraph)?;
        let room = self
            .gates
            .autospace
            .room_before(&self.stages, paragraph, at);
        let room = InlineLayoutUnit::from_text(room);
        (room != InlineLayoutUnit::ZERO).then_some(room)
    }

    /// Adds a gap for the room of an autospace seam at `at` that the text
    /// after it draws, where there is one: at `level`, of `node`, in the box
    /// `owner` and raised `shift`, so it stays with that text.
    #[cold]
    #[inline(never)]
    fn seam_gap(
        &self,
        pieces: &mut LinePieces,
        at: ClusterId,
        node: NodeId,
        level: BidiLevel,
        (owner, shift): (Option<LineBoxId>, LayoutUnit),
    ) {
        if let Some(room) = self.seam_before(at) {
            let gap = Piece::new(PieceKind::Gap, node, level, at..at, room, owner, shift);
            pieces.push(gap);
        }
    }

    /// Returns the advance of clusters `u..v`, using the reshaped line edges
    /// where they hold some of them.
    ///
    /// A line with no reshaped edge, as most are, reads the prefix sums
    /// alone; the edges are read out of line.
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn advance(
        &self,
        shapes: &[EdgeShape],
        bounds: Range<ClusterId>,
        u: ClusterId,
        v: ClusterId,
        rest: InlineLayoutUnit,
        closes: InlineLayoutUnit,
    ) -> InlineLayoutUnit {
        let advance = self.exact_advance(bounds.clone(), u, v, rest, closes);
        if shapes.is_empty() {
            return advance;
        }
        self.reshaped_advance(advance, shapes, bounds, u, v, (rest, closes))
    }

    /// Returns `advance`, clusters `u..v`'s from the prefix sums, with the
    /// reshaped line edges in `shapes` that hold some of them read instead.
    #[inline(never)]
    fn reshaped_advance(
        &self,
        mut advance: InlineLayoutUnit,
        shapes: &[EdgeShape],
        bounds: Range<ClusterId>,
        u: ClusterId,
        v: ClusterId,
        (rest, closes): (InlineLayoutUnit, InlineLayoutUnit),
    ) -> InlineLayoutUnit {
        for shape in shapes {
            let clusters = shape.clusters();
            let (from, to) = (u.max(clusters.start), v.min(clusters.end));
            if from >= to {
                continue;
            }
            let reshaped = self.lines.edges().advance_sum(shape.entries(from..to));
            advance =
                advance + reshaped - self.exact_advance(bounds.clone(), from, to, rest, closes);
        }
        advance
    }

    /// Returns the bidi level of shaping run `run`: its script run's.
    ///
    /// A shaping run never crosses a script run, which has one level. Returns
    /// `paragraph` without mixed levels, or past the runs.
    pub(super) fn run_level(&self, run: Option<&ShapedRun>, paragraph: BidiLevel) -> BidiLevel {
        if !self.gates.has_mixed_levels {
            return paragraph;
        }
        run.and_then(|run| self.stages.analysis.runs.get(run.script_run))
            .map_or(paragraph, |run| run.level)
    }

    /// Returns how the glyphs of shaping run `run` stand: its script run's
    /// orientation, or horizontal unless some text stands upright.
    pub(super) fn orientation(&self, run: Option<&ShapedRun>) -> RunOrientation {
        if !self.gates.has_upright {
            return RunOrientation::Horizontal;
        }
        run.and_then(|run| self.stages.analysis.runs.get(run.script_run))
            .map_or(RunOrientation::Horizontal, |run| run.orientation)
    }

    /// Returns the shaping run holding `cluster`, moving the run cursor
    /// forward to it.
    ///
    /// The walk meets clusters in order, so the cursor never moves back.
    pub(super) fn advance_run(&mut self, cluster: ClusterId) -> Option<&'a ShapedRun> {
        let runs = &self.stages.shaped.runs;
        while self.run.end() <= cluster && self.run.id().get() + 1 < runs.len() {
            runs.step(&mut self.run);
        }
        runs.get(self.run.id())
    }

    /// Returns whether `cluster` is of `class`.
    pub(super) fn is(&self, cluster: ClusterId, class: ClusterClass) -> bool {
        self.clusters.class(cluster) == Some(class)
    }

    /// Adds the pieces of text item `item`, holding `bounds`, for its
    /// clusters on `line`.
    ///
    /// Each piece belongs to the innermost open box. `rest` is the room of
    /// the items before it at its first boundary. `trailing` is where the
    /// line's trailing preserved white space starts, and `removable` where
    /// the trailing spaces a collapsing text removes may start.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &LineBoxes,
        line: &LineView<'_>,
        shapes: &[EdgeShape],
        id: ItemId,
        item: &Item,
        bounds: Range<ClusterId>,
        clusters: Range<ClusterId>,
        rest: InlineLayoutUnit,
        trailing: ClusterId,
        removable: ClusterId,
    ) {
        let (x, y) = (clusters.start, clusters.end);
        let content_end = line.content_end;
        let paragraph = self.line_state.level;
        let node = item.node;
        let owner = boxes.innermost();
        // Text sits on its box's baseline. So does its hanging part, set
        // where its box is.
        let shift = boxes.innermost_shifts().0;
        let facts = self.stages.content.facts.text(self.stages.text_facts(node));
        // Every piece of the text carries its emphasis flag.
        let marks = FragmentItemFlags::from_text(facts);
        let closes = self.closes_after(id, bounds.end, y);
        let piece_advance = |placer: &Self, u: ClusterId, v: ClusterId| -> InlineLayoutUnit {
            placer.advance(shapes, bounds.clone(), u, v, rest, closes)
        };
        // The piece of one cluster no glyph draws, at `level`, flagged
        // `flags`, in shaping run `run`. A tab is flagged `TAB` too, and is
        // as wide as the distance to its stop from its pen. The pen comes
        // from the prefix sums, past the room of the items before for the
        // item's first cluster.
        let cluster_piece = |placer: &mut Self,
                             cluster: ClusterId,
                             level: BidiLevel,
                             flags: FragmentItemFlags,
                             run: Option<ShapedRunId>| {
            let next = ClusterId::new(cluster.get() + 1);
            let mut advance = piece_advance(placer, cluster, next);
            let mut flags = flags.union(marks);
            if placer.is(cluster, ClusterClass::Tab) {
                let pen = placer.stages.measured.prefix.get(cluster);
                let pen = if cluster == bounds.start {
                    pen + rest
                } else {
                    pen
                };
                advance += placer.tab_width(cluster, pen, node, shapes);
                flags.insert(FragmentItemFlags::TAB);
            }
            Piece {
                flags,
                run: FragmentItem::link(run),
                item: FragmentItem::link(Some(id)),
                ..Piece::new(
                    PieceKind::Text,
                    node,
                    level,
                    cluster..next,
                    advance,
                    owner,
                    shift,
                )
            }
        };
        // The level of the last text placed, which a hyphen after it takes.
        let mut level = paragraph;
        let mut u = x;
        // Spaces a ruby base ends the line with are removed, though the
        // annotation's clusters come after them.
        let content = y.min(content_end.max(x)).min(removable.max(x));
        // Trailing white space inside one stretch of this item's text keeps
        // its level.
        let trailing = if x < trailing && trailing < content && content < bounds.end {
            self.trailing_level_split(trailing, content, paragraph)
        } else {
            trailing
        };
        // The content: text by used font, and each tab on its own, in this
        // item's own pieces. A grapheme split by a style boundary still gets
        // a piece per part, though both parts share a shaping run, as Chrome
        // gives a span's text its own item.
        while u < content {
            let run = self.advance_run(u);
            let run_id = run.map(|_| self.run.id());
            let mut to = self.run.end().min(content);
            if to <= u {
                // Every cluster is in a shaping run that ends past it. If
                // not, the rest of the item makes no piece, so the walk
                // can't stall.
                debug_assert!(false, "{u:?} lies past the shaping runs");
                break;
            }
            if u < trailing && trailing < to {
                to = trailing;
            }
            // The level is the shaping run's, or the paragraph's for the
            // line's trailing preserved white space (L1).
            level = if u >= trailing {
                paragraph
            } else {
                self.run_level(run, paragraph)
            };
            let shaped = self.clusters.class(u).is_some_and(ClusterClass::is_shaped);
            // An autospace seam's room the text before left out stands
            // before this text, at its level and in its box.
            if self.gates.has_seams_before && u > line.clusters().start {
                self.seam_gap(pieces, u, node, level, (owner, shift));
            }
            if shaped {
                let piece = Piece {
                    font: run.map(|run| run.font),
                    run: FragmentItem::link(run_id),
                    item: FragmentItem::link(Some(id)),
                    orientation: self.orientation(run),
                    flags: marks,
                    ..Piece::new(
                        PieceKind::Text,
                        node,
                        level,
                        u..to,
                        piece_advance(self, u, to),
                        owner,
                        shift,
                    )
                };
                // Text inside a spread ruby base takes its share of room.
                // Text elsewhere takes none.
                if self.line_ruby.spread.is_some() {
                    self.spread_text(pieces, piece, &piece_advance);
                } else {
                    pieces.push_or_join(piece);
                }
            } else {
                // Clusters no glyph draws. Each tab is a piece of its own,
                // as wide as the distance to its stop. The rest take no room
                // and join the text around them.
                for cluster in (u..to).ids() {
                    let piece =
                        cluster_piece(self, cluster, level, FragmentItemFlags::NONE, run_id);
                    match &self.line_ruby.spread {
                        Some(spread) => push_spread_cluster(pieces, piece, spread, cluster),
                        None => pieces.push_or_join(piece),
                    }
                }
            }
            u = to;
        }
        // A line broken at a soft hyphen draws a hyphen right after it, in
        // its box.
        if line.flags.contains(LineFlags::HYPHENATED) && y == line.clusters().end && content == y {
            self.hyphen(pieces, x..y, node, level, owner, shift);
        }
        // What hangs:
        // - preserved white space becomes a piece past the content, at the
        //   paragraph's level (L1), in its box so the box covers it, as in
        //   Chrome;
        // - a collapsible space at the line's very end is removed, as
        //   Blink's breaker removes it; one before a hanging ideographic
        //   space hangs with it;
        // - a separator, or a break opportunity the builder made, draws
        //   nothing.
        let collapsible = facts.collapse.collapses_spaces();
        for cluster in (content.max(x)..y).ids() {
            if self.is(cluster, ClusterClass::Separator)
                || self.is(cluster, ClusterClass::BreakOpportunity)
                || (collapsible && cluster >= removable && self.is(cluster, ClusterClass::Space))
            {
                continue;
            }
            let run = self.advance_run(cluster).map(|_| self.run.id());
            let piece = cluster_piece(self, cluster, paragraph, FragmentItemFlags::HANGS, run);
            pieces.push_or_join(piece);
        }
    }

    /// Adds the hyphen after `clusters` of `node`, where a break after the
    /// last draws one, as Blink's `PlaceHyphen` adds it after the item.
    ///
    /// It uses the soft hyphen's style, at `level`, in the box `owner`,
    /// raised `shift`. Kept out of line, since only a line or two a paragraph
    /// needs it.
    #[inline(never)]
    fn hyphen(
        &mut self,
        pieces: &mut LinePieces,
        clusters: Range<ClusterId>,
        node: NodeId,
        level: BidiLevel,
        owner: Option<LineBoxId>,
        shift: LayoutUnit,
    ) {
        let Some(last) = clusters.end.get().checked_sub(1).map(ClusterId::new) else {
            return;
        };
        let analysis = self.stages.analysis;
        let hyphenated = (analysis.clusters.attrs(last))
            .is_some_and(|attrs| analysis.breaks_with_hyphen(last, attrs));
        if last < clusters.start || !hyphenated {
            return;
        }
        let facts = self.stages.text_facts(node);
        let generated = self.input.stages.measured.generated();
        self.line_state.hyphen = generated.hyphen(facts);
        if let Some(hyphen) = self.line_state.hyphen {
            pieces.push_hyphen(Piece::new(
                PieceKind::Hyphen,
                node,
                level,
                clusters.end..clusters.end,
                generated.advance(hyphen),
                owner,
                shift,
            ));
        }
    }

    /// Returns the width of the tab `tab` of `node` where it lands, and
    /// counts it among the line's tabs.
    ///
    /// The width runs from its pen `pen`, adjusted for the indent, earlier
    /// reshapes and earlier tabs, to the next stop. The breaker used the same
    /// function, so the pieces add up to the width it fitted.
    fn tab_width(
        &mut self,
        tab: ClusterId,
        pen: InlineLayoutUnit,
        node: NodeId,
        shapes: &[EdgeShape],
    ) -> InlineLayoutUnit {
        if !self.line_state.tab_line.any {
            return InlineLayoutUnit::ZERO;
        }
        let measured = self.input.stages.measured;
        let Some(metrics) = measured.text_metrics(self.stages.text_facts(node)) else {
            return InlineLayoutUnit::ZERO;
        };
        let stops = metrics.tab;
        let moved = shapes
            .iter()
            .filter(|shape| shape.clusters().end <= tab)
            .fold(InlineLayoutUnit::ZERO, |sum, shape| {
                sum + self.shape_delta(shape)
            });
        let line = self.line_state.tab_line;
        let position = pen - line.start + moved + line.before;
        let reach = self.stages.measured.ruby_columns().tab_reach(tab);
        let width =
            InlineLayoutUnit::from_layout(tab_advance_reached(line.origin, position, stops, reach));
        self.line_state.tab_line.before += width;
        width
    }

    /// Returns how far a reshaped edge moves what follows it: its reshaped
    /// advance less the prefix sums' over the same clusters.
    fn shape_delta(&self, shape: &EdgeShape) -> InlineLayoutUnit {
        let clusters = shape.clusters();
        let reshaped = self
            .lines
            .edges()
            .advance_sum(shape.entries(clusters.clone()));
        let prefix = &self.stages.measured.prefix;
        reshaped - prefix.advance(clusters.start, clusters.end)
    }

    /// Returns whether the box `node` keeps a fragment, moving the kept-box
    /// cursor to it.
    ///
    /// Boxes open in node order.
    fn keeps(&mut self, node: NodeId) -> bool {
        self.next_kept = self.kept_boxes.walk_to(self.next_kept, node);
        self.kept_boxes.node(self.next_kept) == Some(node)
    }

    /// Returns how far `node`'s baseline is raised from `line`'s, and the
    /// pinned shift for `top` or `bottom` boxes inside it.
    ///
    /// The shift is `parent` plus the node's measured shift and what the
    /// line settled. A `top` or `bottom` box takes `pinned` plus what the
    /// line settled, whatever the boxes between do.
    pub(super) fn shift(
        &mut self,
        line: &LineView<'_>,
        node: NodeId,
        (parent, pinned): (LayoutUnit, LayoutUnit),
    ) -> (LayoutUnit, LayoutUnit) {
        // The initial letter is set from the first line's top, where
        // measurement put its baseline, whatever the line holds, and moved
        // down as far as the line's annotations moved it.
        if let Some(letter) = self.stages.measured.initial_letter()
            && letter.node == node
        {
            let mut baseline = letter.baseline + self.lines.letter_shift();
            // Measurement places the box from the line's block-start side.
            // That is its under side where lines flip, so the box turns
            // about the line box's middle, its over side kept over.
            if self.stages.content.block.writing_mode.flips_lines() {
                let height = line.extent.zero_if_none().box_height();
                baseline = height - baseline + letter.extent.ascent() - letter.extent.descent();
            }
            return (line.ascent() - baseline, pinned);
        }
        let settled = self.lines.shift(line, node);
        match self.box_facts(node).align {
            VerticalAlign::Top | VerticalAlign::Bottom => {
                let shift = pinned + settled;
                (shift, shift)
            }
            _ => (parent + self.measured_shift(node) + settled, pinned),
        }
    }

    /// Returns how far `node`'s baseline is raised from its parent's by
    /// what doesn't depend on its line.
    ///
    /// The boxes are met in node order, so the measured shifts are read by
    /// a cursor, sought the first time. Without any, nothing is read.
    fn measured_shift(&mut self, node: NodeId) -> LayoutUnit {
        let shifts = self.stages.measured.shifts();
        if shifts.is_empty() {
            return LayoutUnit::ZERO;
        }
        self.shift_rows
            .get_or_insert_with(|| shifts.cursor(node))
            .get(node)
            .map_or(LayoutUnit::ZERO, |&(_, shift)| shift)
    }

    /// Handles the opening or closing edge `item` at boundary `at`, taking
    /// `room`.
    ///
    /// A kept box opens or closes in `boxes`, a kept ruby container's as
    /// any other's; its edges go with the box's parts. The edge of a ruby
    /// container that keeps no box becomes a `Room` piece.
    #[allow(clippy::too_many_arguments)]
    fn edge(
        &mut self,
        pieces: &mut LinePieces,
        boxes: &mut LineBoxes,
        line: &LineView<'_>,
        item: &Item,
        at: ClusterId,
        opens: bool,
        room: InlineLayoutUnit,
    ) {
        let node = item.node;
        let ruby = matches!(item.kind, ItemKind::RubyOpen | ItemKind::RubyClose);
        // A kept box opens where the kept-box cursor finds it, and closes as
        // the innermost box open: a ruby container's as any other's.
        if opens && self.keeps(node) {
            let shifts = self.shift(line, node, boxes.innermost_shifts());
            boxes.open_box(pieces, (node, Some(self.next_kept)), at, true, shifts);
            return;
        }
        if !opens
            && let Some(index) = boxes.innermost()
            && let Some(open) = boxes.line_boxes.get_mut(index)
            && open.node == node
        {
            open.closes = true;
            boxes.stack.pop();
            return;
        }
        if !ruby {
            return;
        }
        if room == InlineLayoutUnit::ZERO {
            return;
        }
        pieces.push(Piece::new(
            PieceKind::Room,
            node,
            BidiLevel::LTR,
            at..at,
            room,
            boxes.innermost(),
            LayoutUnit::ZERO,
        ));
    }
}

/// Adds `piece`, the one cluster `cluster` of a spread ruby base, with the
/// room `spread` gives it before and after.
///
/// Out of line, as only a line with a spread ruby base gives room.
#[cold]
#[inline(never)]
fn push_spread_cluster(
    pieces: &mut LinePieces,
    piece: Piece,
    spread: &Justified<'_>,
    cluster: ClusterId,
) {
    pieces.push_with_room(piece, spread.room(cluster));
}
