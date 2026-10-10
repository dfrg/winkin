//! Writes a line's ruby annotation lines.
//!
//! Once the line is placed, each annotation is set as a line of its own
//! over the column's box. `ruby-align` spreads it, as Chrome's
//! `PlaceRubyAnnotation` does. Its baseline comes from `LevelBands::stack`,
//! the function the breaker sized the line with. The annotation lines'
//! items follow the line's own.

use alloc::vec::Vec;
use core::iter::from_fn;
use core::ops::Range;

use super::justify::Justified;
use super::place::{LinePieces, PieceId};
use super::read::AnnotationText;
use super::ruby::LineRubies;
use super::{
    FragmentItem, FragmentItemFlags, FragmentItemKind, Fragments, Piece, PieceKind, Placer,
    spreads_inside,
};
use crate::data::IdRange;
use crate::data::{Id, Table, define_id, heap_bytes, stable_sort_by_key};
use crate::stages::analysis::{BidiLevel, ClusterId};
use crate::stages::content::{ItemFlags, ItemId, ItemKind, NodeId, NodeKind};
use crate::stages::lines::{LineId, LineView};
use crate::stages::measure::{
    JustifyOpportunities, KeptBoxId, KeptBoxes, RubyColumnId, RubyLevel, RubyLevelId, RubySpread,
};
use crate::stages::{Segments, Step, measure};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

define_id! {
    /// Names a box inside the annotation line being set, in
    /// [`BoxesInAnnotation`].
    struct AnnotationBoxId(u32);
}

/// A box inside the annotation line being set: its node, its kept box, its
/// range of the annotation's pieces in logical order, and their reach once
/// placed.
#[derive(Copy, Clone, Debug)]
struct BoxInAnnotation {
    node: NodeId,
    /// Its row among the kept boxes of the line's variant, or `None` where
    /// it keeps no fragment and writes no item.
    kept: Option<KeptBoxId>,
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
    /// Where the walk over the kept boxes stands. The annotation's element
    /// seeks it, and its boxes, met in node order, walk on from there.
    kept: Option<KeptBoxId>,
}

impl BoxesInAnnotation {
    /// Makes an empty table, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            boxes: Table::new(),
            open: Vec::new(),
            kept: None,
        }
    }

    /// Forgets the last annotation line's boxes, and returns the kept row of
    /// `annotation`, the element of the next, among `kept_boxes`.
    fn begin(&mut self, kept_boxes: &KeptBoxes, annotation: NodeId) -> Option<KeptBoxId> {
        self.boxes.clear();
        self.open.clear();
        let at = kept_boxes.first_from(annotation);
        self.kept = Some(at);
        (kept_boxes.node(at) == Some(annotation)).then_some(at)
    }

    /// Opens the box `node` before piece `at`, with its kept row among
    /// `kept_boxes`.
    fn open(&mut self, kept_boxes: &KeptBoxes, node: NodeId, at: PieceId) {
        let row = match self.kept {
            Some(from) => kept_boxes.walk_to(from, node),
            None => kept_boxes.first_from(node),
        };
        self.kept = Some(row);
        let kept = (kept_boxes.node(row) == Some(node)).then_some(row);
        let opened = self.boxes.push(BoxInAnnotation {
            node,
            kept,
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
    fn open_around(
        &mut self,
        kept_boxes: &KeptBoxes,
        nodes: impl Iterator<Item = NodeId>,
        at: PieceId,
    ) {
        let from = self.boxes.next_id();
        for node in nodes {
            self.open(kept_boxes, node, at);
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
    BoxesInAnnotation { boxes, open; kept }
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

impl<'a> Placer<'a> {
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
        let split = columns
            .iter()
            .position(|open| !open.continued)
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
                .map(|ruby| (ruby, self.line_state.column_parts)),
        );
        // Closing-item order is postorder: child annotations precede their
        // parent's, while siblings retain text order.
        if columns.iter().any(|open| {
            self.rubies
                .get(open.column)
                .is_some_and(|column| column.parent.is_some())
        }) {
            stable_sort_by_key(&mut columns[..], |open| open.close);
        }
        let baseline = line.ascent();
        for index in 0..columns.len() {
            let Some(&open) = columns.get(index) else {
                continue;
            };
            let (Some(column), Some(trailing)) = (self.rubies.get(open.column), open.trailing)
            else {
                continue;
            };
            // A split column's levels are its part's levels.
            let part = if open.split {
                self.line_part(open.column)
            } else {
                None
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
            let width = (right - left).max(InlineLayoutUnit::from_layout(open.sizes.width));
            let container = open.container;
            let levels = column.levels.clone();
            for id in levels.ids() {
                work::step();
                let Some(level) = self.rubies.level(id) else {
                    continue;
                };
                let mut level = level.clone();
                let mut first = level.open;
                if let Some(part) = part {
                    let Some(share) = self
                        .lines
                        .ruby
                        .as_ref()
                        .and_then(|ruby| ruby.level(part, id))
                    else {
                        continue;
                    };
                    if share.clusters.is_empty() {
                        continue;
                    }
                    if share.clusters.start > level.clusters.start {
                        first = share.item;
                    }
                    level.clusters = share.clusters.clone();
                    level.clusters.end = share.visible_end;
                    if level.clusters.is_empty() {
                        continue;
                    }
                    level.width = share.width;
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
        let rtl =
            self.gates.ruby_in_bidi && !clusters.is_empty() && first.is_some_and(BidiLevel::is_rtl);
        let spread = RubySpread::from_annotation(
            self.box_facts(place.container).ruby.align,
            self.gates.text_align,
            place.width.to_layout() - level.width,
            level.opportunities,
            size,
            rtl,
        );
        let spreading = Justified::new(
            || {
                JustifyOpportunities::new(
                    content,
                    self.stages.analysis,
                    clusters.clone(),
                    text.justify,
                )
            },
            clusters.clone(),
            InlineLayoutUnit::from_layout(spread.inside),
        );
        let kept = boxes.begin(self.kept_boxes, node);
        let parts = (node, place.first);
        let walk = (walk, first);
        self.annotation_pieces(pieces, boxes, line, level, parts, walk, spreading.as_ref());
        // Reorder by level as a line of its own, and set from the box's left.
        // Right to left, the logical start is on the right.
        let placed = pieces.logical.as_slice().iter();
        let base = placed.clone().map(|piece| piece.level).min();
        let total = placed.fold(InlineLayoutUnit::ZERO, |sum, piece| sum + piece.advance);
        pieces.reorder(base.unwrap_or(self.line_state.level));
        let start = InlineLayoutUnit::from_layout(spread.start);
        let lead = if rtl {
            place.width - start - total
        } else {
            start
        };
        // The text takes in the room spread at its opportunities, and the
        // room on its left and right, as Chrome's expansion is part of its
        // glyphs.
        pieces.absorb_spread();
        let all = PieceId::new(0)..pieces.logical.next_id();
        let trail = (place.width - lead - total).max(InlineLayoutUnit::ZERO);
        let taken = pieces.absorb_end(all.clone(), (lead, true), |_| false);
        pieces.absorb_end(all, (trail, false), |_| false);
        pieces.place(place.left + lead - taken, &Table::new(), None);
        boxes.reach(pieces);
        self.annotation_items(pieces, boxes, out, line.id, (node, kept), place);
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
    /// first, found up its first item's ancestors. `leading` is the level of
    /// the walk's first segment, which an edge before any text takes.
    #[allow(clippy::too_many_arguments)]
    fn annotation_pieces(
        &self,
        pieces: &mut LinePieces,
        boxes: &mut BoxesInAnnotation,
        line: &LineView<'_>,
        level: &RubyLevel,
        (node, first): (NodeId, ItemId),
        (mut walk, leading): (Segments<'a>, Option<BidiLevel>),
        spreading: Option<&Justified<'_>>,
    ) {
        pieces.clear();
        if first != level.open {
            let around = self.boxes_around(first, node);
            boxes.open_around(self.kept_boxes, around, pieces.logical.next_id());
        }
        let clusters = level.clusters.clone();
        let words = self.input.stages.measured.word_spacing_rule;
        // The line's reshaped edges that hold some of the level.
        let shapes = self.lines.edges.line_edges_touching(line, &clusters);
        // The bidi level of the last text segment, which a closing edge
        // after it takes, as does an opening edge after the last.
        let mut before = None;
        // The level of the first segment from `ahead`. Before the walk meets
        // a segment, that is the walk's first, found already.
        let mut met = false;
        let after = |ahead: Segments<'a>, met: bool| {
            if met {
                self.first_level(ahead, clusters.end)
            } else {
                leading
            }
        };
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
                        let beside = after(ahead, met);
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
                        let beside = before.or_else(|| after(ahead, met));
                        self.annotation_edge(pieces, item.node, beside, at, room);
                        break;
                    }
                    let opens = item.kind == ItemKind::Open;
                    let actual = self.item_clusters.start(id);
                    if actual < clusters.start {
                        if opens {
                            boxes.open(self.kept_boxes, item.node, pieces.logical.next_id());
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
                    let beside = if opens {
                        after(ahead, met).or(before)
                    } else {
                        before.or_else(|| after(ahead, met))
                    };
                    if opens {
                        boxes.open(self.kept_boxes, item.node, pieces.logical.next_id());
                    }
                    pieces.push(Piece::new(
                        PieceKind::Gap,
                        item.node,
                        beside.unwrap_or(self.line_state.level),
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
            met = true;
            let Some(item) = self.items.get(segment.item) else {
                continue;
            };
            if !item.flags.contains(ItemFlags::ANNOTATION) {
                continue;
            }
            let run = self.stages.shaped.runs.get(segment.run);
            let bidi = self.run_level(run, self.line_state.level);
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
            let facts = self
                .stages
                .content
                .facts
                .text(self.stages.text_facts(item.node));
            let flags = FragmentItemFlags::from_text(facts);
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
                    run: Some(segment.run),
                    item: Some(segment.item),
                    orientation,
                    flags,
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
            beside.unwrap_or(self.line_state.level),
            at..at,
            room,
            None,
            LayoutUnit::ZERO,
        ));
    }

    /// Writes the items of the annotation line for `node`, kept row `kept`,
    /// at `place`: its head, then its kept boxes, then its text and atomic
    /// inlines.
    fn annotation_items(
        &self,
        pieces: &LinePieces,
        boxes: &BoxesInAnnotation,
        out: &mut Fragments,
        line: LineId,
        (node, kept): (NodeId, Option<KeptBoxId>),
        place: AnnotationPlace,
    ) {
        // Kept boxes go after the head and before the text, outermost first,
        // in paint order. Each spans its border box over all its pieces, as
        // Chrome gives a box in an annotation its own fragment. Where
        // reordering splits a box's pieces, one item covers them all.
        let kept_count = boxes
            .boxes
            .as_slice()
            .iter()
            .filter(|found| found.reach.is_some() && found.kept.is_some())
            .count();
        let leaves = pieces
            .logical
            .as_slice()
            .iter()
            .filter(|piece| matches!(piece.kind, PieceKind::Text | PieceKind::Atomic))
            .count();
        let head = FragmentItem::from_annotation(
            node,
            self.line_state.level,
            place.id,
            place.left,
            place.width.to_layout(),
            place.baseline,
            kept_count + leaves,
            kept,
        );
        out.items
            .push_bounded(head, "no more items than a FragmentItemId names");
        for found in boxes.boxes.as_slice() {
            let (Some((reach_left, reach_right)), Some(kept)) = (found.reach, found.kept) else {
                continue;
            };
            let (left, right) = self.box_facts(found.node).margin_line;
            let from = reach_left + InlineLayoutUnit::from_layout(left);
            let to = (reach_right - InlineLayoutUnit::from_layout(right)).max(from);
            let level = pieces
                .logical
                .get(found.start)
                .map_or(self.line_state.level, |piece| piece.level);
            let item = FragmentItem::from_box_part(
                found.node,
                level,
                from,
                (to - from).to_layout(),
                place.baseline,
                FragmentItemFlags::NONE,
                0,
                None,
            )
            .with_kept_box(kept);
            out.items
                .push_bounded(item, "no more items than a FragmentItemId names");
        }
        for &logical in pieces.order.as_slice() {
            let Some(piece) = pieces.logical.get(logical) else {
                continue;
            };
            let kind = match piece.kind {
                PieceKind::Text => FragmentItemKind::Text,
                PieceKind::Atomic => FragmentItemKind::Atomic,
                _ => continue,
            };
            if piece.room != (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO) {
                out.push_spread(line, piece);
            }
            let flags = piece.flags.union(FragmentItemFlags::ANNOTATION);
            let item = piece.fragment_item(kind, flags, place.baseline);
            out.items
                .push_bounded(item, "no more items than a FragmentItemId names");
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
        Some(self.run_level(
            self.stages.shaped.runs.get(segment.run),
            self.line_state.level,
        ))
    }
}
