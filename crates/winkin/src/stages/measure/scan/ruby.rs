//! Ruby columns, each sized where the scan meets it, before its first
//! cluster is measured.
//!
//! A look-ahead runs over the column with a copy of the scan's walk. It
//! measures the base as the scan is about to, each annotation as a line of
//! its own, and how far the column overhangs the text beside it.

use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

use super::overhang::{Overhanging, Ruby, TextBeside};
use super::{BoundaryRoom, Scan, ScanPrefix, ScanWalk};
use crate::config::RubyOverhangRule;
use crate::data::{Id, IdRange, sort_by_key};
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::{
    Content, Item, ItemFlags, ItemId, ItemKind, NodeId, TextFactsId, TextFlags,
};
use crate::stages::fonts::FontRunId;
use crate::stages::measure::autospace::Seams;
use crate::stages::measure::edges::edge_room;
use crate::stages::measure::ruby::RubySpread;
use crate::stages::measure::ruby_columns::{ColumnRoom, ColumnRoomId, StepId, is_monolithic};
use crate::stages::measure::{
    Extent, JustifyOpportunities, MeasuredText, RubyColumn, RubyColumnId, RubyLevel, RubyLevelId,
    RubySide, TabReach, ruby,
};
use crate::style::TextWrapStyle;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// A level of a ruby column being measured: where it opened, and what it
/// has taken so far.
#[derive(Copy, Clone, Debug)]
struct OpenLevel {
    item: ItemId,
    start: ClusterId,
    width: InlineLayoutUnit,
    steps: Option<StepId>,
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
        prefix: ScanPrefix<'_>,
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
        prefix: ScanPrefix<'_>,
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
        let items = &scan.input.content.items;
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
        let opening = room.ruby_open.filter(|&opens| {
            ended.as_ref().is_none_or(|open| {
                out.ruby_columns()
                    .get(open.id)
                    .is_none_or(|column| column.close < opens)
            })
        });
        match opening {
            Some(opens) if self.column.is_none() => {
                self.container =
                    (scan.input.content.items.get(opens)).map_or(self.container, |item| item.node);
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
pub(super) struct Sizing<'s, 'a> {
    pub(super) scan: &'s Scan<'a>,
    /// The running sums before the scan's boundary, and the shaped advances
    /// from it on.
    pub(super) prefix: ScanPrefix<'s>,
    /// The ruby container it is in.
    container: NodeId,
    parent: Option<RubyColumnId>,
    /// How far its start overhang may reach before meeting the last
    /// column's annotation, where that column is near enough to meet.
    pub(super) start_limit: Option<LayoutUnit>,
    /// The last tab the scan passed, and how far it reaches on its
    /// paragraph's line.
    pub(super) tab_before: Option<(ClusterId, LayoutUnit)>,
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
        let content = scan.input.content;
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
        let steps_start = out.ruby_columns().steps.next_id();
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
            annotation_follows: false,
            ended: false,
        };
        let (end, index) = self.measure_parts(&mut state, id, at, first, out)?;
        let close = state.close.unwrap_or(index);
        // Base text after the last annotation is the line's, not a column.
        let next =
            (state.count > 0 && !is_container_end(content, close) && state.annotation_follows)
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
            scan.input.analysis,
            at..state.base_end,
            justify,
            out.ruby_columns(),
            (id, id),
            None,
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
            ruby::strut_em(scan.input.fonts, facts.text_request(text))
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
        let count = scan.input.analysis.clusters.len();
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
            if let Some(item) = scan.input.content.items.get(index)
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
                let _ = out.rare_mut().rubies.steps.push(level.width);
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
        let content = self.scan.input.content;
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
        let attrs = scan.input.analysis.clusters.attrs(c);
        let class = attrs.map_or(ClusterClass::Text, ClusterAttrs::class);
        let advance = self.prefix.get(c).unwrap_or_default();
        let gap = if state.look.has_seams {
            InlineLayoutUnit::from_text(state.look.seam_after(scan, &mut state.seams, c))
        } else {
            InlineLayoutUnit::ZERO
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
        let content = self.scan.input.content;
        let clusters = &self.scan.input.analysis.clusters;
        let rubies = out.ruby_columns();
        let inside = || record.base.start..ClusterId::new(record.base.end.get() - 1);
        self.parent.is_none()
            && content.facts.text(text).has(TextFlags::WRAPS)
            && matches!(
                content.block.text_wrap_style,
                TextWrapStyle::Auto | TextWrapStyle::Stable
            )
            && record.base_width > LayoutUnit::ZERO
            && !is_monolithic(
                content,
                self.scan.input.analysis,
                self.scan.shaped,
                (record.open, record.base.clone()),
                (rubies.levels(record).iter()).map(|level| (level.open, level.clusters.clone())),
            )
            && record.base.end.get() > record.base.start.get() + 1
            && (rubies
                .opportunity(clusters, inside(), Some(id), (false, false), &Cell::new(id))
                .is_some()
                || rubies
                    .opportunity(clusters, inside(), Some(id), (false, true), &Cell::new(id))
                    .is_some())
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
        let content = self.scan.input.content;
        let items = &content.items;
        let item_clusters = &self.scan.input.analysis.item_clusters;
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
                            out.rare_mut().rubies.steps.push(InlineLayoutUnit::ZERO)
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
                                let _ = steps.push(width);
                            }
                            out.rare_mut().rubies.close_floors();
                        }
                        self.record_level(state, level, at, index, width, out)?;
                        state.after_close = ItemId::new(index.get() + 1);
                        state.annotation_follows =
                            item.flags.contains(ItemFlags::ANNOTATION_FOLLOWS);
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
        let (content, analysis) = (scan.input.content, scan.input.analysis);
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
        let (content, analysis, fonts) =
            (scan.input.content, scan.input.analysis, scan.input.fonts);
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
        let content = self.scan.input.content;
        let items = &content.items;
        let analysis = self.scan.input.analysis;
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
        let width = self.prefix.total(first..last);
        Some(TextBeside {
            item: holding,
            width,
            cluster: first,
            end: InlineLayoutUnit::ZERO,
        })
    }
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
    /// Another annotation of the container follows its last annotation so
    /// far.
    annotation_follows: bool,
    /// It has ended.
    ended: bool,
}

/// Takes back the running widths a column that may not split recorded for
/// its levels: the steps from `steps_start` on, and each level's mark.
fn forget_steps(record: &RubyColumn, steps_start: StepId, out: &mut MeasuredText) {
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
