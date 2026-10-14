//! The breaker's line loop: lays out a block's paragraphs line by line and
//! keeps each line.
//!
//! Sibling files add to [`Breaker`]'s `impl`: `search` chooses a line's end,
//! `candidate` measures one, `reshape` reshapes unsafe edges, `floats`
//! places floats, `text_fit` scales text, `height` measures a line's height,
//! `wrap` balances and scores, and `ruby` splits ruby columns.
//!
//! [`Breaker::line`] fits a line in the band the host leaves, measures its
//! height, and fits it again where floats or its height change the band.

use super::{
    Breaker, BreakerParagraph, Cursor, Fitted, LineHead, ReshapedPiece, ReshapedPieces, StartWindow,
};
use core::cell::Cell;
use core::mem;
use core::ops::Range;

use super::Clamp;
use super::annotate::{AnnotationRoom, Carry};
use super::floats::Placing;
use super::height::{self, Height};
use super::score::{LineChoice, PlannedEnds, WrapLineId};
use super::trim::TextBoxTrims;
use super::wrap::{LayoutMark, RecentMarks};
use super::{
    Area, BlockExtents, BreakInput, BreakScratch, EdgeMark, Exclusions, LineBand, LineFlags,
    LineRecord, Lines, PendingLine, TextFitting,
};
use crate::data::Id;
use crate::stages::analysis::{ClusterClass, ClusterId, Paragraph, ParagraphFlags, ParagraphId};
use crate::stages::content::{ContentFlags, ItemFlags, ItemId, NodeId, TextFlags};
use crate::stages::measure::{
    AutospaceRules, InitialLetter, LineEdgeCost, MeasureFlags, RubyColumnId, edge_room,
};
use crate::stages::shape::ShapeSession;
use crate::style::{FirstLineVariant, TextOverflow};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// The most times a line is fitted again because its height meets a
/// narrower band.
pub(super) const SETTLES: usize = 2;

/// The most times one line moves down past floats before it stays and
/// overflows.
///
/// It bounds a line's work against a host that reports each band change a
/// hair lower. A real line moves a handful of times.
pub(super) const MAX_MOVES_DOWN: usize = 256;

/// Returns where a line at `top` moves when the floats leave it no room.
///
/// That is the host's next band change, rounded up onto the 1/64 grid.
/// `None` where the answer is missing, not finite, or not below `top`.
pub(super) fn moves_to(exclusions: &dyn Exclusions, top: LayoutUnit) -> Option<LayoutUnit> {
    let px = exclusions.below(top.to_px())?;
    if !px.is_finite() {
        return None;
    }
    let next = LayoutUnit::from_px_ceil(px);
    (next > top).then_some(next)
}

/// A paragraph being laid out, and the area its lines are in.
pub(super) struct ParagraphLines<'p> {
    pub(super) para: &'p BreakerParagraph,
    pub(super) area: LineBand,
}

/// The rule a layout of a paragraph chooses its lines by.
#[derive(Copy, Clone, Debug)]
pub(super) enum ParagraphPlan {
    /// The greedy fit.
    Greedy,
    /// Every line's room narrowed by `by`, as balancing tries it.
    ///
    /// A trial lays out at most `most` lines and stops at the first that
    /// overflows. `None` lays out every line.
    Narrowed { by: LayoutUnit, most: Option<usize> },
    /// The scorer's ends, each kept while its line fits.
    Planned(PlannedEnds),
}

impl ParagraphPlan {
    /// Returns what the paragraph's `line` is held to, or `None` where a
    /// trial stops before it.
    fn hold(&self, line: WrapLineId) -> Option<Hold> {
        match self {
            Self::Greedy => Some(Hold::Free),
            &Self::Narrowed { by, most } => most
                .is_none_or(|most| line.get() < most)
                .then_some(Hold::Narrowed(by)),
            Self::Planned(plan) => Some(plan.get(line).map_or(Hold::Free, Hold::End)),
        }
    }

    /// Returns whether it is a trial of a narrower room.
    fn is_narrowing_trial(&self) -> bool {
        matches!(self, Self::Narrowed { most: Some(_), .. })
    }
}

/// What the line being fitted is held to.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Hold {
    /// Nothing: the greedy fit.
    Free,
    /// A room narrowed by this much.
    Narrowed(LayoutUnit),
    /// An end at this cluster boundary, where the line fits.
    End(ClusterId),
}

impl Hold {
    /// Returns how much it narrows a line's room.
    pub(super) fn narrowing(self) -> LayoutUnit {
        match self {
            Self::Narrowed(by) => by,
            Self::Free | Self::End(_) => LayoutUnit::ZERO,
        }
    }
}

/// The start of a layout of a paragraph: a line's first cluster, and which
/// line of the paragraph it is.
#[derive(Copy, Clone, Debug)]
pub(super) struct Resume {
    start: ClusterId,
    pub(super) line: WrapLineId,
}

impl Resume {
    pub(super) fn new(start: ClusterId, line: WrapLineId) -> Self {
        Self { start, line }
    }
}

/// The count of a paragraph's lines laid out, counting from its first line,
/// where the last ends, and whether one overflows.
#[derive(Copy, Clone, Debug)]
pub(super) struct Tally {
    lines: usize,
    end: ClusterId,
    overflowed: bool,
}

impl Tally {
    fn new(from: Resume) -> Self {
        Self {
            lines: from.line.get(),
            end: from.start,
            overflowed: false,
        }
    }

    fn next_line(&self) -> WrapLineId {
        WrapLineId::new(self.lines)
    }

    fn take(&mut self, line: &LineRecord) {
        self.lines += 1;
        self.end = line.end;
        self.overflowed |= line.flags.overflowed();
    }

    /// Returns whether the paragraph fits as balancing asks: in at most
    /// `lines` lines, reaching `end`, none overflowing or ending at an
    /// emergency break, as Chrome's bisection asks.
    pub(super) fn holds(&self, lines: usize, end: ClusterId) -> bool {
        !self.overflowed && self.lines <= lines && self.end >= end
    }
}

/// A line start's reshaped piece, shaped once and reused by every candidate.
#[derive(Copy, Clone, Debug)]
pub(super) struct ReshapedStart {
    /// The piece, where one could be shaped.
    pub(super) piece: Option<ReshapedPiece>,
    /// The edge tables' mark after the piece, which each candidate rewinds
    /// to.
    pub(super) after: EdgeMark,
}

/// A line ending at `end`, measured with its reshaped pieces, which stay in
/// the edge tables until the next candidate.
#[derive(Copy, Clone, Debug)]
pub(super) struct Candidate {
    /// The boundary the line ends at, what hangs included.
    pub(super) end: ClusterId,
    /// The boundary its content ends at, before what hangs.
    pub(super) content_end: ClusterId,
    /// Its content's width, without what hangs or the costs.
    pub(super) content: LayoutUnit,
    /// Its whole reach, what hangs included.
    pub(super) full: LayoutUnit,
    /// How far what hangs before its paragraph-ending white space reaches,
    /// where that white space hangs only on overflow.
    pub(super) conditional: Option<InlineLayoutUnit>,
    /// What it pays or gains at its end.
    pub(super) cost: LineEdgeCost,
    pub(super) pieces: ReshapedPieces,
}

/// A line fitted at one position, before it is kept.
#[derive(Copy, Clone, Debug)]
pub(super) struct Trial {
    pub(super) band: LineBand,
    pub(super) fitted: Fitted,
    pub(super) height: Height,
}

/// A line being fitted at one top.
#[derive(Copy, Clone)]
pub(super) struct LineTrial<'p> {
    pub(super) para: &'p BreakerParagraph,
    pub(super) start: ClusterId,
    /// The first item at `start`.
    pub(super) first_item: ItemId,
    /// The reshape window of an unsafe start.
    pub(super) window: Option<StartWindow>,
    pub(super) top: LayoutUnit,
    /// The height the line's band is asked at.
    pub(super) reserve: LayoutUnit,
    pub(super) area: LineBand,
    /// Whether it is the block's first line.
    pub(super) first: bool,
}

impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Returns the ruby columns whose bases start in a line's `clusters`,
    /// walking from the column the last search ended near.
    pub(super) fn line_columns(&self, clusters: Range<ClusterId>) -> Range<RubyColumnId> {
        let columns = self
            .stages
            .measured
            .ruby_columns()
            .line_columns(self.column.get(), clusters);
        self.column.set(columns.start);
        columns
    }

    /// Returns the first break opportunity in `range`, or the last where
    /// `back`; an `overflow-wrap` emergency one where `emergency`.
    ///
    /// A ruby column is one unit.
    pub(super) fn opportunity(
        &self,
        range: Range<ClusterId>,
        back: bool,
        emergency: bool,
    ) -> Option<ClusterId> {
        if self.has_ruby_columns {
            let rubies = self.stages.measured.ruby_columns();
            let kind = (back, emergency);
            return rubies.opportunity(self.clusters, range, None, kind, &self.column);
        }
        match (back, emergency) {
            (false, false) => self.clusters.first_opportunity(range),
            (true, false) => self.clusters.last_opportunity(range),
            (false, true) => self.clusters.first_emergency(range),
            (true, true) => self.clusters.last_emergency(range),
        }
    }

    /// Returns the breaker for `input`.
    ///
    /// `room_above` is the room over the first line for annotations and
    /// marks. `fitting` is the block's `text-fit`, where it applies.
    pub(super) fn new(
        input: &BreakInput<'a>,
        shaping: &'c mut ShapeSession<'m, 'a>,
        scratch: &'c mut BreakScratch,
        exclusions: &'c mut dyn Exclusions,
        clamp: Clamp,
        room_above: LayoutUnit,
        fitting: Option<TextFitting>,
    ) -> Self {
        let stages = input.stages;
        let (content, analysis) = (stages.content, stages.analysis);
        // Content never built has no block node and no line to wrap.
        let block = NodeId::BLOCK;
        let wraps = content.nodes.kind(block).is_none_or(|_| {
            let text = content.nodes.text_facts(block, FirstLineVariant::Standard);
            content.facts.text(text).has(TextFlags::WRAPS)
        });
        let annotated = AnnotationRoom::wanted(content, stages.measured);
        Self {
            stages: stages.variant(FirstLineVariant::Standard),
            first_line: stages.first_line(),
            clusters: &analysis.clusters,
            annotation: annotated
                .then(|| AnnotationRoom::new(content, stages.measured, input.emphasis_room)),
            measured: stages.measured,
            carry: room_above,
            wraps,
            ellipsis: content.block.text_overflow == TextOverflow::Ellipsis,
            clamp,
            choice: LineChoice::new(content.block.text_wrap_style, input.pretty, wraps),
            hold: Hold::Free,
            shaping,
            scratch,
            exclusions,
            trims: TextBoxTrims::new(content, stages.fonts),
            shaped: stages.shaped.flags,
            words: stages.measured.word_spacing_rule,
            seams: AutospaceRules::new(content),
            has_ruby_columns: !stages
                .measured
                .text(FirstLineVariant::Standard)
                .ruby_columns()
                .is_empty(),
            column: Cell::new(RubyColumnId::new(0)),
            ruby_break_within: input.ruby_break_within,
            continued: None,
            text_align: content.block.text_align,
            text_indent: content.block.text_indent,
            line_fonts: height::LineFonts::default(),
            text_fit: fitting,
            line_scale: 1.0,
        }
    }

    /// Swaps the first line's variant with the standard one, before and
    /// after breaking the first line.
    fn trade_views(&mut self) {
        if let Some(first_line) = &mut self.first_line {
            mem::swap(&mut self.stages, first_line);
            self.line_fonts = height::LineFonts::default();
        }
    }

    /// Returns how the block's paragraphs choose their lines.
    pub(super) fn choice(&self) -> LineChoice {
        self.choice
    }

    /// Returns the breaker with `choice` in place of `text-wrap-style`'s.
    pub(super) fn with_choice(mut self, choice: LineChoice) -> Self {
        self.choice = choice;
        self
    }

    /// Breaks the block into lines in `input`'s area, and returns where the
    /// lines got to.
    pub(super) fn lines(&mut self, out: &mut Lines, input: &BreakInput<'_>) -> Cursor {
        let cursor = if self.stages.content.flags.contains(ContentFlags::FLOATS) {
            self.each_line::<true>(out, input.area)
        } else {
            self.each_line::<false>(out, input.area)
        };
        out.block = super::BlockResult::new(
            out,
            &self.scratch.placements,
            cursor.top,
            cursor.trimmed,
            cursor.cleared,
            self.trims,
        );
        // Without annotations, `Lines::room_below` works out the last line's
        // carry when a host asks.
        if self.annotation.is_some() && !out.lines.is_empty() {
            out.rare_mut().annotated_carry = self.carry;
        }
        cursor
    }

    /// Returns the band the host leaves the next line, its box from `top`,
    /// `height` tall.
    ///
    /// Each trial takes back its lines, so the host sees the index the line
    /// is kept at.
    pub(super) fn band(&self, out: &Lines, top: LayoutUnit, height: LayoutUnit) -> LineBand {
        let block = BlockExtents {
            start: top.to_px(),
            end: (top + height).to_px(),
        };
        LineBand::from(self.exclusions.band(out.lines.next_id().get(), block))
    }

    /// Returns where the block's content would end, in the host's
    /// positions, if `line` were its last.
    ///
    /// `trimmed` is what `text-box-trim` takes off the first line.
    fn block_end_if_last(&self, line: &super::PendingLine, trimmed: LayoutUnit) -> LayoutUnit {
        let under = self.trims.map_or(LayoutUnit::ZERO, |trims| {
            trims.end(line.extent, line.scale, line.variant())
        });
        line.facts.block_start + line.extent.box_height() - trimmed - under
    }

    /// Breaks every paragraph into lines, and returns where the lines got to.
    ///
    /// `FLOATS` keeps float code out of the plain-text loop.
    ///
    /// The first line is measured in the first line's variant, whose
    /// clusters are the text's. The next line starts where it ends, in the
    /// standard variant, reshaped there if unsafe.
    fn each_line<const FLOATS: bool>(&mut self, out: &mut Lines, area: Area) -> Cursor {
        let area_band = LineBand::from(area.inline);
        let mut cursor = Cursor::new(LayoutUnit::from_px(area.block_start));
        for (id, paragraph) in self.stages.analysis.paragraphs.iter() {
            // A text ending in a separator ends in an empty paragraph, which
            // makes no line, as in Chrome, unless a box there has an inline
            // margin, border or padding.
            let paragraphs = &self.stages.analysis.paragraphs;
            let empty = paragraphs.clusters(id).is_empty();
            if empty && !paragraphs.has_end_line() {
                continue;
            }
            let para = self.breaker_paragraph(id, paragraph);
            let lines = ParagraphLines {
                para: &para,
                area: area_band,
            };
            if self.choice == LineChoice::Greedy || empty {
                let from = Resume::new(para.start, WrapLineId::new(0));
                self.paragraph_lines::<FLOATS>(
                    out,
                    &lines,
                    ParagraphPlan::Greedy,
                    from,
                    &mut cursor,
                    None,
                );
            } else {
                self.wrapped_paragraph::<FLOATS>(out, &lines, &mut cursor);
            }
            if cursor.clamped {
                break;
            }
        }
        // Floats past the clamp anchor text on no line.
        if FLOATS && !cursor.clamped {
            let top = cursor.top - cursor.trimmed;
            self.place_unreached(out, top, &mut cursor.floats);
        }
        cursor
    }

    /// Lays out a paragraph's lines from `from` as `plan` says, and moves
    /// `cursor` past them.
    ///
    /// Stops at the paragraph's end, a clamp, or where `plan` stops a trial.
    /// Where given, `recent` records a mark at each line's start.
    pub(super) fn paragraph_lines<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        lines: &ParagraphLines<'_>,
        mut plan: ParagraphPlan,
        from: Resume,
        cursor: &mut Cursor,
        mut recent: Option<&mut RecentMarks>,
    ) -> Tally {
        let id = lines.para.id;
        let mut start = from.start;
        let mut tally = Tally::new(from);
        // An empty paragraph that makes a line makes one.
        let mut empty = start == lines.para.end;
        while start < lines.para.end || mem::take(&mut empty) {
            let Some(hold) = plan.hold(tally.next_line()) else {
                break;
            };
            if let Some(recent) = recent.as_deref_mut() {
                let here = Resume::new(start, tally.next_line());
                let mark = LayoutMark::new(here, self.carry, cursor, out, self.exclusions);
                recent.record(mark);
            }
            self.hold = hold;
            let first_line = out.lines.is_empty() && self.first_line.is_some();
            let first_para = if first_line {
                self.trade_views();
                let paragraphs = &self.stages.analysis.paragraphs;
                let paragraph = paragraphs.get(id);
                paragraph.map(|paragraph| self.breaker_paragraph(id, paragraph))
            } else {
                None
            };
            let para = first_para.as_ref().unwrap_or(lines.para);
            let (mut line, carry) = self.line::<FLOATS>(out, para, start, lines.area, cursor);
            if start == para.end {
                self.widen_end_line(&mut line);
            }
            self.hold = Hold::Free;
            // The initial letter goes to the host once its line is kept.
            let content = self.stages.content;
            let letter =
                if content.flags.contains(ContentFlags::INITIAL_LETTER) && out.lines.is_empty() {
                    self.stages.measured.initial_letter().copied()
                } else {
                    None
                };
            if first_line {
                line.flags.insert(LineFlags::FIRST_LINE);
                self.trade_views();
            }
            // A line that misses its planned end does not fit there; the
            // rest of the paragraph is greedy.
            if let Hold::End(end) = hold {
                if line.end == end {
                    line.flags.insert(LineFlags::SCORED);
                } else {
                    plan = ParagraphPlan::Greedy;
                }
            }
            tally.take(&line);
            // A narrowing trial stops at an overflowing line, as Chrome's
            // bisection does.
            let too_narrow = plan.is_narrowing_trial() && tally.overflowed;
            debug_assert!(line.end > start || line.end == lines.para.end);
            start = line.end;
            let kept = self.keep(out, line, carry, letter, cursor);
            self.clear_floats(out, start, lines.area, cursor);
            if !kept || too_narrow {
                break;
            }
        }
        tally
    }

    /// Returns the line starting at `start`, and the room it leaves the
    /// next.
    ///
    /// The line sits in `area` narrowed by the host's bands, at `cursor` or
    /// lower where the floats leave no room. Moves the cursor past the
    /// line's items and floats.
    pub(super) fn line<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        area: LineBand,
        cursor: &mut Cursor,
    ) -> (PendingLine, Carry) {
        let top = cursor.top - cursor.trimmed;
        if FLOATS {
            self.place_leading(out, start, top, &mut cursor.floats);
        }
        // Every fit of the line shares one reshaped start.
        let (window, mut head) = self.line_head(out, para, start);
        // Fit at `top`, or move down past floats that make the line
        // overflow: Blink's layout opportunities.
        let mut top = top;
        let mut moves = 0;
        let trial = loop {
            let from = FLOATS.then(|| Placing::new(self.exclusions, out, cursor.floats));
            let line = LineTrial {
                para,
                start,
                first_item: cursor.items,
                window,
                top,
                reserve: self.reserve(start),
                area,
                first: out.lines.is_empty(),
            };
            let trial = self.trial::<FLOATS>(out, &mut head, line, cursor);
            let movable = trial.fitted.flags.contains(LineFlags::OVERFLOWS)
                && self.wraps
                && moves < MAX_MOVES_DOWN
                && trial.band.width() < area.width()
                && self.hold.narrowing() == LayoutUnit::ZERO;
            if !movable {
                break trial;
            }
            // Ask where the bands change without the line's own floats, as
            // Blink's opportunities do.
            let placed = from.is_some_and(|from| from.placed_since(out));
            if placed && let Some(from) = from {
                from.rewind(self.exclusions, out, &mut cursor.floats);
            }
            match moves_to(self.exclusions, top) {
                Some(next) => {
                    top = next;
                    moves += 1;
                }
                // Nowhere lower: the line stays and overflows, fitted once
                // more to place the floats taken back.
                None if placed => moves = MAX_MOVES_DOWN,
                None => break trial,
            }
            out.rewind_edges(head.after());
        };
        let first_item = cursor.items;
        cursor.items = trial.height.next_item;
        // Floats not placed beside the line go below it, as in Blink's
        // `PlaceFloatingObjects`.
        if FLOATS {
            let bottom = top + trial.height.extent.box_height();
            self.place_after(out, para, start, &trial.fitted, bottom, &mut cursor.floats);
        }
        (
            self.kept(out, para, (start, first_item), top, &trial),
            trial.height.carry,
        )
    }

    /// Fits `line` in the band asked at its reserve height, and returns the
    /// trial.
    ///
    /// Places the line's floats where `FLOATS`, and sets its `text-fit`
    /// scale. A line taller than its reserve that meets a narrower band is
    /// fitted again, at most [`SETTLES`] times.
    fn trial<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        head: &mut LineHead,
        mut line: LineTrial<'_>,
        cursor: &mut Cursor,
    ) -> Trial {
        let LineTrial {
            para,
            start,
            top,
            area,
            ..
        } = line;
        let mut settles = 0;
        loop {
            let from = FLOATS.then(|| Placing::new(self.exclusions, out, cursor.floats));
            let mut band = area.meet(self.band(out, top, line.reserve));
            let mut fitted = self.fit(out, &line, band, head);
            if FLOATS {
                self.place_on_line(out, head, &line, &mut cursor.floats, &mut band, &mut fitted);
            }
            if let Some(fitting) = self.text_fit {
                let line = (cursor.items, start);
                let last = fitted.end == para.end;
                self.line_scale = self.fit_text(out, fitting, line, band, &mut fitted, last);
            }
            let height = self.extent(out, para, cursor.items, start, fitted.end);
            let tall = height.extent.box_height();
            if settles < SETTLES
                && tall > line.reserve
                && area.meet(self.band(out, top, tall)).width() < band.width()
            {
                if let Some(from) = from {
                    from.rewind(self.exclusions, out, &mut cursor.floats);
                }
                out.rewind_edges(head.after());
                line.reserve = tall;
                settles += 1;
                continue;
            }
            return Trial {
                band,
                fitted,
                height,
            };
        }
    }

    /// Keeps `line` where `cursor` stands, and moves the cursor past it.
    ///
    /// Applies the first line's trim and `line-clamp`, and reports `letter`
    /// to the host. Returns `false` after a clamp.
    #[inline(always)]
    fn keep(
        &mut self,
        out: &mut Lines,
        mut line: super::PendingLine,
        carry: Carry,
        letter: Option<InitialLetter>,
        cursor: &mut Cursor,
    ) -> bool {
        line.facts.block_start = line.facts.block_start + cursor.trimmed;
        // `line-clamp`: the last kept line takes an ellipsis where text
        // follows. A clamp the text does not reach clamps nothing, as Chrome
        // relays the block out without it (`kNeedsLineClampRelayout`).
        if let Clamp::Lines(limit) = self.clamp
            && out.lines.len() + 1 >= usize::try_from(limit).unwrap_or(usize::MAX)
            && line.end < self.clusters.end_id()
        {
            line.flags.insert(LineFlags::ELLIPSIS);
            cursor.clamped = true;
        }
        cursor.top = line.facts.block_start + line.extent.box_height();
        cursor.cleared = None;
        if out.lines.is_empty()
            && let Some(trims) = self.trims
        {
            cursor.trimmed = trims.start(line.extent, line.scale, line.variant());
        }
        // `line-clamp: auto`: a line that would end the block past its end
        // is not kept.
        if let Clamp::Height(end) = self.clamp
            && self.block_end_if_last(&line, cursor.trimmed) > end
        {
            cursor.overran = true;
            cursor.clamped = true;
            return false;
        }
        let super::PendingLine {
            record,
            facts,
            scale,
        } = line;
        if out.lines.push(record).is_none() {
            debug_assert!(false, "no more lines than clusters, which a LineId names");
            return false;
        }
        if self.text_fit.is_some() {
            out.rare_mut()
                .scales
                .push_bounded(scale, "one scale per kept line");
        }
        self.scratch
            .placements
            .push_bounded(facts, "one placement fact per retained line");
        self.carry = carry;
        if let Some(letter) = &letter {
            self.sink_initial_letter(out, letter, cursor.trimmed);
        }
        !cursor.clamped
    }

    /// Adds to `line`, an empty line at the text's end, the room of the box
    /// edges on it, which no prefix sum holds.
    #[cold]
    #[inline(never)]
    fn widen_end_line(&self, line: &mut super::PendingLine) {
        let (content, analysis) = (self.stages.content, self.stages.analysis);
        let end = line.end;
        let mut room = InlineLayoutUnit::ZERO;
        let mut id = line.first_item;
        while let Some(item) = content.items.get(id) {
            work::step();
            let clusters = analysis.item_clusters.range(id);
            if clusters.start != end || !clusters.is_empty() {
                break;
            }
            let (opens, closes) = (item.kind.is_open(), item.kind.is_close());
            if !item.flags.contains(ItemFlags::ANNOTATION) && (opens || closes) {
                room += edge_room(content, None, item, opens);
            }
            id = ItemId::new(id.get() + 1);
        }
        line.record.width = line.record.width + room.to_layout();
    }

    fn breaker_paragraph(&self, id: ParagraphId, paragraph: &Paragraph) -> BreakerParagraph {
        let range = self.stages.analysis.paragraphs.clusters(id);
        let measured = self.stages.measured.paragraph(id);
        let last = range.end.get().checked_sub(1).map(ClusterId::new);
        BreakerParagraph {
            id,
            start: range.start,
            end: range.end,
            origin: self.stages.measured.prefix.get(range.start),
            text: self.clusters.start(range.start)..self.clusters.start(range.end),
            level: paragraph.level,
            monotone: !measured.contains(MeasureFlags::NONMONOTONE),
            tabs: paragraph.flags.contains(ParagraphFlags::HAS_TABS),
            breakable_ruby: measured.contains(MeasureFlags::HAS_BREAKABLE_RUBY),
            costs: measured.contains(MeasureFlags::HAS_EDGE_COSTS),
            end_cost: if measured.contains(MeasureFlags::HAS_EDGE_COSTS) {
                let costs = self.stages.measured.edge_costs();
                costs.get(range.end).copied().unwrap_or_default()
            } else {
                LineEdgeCost::default()
            },
            end_tail: self.hanging_tail(range.start, range.end, |_| None),
            seams: measured.contains(MeasureFlags::HAS_AUTOSPACE),
            forced: last
                .and_then(|last| self.clusters.class(last))
                .is_some_and(ClusterClass::is_forced_break),
            exact_end: self
                .text_align
                .needs_exact_end(self.stages.content.block.text_align_last),
            struts: measured.contains(MeasureFlags::BOXES_PAST_STRUT) || !work::fast_paths(),
        }
    }

    /// Returns the record of a kept line, fitted as `trial` is, its line box
    /// from `top`.
    ///
    /// Always inlined: out of line, the record goes through memory, which
    /// costs a 400-character relayout 1.2% more instructions.
    #[inline(always)]
    pub(super) fn kept(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        (start, first_item): (ClusterId, ItemId),
        top: LayoutUnit,
        trial: &Trial,
    ) -> super::PendingLine {
        let Trial {
            band,
            fitted,
            height,
        } = trial;
        let next = out.edges().shapes.next_id();
        let from = out
            .ruby
            .as_deref()
            .and_then(|ruby| ruby.line_first_shape(out.lines.next_id()))
            .map_or(next, |first| first.min(next));
        for piece in fitted.pieces.iter() {
            out.edges_mut()
                .shapes
                .push_bounded(piece.shape, "no more pieces than clusters");
        }
        let mut flags = fitted.flags;
        // `text-overflow: ellipsis` cuts a line that overflows its band, as
        // Chrome's `LineInfo::HasOverflow` does.
        if self.ellipsis && flags.contains(LineFlags::OVERFLOWS) {
            flags.insert(LineFlags::ELLIPSIS);
        }
        // The kept line was measured last, so its shifts follow the kept
        // lines'.
        let shifts = out.kept_shifts_end();
        if start == ClusterId::new(0) {
            out.set_letter_shift(height.letter_shift);
        }
        super::PendingLine {
            facts: super::PlacementFacts {
                content_end: fitted.content_end,
                block_start: top,
            },
            record: LineRecord {
                start,
                end: fitted.end,
                paragraph: para.id,
                first_item,
                band: *band,
                indent: fitted.indent,
                width: fitted.width,
                hang: fitted.hang,
                extent: height.extent,
                shapes: from..out.edges().shapes.next_id(),
                shifts: shifts..out.shift_end(),
                flags,
            },
            scale: self.line_scale,
        }
    }
}
