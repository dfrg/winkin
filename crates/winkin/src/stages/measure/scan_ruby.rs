//! Ruby columns, each sized where the scan meets it, before its first
//! cluster is measured.
//!
//! A look-ahead runs over the column with a copy of the scan's walk. It
//! measures the base as the scan is about to, each annotation as a line of
//! its own, and how far the column overhangs the text beside it.

use crate::stages::fonts::FontRunId;
use crate::stages::shape::ClusterGlyphs;
use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

use super::BoundaryRoom;
use super::autospace::Seams;
use super::edges::edge_room;
use super::ruby::RubySpread;
use super::ruby_columns::{ColumnRoom, ColumnRoomId};
use super::{
    Extent, JustifyOpportunities, MeasuredText, RubyColumn, RubyColumnId, RubyLevel, RubyLevelId,
    RubySide, Scan, ScanWalk, TabReach, ruby,
};
use crate::config::RubyOverhangRule;
use crate::data::{Id, IdRange, sort_by_key};
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::{
    Content, Item, ItemFlags, ItemId, ItemKind, NodeId, TextFactsId, TextFlags,
};
use crate::style::{RubyAlign, RubyGroup, RubyOverhang, TextWrapStyle};
use crate::unicode::TextSpacingClass;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::{unicode, work};

/// A level of a ruby column being measured: where it opened, and what it
/// has taken so far.
#[derive(Copy, Clone, Debug)]
struct OpenLevel {
    item: ItemId,
    start: ClusterId,
    width: InlineLayoutUnit,
    steps: Option<u32>,
}

/// The ruby column the scan is in, sized where it started.
#[derive(Copy, Clone, Debug)]
pub(super) struct OpenColumn {
    id: RubyColumnId,
    pub(super) start: ClusterId,
    pub(super) may_break: bool,
    /// The boundary it ends at, after its last annotation.
    pub(super) end: ClusterId,
    /// What it takes along the line past its base: its width, less the
    /// base's, less its overhang either side.
    extra: InlineLayoutUnit,
    /// How far it overhangs the text before it and after it.
    ///
    /// A line it starts or ends has no text there, so that line pays the
    /// overhang.
    start_overhang: LayoutUnit,
    end_overhang: LayoutUnit,
    /// The first item of the next column of its container, where one starts
    /// where this one ends.
    next: Option<ItemId>,
}

/// What the scan carries about ruby from one boundary to the next.
#[derive(Copy, Clone, Debug)]
pub(super) struct RubyBoundaries {
    /// The ruby column the scan is in, sized where it started.
    pub(super) column: Option<OpenColumn>,
    /// The column that just ended, as the min-content lines take it: its
    /// whole width, and its first, last and widest pieces.
    pub(super) ended: Option<(LayoutUnit, LayoutUnit, LayoutUnit, LayoutUnit)>,
    /// The ruby container the scan is in.
    container: NodeId,
    /// The next room a column leaves for the columns nested in its base.
    child_room: ColumnRoomId,
    /// The last column that ended, as the next one's start overhang reads it.
    last: Option<EndedColumn>,
    /// The last tab passed, and how far it reaches on its paragraph's line.
    last_tab: Option<(ClusterId, LayoutUnit)>,
    /// The font run the last column's em boxes ended in, which the next
    /// column's walk starts from.
    font_run: FontRunId,
}

/// A ruby column that ended, as the next column's start overhang reads it.
#[derive(Copy, Clone, Debug)]
struct EndedColumn {
    /// One past its last item.
    close: ItemId,
    /// The running sum where the text after it starts.
    after: InlineLayoutUnit,
    /// How far it overhangs the text after it.
    end_overhang: LayoutUnit,
}

impl RubyBoundaries {
    /// Returns the state before the first boundary: no column, in the block.
    pub(super) fn new() -> Self {
        Self {
            column: None,
            ended: None,
            container: NodeId::BLOCK,
            child_room: ColumnRoomId::new(0),
            last: None,
            last_tab: None,
            font_run: FontRunId::new(0),
        }
    }

    /// Notes the tab at `tab`, which reaches `width` on its paragraph's
    /// line, for a column starting after it.
    pub(super) fn pass_tab(&mut self, tab: ClusterId, width: InlineLayoutUnit) {
        self.last_tab = Some((tab, width.to_layout()));
    }

    /// Closes the ruby column ending at boundary `at`, sizes one starting
    /// there, and adds the room of both to `room`.
    ///
    /// The scan's walk `walk` stands at `at`, its items crossed, and `sum` is
    /// the prefix before them. A look-ahead sizes a new column over a copy of
    /// `walk` and `seams`, so the scan's own stay where they are.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cross<'a>(
        &mut self,
        scan: &Scan<'a>,
        walk: &ScanWalk<'a>,
        seams: Seams,
        prefix: &[InlineLayoutUnit],
        ruby_starts: &mut Vec<ClusterId>,
        at: ClusterId,
        sum: InlineLayoutUnit,
        room: &mut BoundaryRoom,
        out: &mut MeasuredText,
    ) {
        let ended = self.close(scan, prefix, ruby_starts, at, sum, room, out);
        if let Some(first) = self.next_first(scan, ended, room, out) {
            let start = sum + room.leading_closes;
            let before = walk.leaf_before(scan, prefix, at, start);
            let font_run = Cell::new(self.font_run);
            let sizing = Sizing {
                scan,
                prefix,
                container: self.container,
                parent: None,
                start_limit: self.start_limit(scan, first, start),
                tab_before: self.last_tab,
                font_run: &font_run,
            };
            let first_room = out.ruby_columns().rooms.next_id();
            let mut look = *walk;
            self.column = sizing.column(&mut look, seams, at, first, before, out);
            self.font_run = font_run.get();
            let past_room = out.ruby_columns().rooms.next_id();
            if let Some(rooms) = out
                .rare_mut()
                .rubies
                .rooms
                .get_slice_mut(first_room..past_room)
            {
                sort_by_key(rooms, |room| room.at);
            }
            if let Some(open) = &self.column {
                room.overhang.start = InlineLayoutUnit::from_layout(open.start_overhang);
            }
            // A column with no clusters ends where it starts. Its room starts
            // the line after, and is no part of a glyph's advance, as a nested
            // one's is.
            if let Some(open) = self.column.take_if(|open| open.end == at) {
                let extra = open.extra.to_layout();
                if extra != LayoutUnit::ZERO {
                    let _ = out.rare_mut().rubies.rooms.push(ColumnRoom {
                        at,
                        extra,
                        empty: true,
                    });
                }
            }
        }
        self.pass_child_rooms(at, room, out);
    }

    /// Closes the column ending at `at`, if any, and returns it.
    ///
    /// The column takes what it adds past its base, as a closing edge does,
    /// and its overhang after it. `sum` is the prefix before the items at
    /// `at`.
    #[allow(clippy::too_many_arguments)]
    fn close(
        &mut self,
        scan: &Scan<'_>,
        prefix: &[InlineLayoutUnit],
        ruby_starts: &mut Vec<ClusterId>,
        at: ClusterId,
        sum: InlineLayoutUnit,
        room: &mut BoundaryRoom,
        out: &MeasuredText,
    ) -> Option<OpenColumn> {
        let open = self.column.take_if(|open| open.end == at)?;
        let rubies = out.ruby_columns();
        if let Some((first, last, widest)) =
            rubies.min_pieces(open.id, scan.input, scan.shaped, prefix, ruby_starts)
            && let Some(column) = rubies.get(open.id)
        {
            let whole = column.width - column.overhang.0 - column.overhang.1;
            self.ended = Some((whole, first, last, widest));
        }
        room.leading_closes += open.extra;
        room.overhang.end = InlineLayoutUnit::from_layout(open.end_overhang);
        self.last = rubies.get(open.id).map(|column| EndedColumn {
            close: column.close,
            after: sum + room.leading_closes,
            end_overhang: open.end_overhang,
        });
        Some(open)
    }

    /// Returns how far a column whose first item is `first`, starting at the
    /// running sum `start`, may overhang the text before it without meeting
    /// the last column's annotation.
    ///
    /// Chrome takes the last column's end overhang off the text between them
    /// where only text and tags stand between (`GetOverhang`,
    /// `FindPreviousRubyIndex`). Anything else between leaves no bound.
    fn start_limit(
        &self,
        scan: &Scan<'_>,
        first: ItemId,
        start: InlineLayoutUnit,
    ) -> Option<LayoutUnit> {
        let last = self.last?;
        let items = &scan.content.items;
        for id in (last.close..first).ids() {
            work::step();
            let item = items.get(id)?;
            let passes = !item.flags.contains(ItemFlags::ANNOTATION)
                && matches!(
                    item.kind,
                    ItemKind::Text
                        | ItemKind::Open
                        | ItemKind::Close
                        | ItemKind::RubyOpen
                        | ItemKind::RubyClose
                );
            if !passes {
                return None;
            }
        }
        let between = (start - last.after).to_layout();
        Some((between - last.end_overhang).max(LayoutUnit::ZERO))
    }

    /// Returns the first item of a column starting here: the first in a
    /// container opening in `room`, or the next after `ended`.
    fn next_first(
        &mut self,
        scan: &Scan<'_>,
        ended: Option<OpenColumn>,
        room: &BoundaryRoom,
        out: &MeasuredText,
    ) -> Option<ItemId> {
        // Opening marks before the ending root's close belong to descendants
        // that root's look-ahead sized already.
        let opening = room.ruby.filter(|&opens| {
            ended.as_ref().is_none_or(|open| {
                out.ruby_columns()
                    .get(open.id)
                    .is_none_or(|column| column.close < opens)
            })
        });
        match opening {
            Some(opens) if self.column.is_none() => {
                self.container =
                    (scan.content.items.get(opens)).map_or(self.container, |item| item.node);
                Some(ItemId::new(opens.get() + 1))
            }
            _ => ended.and_then(|open| open.next),
        }
    }

    /// Adds to `room` what the columns nested in a base leave at `at`.
    ///
    /// An empty one's room starts the line after. Another's ends the line
    /// before, as a closing edge does.
    fn pass_child_rooms(&mut self, at: ClusterId, room: &mut BoundaryRoom, out: &MeasuredText) {
        while let Some(child) = out.ruby_columns().rooms.get(self.child_room) {
            if child.at > at {
                break;
            }
            let extra = InlineLayoutUnit::from_layout(child.extra);
            if child.empty {
                room.rest += extra;
            } else {
                room.leading_closes += extra;
            }
            self.child_room = ColumnRoomId::new(self.child_room.get() + 1);
        }
    }
}

/// A ruby column starting where the scan is, to be sized, and what sizing
/// reads beyond the scan's walk.
struct Sizing<'s, 'a> {
    scan: &'s Scan<'a>,
    /// The running sums before the scan's boundary, and the shaped advances
    /// from it on.
    prefix: &'s [InlineLayoutUnit],
    /// The ruby container it is in.
    container: NodeId,
    parent: Option<RubyColumnId>,
    /// How far its start overhang may reach before meeting the last
    /// column's annotation, where that column is near enough to meet.
    start_limit: Option<LayoutUnit>,
    /// The last tab the scan passed, and how far it reaches on its
    /// paragraph's line.
    tab_before: Option<(ClusterId, LayoutUnit)>,
    /// The font run the last em box walk ended in, which the next starts
    /// from: the columns and their levels come nearly in text order.
    font_run: &'s Cell<FontRunId>,
}

impl<'a> Sizing<'_, 'a> {
    /// Sizes the ruby column starting at boundary `at` into `out`'s columns
    /// and levels.
    ///
    /// `first` is the column's first item. The look-ahead runs over a copy
    /// of the scan's walk `walk` and seams `seams`. The scan stands at `at`,
    /// with the items there passed and the cluster there not yet measured.
    /// `before` is the text before the column.
    ///
    /// Returns what the scan needs where the column ends, or `None` where
    /// the tables are full.
    ///
    /// - The base is measured as the scan is about to measure it, cluster by
    ///   cluster and edge by edge, so its width is what the prefix will hold.
    /// - Each annotation is measured as a line of its own: its shaped
    ///   advances, its own spacing and the edges of the boxes in it. Chrome
    ///   measures an annotation's line at its max-content (`HandleRuby`).
    /// - The column ends where its container does, or where base text
    ///   follows its last annotation. That text starts the container's next
    ///   column.
    fn column(
        &self,
        walk: &mut ScanWalk<'a>,
        seams: Seams,
        at: ClusterId,
        first: ItemId,
        before: Option<TextBeside>,
        out: &mut MeasuredText,
    ) -> Option<OpenColumn> {
        let scan = self.scan;
        let content = scan.content;
        // Reserve the parent row before its descendants: columns stay in
        // preorder, with increasing base starts, even at equal boundaries.
        let id = out.rare_mut().rubies.columns.push(RubyColumn {
            open: first,
            base_end: first,
            close: first,
            base: at..at,
            parent: self.parent,
            may_break: false,
            levels: Range::default(),
            width: LayoutUnit::ZERO,
            base_width: LayoutUnit::ZERO,
            room_before: LayoutUnit::ZERO,
            room_inside: LayoutUnit::ZERO,
            overhang: (LayoutUnit::ZERO, LayoutUnit::ZERO),
            base_em: Extent::NONE,
        })?;
        let steps_start = out.ruby_columns().steps.len();
        let mut state = Measuring {
            look: *walk,
            seams,
            base: InlineLayoutUnit::ZERO,
            in_base: true,
            base_end: at,
            base_end_item: first,
            level: None,
            levels: out.rare_mut().rubies.levels.next_id(),
            count: 0,
            depths: (0, 0),
            own_depths: (0, 0),
            widest: None,
            close: None,
            after_close: first,
            ended: false,
        };
        let (end, index) = self.measure_parts(&mut state, id, at, first, out)?;
        let close = state.close.unwrap_or(index);
        // Base text after the last annotation is the line's, not a column.
        let next = (state.count > 0
            && !is_container_end(content, close)
            && content.annotation_follows(close))
        .then_some(close);
        // Add up what the column's parts measure, and so what it takes.
        let levels_end = out.rare_mut().rubies.levels.next_id();
        let levels = if state.count == 0 {
            levels_end..levels_end
        } else {
            state.levels..levels_end
        };
        let base_width = state.base.to_layout();
        let width = out
            .ruby_columns()
            .levels
            .slice(levels.clone())
            .iter()
            .fold(base_width, |width, level| width.max(level.width));
        let space = width - base_width;
        // Read the container's facts: its ruby properties, and its text's
        // `text-justify` and size.
        let (nodes, facts) = (&content.nodes, &content.facts);
        let ruby = facts
            .box_facts(nodes.box_facts(self.container, scan.variant))
            .ruby;
        let text = nodes.text_facts(self.container, scan.variant);
        let justify = facts.text(text).justify;
        let opportunities = JustifyOpportunities::from_base(
            content,
            scan.analysis,
            at..state.base_end,
            justify,
            out.ruby_columns(),
            (id, id),
        )
        .map_or(0, |found| found.count_range(at..state.base_end));
        // Work out the overhang, and so where the base sits in the column.
        // It never reaches the container's next column, whose base is the
        // text after it: Chrome reaches over no ruby column
        // (`CommitPendingEndOverhang`).
        let after = match next {
            Some(_) => None,
            None => self.after_column(&state.look, end, close),
        };
        // The base's em box, or the container's font's where the base holds
        // no text: an empty base keeps its annotations, which stand on it.
        let base_em = self.em(first, state.base_end_item, false, out);
        let empty = base_em.is_none();
        let base_em = if empty {
            ruby::strut_em(scan.fonts, facts.text_request(text))
        } else {
            base_em
        };
        // Chrome overhangs nothing beside an empty base.
        let (overhang, tab_reaches) = if self.parent.is_some() || empty {
            ((LayoutUnit::ZERO, LayoutUnit::ZERO), [None, None])
        } else {
            let column = Overhanging {
                space,
                opportunities,
                ruby: Ruby {
                    group: ruby,
                    size: scan.input.size(text),
                },
                before,
                after,
            };
            self.overhang(column, state.widest)
        };
        let spread = match scan.input.ruby_overhang {
            // Chrome insets the base as if nothing overhung, and moves the
            // column over the text beside it.
            RubyOverhangRule::AdjacentText => {
                RubySpread::from_base(ruby.align, space, opportunities)
            }
            // JLREQ gives the base only what the overhang leaves, the
            // annotation reaching past it either side.
            RubyOverhangRule::KanaOnly => {
                let left = space - overhang.0 - overhang.1;
                let spread = RubySpread::from_base(ruby.align, left, opportunities);
                spread.with_start(spread.start + overhang.0)
            }
        };
        let mut record = RubyColumn {
            open: first,
            base_end: state.base_end_item,
            close,
            base: at..state.base_end,
            parent: self.parent,
            may_break: false,
            levels,
            width,
            base_width,
            room_before: spread.start,
            room_inside: spread.inside,
            overhang,
            base_em,
        };
        let may_break = self.may_break(&record, text, id, out);
        record.may_break = may_break;
        if !may_break && self.parent.is_none() {
            forget_steps(&record, steps_start, out);
        }
        *out.rare_mut().rubies.columns.get_mut(id)? = record;
        keep_tab_reaches(tab_reaches, out);
        if self.parent.is_some() {
            let extra = width - base_width;
            if extra != LayoutUnit::ZERO {
                let _ = out.rare_mut().rubies.rooms.push(ColumnRoom {
                    at: end,
                    extra,
                    empty: end == at,
                });
            }
        }
        let extra = InlineLayoutUnit::from_layout(width - overhang.0 - overhang.1) - state.base;
        *walk = state.look;
        Some(OpenColumn {
            id,
            start: at,
            may_break,
            end,
            extra,
            start_overhang: overhang.0,
            end_overhang: overhang.1,
            next,
        })
    }

    /// Measures the parts of the column `id`, from boundary `at` and its
    /// first item `first`: its base, cluster by cluster, the columns nested
    /// in it, and its annotations.
    ///
    /// Returns the boundary the column ends at and the first item past what
    /// was crossed there, or `None` where a table is full.
    fn measure_parts(
        &self,
        state: &mut Measuring<'a>,
        id: RubyColumnId,
        at: ClusterId,
        first: ItemId,
        out: &mut MeasuredText,
    ) -> Option<(ClusterId, ItemId)> {
        let scan = self.scan;
        let count = scan.analysis.clusters.len();
        // The items at the column's start: the edges of the boxes its base
        // opens with, and an annotation where its base is empty.
        let mut index = self.ruby_marks(state, at, first, out)?;
        let mut c = at;
        loop {
            work::step();
            if state.ended {
                return Some((c, index));
            }
            // A nested container is one unit of this base. Its own columns
            // are measured independently and never overhang their siblings.
            if let Some(item) = scan.content.items.get(index)
                && item.kind == ItemKind::RubyOpen
                && item.node != self.container
            {
                (c, index) = self.nested(state, id, item, c, index, out)?;
                continue;
            }
            if c.get() >= count {
                return Some((c, index));
            }
            self.measure_cluster(state, c);
            // Cross the boundary after it: its ruby marks and the edges of
            // the base's boxes, in order, from past the item holding the
            // cluster. The copy then moves into the next segment.
            let after = ClusterId::new(c.get() + 1);
            let past = state
                .look
                .item()
                .map_or(index, |(id, _)| ItemId::new(id.get() + 1));
            index = self.ruby_marks(state, after, index.max(past), out)?;
            if let Some(level) = state.level
                && level.steps.is_some()
                && level.start < after
            {
                out.rare_mut().rubies.steps.push(level.width);
            }
            state.look.cross(scan, after, None);
            // Base text after the column's annotations starts the next
            // column.
            let next = !state.ended
                && state.level.is_none()
                && state.count > 0
                && !state.look.annotation()
                && after.get() < count;
            if next {
                state.ended = true;
                state.close = Some(state.after_close);
            }
            c = after;
        }
    }

    /// Measures the ruby container `item` nested in the base of column
    /// `parent`, opening at its item `index` and boundary `at`, as one unit
    /// of the base: its edges and each of its columns, sized on their own.
    ///
    /// Returns where the base goes on, past the container's closing marks,
    /// or `None` where a table is full.
    fn nested(
        &self,
        state: &mut Measuring<'a>,
        parent: RubyColumnId,
        item: &Item,
        at: ClusterId,
        index: ItemId,
        out: &mut MeasuredText,
    ) -> Option<(ClusterId, ItemId)> {
        let content = self.scan.content;
        let child = Sizing {
            scan: self.scan,
            prefix: self.prefix,
            container: item.node,
            parent: Some(parent),
            start_limit: None,
            tab_before: None,
            font_run: self.font_run,
        };
        state.base += edge_room(content, None, item, true);
        let (mut c, mut index) = (at, index);
        let mut child_first = ItemId::new(index.get() + 1);
        loop {
            let child_id = out.ruby_columns().columns.next_id();
            let open = child.column(&mut state.look, state.seams, c, child_first, None, out)?;
            let record = out.ruby_columns().get(child_id)?;
            state.base += InlineLayoutUnit::from_layout(record.width);
            let child_end = out.ruby_columns().columns.next_id();
            for descendant in out.ruby_columns().slice(child_id..child_end) {
                for level in out.ruby_columns().levels(descendant) {
                    let depth = match level.side {
                        RubySide::Over => &mut state.depths.0,
                        RubySide::Under => &mut state.depths.1,
                    };
                    *depth = (*depth).max(level.depth.saturating_add(1));
                }
            }
            c = open.end;
            index = record.close;
            if let Some(next) = open.next {
                child_first = next;
            } else {
                break;
            }
        }
        if let Some(close) = content.items.get(index) {
            state.base += edge_room(content, None, close, false);
        }
        let index = self.ruby_marks(state, c, ItemId::new(index.get() + 1), out)?;
        Some((c, index))
    }

    /// Measures cluster `c` as the scan will, in the segment the copy of the
    /// walk holds: into the annotation open, or into the base.
    fn measure_cluster(&self, state: &mut Measuring<'a>, c: ClusterId) {
        let scan = self.scan;
        let attrs = scan.analysis.clusters.attrs(c);
        let class = attrs.map_or(ClusterClass::Text, ClusterAttrs::class);
        let advance = self.prefix.get(c.get()).copied().unwrap_or_default();
        let gap = match state.look.segment {
            Some(segment)
                if scan
                    .analysis
                    .paragraphs
                    .get(segment.paragraph)
                    .is_some_and(|para| scan.has_seams(para)) =>
            {
                InlineLayoutUnit::from_text(state.look.seam_after(scan, &mut state.seams, c))
            }
            _ => InlineLayoutUnit::ZERO,
        };
        if state.look.annotation() {
            if let Some(level) = state.level.as_mut() {
                // An atomic inline takes its margin box, as on a line.
                let own = match state.look.atomic {
                    Some(width) if class == ClusterClass::Object => width,
                    _ => advance,
                };
                level.width += own + state.look.spacing(scan, class, c);
            }
        } else if state.in_base {
            state.base += state.look.step(scan, class, advance, c) + gap;
        }
    }

    /// Returns whether the column `record`, whose container's text facts are
    /// `text`, may split across lines, as Chrome lets it.
    ///
    /// A root column splits where its container wraps, the block's
    /// `text-wrap-style` allows it, and its base has text and an opportunity
    /// inside. Its base must hold more than four glyphs, or one of its levels
    /// more than eight.
    fn may_break(
        &self,
        record: &RubyColumn,
        text: TextFactsId,
        id: RubyColumnId,
        out: &MeasuredText,
    ) -> bool {
        let content = self.scan.content;
        let clusters = &self.scan.analysis.clusters;
        let rubies = out.ruby_columns();
        let inside = || record.base.start..ClusterId::new(record.base.end.get() - 1);
        self.parent.is_none()
            && content.facts.text(text).has(TextFlags::WRAPS)
            && matches!(
                content.block.text_wrap_style,
                TextWrapStyle::Auto | TextWrapStyle::Stable
            )
            && record.base_width > LayoutUnit::ZERO
            && (self.glyph_count(record.open, record.base.clone(), 5, false) > 4
                || rubies
                    .levels(record)
                    .iter()
                    .any(|level| self.glyph_count(level.open, level.clusters.clone(), 9, true) > 8))
            && record.base.end.get() > record.base.start.get() + 1
            && (rubies
                .opportunity(clusters, inside(), Some(id), (false, false), &Cell::new(id))
                .is_some()
                || rubies
                    .opportunity(clusters, inside(), Some(id), (false, true), &Cell::new(id))
                    .is_some())
    }

    /// Counts the glyphs of `range` in the base's items or, with
    /// `annotation`, in the annotations' items, stopping at `limit`.
    ///
    /// The items are walked from `from`, the base's or the level's opening
    /// item.
    fn glyph_count(
        &self,
        from: ItemId,
        range: Range<ClusterId>,
        limit: usize,
        annotation: bool,
    ) -> usize {
        let scan = self.scan;
        let item_clusters = &scan.analysis.item_clusters;
        let mut count = 0;
        let mut cursor = item_clusters.cursor(item_clusters.walk_to(from, range.start));
        for at in range.ids() {
            while cursor.end() <= at {
                item_clusters.step(&mut cursor);
            }
            if scan
                .content
                .items
                .get(cursor.id())
                .is_none_or(|item| item.flags.contains(ItemFlags::ANNOTATION) != annotation)
            {
                continue;
            }
            count += match scan.shaped.glyphs.glyphs(at) {
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

    /// Crosses the ruby marks among the items at boundary `at`, from
    /// `index`, in order.
    ///
    /// - An annotation opening ends the base and starts a level.
    /// - An annotation closing records its level.
    /// - The edges of boxes in an annotation widen its line; those in the
    ///   base widen the base.
    /// - The container closing ends the column.
    ///
    /// Returns the first item past them, or `None` where the levels' table
    /// is full.
    fn ruby_marks(
        &self,
        state: &mut Measuring<'a>,
        at: ClusterId,
        index: ItemId,
        out: &mut MeasuredText,
    ) -> Option<ItemId> {
        let content = self.scan.content;
        let items = &content.items;
        let item_clusters = &self.scan.analysis.item_clusters;
        let mut index = index;
        while let Some(item) = items.get(index) {
            work::step();
            let clusters = item_clusters.range(index);
            if clusters.start != at || !clusters.is_empty() || state.ended {
                break;
            }
            let annotation = item.flags.contains(ItemFlags::ANNOTATION);
            match item.kind {
                ItemKind::RubyOpen if item.node != self.container && !annotation => break,
                ItemKind::AnnotationOpen if state.level.is_none() => {
                    if state.count == 0 {
                        state.levels = out.rare_mut().rubies.levels.next_id();
                    }
                    if state.in_base {
                        state.in_base = false;
                        state.base_end = at;
                        state.base_end_item = index;
                    }
                    state.level = Some(OpenLevel {
                        item: index,
                        start: at,
                        width: edge_room(content, None, item, true),
                        steps: if self.parent.is_none() {
                            let steps = &mut out.rare_mut().rubies.steps;
                            let first = u32::try_from(steps.len()).ok();
                            steps.push(InlineLayoutUnit::ZERO);
                            first
                        } else {
                            None
                        },
                    });
                }
                ItemKind::AnnotationClose => {
                    if let Some(level) = state.level.take() {
                        let width = level.width + edge_room(content, None, item, false);
                        if level.steps.is_some() {
                            let steps = &mut out.rare_mut().rubies.steps;
                            if level.start == at {
                                if let Some(last) = steps.last_mut() {
                                    *last = width;
                                }
                            } else {
                                steps.push(width);
                            }
                            out.rare_mut().rubies.close_floors();
                        }
                        self.record_level(state, level, at, index, width, out)?;
                        state.after_close = ItemId::new(index.get() + 1);
                    }
                }
                ItemKind::Open | ItemKind::Close if annotation => {
                    if let Some(level) = state.level.as_mut() {
                        let opens = item.kind == ItemKind::Open;
                        level.width += edge_room(content, None, item, opens);
                    }
                }
                ItemKind::Open | ItemKind::Close if state.in_base => {
                    // The builder puts no initial letter inside ruby.
                    let opens = item.kind == ItemKind::Open;
                    state.base += edge_room(content, None, item, opens);
                }
                ItemKind::RubyClose | ItemKind::RubyOpen if !annotation => {
                    // The container ends, and the column with it.
                    if state.in_base {
                        state.in_base = false;
                        state.base_end = at;
                        state.base_end_item = index;
                    }
                    state.close = Some(index);
                    state.ended = true;
                    break;
                }
                _ => {}
            }
            index = ItemId::new(index.get() + 1);
        }
        Some(index)
    }

    /// Records the level `level` of the column being measured.
    ///
    /// It ends at boundary `at` with its closing item `close`, and is
    /// `width` wide.
    fn record_level(
        &self,
        state: &mut Measuring<'a>,
        level: OpenLevel,
        at: ClusterId,
        close: ItemId,
        width: InlineLayoutUnit,
        out: &mut MeasuredText,
    ) -> Option<()> {
        let scan = self.scan;
        let (content, analysis) = (scan.content, scan.analysis);
        let (nodes, facts) = (&content.nodes, &content.facts);
        let annotation = content.items.get(level.item);
        // The side is the `ruby-position` of the container the annotation
        // opened in, as Chrome reads it off the annotation's parent ruby.
        // That includes a ruby written as a box inside another, whose box
        // closes before its annotation opens. The builder writes the
        // container's position into the annotation's style for that case.
        let node = annotation.map_or(self.container, |item| item.node);
        let ruby = facts.box_facts(nodes.box_facts(node, scan.variant)).ruby;
        let side = RubySide::from_position(ruby.position, state.count);
        let width = width.to_layout();
        let extent = self.em(level.item, close, true, out);
        let clusters = level.start..at;
        // What line layout spreads a column wider than the level over.
        let text = nodes.text_facts(node, scan.variant);
        let justify = facts.text(text).justify;
        let opportunities = JustifyOpportunities::new(content, analysis, clusters.clone(), justify)
            .map_or(0, |found| found.count_range(clusters.clone()));
        out.rare_mut().rubies.levels.push(RubyLevel {
            clusters,
            open: level.item,
            side,
            width,
            extent,
            opportunities,
            steps: level.steps,
            depth: match side {
                RubySide::Over => state.depths.0 + state.own_depths.0,
                RubySide::Under => state.depths.1 + state.own_depths.1,
            },
        })?;
        match side {
            RubySide::Over => state.own_depths.0 += 1,
            RubySide::Under => state.own_depths.1 += 1,
        };
        state.count += 1;
        let size = scan.input.size(text);
        if state.widest.is_none_or(|(widest, _)| width > widest) {
            state.widest = Some((width, size));
        }
        Some(())
    }

    /// Returns the em box of the text and atomic inlines among items `from`
    /// to `to`: a column's base, or one annotation, as `annotation` says.
    ///
    /// An annotation's em box includes its own emphasis marks, as Chrome
    /// adds them (`ComputeEmphasisHeights` in `AccumulateColumnOffsets` and
    /// `PlaceLines`). So the next level out stands past them.
    ///
    /// Each item is taken where the shifts of the boxes around it, inside
    /// the column, move it from the ruby container's baseline. Chrome's
    /// `ComputeEmHeight` also takes each item where it is placed, so an
    /// annotation over a raised base stands on the raised text. A running
    /// sum of the open boxes' shifts, one entry a box, keeps the walk linear
    /// however deep they nest. A box opened before `from` isn't the
    /// column's.
    fn em(&self, from: ItemId, to: ItemId, annotation: bool, text: &MeasuredText) -> Extent {
        let scan = self.scan;
        let (content, analysis, fonts) = (scan.content, scan.analysis, scan.fonts);
        let variant = scan.variant;
        let items = &content.items;
        let item_clusters = &analysis.item_clusters;
        let mut em = Extent::NONE;
        let mut marks = (LayoutUnit::ZERO, LayoutUnit::ZERO);
        // How far the boxes open around the text raise it: their shifts,
        // and how many of them open with none.
        let mut shift = LayoutUnit::ZERO;
        let mut level = 0u32;
        let shifting = !text.shifts().is_empty();
        // The font runs its text is drawn in, walked to its first text item
        // from where the last walk ended, and on from there, since the
        // items come in order.
        let (runs, text_end) = (fonts.runs(variant), analysis.clusters.end_id());
        let mut run = None;
        for item_id in (from..to).ids() {
            work::step();
            let Some(item) = items.get(item_id) else {
                break;
            };
            if item.flags.contains(ItemFlags::ANNOTATION) != annotation {
                continue;
            }
            let Range {
                start: first,
                end: next,
            } = item_clusters.range(item_id);
            match item.kind {
                ItemKind::Open if shifting => {
                    shift = shift + text.shift(item.node);
                    level += 1;
                }
                ItemKind::Close if shifting && level > 0 => {
                    shift = shift - text.shift(item.node);
                    level -= 1;
                }
                ItemKind::Text if first < next => {
                    let Some(cursor) =
                        run.or_else(|| runs.cursor_from(self.font_run.get(), first, text_end))
                    else {
                        continue;
                    };
                    let mut cursor = cursor;
                    runs.step_to(&mut cursor, first, text_end);
                    run = Some(cursor);
                    self.font_run.set(cursor.id());
                    let id = content.nodes.text_facts(item.node, variant);
                    let request = content.facts.text_request(id);
                    let (own, _) = ruby::em_box(fonts, request, first..next, variant, cursor.id());
                    em = em.unite(own.raised(shift));
                    let facts = content.facts.text(id);
                    if annotation && facts.has(TextFlags::EMPHASIS) {
                        let height = scan
                            .metrics
                            .get(id)
                            .map_or(LayoutUnit::ZERO, |metrics| metrics.mark.height());
                        match RubySide::from(facts.emphasis_side) {
                            RubySide::Over => marks.0 = marks.0.max(height),
                            RubySide::Under => marks.1 = marks.1.max(height),
                        }
                    }
                }
                ItemKind::Atomic => {
                    if let Some(atomic) = content.item_atomic(item_id) {
                        let own = Extent::from_atomic(atomic, scan.writing_mode, scan.baseline);
                        em = em.unite(own.raised(shift + text.shift(item.node)));
                    }
                }
                _ => {}
            }
        }
        if em.is_none() {
            return em;
        }
        Extent::new(em.ascent() + marks.0, em.descent() + marks.1)
    }

    /// Returns the text after the column ending at `end`, as its end
    /// overhang reads it.
    ///
    /// The column's last item is before `close`. The text is the item the
    /// scan's copy `look` holds there, where only tags stand between.
    fn after_column(
        &self,
        look: &ScanWalk<'a>,
        end: ClusterId,
        close: ItemId,
    ) -> Option<TextBeside> {
        let content = self.scan.content;
        let items = &content.items;
        let analysis = self.scan.analysis;
        if end >= analysis.clusters.end_id() {
            return None;
        }
        // Check the items from the column's end to the text. A tag or the
        // container's end is a position; anything else stands between.
        let (holding, _) = look.item()?;
        for id in (close..holding).ids() {
            work::step();
            let item = items.get(id)?;
            let kind = item.kind;
            let passes = !item.flags.contains(ItemFlags::ANNOTATION)
                && (kind == ItemKind::Open || kind.is_close());
            if !passes {
                return None;
            }
        }
        let item = items.get(holding)?;
        if item.kind != ItemKind::Text || item.flags.contains(ItemFlags::ANNOTATION) {
            return None;
        }
        let Range {
            start: first,
            end: last,
        } = analysis.item_clusters.range(holding);
        // Its whole width as shaped, from the advances ahead of the scan.
        let width = self
            .prefix
            .get(first.get()..last.get())
            .unwrap_or_default()
            .iter()
            .fold(InlineLayoutUnit::ZERO, |sum, &advance| sum + advance);
        Some(TextBeside {
            item: holding,
            width,
            cluster: first,
            end: InlineLayoutUnit::ZERO,
        })
    }

    /// Returns how far `column` overhangs the text before it and after it,
    /// by `Config::ruby_overhang`'s rule.
    ///
    /// `widest` is its widest annotation's width and font size. The overhang
    /// is zero where the text beside it can't be reached over.
    ///
    /// Also returns how far the overhang reaches into a tab beside the
    /// column, before it and after it, where it does.
    fn overhang(
        &self,
        column: Overhanging,
        widest: Option<(LayoutUnit, f32)>,
    ) -> ((LayoutUnit, LayoutUnit), [Option<TabReach>; 2]) {
        let none = ((LayoutUnit::ZERO, LayoutUnit::ZERO), [None, None]);
        let Some((_, size)) = widest else {
            return none;
        };
        if column.space <= LayoutUnit::ZERO || column.ruby.group.overhang == RubyOverhang::None {
            return none;
        }
        let (start, end, tabs) = match self.scan.input.ruby_overhang {
            RubyOverhangRule::AdjacentText => self.chrome_overhang(column, size),
            RubyOverhangRule::KanaOnly => self.jlreq_overhang(column, size),
        };
        // The overhang reaches into a tab past the blank before it.
        let reach = |tab: Option<(ClusterId, LayoutUnit)>, overhang: LayoutUnit, after: bool| {
            tab.filter(|&(_, blank)| overhang > blank)
                .map(|(tab, blank)| TabReach {
                    tab,
                    reach: overhang - blank,
                    after,
                })
        };
        let reaches = [reach(tabs.0, start, false), reach(tabs.1, end, true)];
        ((start, end), reaches)
    }

    /// Returns Chrome's overhang (Blink's `GetOverhang`,
    /// `CanApplyStartOverhang` and `CommitPendingEndOverhang`).
    ///
    /// - Under `auto`: the base's inset or half the annotation's font size,
    ///   whichever is less, at each side. Under `ruby-align: start`, at the
    ///   end only. Never further than half the text there.
    ///   It never reaches a tab.
    /// - Under `spaces`: as far as the blank beside the column reaches,
    ///   within the inset. A tab after the column is blank. The start stops
    ///   where the last column's end overhang reaches.
    /// - Under `space-between`: none.
    ///
    /// It reaches only over text no larger than the ruby's, and not marked
    /// for emphasis on the annotation's side.
    ///
    /// Also returns the tabs the blank reaches before and after the column,
    /// each with the blank nearer the column.
    fn chrome_overhang(
        &self,
        column: Overhanging,
        size: f32,
    ) -> (LayoutUnit, LayoutUnit, BesideTabs) {
        let Overhanging {
            space,
            opportunities,
            ruby,
            before,
            after,
        } = column;
        let align = ruby.group.align;
        if align == RubyAlign::SpaceBetween {
            return (LayoutUnit::ZERO, LayoutUnit::ZERO, (None, None));
        }
        // Half the annotation's font size, halved as a whole number of
        // pixels, as Blink takes it: `FontSize() / 2`. `FontSize()` is the
        // computed size rounded to a pixel (`ComputedPixelSize`). So a 17px
        // annotation overhangs by 8, not 8.5 (measured with Chrome 153).
        let half = LayoutUnit::from_px_whole(size).half().floor_px();
        let inset = ruby::base_inset(space, opportunities);
        let spaces = ruby.group.overhang == RubyOverhang::Spaces;
        let (mut start, mut end) = match (spaces, align) {
            (false, RubyAlign::Start) => (LayoutUnit::ZERO, space.min(half)),
            (false, _) => (inset.min(half), inset.min(half)),
            (true, RubyAlign::Start) => (LayoutUnit::ZERO, space.min(inset + inset)),
            (true, _) => (inset, inset),
        };
        let scan = self.scan;
        let side = RubySide::from_position(ruby.group.position, 0);
        // The text beside may be overhung if it is no larger than the ruby
        // and not marked for emphasis on the annotation's side.
        let may = |beside: &TextBeside| {
            let text = self.item_text(beside.item);
            let facts = scan.content.facts.text(text);
            let marked =
                facts.has(TextFlags::EMPHASIS) && RubySide::from(facts.emphasis_side) == side;
            scan.input.size(text) <= ruby.size && !marked
        };
        // Under `spaces` the blank is the bound; otherwise half the text,
        // where no tab stands beside the column.
        let reach = |beside: &TextBeside, forward: bool| {
            if spaces {
                self.blank(beside, forward)
            } else if scan.analysis.clusters.is_tab(beside.cluster) {
                Blank::NONE
            } else {
                Blank {
                    width: beside.width.to_layout().half(),
                    tab: None,
                }
            }
        };
        let mut tabs = (None, None);
        start = match before {
            Some(beside) if may(&beside) => {
                let blank = reach(&beside, false);
                tabs.0 = blank.tab;
                start.min(blank.width)
            }
            _ => LayoutUnit::ZERO,
        };
        if spaces && let Some(limit) = self.start_limit {
            start = start.min(limit);
        }
        end = match after {
            Some(beside) if may(&beside) => {
                let blank = reach(&beside, true);
                tabs.1 = blank.tab;
                end.min(blank.width)
            }
            _ => LayoutUnit::ZERO,
        };
        (start, end, tabs)
    }

    /// Returns how much blank the text `beside` a column offers it, as
    /// Chrome's `spaces` reads it.
    ///
    /// It counts from the cluster next to the column, forward into the text
    /// after it (`forward`) or back into the text before. The blank is the
    /// space separators there, then the blank half of a full-width mark
    /// facing the column, or a quarter of a middle dot's. `prefix` holds
    /// sums before the scan's boundary and advances after.
    ///
    /// A tab ends the blank. The prefix gives it no width, and its width
    /// depends on where its line starts. After the column it counts as wide
    /// as a tab under its stops can be. Before it, it counts as wide as on
    /// its paragraph's line, which the scan has passed.
    fn blank(&self, beside: &TextBeside, forward: bool) -> Blank {
        let prefix = self.prefix;
        let cluster = beside.cluster;
        let (content, analysis) = (self.scan.content, self.scan.analysis);
        let clusters = &analysis.clusters;
        let count = clusters.len();
        // A cluster's advance. Ahead of the scan, `prefix` holds the
        // advance. Behind it, the advance is the difference of two sums,
        // the last of them the scan's own.
        let advance = |at: ClusterId| -> InlineLayoutUnit {
            if forward {
                prefix.get(at.get()).copied().unwrap_or_default()
            } else {
                let from = prefix.get(at.get()).copied().unwrap_or_default();
                let to = if at == cluster {
                    beside.end
                } else {
                    prefix.get(at.get() + 1).copied().unwrap_or(from)
                };
                to - from
            }
        };
        let mut blank = InlineLayoutUnit::ZERO;
        let mut tab = None;
        let mut at = cluster;
        let mark = loop {
            work::step();
            if at.get() >= count {
                break None;
            }
            let class = clusters.attrs(at).map(ClusterAttrs::class);
            if class == Some(ClusterClass::Tab) {
                let width = if forward {
                    let text = self.text(beside.item, at, forward);
                    let stops = self.scan.tab_stops(text);
                    stops.map_or(LayoutUnit::ZERO, |stops| stops.widest())
                } else {
                    self.tab_before
                        .filter(|&(tab, _)| tab == at)
                        .map_or(LayoutUnit::ZERO, |(_, width)| width)
                };
                tab = Some((at, blank.to_layout()));
                blank += InlineLayoutUnit::from_layout(width);
                break None;
            }
            if !matches!(
                class,
                Some(ClusterClass::Space | ClusterClass::OtherSpace | ClusterClass::NoBreakSpace)
            ) {
                break Some(at);
            }
            blank += advance(at);
            at = if forward {
                ClusterId::new(at.get() + 1)
            } else if at.get() == 0 {
                break None;
            } else {
                ClusterId::new(at.get() - 1)
            };
        };
        if let Some(mark) = mark
            && let Some(ch) = clusters.first_char(content.text(self.scan.variant), mark)
        {
            let size = self.size(beside.item, mark, forward);
            let facing = match unicode::rare_props(ch).text_spacing() {
                TextSpacingClass::Open if forward => size.half(),
                TextSpacingClass::Close if !forward => size.half(),
                TextSpacingClass::Middle => size.half().half(),
                _ => LayoutUnit::ZERO,
            };
            // A mark set narrower than full width has no blank half left.
            let full = advance(mark).to_layout() + LayoutUnit::EPSILON >= size - size.divided(10);
            if full {
                blank += InlineLayoutUnit::from_layout(facing);
            }
        }
        Blank {
            width: blank.to_layout(),
            tab,
        }
    }

    /// Returns the computed font size of the text holding `cluster`,
    /// truncated onto the grid as Chrome's `LayoutUnit(font_size)` is.
    ///
    /// The holding item is found from `item`, the text beside a column, by
    /// stepping over the items after it (`forward`) or before it, as the
    /// blank was walked. Where no item holds it, the block's size is used.
    fn size(&self, item: ItemId, cluster: ClusterId, forward: bool) -> LayoutUnit {
        let text = self.text(item, cluster, forward);
        LayoutUnit::from_px_truncated(self.scan.input.size(text))
    }

    /// Returns the text facts of the item holding `cluster`, found from
    /// `item` as [`size`](Self::size) finds it.
    fn text(&self, item: ItemId, cluster: ClusterId, forward: bool) -> TextFactsId {
        let scan = self.scan;
        let item_clusters = &scan.analysis.item_clusters;
        let mut id = item;
        loop {
            work::step();
            let clusters = item_clusters.range(id);
            if clusters.contains(&cluster) {
                return self.item_text(id);
            }
            let next = if forward {
                Some(id.get() + 1).filter(|&next| next < scan.content.items.len())
            } else {
                id.get().checked_sub(1)
            };
            match next {
                Some(next) => id = ItemId::new(next),
                None => return scan.content.nodes.text_facts(NodeId::BLOCK, scan.variant),
            }
        }
    }

    /// Returns the text facts of item `id`'s node in the variant, or the
    /// block's past the last item.
    fn item_text(&self, id: ItemId) -> TextFactsId {
        let content = self.scan.content;
        let node = content
            .items
            .get(id)
            .map_or(NodeId::BLOCK, |item| item.node);
        content.nodes.text_facts(node, self.scan.variant)
    }

    /// Returns the overhang of the Japanese Layout Requirements (JLREQ
    /// 3.3.7).
    ///
    /// - Under `auto`: over kana only, by at most one annotation em and half
    ///   the difference.
    /// - Under `spaces`: into blank only, by at most half the difference.
    ///   Blank is a space's whole advance, the blank half of a full-width
    ///   mark facing the column, or a quarter of a middle dot's.
    ///
    /// It never reaches over another column, whose annotation is already
    /// there.
    fn jlreq_overhang(
        &self,
        column: Overhanging,
        size: f32,
    ) -> (LayoutUnit, LayoutUnit, BesideTabs) {
        let Overhanging {
            space,
            ruby,
            before,
            after,
            ..
        } = column;
        let ruby = ruby.group;
        let content = self.scan.content;
        let clusters = &self.scan.analysis.clusters;
        let half = space.half();
        let em = LayoutUnit::from_px(size);
        let spaces = ruby.overhang == RubyOverhang::Spaces;
        let mut tabs = (None, None);
        let mut reach = |beside: Option<TextBeside>, forward: bool| {
            let Some(beside) = beside else {
                return LayoutUnit::ZERO;
            };
            if spaces {
                let blank = self.blank(&beside, forward);
                if forward {
                    tabs.1 = blank.tab;
                } else {
                    tabs.0 = blank.tab;
                }
                return blank.width.min(half);
            }
            let source = content.text(self.scan.variant);
            let kana = clusters
                .first_char(source, beside.cluster)
                .is_some_and(|ch| {
                    matches!(
                        u32::from(ch),
                        0x3041..=0x309F | 0x30A0..=0x30FA | 0x30FC..=0x30FF | 0x31F0..=0x31FF
                    )
                });
            if kana { em.min(half) } else { LayoutUnit::ZERO }
        };
        let start = reach(before, false);
        let end = reach(after, true);
        (start, end, tabs)
    }
}

/// The tabs ending the blank before and after a ruby column, each with the
/// blank between it and the column.
type BesideTabs = (
    Option<(ClusterId, LayoutUnit)>,
    Option<(ClusterId, LayoutUnit)>,
);

/// The blank beside a ruby column, as `ruby-overhang: spaces` reads it.
#[derive(Copy, Clone, Debug)]
struct Blank {
    width: LayoutUnit,
    /// The tab ending it, and the blank between the tab and the column.
    tab: Option<(ClusterId, LayoutUnit)>,
}

impl Blank {
    /// No blank.
    const NONE: Self = Self {
        width: LayoutUnit::ZERO,
        tab: None,
    };
}

/// A column as its overhang reads it: how much wider than its base it is,
/// its container, and the text either side.
#[derive(Copy, Clone, Debug)]
struct Overhanging {
    /// How much wider the column is than its base.
    space: LayoutUnit,
    /// How many justification opportunities its base has.
    opportunities: u32,
    ruby: Ruby,
    /// The text before the column and after it, where it can be reached.
    before: Option<TextBeside>,
    after: Option<TextBeside>,
}

/// A ruby container as a column's overhang reads it: its ruby properties,
/// and its text's computed size.
#[derive(Copy, Clone, Debug)]
struct Ruby {
    group: RubyGroup,
    size: f32,
}

/// The state sizing a ruby column keeps as it looks ahead over it.
#[derive(Copy, Clone)]
struct Measuring<'a> {
    /// The copy of the scan's walk, and of its seams, that the look-ahead
    /// moves.
    look: ScanWalk<'a>,
    seams: Seams,
    /// The base's width so far, as the prefix will hold it.
    base: InlineLayoutUnit,
    /// The base hasn't ended yet.
    in_base: bool,
    /// Where the base ends, and the first item past it: the first
    /// annotation's opening item, or the container's end.
    base_end: ClusterId,
    base_end_item: ItemId,
    /// The annotation open, if any.
    level: Option<OpenLevel>,
    /// The column's first level in the stage's table.
    levels: RubyLevelId,
    /// How many levels it has so far.
    count: usize,
    depths: (u32, u32),
    own_depths: (u32, u32),
    /// Its widest level's width and font size.
    widest: Option<(LayoutUnit, f32)>,
    /// One past its last item.
    close: Option<ItemId>,
    /// One past its last annotation's closing item so far. The next column
    /// starts there, where base text follows.
    after_close: ItemId,
    /// It has ended.
    ended: bool,
}

/// Takes back the running widths a column that may not split recorded for
/// its levels: the steps from `steps_start` on, and each level's mark.
fn forget_steps(record: &RubyColumn, steps_start: usize, out: &mut MeasuredText) {
    out.rare_mut().rubies.steps.truncate(steps_start);
    out.rare_mut().rubies.floors.truncate(steps_start);
    for level in record.levels.clone().ids() {
        if let Some(level) = out.rare_mut().rubies.levels.get_mut(level) {
            level.steps = None;
        }
    }
}

/// Keeps the tabs a column's overhang reaches into.
///
/// Columns are sized in text order, so the tabs they reach come in order. A
/// tab between two columns keeps the first one's reach.
fn keep_tab_reaches(reaches: [Option<TabReach>; 2], out: &mut MeasuredText) {
    for reach in reaches.into_iter().flatten() {
        let reaches = &mut out.rare_mut().rubies.tab_reaches;
        if reaches.last().is_none_or(|last| last.tab < reach.tab) {
            reaches.push(reach);
        }
    }
}

/// Returns whether a ruby column whose items end before `close` ends with
/// its container.
///
/// It does where `close` is the container's closing item, or a nested
/// container's opening one, and no column of the container follows.
fn is_container_end(content: &Content, close: ItemId) -> bool {
    content
        .items
        .get(close)
        .is_none_or(|item| matches!(item.kind, ItemKind::RubyClose | ItemKind::RubyOpen))
}

/// The text beside a ruby column, as its overhang reads it.
#[derive(Copy, Clone, Debug)]
pub(super) struct TextBeside {
    pub(super) item: ItemId,
    pub(super) width: InlineLayoutUnit,
    /// The cluster beside the column: the text's last before it, its first
    /// after it.
    pub(super) cluster: ClusterId,
    /// Before the column, the running sum where the text ends, which the
    /// scan's prefix doesn't hold yet. Zero after the column.
    pub(super) end: InlineLayoutUnit,
}
