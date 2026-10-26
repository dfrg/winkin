//! Ruby columns, each sized where the scan meets it by a look-ahead over a
//! copy of the scan's walk: the base as the scan will measure it, each
//! annotation as a line of its own, and the overhang.

use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

use super::overhang::{Overhanging, Ruby, TextBeside};
use super::{BoundaryRoom, Scan, ScanPrefix, ScanWalk};
use crate::config::RubyOverhangRule;
use crate::data::{Id, IdRange};
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::{
    Content, Item, ItemFlags, ItemId, ItemKind, NodeId, TextFactsId, TextFlags,
};
use crate::stages::fonts::FontRunId;
use crate::stages::measure::autospace::Seams;
use crate::stages::measure::edges::edge_room;
use crate::stages::measure::ruby::RubySpread;
use crate::stages::measure::ruby_columns::{
    ColumnRoom, ColumnRoomId, StepId, TrimmedEm, is_monolithic,
};
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
    /// Its width less its base's and its overhang either side.
    extra: InlineLayoutUnit,
    /// How far it overhangs the text before and after it, which a line it
    /// starts or ends pays.
    start_overhang: LayoutUnit,
    end_overhang: LayoutUnit,
    /// The first item of its container's next column, where one follows.
    next: Option<ItemId>,
}

/// What the scan carries about ruby from one boundary to the next.
#[derive(Copy, Clone, Debug)]
pub(super) struct RubyBoundaries {
    /// The ruby column the scan is in, sized where it started.
    pub(super) column: Option<OpenColumn>,
    /// The last column that ended in pieces, for the min-content sizes: its
    /// width less its overhang, and its first, last and widest pieces.
    pub(super) ended: Option<(LayoutUnit, LayoutUnit, LayoutUnit, LayoutUnit)>,
    /// The ruby container the scan is in.
    container: NodeId,
    /// The next room a column leaves for the columns nested in its base.
    child_room: ColumnRoomId,
    /// The last column that ended, as the next one's start overhang reads it.
    last: Option<EndedColumn>,
    /// The tabs of the blank run the scan last passed a tab in.
    tab_run: Option<TabRun>,
    /// The font run the last em box walk ended in, where the next starts.
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
            tab_run: None,
            font_run: FontRunId::new(0),
        }
    }

    /// Notes the tab at `tab`, `width` wide, for a column after it. It joins
    /// the last run where only spaces stand between.
    pub(super) fn pass_tab(&mut self, scan: &Scan<'_>, tab: ClusterId, width: InlineLayoutUnit) {
        let clusters = &scan.input.analysis.clusters;
        let blank = |at: ClusterId| {
            work::step();
            matches!(
                clusters.class(at),
                Some(ClusterClass::Space | ClusterClass::OtherSpace | ClusterClass::NoBreakSpace)
            )
        };
        let width = width.to_layout();
        self.tab_run = match self.tab_run {
            Some(run) if (ClusterId::new(run.last.get() + 1)..tab).ids().all(blank) => {
                Some(TabRun {
                    last: tab,
                    width: run.width + width,
                })
            }
            _ => Some(TabRun { last: tab, width }),
        };
    }

    /// Closes the ruby column ending at boundary `at`, sizes one starting
    /// there, and adds their room and nested columns' to `room`.
    ///
    /// `walk` stands at `at` with its items crossed. `sum` is the running sum
    /// before them; `line` turns a running sum into a place on the
    /// paragraph's line.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn cross<'a>(
        &mut self,
        scan: &Scan<'a>,
        walk: &ScanWalk<'a>,
        seams: Seams,
        prefix: ScanPrefix<'_>,
        ruby_starts: &mut Vec<ClusterId>,
        at: ClusterId,
        (sum, line): (InlineLayoutUnit, InlineLayoutUnit),
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
                tabs_before: self.tab_run,
                line_start: start + line,
                font_run: &font_run,
            };
            let mut look = *walk;
            self.column = sizing.column(&mut look, seams, at, first, before, out);
            self.font_run = font_run.get();
            if let Some(open) = &self.column {
                room.overhang.start = InlineLayoutUnit::from_layout(open.start_overhang);
            }
            // A column with no clusters: its room starts the line after.
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

    /// Closes the column ending at `at`, if any, and returns it. Its room
    /// joins `room` as a closing edge does.
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

    /// Returns how far a column starting at item `first` and running sum
    /// `start` may overhang the text before it without meeting the last
    /// column's, as Blink's `GetOverhang` bounds it. `None` where more than
    /// text and tags stand between.
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

    /// Returns the first item of a column starting here, or `None`.
    fn next_first(
        &mut self,
        scan: &Scan<'_>,
        ended: Option<OpenColumn>,
        room: &BoundaryRoom,
        out: &MeasuredText,
    ) -> Option<ItemId> {
        // A container opening before the ended column's close is nested in
        // it, and that column's look-ahead sized it already.
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

    /// Adds to `room` what nested or empty columns leave at `at`.
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

/// A ruby column to size where the scan stands.
pub(super) struct Sizing<'s, 'a> {
    pub(super) scan: &'s Scan<'a>,
    /// The running sums before the scan's boundary, and the shaped advances
    /// from it on.
    pub(super) prefix: ScanPrefix<'s>,
    /// The ruby container it is in.
    container: NodeId,
    /// The column whose base holds it, or `None` at the top level.
    parent: Option<RubyColumnId>,
    /// How far its start overhang may reach, where the last column bounds it.
    pub(super) start_limit: Option<LayoutUnit>,
    /// The tabs of the blank run the scan last passed a tab in.
    pub(super) tabs_before: Option<TabRun>,
    /// Where the column starts along its paragraph's line.
    pub(super) line_start: InlineLayoutUnit,
    /// The font run the last em box walk ended in, where the next starts.
    font_run: &'s Cell<FontRunId>,
}

impl<'a> Sizing<'_, 'a> {
    /// Sizes the ruby column starting at boundary `at`, item `first`, into
    /// `out`, and moves `walk` past it. `before` is the text before it.
    ///
    /// The base is measured as the prefix will hold it. Each annotation is
    /// measured at its max-content, as Blink's `HandleRuby` does. The column
    /// ends with its container, or where base text follows its last
    /// annotation. Returns `None` where a table is full.
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
        // The row goes in before its descendants', in preorder.
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
            base_end: None,
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
        let (base_end, base_end_item) = state.base_end.unwrap_or((at, first));
        let close = state.close.unwrap_or(index);
        // Base text after the last annotation is the line's, not a column.
        let next =
            (state.count > 0 && !is_container_end(content, close) && state.annotation_follows)
                .then_some(close);
        // The column's width from its parts.
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
        let (nodes, facts) = (&content.nodes, &content.facts);
        let ruby = facts
            .box_facts(nodes.box_facts(self.container, scan.variant))
            .ruby;
        let text = nodes.text_facts(self.container, scan.variant);
        let justify = facts.text(text).justify;
        let opportunities = JustifyOpportunities::from_base(
            content,
            scan.input.analysis,
            at..base_end,
            justify,
            out.ruby_columns(),
            (id, id),
            None,
        )
        .map_or(0, |found| found.count_range(at..base_end));
        // The text after the column, which its end overhang may reach. A
        // column followed by its container's next column has none: no ruby
        // column is overhung (Blink's `CommitPendingEndOverhang`).
        let after = match next {
            Some(_) => None,
            None => self.after_column(&state.look, end, close),
        };
        // An empty base takes the container's font's em box.
        let base_em = self.em(first, base_end_item, false, at..base_end, out);
        let empty = base_em.is_none();
        let strut = || ruby::strut_em(scan.input.fonts, facts.text_request(text));
        let base_em = if empty { strut() } else { base_em };
        // The em box without trailing collapsible spaces, for a line the
        // column ends.
        let kept = self.kept_clusters(at..base_end, (first, base_end_item), false);
        if kept.end < base_end {
            let trimmed = self.em(first, base_end_item, false, kept, out);
            let trimmed = if trimmed.is_none() { strut() } else { trimmed };
            if trimmed != base_em {
                let end = scan.input.analysis.item_clusters.start(close);
                keep_trimmed_em(
                    TrimmedEm {
                        column: id,
                        end,
                        em: trimmed,
                    },
                    out,
                );
            }
        }
        // A nested column, or one with an empty base, overhangs nothing.
        let overhang = if self.parent.is_some() || empty {
            (LayoutUnit::ZERO, LayoutUnit::ZERO)
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
                line_start: self.line_start,
                width,
            };
            self.overhang(column, state.widest, out)
        };
        let spread = match scan.input.ruby_overhang {
            // The base is inset as if nothing overhung.
            RubyOverhangRule::AdjacentText => {
                RubySpread::from_base(ruby.align, space, opportunities)
            }
            // JLREQ gives the base only what the overhang leaves.
            RubyOverhangRule::KanaOnly => {
                let left = space - overhang.0 - overhang.1;
                let spread = RubySpread::from_base(ruby.align, left, opportunities);
                RubySpread {
                    start: spread.start + overhang.0,
                    ..spread
                }
            }
        };
        let mut record = RubyColumn {
            open: first,
            base_end: base_end_item,
            close,
            base: at..base_end,
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

    /// Measures column `id`'s base, nested columns and annotations, from
    /// boundary `at` and item `first`. Returns its end boundary and the
    /// first item past it, or `None` where a table is full.
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
        let mut index = self.ruby_marks(state, at, first, out)?;
        let mut c = at;
        loop {
            work::step();
            if state.ended {
                return Some((c, index));
            }
            // A nested container is one unit of this base.
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
            // Base text after the annotations starts the next column.
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

    /// Measures the ruby container `item`, at item `index` and boundary
    /// `at`, as one unit of column `parent`'s base. Returns where the base
    /// goes on, or `None` where a table is full.
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
            tabs_before: None,
            line_start: InlineLayoutUnit::ZERO,
            font_run: self.font_run,
        };
        state.base += edge_room(content, None, item, true);
        let (mut c, mut index) = (at, index);
        let mut child_first = ItemId::new(index.get() + 1);
        loop {
            let rubies = out.ruby_columns();
            let (child_id, levels) = (rubies.columns.next_id(), rubies.levels.next_id());
            let open = child.column(&mut state.look, state.seams, c, child_first, None, out)?;
            let rubies = out.ruby_columns();
            let record = rubies.get(child_id)?;
            state.base += InlineLayoutUnit::from_layout(record.width);
            let written = rubies.levels.get_slice(levels..rubies.levels.next_id());
            for level in written.unwrap_or_default() {
                let depth = match level.side {
                    RubySide::Over => &mut state.depths.0,
                    RubySide::Under => &mut state.depths.1,
                };
                *depth = (*depth).max(level.depth.saturating_add(1));
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

    /// Measures cluster `c` as the scan will, into the open annotation or
    /// the base.
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
        } else if state.base_end.is_none() {
            state.base += state.look.step(scan, class, advance, c) + gap;
        }
    }

    /// Returns whether column `record`, whose container's text facts are
    /// `text`, may split across lines.
    ///
    /// Only a top-level column that wraps under `text-wrap-style: auto` or
    /// `stable` splits, at an opportunity inside a base of two or more
    /// clusters, where it is not [`is_monolithic`].
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

    /// Crosses the ruby marks and box edges among the items at boundary
    /// `at`, from `index`. Returns the first item past them, or `None` where
    /// the levels table is full.
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
                // An annotation over an empty base of its own starts the next
                // column.
                ItemKind::AnnotationOpen
                    if state.level.is_none()
                        && state.count > 0
                        && item.flags.contains(ItemFlags::EMPTY_BASE) =>
                {
                    state.close = Some(state.after_close);
                    state.ended = true;
                    break;
                }
                ItemKind::AnnotationOpen if state.level.is_none() => {
                    if state.count == 0 {
                        state.levels = out.rare_mut().rubies.levels.next_id();
                    }
                    state.base_end.get_or_insert((at, index));
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
                // The builder puts no initial letter inside ruby.
                ItemKind::Open | ItemKind::Close => {
                    let room = edge_room(content, None, item, item.kind == ItemKind::Open);
                    if annotation {
                        if let Some(level) = state.level.as_mut() {
                            level.width += room;
                        }
                    } else if state.base_end.is_none() {
                        state.base += room;
                    }
                }
                ItemKind::RubyClose | ItemKind::RubyOpen if !annotation => {
                    // The container ends, and the column with it.
                    state.base_end.get_or_insert((at, index));
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

    /// Records `level`, ending at boundary `at` and item `close`, `width`
    /// wide.
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
        // The builder writes the container's `ruby-position` into the
        // annotation's style, as it is read off the parent ruby.
        let node = annotation.map_or(self.container, |item| item.node);
        let boxes = facts.box_facts(nodes.box_facts(node, scan.variant));
        let side = RubySide::from_position(boxes.ruby.position, state.count);
        let width = width.to_layout();
        // As a line of its own, it keeps no collapsible space at its ends.
        let clusters = level.start..at;
        let kept = self.kept_clusters(clusters.clone(), (level.item, close), true);
        let extent = self.em(level.item, close, true, kept, out);
        // The levels stack by the margin box, the annotation's box.
        let extent = if extent.is_none() {
            extent
        } else {
            let (margin, across) = (boxes.margin_across, boxes.across);
            Extent::new(
                extent.ascent() + margin.0 + across.0,
                extent.descent() + margin.1 + across.1,
            )
        };
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

    /// Returns the em box of a base or an annotation: the text and atomic
    /// inlines among items `from` to `to`, within clusters `within`, each
    /// raised by the boxes around it as Blink's `ComputeEmHeight` places
    /// it. [`Extent::NONE`] where there are none.
    ///
    /// An annotation's box includes its emphasis marks, as Blink's
    /// `ComputeEmphasisHeights` adds them.
    fn em(
        &self,
        from: ItemId,
        to: ItemId,
        annotation: bool,
        within: Range<ClusterId>,
        text: &MeasuredText,
    ) -> Extent {
        let scan = self.scan;
        let (content, analysis, fonts) =
            (scan.input.content, scan.input.analysis, scan.input.fonts);
        let variant = scan.variant;
        let items = &content.items;
        let item_clusters = &analysis.item_clusters;
        let mut em = Extent::NONE;
        let mut marks = (LayoutUnit::ZERO, LayoutUnit::ZERO);
        // The count skips the close of a box opened before `from`.
        let mut shift = LayoutUnit::ZERO;
        let mut level = 0u32;
        let shifting = !text.shifts().is_empty();
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
            let (first, next) = (first.max(within.start), next.min(within.end));
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

    /// Returns `clusters`, held by items `first` to `to`, less the
    /// collapsible spaces at their end, and for an annotation at their start.
    #[inline(never)]
    fn kept_clusters(
        &self,
        clusters: Range<ClusterId>,
        (first, to): (ItemId, ItemId),
        annotation: bool,
    ) -> Range<ClusterId> {
        let input = self.scan.input;
        let (analysis, content) = (input.analysis, input.content);
        let is_space = |at: ClusterId| analysis.clusters.class(at) == Some(ClusterClass::Space);
        let collapses = |at: ClusterId| {
            (first..to).ids().any(|id| {
                work::step();
                let Some(item) = content.items.get(id) else {
                    return false;
                };
                let holds = item.kind == ItemKind::Text
                    && item.flags.contains(ItemFlags::ANNOTATION) == annotation
                    && analysis.item_clusters.range(id).contains(&at);
                let facts = content.nodes.text_facts(item.node, self.scan.variant);
                holds && content.facts.text(facts).collapse.collapses_spaces()
            })
        };
        let mut end = clusters.end;
        while end > clusters.start && is_space(ClusterId::new(end.get() - 1)) {
            work::step();
            end = ClusterId::new(end.get() - 1);
        }
        if end < clusters.end && !collapses(ClusterId::new(clusters.end.get() - 1)) {
            end = clusters.end;
        }
        let mut start = clusters.start;
        if annotation {
            while start < end && is_space(start) {
                work::step();
                start = ClusterId::new(start.get() + 1);
            }
            if start > clusters.start && !collapses(clusters.start) {
                start = clusters.start;
            }
        }
        start..end
    }

    /// Returns the text item after the column ending at `end` and item
    /// `close`, where only tags stand between.
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
    /// The look-ahead's copies of the scan's walk and seams.
    look: ScanWalk<'a>,
    seams: Seams,
    /// The base's width so far, as the prefix will hold it.
    base: InlineLayoutUnit,
    /// Where the base ends, and the first item past it. `None` while the
    /// base goes on.
    base_end: Option<(ClusterId, ItemId)>,
    /// The annotation open, if any.
    level: Option<OpenLevel>,
    /// The column's first level in the levels table.
    levels: RubyLevelId,
    /// How many levels it has so far.
    count: usize,
    /// Over and under, one past the deepest band the levels of its nested
    /// columns reach.
    depths: (u32, u32),
    /// Over and under, how many of its own levels it has so far.
    own_depths: (u32, u32),
    /// Its widest level's width and font size.
    widest: Option<(LayoutUnit, f32)>,
    /// One past its last item.
    close: Option<ItemId>,
    /// One past its last annotation's closing item so far.
    after_close: ItemId,
    /// Another annotation follows its last so far.
    annotation_follows: bool,
    /// The column has ended.
    ended: bool,
}

/// Drops the steps and floors a column that may not split wrote, from
/// `steps_start` on, and clears its levels' `steps`.
fn forget_steps(record: &RubyColumn, steps_start: StepId, out: &mut MeasuredText) {
    let rubies = &mut out.rare_mut().rubies;
    rubies.steps.truncate(steps_start);
    rubies.floors.truncate(steps_start);
    let levels = rubies.levels.get_slice_mut(record.levels.clone());
    for level in levels.unwrap_or_default() {
        level.steps = None;
    }
}

/// Keeps `trimmed` in the columns' table of trimmed em boxes, in column
/// order: a nested column finishes before the column around it.
#[cold]
#[inline(never)]
fn keep_trimmed_em(trimmed: TrimmedEm, out: &mut MeasuredText) {
    let ems = &mut out.rare_mut().rubies.trimmed_ems;
    let found = ems.binary_search_by_key(&trimmed.column, |kept| kept.column);
    ems.insert(found.unwrap_or_else(|at| at), trimmed);
}

/// Keeps `reach` in text order. A tab between two columns keeps the first
/// one's reach.
pub(super) fn keep_tab_reach(reach: TabReach, out: &mut MeasuredText) {
    let reaches = &mut out.rare_mut().rubies.tab_reaches;
    if reaches.last().is_none_or(|last| last.tab < reach.tab) {
        reaches.push(reach);
    }
}

/// The tabs of one blank run, as a ruby column after it reads them.
#[derive(Copy, Clone, Debug)]
pub(super) struct TabRun {
    /// The run's last tab.
    pub(super) last: ClusterId,
    /// How far the run's tabs reach together on their paragraph's line.
    pub(super) width: LayoutUnit,
}

/// Returns whether a ruby column ending before item `close` ends with its
/// container: `close` opens or closes a ruby container, or is past the end.
fn is_container_end(content: &Content, close: ItemId) -> bool {
    content
        .items
        .get(close)
        .is_none_or(|item| matches!(item.kind, ItemKind::RubyClose | ItemKind::RubyOpen))
}
