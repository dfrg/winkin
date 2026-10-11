//! The breaker: fits one line at a time by searching the prefix, and measures
//! each exactly, reshaped edges included.
//!
//! Its siblings each add to [`Breaker`]'s `impl`:
//! - `search` chooses a line's end in its band.
//! - `candidate` measures one candidate end.
//! - `reshape` shapes a line's unsafe edges again.
//! - `floats` places the floats a line reaches.
//! - `wrap` balances and scores a paragraph's lines.
//! - `ruby` splits a ruby column across lines.
//! - `height` measures a fitted line's height.
//!
//! The breaker and the records its files share are defined in the stage's
//! root. This module builds the breaker, and lays out and keeps the lines.
//! [`Breaker::paragraph_lines`] lays out a paragraph at a time. Each line is
//! held to nothing, to a narrowed room, or to a chosen end. A block without
//! `balance` or `pretty` pays one test a paragraph and one hold a line.
//!
//! [`Breaker::line`] lays out one line: it fits the line in the band the
//! host leaves, measures its height, and fits it again where the floats or
//! its own height change its band.

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
    LineFloatId, LineRecord, Lines, PendingLine, TextFitting,
};
use crate::data::Id;
use crate::stages::analysis::{ClusterClass, ClusterId, Paragraph, ParagraphFlags, ParagraphId};
use crate::stages::content::{ContentFlags, ItemId, NodeId, TextFlags};
use crate::stages::measure::{
    AutospaceRules, InitialLetter, LineEdgeCost, MeasureFlags, RubyColumnId,
};
use crate::stages::shape::ShapeSession;
use crate::style::{FirstLineVariant, TextOverflow};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// The most times a line is fitted again in a narrower band, when it turns
/// out taller than the strut it was first fitted at.
pub(super) const SETTLES: usize = 2;

/// The most times one line moves down past the floats beside it, before it
/// stays and overflows.
///
/// A host could report each band change a hair lower, forever. Each move
/// is one fit, so this bounds a line's work. A real line moves once per
/// float bottom it has no room beside, a handful on a page.
pub(super) const MAX_MOVES_DOWN: usize = 256;

/// Returns where a line at `top` moves when the floats leave it no room.
///
/// That is where the host says the bands next change, rounded up onto the
/// grid. Returns `None` where the host's answer is missing, not finite, or
/// not below `top`.
pub(super) fn moves_to(exclusions: &dyn Exclusions, top: LayoutUnit) -> Option<LayoutUnit> {
    let px = exclusions.below(top.to_px())?;
    if !px.is_finite() {
        return None;
    }
    let next = LayoutUnit::from_px_ceil(px);
    (next > top).then_some(next)
}

/// A paragraph being laid out: its breaker record, and the area its lines
/// are in.
pub(super) struct ParagraphLines<'p> {
    pub(super) para: &'p BreakerParagraph,
    pub(super) area: LineBand,
}

/// How a layout of a paragraph chooses its lines.
#[derive(Copy, Clone, Debug)]
pub(super) enum ParagraphPlan {
    /// The greedy fit.
    Greedy,
    /// Each line's room narrowed by `by`, as balancing tries it.
    ///
    /// A trial lays out at most `most` lines, and stops after the first that
    /// overflows. With `most` as `None`, the chosen room lays out them all.
    Narrowed { by: LayoutUnit, most: Option<usize> },
    /// Each line ends where the scorer's plan says, while it fits.
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

    /// Returns whether it is a trial of a narrower room, which an
    /// overflowing line ends.
    fn is_narrowing_trial(&self) -> bool {
        matches!(self, Self::Narrowed { most: Some(_), .. })
    }
}

/// What the line being fitted is held to.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum Hold {
    /// Nothing: the greedy fit.
    Free,
    /// Its room narrowed by this much.
    Narrowed(LayoutUnit),
    /// Ending here, where the line fits.
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

/// Where a layout of a paragraph starts: a line's start, and which line of
/// the paragraph it is.
#[derive(Copy, Clone, Debug)]
pub(super) struct Resume {
    start: ClusterId,
    pub(super) line: WrapLineId,
}

impl Resume {
    /// Returns the paragraph's `line`, starting at `start`.
    pub(super) fn new(start: ClusterId, line: WrapLineId) -> Self {
        Self { start, line }
    }
}

/// What a layout of a paragraph laid out: how many lines, where the last
/// ends, and whether one overflows.
///
/// The count includes the lines before where the layout started.
#[derive(Copy, Clone, Debug)]
pub(super) struct Tally {
    lines: usize,
    end: ClusterId,
    overflowed: bool,
}

impl Tally {
    /// Returns an empty tally from `from`.
    fn new(from: Resume) -> Self {
        Self {
            lines: from.line.get(),
            end: from.start,
            overflowed: false,
        }
    }

    /// Returns which of the paragraph's lines the next is.
    fn next_line(&self) -> WrapLineId {
        WrapLineId::new(self.lines)
    }

    /// Counts `line`, the next one laid out.
    fn take(&mut self, line: &LineRecord) {
        self.lines += 1;
        self.end = line.end;
        self.overflowed |= line.flags.overflowed();
    }

    /// Returns whether the paragraph fits as balancing asks: in `lines`
    /// lines at most, reaching `end`, none overflowing and none ending at an
    /// emergency break, as Chrome's bisection declines both.
    pub(super) fn holds(&self, lines: usize, end: ClusterId) -> bool {
        !self.overflowed && self.lines <= lines && self.end >= end
    }
}

/// A line start's window in the edge tables, shaped once and reused by every
/// candidate.
#[derive(Copy, Clone, Debug)]
pub(super) struct ReshapedStart {
    /// The piece, where one could be shaped.
    pub(super) piece: Option<ReshapedPiece>,
    /// Where the tables stand after it, which a candidate's own pieces rewind
    /// to.
    pub(super) after: EdgeMark,
}

/// A line ending at `end`, measured with its pieces.
///
/// The pieces stay in the edge tables until the next candidate is measured.
#[derive(Copy, Clone, Debug)]
pub(super) struct Candidate {
    pub(super) end: ClusterId,
    pub(super) content_end: ClusterId,
    /// Its content's width, without what hangs or the costs.
    pub(super) content: LayoutUnit,
    /// Its whole reach, what hangs included.
    pub(super) full: LayoutUnit,
    /// Where it ends its paragraph with white space that hangs only where it
    /// overflows, how far what hangs before that white space reaches.
    pub(super) conditional: Option<InlineLayoutUnit>,
    /// What it pays or gains at its end.
    pub(super) cost: LineEdgeCost,
    pub(super) pieces: ReshapedPieces,
}

/// A line fitted at one position, before it is kept: its band, its fit and
/// its height.
#[derive(Copy, Clone, Debug)]
pub(super) struct Trial {
    pub(super) band: LineBand,
    pub(super) fitted: Fitted,
    pub(super) height: Height,
}

/// A line being tried beside floats: its paragraph and start, its start's
/// reshape window, its top, the height its band is asked at, the area, and
/// whether it is the first line.
#[derive(Copy, Clone)]
pub(super) struct LineTrial<'p> {
    pub(super) para: &'p BreakerParagraph,
    pub(super) start: ClusterId,
    /// The first item at `start`, where the breaker's item cursor stands.
    pub(super) first_item: ItemId,
    /// The reshape window of an unsafe start, shaped once for the line.
    pub(super) window: Option<StartWindow>,
    pub(super) top: LayoutUnit,
    pub(super) reserve: LayoutUnit,
    pub(super) area: LineBand,
    pub(super) first: bool,
}

impl<'a: 'c, 'c, 'm, 'provider> Breaker<'a, 'c, 'm, 'provider> {
    /// Returns the ruby columns whose bases start among `clusters`, a
    /// line's, walked to from the column the last search ended near.
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
    /// `back`: an `overflow-wrap` emergency one where `emergency`.
    ///
    /// Each ruby column is stepped over whole, as one indivisible unit.
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

    /// Returns the breaker for `input`, around what `exclusions` answers.
    ///
    /// `room_above` is the room over the first line for annotations and
    /// marks. `fitting` says how `text-fit` scales each line's text.
    pub(super) fn new(
        input: &BreakInput<'a>,
        shaping: &'c mut ShapeSession<'m, 'provider>,
        scratch: &'c mut BreakScratch,
        exclusions: &'c mut dyn Exclusions,
        clamp: Clamp,
        room_above: LayoutUnit,
        fitting: Option<TextFitting>,
    ) -> Self {
        let stages = input.stages;
        let (content, analysis) = (stages.content, stages.analysis);
        // The block's text facts are node 0's. Content never built has no
        // node, and no line to wrap.
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
    ///
    /// [`break_lines`](super::break_lines) uses it to break greedily while
    /// `line-clamp: auto` measures the lines.
    pub(super) fn with_choice(mut self, choice: LineChoice) -> Self {
        self.choice = choice;
        self
    }

    /// Breaks the block into lines in `input`'s area, and returns where the
    /// lines got to.
    ///
    /// Writes the block's end and the room its last line leaves into `out`.
    /// Floats are placed only where the block has one.
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
        // With annotations, each line works out its room, and the last
        // line's carry is the block's. Without, `Lines::room_below` works out
        // the last line's room when a host asks, so a relayout pays nothing.
        out.annotated_carry = if self.annotation.is_some() && !out.lines.is_empty() {
            self.carry
        } else {
            LayoutUnit::ZERO
        };
        cursor
    }

    /// Returns the band the host leaves the next line, its box from `top`,
    /// `height` tall.
    ///
    /// Every trial of a line takes back what was pushed since. So the index
    /// the host sees is where the line will be kept.
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
    /// That is the line box's bottom, less `trimmed` (what `text-box-trim`
    /// takes off the first line) and what it takes off the last.
    fn block_end_if_last(&self, line: &super::PendingLine, trimmed: LayoutUnit) -> LayoutUnit {
        let under = self.trims.map_or(LayoutUnit::ZERO, |trims| {
            trims.end(line.extent, line.scale())
        });
        line.facts.block_start + line.extent.box_height() - trimmed - under
    }

    /// Breaks every paragraph into lines, and returns where the lines got to.
    ///
    /// Places floats as the lines reach them where `FLOATS`. It is generic so
    /// the plain-text loop holds nothing for floats.
    ///
    /// The first line is fitted, measured and placed in the first line's
    /// measure, whose clusters are the text's. The next line starts where it
    /// ends, in the standard measure, reshaped there if unsafe. Nothing is
    /// prepared again, so the first line's end moves with the width.
    fn each_line<const FLOATS: bool>(&mut self, out: &mut Lines, area: Area) -> Cursor {
        let area_band = LineBand::from(area.inline);
        let mut cursor = Cursor::new(LayoutUnit::from_px(area.block_start));
        for (id, paragraph) in self.stages.analysis.paragraphs.iter() {
            // A text ending in a separator ends in an empty paragraph, which
            // draws no line: the separator's line is the last, as in Chrome.
            // An empty text draws none at all.
            if self.stages.analysis.paragraphs.clusters(id).is_empty() {
                continue;
            }
            let para = self.breaker_paragraph(id, paragraph);
            let lines = ParagraphLines {
                para: &para,
                area: area_band,
            };
            // `text-wrap-style` balances or scores a paragraph at a time, as
            // Chrome does. Otherwise the fit is greedy.
            if self.choice == LineChoice::Greedy {
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
        // Floats past the clamp are not placed, since their text is on no
        // line.
        if FLOATS && !cursor.clamped {
            let top = cursor.top - cursor.trimmed;
            self.place_unreached(out, top, &mut cursor.floats);
        }
        cursor
    }

    /// Lays out a paragraph's lines from `from`, each chosen as `plan` says,
    /// and moves `cursor` past them.
    ///
    /// Stops at the paragraph's end, a clamp, or where `plan` stops a trial.
    /// Where given, `recent` records where each of the last lines started, so
    /// a later layout can go back to one.
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
        while start < lines.para.end {
            let Some(hold) = plan.hold(tally.next_line()) else {
                // A trial of the room, past its line count.
                break;
            };
            if let Some(recent) = recent.as_deref_mut() {
                let here = Resume::new(start, tally.next_line());
                let mark = LayoutMark::new(here, self.carry, cursor, out, self.exclusions);
                recent.record(mark);
            }
            self.hold = hold;
            // The first line, in its own variant where it has one.
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
            self.hold = Hold::Free;
            // The initial letter, as the first line's variant measured it.
            // Its room is reported once the line is kept.
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
            // A line that does not end where its plan says no longer fits
            // there. The rest of the paragraph is fitted greedily.
            if let Hold::End(end) = hold {
                if line.end == end {
                    line.flags.insert(LineFlags::SCORED);
                } else {
                    plan = ParagraphPlan::Greedy;
                }
            }
            tally.take(&line);
            // A trial of a narrower room ends at a line that overflows it,
            // as Chrome's bisection stops there.
            let too_narrow = plan.is_narrowing_trial() && tally.overflowed;
            // Every line holds a cluster at least, so this ends.
            debug_assert!(line.end > start, "a line holds something");
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
    /// next at its end.
    ///
    /// The line sits in `area` narrowed by the host's bands, at `cursor` or
    /// below where the floats leave no room. Moves the cursor's items and
    /// floats past the line's. Without `FLOATS`, it places no float and asks
    /// for no checkpoint.
    pub(super) fn line<const FLOATS: bool>(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        area: LineBand,
        cursor: &mut Cursor,
    ) -> (PendingLine, Carry) {
        let placed = out.floats.next_id();
        let top = cursor.top - cursor.trimmed;
        if FLOATS {
            self.place_leading(out, start, top, &mut cursor.floats);
        }
        // An unsafe start is reshaped once, and every fit of the line shares
        // the piece.
        let (window, mut head) = self.line_head(out, para, start);
        // Fit where it starts, or below the floats where they narrow it
        // until it overflows, taking back each try's floats. These are
        // Blink's layout opportunities, one after another.
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
            // Ask where the bands change below the line as they stood before
            // it. Its own floats would only move it past themselves. Blink's
            // opportunities also use the exclusion space before the line.
            let placed = from.is_some_and(|from| from.placed_since(out));
            if placed && let Some(from) = from {
                from.rewind(self.exclusions, out, &mut cursor.floats);
            }
            match moves_to(self.exclusions, top) {
                Some(next) => {
                    top = next;
                    moves += 1;
                }
                // Nowhere lower: the line stays and overflows. Where it placed
                // floats, it is fitted again to place them again.
                None if placed => moves = MAX_MOVES_DOWN,
                None => break trial,
            }
            // Fit again, taking back what this try reshaped past the head.
            out.edges.rewind(head.after());
        };
        // The line's first item, where the cursor stood, and the next's.
        let first_item = cursor.items;
        cursor.items = trial.height.next_item;
        // Floats the line reached but did not place beside it go below it,
        // now that its height is known (Blink's `PlaceFloatingObjects`).
        if FLOATS {
            let bottom = top + trial.height.extent.box_height();
            self.place_after(out, para, start, &trial.fitted, bottom, &mut cursor.floats);
        }
        (
            self.kept(out, para, (start, first_item), top, &trial, placed),
            trial.height.carry,
        )
    }

    /// Fits `line` in the band asked at its reserve height, and returns the
    /// trial.
    ///
    /// Where `FLOATS`, places the line's floats beside it or leaves them to
    /// go below. Where the line's own height meets a narrower band, it is
    /// fitted again there, at most [`SETTLES`] times. Each refit takes back
    /// its floats, and the edge tables to the line's `head`.
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
                out.edges.rewind(head.after());
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
    /// Applies the first line's trim and a clamp by count or by height.
    /// Reports the initial letter, `letter`, to the host once its line is
    /// kept. `carry` is the room the line leaves the next.
    ///
    /// Returns whether the lines go on after it, which they don't after a
    /// clamp.
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
        // `line-clamp`: the last kept line is cut for an ellipsis where text
        // follows it, and nothing after it is broken. A clamp the text does
        // not reach clamps nothing, as Chrome lays the block out again
        // without it (`kNeedsLineClampRelayout`).
        if let Clamp::Lines(limit) = self.clamp
            && out.lines.len() + 1 >= usize::try_from(limit).unwrap_or(usize::MAX)
            && line.end < self.clusters.end_id()
        {
            line.flags.insert(LineFlags::ELLIPSIS);
            cursor.clamped = true;
        }
        // A line the floats moved down starts below where the last ended.
        cursor.top = line.facts.block_start + line.extent.box_height();
        cursor.cleared = None;
        if out.lines.is_empty()
            && let Some(trims) = self.trims
        {
            cursor.trimmed = trims.start(line.extent, line.scale());
        }
        // `line-clamp: auto`: a line that would take the block's content
        // past its end, if it were the last, is not kept.
        if let Clamp::Height(end) = self.clamp
            && self.block_end_if_last(&line, cursor.trimmed) > end
        {
            cursor.overran = true;
            cursor.clamped = true;
            return false;
        }
        let super::PendingLine { record, facts } = line;
        if out.lines.push(record).is_none() {
            debug_assert!(false, "no more lines than clusters, which a LineId names");
            return false;
        }
        self.scratch
            .placements
            .push_bounded(facts, "one placement fact per retained line");
        // The room it leaves the next line at its end.
        self.carry = carry;
        if let Some(letter) = &letter {
            self.sink_initial_letter(out, letter, cursor.trimmed);
        }
        !cursor.clamped
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
            seams: measured.contains(MeasureFlags::HAS_AUTOSPACE),
            forced: last
                .and_then(|last| self.clusters.class(last))
                .is_some_and(ClusterClass::is_forced_break),
            exact_end: self.text_align.needs_exact_end(paragraph.level),
            struts: measured.contains(MeasureFlags::BOXES_PAST_STRUT) || !work::fast_paths(),
        }
    }

    /// Returns the record of a kept line, fitted as `trial` is, its line box
    /// from `top`.
    ///
    /// Pushes its reshaped pieces to the edge shapes. Its floats are those
    /// kept from `floats` on, and its shifts those its last measure settled.
    #[inline]
    pub(super) fn kept(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        (start, first_item): (ClusterId, ItemId),
        top: LayoutUnit,
        trial: &Trial,
        floats: LineFloatId,
    ) -> super::PendingLine {
        let Trial {
            band,
            fitted,
            height,
        } = trial;
        let next = out.edges.shapes.next_id();
        let from = out
            .ruby
            .as_deref()
            .and_then(|ruby| ruby.line_first_shape(out.lines.next_id()))
            .map_or(next, |first| first.min(next));
        for piece in fitted.pieces.iter() {
            out.edges
                .shapes
                .push_bounded(piece.shape, "no more pieces than clusters");
        }
        let mut flags = fitted.flags;
        // `text-overflow: ellipsis` cuts a line that overflows its band, as
        // Chrome's `LineInfo::HasOverflow` does: one with no break that fits.
        if self.ellipsis && flags.contains(LineFlags::OVERFLOWS) {
            flags.insert(LineFlags::ELLIPSIS);
        }
        // The shifts its height settled, sorted by node. Every measure writes
        // them after the kept lines, and the kept line is the last measured.
        let shifts = out.kept_shifts_end();
        if start == ClusterId::new(0) {
            out.letter_shift = height.letter_shift;
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
                shapes: from..out.edges.shapes.next_id(),
                floats: floats..out.floats.next_id(),
                shifts: shifts..out.shifts.next_id(),
                flags,
                scale_bits: self.line_scale.to_bits(),
            },
        }
    }
}
