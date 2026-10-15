//! Reshaping a line's unsafe edges, as more of [`Breaker`]'s `impl`.
//!
//! Pieces go in the edge tables on [`Lines`]; a trial rewinds them by mark.
//!
//! **Unsafe edges**, as in Chrome's `ShapingLineBreaker`:
//! - A start the font shaped across is reshaped up to the next safe break,
//!   within its shaping run.
//! - An unsafe end is reshaped from the last safe break before it, never
//!   past its run's start.
//! - A candidate the reshape makes too wide sends the search to the
//!   opportunity before (`ShapeLine`).
//! - Spaces that hang at a line's end keep the paragraph's glyphs, and
//!   the text before them ends at them (`ShapingLineBreaker`). Under
//!   `break-spaces` a break after a space is reshaped only where the line
//!   needs its exact end (`SetDontReshapeEndIfAtSpace`,
//!   [`Breaker::space_break_reshaped`]).
//! - The text before those spaces, where the font shaped across them, is
//!   reshaped up to them only where the line needs its exact end
//!   (`TruncateLineEndResult`): at its paragraph's end for collapsible
//!   spaces, under an alignment that needs it, or where its text's style
//!   paints or decorates. Elsewhere its last letter keeps its kern
//!   ([`Breaker::removed_end`]).
//!
//! **Seam room** (`text-autospace`). A line that ends at a seam gives its
//! room back: the last cluster is copied from the paragraph's glyphs, less
//! the room (Chrome's `UnapplyAutoSpacing`).
//!
//! **Punctuation trim** (`text-spacing-trim`). A reshaped start is shaped as
//! a line start. Under `space-first` and `trim-start`, a wrapped line that
//! starts at an opening mark is reshaped with the mark trimmed
//! (`FirstSafeOffset`). A closing mark that fits only trimmed ends the line
//! trimmed where a break after it is allowed.

use core::ops::Range;

use super::fit::ReshapedStart;
use super::{
    Breaker, BreakerParagraph, EdgeShape, LineHead, LineStart, Lines, ReshapedPiece,
    ReshapedPieces, StartWindow,
};
use crate::data::Id;
use crate::data::IdRange;
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::{BoxFlags, ContentFlags, NodeId, NodeKind};
use crate::stages::measure::LetterWordSpacing;
use crate::stages::shape::{
    NeighbourFonts, ShapedFlags, ShapedRunId, ShapingEdges, ShapingKey, ShapingSource,
    maybe_closing_mark, maybe_opening_mark, shape_range,
};
use crate::stages::{Segments, Step};
use crate::style::TextSpacingTrim;
use crate::unit::{InlineLayoutUnit, TextUnit};

impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Reshapes the start window of a line from the unsafe `start`, up to the
    /// next safe break within its shaping run.
    ///
    /// The piece stays first in the edge tables for the candidates to share.
    /// As in Chrome, nothing checks that it joins the paragraph's glyphs at its
    /// end; a check costs a reshape per window and changes no test or page.
    pub(super) fn start_window(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        trims: bool,
    ) -> Option<(StartWindow, ReshapedStart)> {
        let shaped = self.stages.shaped;
        let run = shaped.runs.containing(start)?;
        let run_end = shaped.runs.run_clusters(run).end.min(para.end);
        let end = shaped
            .glyphs
            .next_safe_to_break(ClusterId::new(start.get() + 1), run_end);
        let edges = StartWindow::edges(trims);
        let piece = self.reshape(out, para, run, start..end, edges, false)?;
        let window = StartWindow {
            run,
            end,
            delta: piece.delta,
            trims,
        };
        let head = ReshapedStart {
            piece: Some(piece),
            after: out.edges().mark(),
        };
        Some((window, head))
    }

    /// Returns the start window of a line from `start`, and the line's head in
    /// the edge tables.
    ///
    /// A wrapped start the font shaped across is reshaped, and so is an opening
    /// mark the style trims. Inlined: out of line, relayout is up to 8% slower.
    #[inline(always)]
    pub(super) fn line_head(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
    ) -> (Option<StartWindow>, LineHead) {
        let mark = out.edges().mark();
        let wrapped = start > para.start;
        let trims = wrapped
            && self
                .stages
                .content
                .flags
                .contains(ContentFlags::TRIMS_WRAPPED_START)
            && self.trims_start(start);
        let shaped_across = wrapped
            && self.shaped.contains(ShapedFlags::HAS_UNSAFE)
            && self.stages.shaped.glyphs.word(start).is_unsafe_to_break();
        // The marks after a space take the space's spacing in the prefix, and
        // their own where they start a line.
        let space_marks = wrapped
            && self
                .stages
                .content
                .flags
                .contains(ContentFlags::NONZERO_SPACING)
            && self.starts_with_space_marks(start);
        let (window, head) = if shaped_across || trims || space_marks {
            self.start_window(out, para, start, trims).unzip()
        } else {
            (None, None)
        };
        (window, LineHead { mark, shaped: head })
    }

    /// Returns whether cluster `start` is the marks after a space
    /// ([`Clusters::is_space_marks`]).
    ///
    /// [`Clusters::is_space_marks`]: crate::stages::analysis::Clusters::is_space_marks
    #[inline(never)]
    fn starts_with_space_marks(&self, start: ClusterId) -> bool {
        let clusters = &self.stages.analysis.clusters;
        clusters
            .first_char(self.stages.text(), start)
            .is_some_and(|ch| clusters.is_space_marks(start, ch))
    }

    /// Returns the piece of the start `window`, reshaping it if a one-piece line
    /// took it back from `head`.
    fn head(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
        window: StartWindow,
        head: &mut LineHead,
    ) -> Option<ReshapedPiece> {
        if let Some(shaped) = head.shaped {
            return shaped.piece;
        }
        out.rewind_edges(head.mark);
        let range = start..window.end;
        let edges = StartWindow::edges(window.trims);
        let piece = self.reshape(out, para, window.run, range, edges, false);
        head.shaped = Some(ReshapedStart {
            piece,
            after: out.edges().mark(),
        });
        piece
    }

    /// Reshapes the edges of `line`, from a line's start to its shaped end, and
    /// returns the pieces.
    ///
    /// The window's piece leads, and the end is reshaped as `end` says. A line
    /// that ends inside the window, or whose end piece may not start after it,
    /// is one piece. Inlined: out of line, relayout is up to 8% slower.
    #[inline(always)]
    pub(super) fn edge_pieces(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        line: Range<ClusterId>,
        window: Option<StartWindow>,
        head: &mut LineHead,
        end: EdgeEnd,
    ) -> ReshapedPieces {
        let mut pieces = ReshapedPieces::default();
        let Range {
            start,
            end: shaped_end,
        } = line.clone();
        let gives_back = end.seam > InlineLayoutUnit::ZERO;
        let Some(window) = window else {
            if end.reshaped
                && let EndPiece::Piece(piece) =
                    self.end_piece(out, para, start, shaped_end, true, end.trims)
            {
                pieces.push(Some(piece));
            } else if gives_back {
                pieces.push(self.given_back(out, shaped_end, end.seam));
            }
            return pieces;
        };
        let inside =
            shaped_end < window.end || (shaped_end == window.end && end.whole_at_window_end);
        if inside {
            out.rewind_edges(head.mark);
            head.shaped = None;
            pieces.push(self.reshape(out, para, window.run, line, end.whole, true));
            return pieces;
        }
        pieces.push(self.head(out, para, start, window, head));
        if !end.reshaped {
            if gives_back {
                pieces.push(self.given_back(out, shaped_end, end.seam));
            }
            return pieces;
        }
        match self.end_piece(out, para, window.end, shaped_end, false, end.trims) {
            EndPiece::Piece(piece) => pieces.push(Some(piece)),
            EndPiece::None if gives_back => {
                pieces.push(self.given_back(out, shaped_end, end.seam));
            }
            EndPiece::None => {}
            EndPiece::Whole(run) => {
                out.rewind_edges(head.mark);
                head.shaped = None;
                pieces = ReshapedPieces::default();
                pieces.push(self.reshape(out, para, run, line, end.whole, true));
            }
        }
        pieces
    }

    /// Reshapes the end of a line that ends at the unsafe `end`, from the last
    /// safe break before it.
    ///
    /// The piece starts no earlier than `floor` or its run's start, and widens
    /// while the shaper says its start may not join. A run's start always joins,
    /// since the paragraph was shaped apart there. `floor` is the line's start
    /// (`floor_is_start`) or the start window's end; a piece that may not start
    /// at the window's end makes the line one piece ([`EndPiece::Whole`]).
    fn end_piece(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        floor: ClusterId,
        end: ClusterId,
        floor_is_start: bool,
        trims_end: bool,
    ) -> EndPiece {
        let (glyphs, runs) = (&self.stages.shaped.glyphs, &self.stages.shaped.runs);
        let Some(run) = end
            .get()
            .checked_sub(1)
            .and_then(|last| runs.containing(ClusterId::new(last)))
        else {
            return EndPiece::None;
        };
        let run_start = runs.run_clusters(run).start;
        let lowest = run_start.max(floor);
        let mut from = glyphs.prev_safe_to_break(end, lowest);
        loop {
            let mark = out.edges().mark();
            // A piece at a wrapped line's start is shaped as a start.
            let edges = ShapingEdges {
                line_start: floor_is_start && from == floor && from > para.start,
                trim_end: trims_end,
                ..ShapingEdges::default()
            };
            let Some(piece) = self.reshape(out, para, run, from..end, edges, true) else {
                return EndPiece::None;
            };
            if !piece.start_unsafe || (from == floor && floor_is_start) || run_start == from {
                return EndPiece::Piece(piece);
            }
            out.rewind_edges(mark);
            if from == floor {
                return EndPiece::Whole(run);
            }
            from = glyphs.prev_safe_to_break(from, lowest);
        }
    }

    /// Shapes `range` of `para` alone into the edge tables, with the paragraph
    /// either side as context.
    ///
    /// `id` is the shaping run containing `range`. Where `ends_line`, no seam
    /// room follows the last cluster. Returns `None` for an empty range or
    /// where a table is full.
    fn reshape(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        id: ShapedRunId,
        range: Range<ClusterId>,
        edges: ShapingEdges,
        ends_line: bool,
    ) -> Option<ReshapedPiece> {
        let count = range.end.get().checked_sub(range.start.get())?;
        if count == 0 {
            return None;
        }
        #[cfg(test)]
        out.reshaped.push((range.clone(), edges));
        let stages = self.stages;
        let runs = &stages.shaped.runs;
        let run = runs.get(id)?;
        // The neighbouring runs' fonts, where the piece reaches its run's edges.
        let clusters = runs.run_clusters(id);
        let before = (range.start == clusters.start)
            .then(|| runs.get(ShapedRunId::new(id.get().checked_sub(1)?)))
            .flatten();
        let after = (range.end == clusters.end)
            .then(|| runs.get(ShapedRunId::new(id.get() + 1)))
            .flatten();
        let neighbours = NeighbourFonts::new(
            stages.fonts,
            before.map(|run| run.font),
            after.map(|run| run.font),
        );
        let (content, analysis) = (stages.content, stages.analysis);
        let key = ShapingKey::new(
            run,
            content,
            analysis,
            stages.fonts,
            self.shaped.contains(ShapedFlags::HALVES_PUNCTUATION),
        );
        let source = ShapingSource::new(content, analysis, stages.variant);
        let mark = out.edges().mark();
        let (start_unsafe, written) = {
            let mut sink = out.edges_mut().sink();
            let start_unsafe = shape_range(
                self.shaping,
                &source,
                para.text.clone(),
                range.clone(),
                &key,
                edges,
                neighbours,
                &mut sink,
            );
            (start_unsafe, sink.words_written())
        };
        let entries = mark.clusters..out.edges().words.next_id();
        if written != count {
            // A table is full: the line keeps the paragraph's glyphs.
            out.rewind_edges(mark);
            return None;
        }
        let seams = para.seams;
        let letter_or_word = content.flags.contains(ContentFlags::NONZERO_SPACING);
        if letter_or_word || seams {
            // Space the piece's own advances as Chrome spaces a reshaped result, so a
            // ligature the piece forms is spaced as the piece has it. Seam room follows
            // every cluster but the last of a piece that ends a line.
            let spaced = LetterWordSpacing::from_shaping(&content.facts, run.shaping, self.words);
            let line_start = edges.line_start;
            let paragraph = analysis.paragraphs.get(para.id);
            let source = self.stages.text();
            let edges = out.edges_mut();
            let words = edges.words.get_slice(entries.clone());
            let advances = edges.advances.get_slice_mut(entries.clone());
            let mut near = None;
            if let (Some(words), Some(advances)) = (words, advances) {
                for (i, (word, advance)) in words.iter().zip(advances.iter_mut()).enumerate() {
                    let cluster = ClusterId::new(range.start.get() + i);
                    let continuation = word.is_continuation();
                    let mut spacing = if i == 0 && line_start {
                        spaced.after_line_start(analysis, source, cluster, continuation)
                    } else {
                        spaced.after_cluster(analysis, source, cluster, continuation)
                    }
                    .raw();
                    if seams
                        && !(i + 1 == count && ends_line)
                        && let Some(paragraph) = paragraph
                    {
                        let room =
                            self.seams
                                .gap_after_near(&self.stages, paragraph, cluster, &mut near);
                        spacing = spacing.saturating_add(room.raw());
                    }
                    *advance = TextUnit::from_raw(advance.raw().saturating_add(spacing));
                }
            }
        }
        let advance = out.edges().advance_sum(entries);
        // No box edge with room lies inside a shaping run, so the prefix sum is
        // the paragraph's advance over the piece.
        let paragraph = stages.measured.prefix.advance(range.start, range.end);
        Some(ReshapedPiece {
            shape: EdgeShape::new(range, mark.clusters),
            delta: advance - paragraph,
            start_unsafe,
        })
    }

    /// Returns the `text-spacing-trim` of the shaping run containing `cluster`.
    fn trim(&self, cluster: ClusterId) -> Option<TextSpacingTrim> {
        let stages = &self.stages;
        let run = stages.shaped.runs.run_containing(cluster)?;
        Some(stages.content.facts.shaping(run.shaping).trim)
    }

    /// Returns whether a wrapped line from `start` may trim its opening mark.
    ///
    /// The text sets `space-first` or `trim-start` (Blink's
    /// `ShouldTrimStartOfWrappedLine`) and the character may be an opening mark.
    /// The reshape finds whether the font trims it.
    pub(super) fn trims_start(&self, start: ClusterId) -> bool {
        self.trim(start)
            .is_some_and(TextSpacingTrim::trims_wrapped_start)
            && self
                .clusters
                .first_char(self.stages.text(), start)
                .is_some_and(maybe_opening_mark)
    }

    /// Returns whether a line may end trimmed after the closing mark `mark`.
    ///
    /// The text trims (Blink's `ShouldTrimEnd`), the character may be a closing
    /// mark, and a line may end after it outside a ruby column. The break test
    /// runs first, since prose rarely breaks there.
    pub(super) fn trims_end(&self, para: &BreakerParagraph, mark: ClusterId) -> bool {
        let next = ClusterId::new(mark.get() + 1);
        mark < para.end
            && (self
                .clusters
                .attrs(mark)
                .is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER))
                || next == para.end)
            && self
                .stages
                .measured
                .ruby_columns()
                .interior(next, &self.column)
                .is_none()
            && self
                .clusters
                .first_char(self.stages.text(), mark)
                .is_some_and(maybe_closing_mark)
            && self
                .trim(mark)
                .is_some_and(TextSpacingTrim::trims_punctuation)
    }

    /// Returns the `text-autospace` room before boundary `end`, which a line
    /// ending there gives back; zero at the paragraph's ends.
    pub(super) fn seam_before(&self, para: &BreakerParagraph, end: ClusterId) -> InlineLayoutUnit {
        if !para.seams || end >= para.end || end <= para.start {
            return InlineLayoutUnit::ZERO;
        }
        let Some(paragraph) = self.stages.analysis.paragraphs.get(para.id) else {
            return InlineLayoutUnit::ZERO;
        };
        let last = ClusterId::new(end.get() - 1);
        InlineLayoutUnit::from_text(self.seams.gap_after(&self.stages, paragraph, last))
    }

    /// Returns the last cluster before `end` as its own piece, copied, with
    /// `room` taken off its advance, as Chrome's `UnapplyAutoSpacing` does.
    fn given_back(
        &mut self,
        out: &mut Lines,
        end: ClusterId,
        room: InlineLayoutUnit,
    ) -> Option<ReshapedPiece> {
        let cluster = ClusterId::new(end.get().checked_sub(1)?);
        let mark = out.edges().mark();
        let own = self.stages.cluster_advance(cluster);
        let advance = own - room;
        let copied = {
            let mut sink = out.edges_mut().sink();
            sink.copy(&self.stages.shaped.glyphs, cluster, advance.raw());
            sink.dropped() == 0
        };
        if !copied || out.edges().words.len() != mark.clusters.get() + 1 {
            out.rewind_edges(mark);
            return None;
        }
        Some(ReshapedPiece {
            shape: EdgeShape::new(cluster..end, mark.clusters),
            delta: InlineLayoutUnit::ZERO - room,
            start_unsafe: false,
        })
    }

    /// Returns where a line's text ends without the spaces it ends with,
    /// where the line is reshaped up to them.
    ///
    /// The line is `line`, ending at `end`. Returns `None` where it ends with
    /// no such space, the text was not shaped across it, or the line keeps
    /// the paragraph's glyphs. Then its last letter stays kerned against the
    /// space.
    ///
    /// Collapsible spaces go, and Chrome removes them before it shapes a
    /// paragraph's end. Elsewhere, as Chrome's `TruncateLineEndResult`
    /// decides, the line is reshaped only where it needs its exact end
    /// ([`Breaker::ends_without_spaces`]). Spaces that hang but stay keep
    /// the paragraph's glyphs, and the text before them is reshaped as
    /// `ShapingLineBreaker` ends it there: only where the line needs its
    /// exact end, and only where the text's item goes on past the break,
    /// since Chrome breaks no item that fits whole.
    pub(super) fn removed_end(&self, line: &LineStart<'_>, end: ClusterId) -> Option<ClusterId> {
        let start = line.start;
        let mut at = end;
        let mut spaces = false;
        while at > start {
            let last = ClusterId::new(at.get() - 1);
            match self.clusters.attrs(last).map(ClusterAttrs::class) {
                Some(ClusterClass::Space) => spaces = true,
                Some(ClusterClass::BreakOpportunity) => {}
                _ => break,
            }
            at = last;
        }
        // Cheapest test first: finding a space's text takes a seek.
        let removed = spaces
            && at > start
            && self.stages.shaped.glyphs.word(at).is_unsafe_to_break()
            && self.ends_without_spaces(line.para, at, end);
        removed.then_some(at)
    }

    /// Returns whether a line ending at `end`, after a space the text was
    /// shaped across, is reshaped up to `end`.
    ///
    /// Chrome's `ShapingLineBreaker` ends the text before spaces that hang,
    /// so they keep the paragraph's glyphs. `TruncateLineEndResult` then
    /// reshapes them only from a safe break inside them, past their first
    /// space. Under `break-spaces` the spaces end the text and are reshaped
    /// as `SetDontReshapeEndIfAtSpace` allows. Either reshape happens only
    /// where the line needs its exact end. A tab ends its text, and its line
    /// is not reshaped.
    ///
    /// Chrome 155, Times New Roman at 20px, whose space is 3.91px kerned
    /// against an `A` and 5px alone: `XX AXX` in 60px keeps its space at
    /// 3.91px under `pre-wrap` at every alignment and with a background,
    /// and reshapes it to 5px under `break-spaces` at `center` or with a
    /// background. `XX  AXX` under `pre-wrap` in 62px reshapes its second
    /// space at `center` or with a background.
    #[cold]
    #[inline(never)]
    pub(super) fn space_break_reshaped(&self, line: &LineStart<'_>, end: ClusterId) -> bool {
        let Some(last) = end.get().checked_sub(1).map(ClusterId::new) else {
            return false;
        };
        let Some(attrs) = self.clusters.attrs(last) else {
            return false;
        };
        if attrs.class() != ClusterClass::Space {
            return false;
        }
        if attrs.has(ClusterAttrs::HANGS) {
            let mut first = last;
            while first > line.start
                && self.clusters.class(ClusterId::new(first.get() - 1)) == Some(ClusterClass::Space)
            {
                first = ClusterId::new(first.get() - 1);
            }
            if self.stages.shaped.glyphs.prev_safe_to_break(end, first) <= first {
                return false;
            }
        }
        line.para.exact_end
            || self
                .item_row(last..end)
                .is_some_and(|(node, _)| self.style_needs_exact_end(node))
    }

    /// Returns the node of the item holding the first of `clusters`, and
    /// where its clusters end, no further than the range's end.
    fn item_row(&self, clusters: Range<ClusterId>) -> Option<(NodeId, ClusterId)> {
        let first = Segments::new(&self.stages, clusters).find_map(|step| match step {
            Step::Segment(segment) => Some(segment),
            Step::Item { .. } => None,
        })?;
        let item = first.item_row(&self.stages)?;
        Some((item.node, first.end))
    }

    /// Returns whether a line ending at `end` with spaces from `at` is
    /// reshaped without them.
    ///
    /// Collapsible spaces at a paragraph's end always are. Elsewhere the line
    /// must need its exact end, Chrome's `NeedsAccurateEndPosition` as
    /// `TruncateLineEndResult` and `ShapingLineBreaker` ask it: the alignment
    /// says so, or the text's style paints a background, border or box
    /// shadow, or has decorations, its own or its ancestors'. And the item
    /// holding the text before `at` must go on past the first space where
    /// the spaces collapse, since Chrome drops a space in a node of its own
    /// without a reshape, or past `end` where they stay, since Chrome breaks
    /// no item that fits whole.
    ///
    /// Chrome 155, `XRAY bravo` in Times New Roman at 20px, 90px wide, its
    /// `Y` 0.75px narrower kerned against the space: the `Y` is reshaped
    /// with a background, a border or a box shadow on the block or on a span
    /// holding `XRAY `, with an underline on the block or on a span around
    /// both, and at `center`. It stays kerned at `start`, with emphasis
    /// marks or an outline, with a background on a span holding only
    /// `XRAY`, and at `center` with the space in a span of its own. Under
    /// `pre-wrap` it is the same, and at `center` the `Y` stays kerned also
    /// where `bravo` is in a span of its own.
    fn ends_without_spaces(&self, para: &BreakerParagraph, at: ClusterId, end: ClusterId) -> bool {
        let Some(before) = at.get().checked_sub(1).map(ClusterId::new) else {
            return false;
        };
        if end >= para.end {
            return self.end_spaces(at..end) == EndSpaces::Collapse;
        }
        // The walk cuts at every item, so a segment past `end` is one item.
        let reach = ClusterId::new(end.get() + 1);
        let Some((node, item_end)) = self.item_row(before..reach) else {
            return false;
        };
        if !para.exact_end && !self.style_needs_exact_end(node) {
            return false;
        }
        match self.end_spaces(at..end) {
            EndSpaces::Collapse => item_end > at,
            EndSpaces::Hang => item_end > end,
            EndSpaces::Kept => false,
        }
    }

    /// Returns whether the text of `node` needs its line's exact end by its
    /// style, as Chrome's `NeedsAccurateEndPosition` reads it.
    ///
    /// Its container paints a background, border or box shadow there, or
    /// decorates its text, or a box around it or the block does. An outline
    /// does not count.
    fn style_needs_exact_end(&self, node: NodeId) -> bool {
        let stages = &self.stages;
        let content = stages.content;
        let (nodes, variant) = (&content.nodes, stages.variant);
        let (block_paints, block_decorates) = content.block.style_paints(variant);
        if block_decorates {
            return true;
        }
        // A text node's style is its container's.
        let container = match nodes.kind(node) {
            Some(NodeKind::Text) => nodes.parent(node),
            _ => node,
        };
        if container == NodeId::BLOCK {
            return block_paints;
        }
        let box_has = |node, flag| {
            content
                .facts
                .box_facts(nodes.box_facts(node, variant))
                .has(flag)
        };
        if box_has(container, BoxFlags::PAINTS) {
            return true;
        }
        if !content.flags.contains(ContentFlags::DECORATED_BOXES) {
            return false;
        }
        // Nodes are in pre-order, so a parent comes first and the walk ends
        // at the block, which is its own parent.
        let mut node = container;
        for _ in 0..nodes.len() {
            if box_has(node, BoxFlags::DECORATES) {
                return true;
            }
            let parent = nodes.parent(node);
            if parent == node {
                break;
            }
            node = parent;
        }
        false
    }

    /// Returns how the spaces in `clusters` end a line that breaks after them.
    ///
    /// Returns [`EndSpaces::Kept`] where a row is missing.
    pub(super) fn end_spaces(&self, clusters: Range<ClusterId>) -> EndSpaces {
        let stages = &self.stages;
        let facts = &stages.content.facts;
        let mut walked = clusters.start;
        let mut found = None;
        for step in Segments::new(&self.stages, clusters.clone()) {
            let Step::Segment(segment) = step else {
                continue;
            };
            walked = segment.end;
            let Some(text) = segment.text_facts(stages) else {
                return EndSpaces::Kept;
            };
            let kind = if facts.text(text).collapse.collapses_spaces() {
                EndSpaces::Collapse
            } else {
                EndSpaces::Hang
            };
            for cluster in (segment.start..segment.end).ids() {
                let Some(attrs) = self.clusters.attrs(cluster) else {
                    return EndSpaces::Kept;
                };
                if attrs.class() != ClusterClass::Space {
                    continue;
                }
                if !attrs.has(ClusterAttrs::HANGS) || found.is_some_and(|found| found != kind) {
                    return EndSpaces::Kept;
                }
                found = Some(kind);
            }
        }
        if walked < clusters.end {
            return EndSpaces::Kept;
        }
        found.unwrap_or(EndSpaces::Collapse)
    }
}

/// How the spaces ending a line end it, as [`Breaker::end_spaces`] finds.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum EndSpaces {
    /// They collapse and hang, so the line drops them.
    Collapse,
    /// They are kept, and hang.
    Hang,
    /// Some do not hang, or some collapse and some are kept.
    Kept,
}

/// How [`Breaker::edge_pieces`] reshapes a line's end.
#[derive(Copy, Clone, Debug)]
pub(super) struct EdgeEnd {
    /// The shaping edges where the line becomes one piece.
    pub(super) whole: ShapingEdges,
    /// Whether the end must be reshaped: shaped across, spaces removed, or
    /// closing mark trimmed.
    pub(super) reshaped: bool,
    /// Whether the end trims its closing mark.
    pub(super) trims: bool,
    /// The seam room the end gives back, or zero.
    pub(super) seam: InlineLayoutUnit,
    /// Whether a line ending at its start window's end is one piece.
    pub(super) whole_at_window_end: bool,
}

/// The reshape of a line's end, as [`Breaker::end_piece`] finds it.
enum EndPiece {
    /// A piece in the edge tables.
    Piece(ReshapedPiece),
    /// No piece may start at the start window's end; the whole line is reshaped
    /// in this run.
    Whole(ShapedRunId),
    /// Nothing could be shaped; the line keeps the paragraph's glyphs.
    None,
}
