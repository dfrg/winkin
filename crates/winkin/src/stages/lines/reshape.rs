//! Shaping a line's unsafe edges again, as more of [`Breaker`]'s `impl`.
//!
//! **Unsafe edges.** A line starting at a cluster the font shaped across is
//! reshaped up to the next safe-to-break cluster, at most to its shaping
//! run's end, as Chrome's `ShapingLineBreaker` does.
//! - A line ending at one is reshaped from the last safe start before it.
//!   The piece widens where its own start may not join, but never past its
//!   shaping run's start.
//! - A candidate is measured with its pieces. A reshape that makes it too
//!   wide sends the search to the opportunity before, as Chrome's
//!   `ShapingLineBreaker::ShapeLine` does.
//! - A break at a space is not reshaped at the line's end unless alignment
//!   needs the exact end (Chrome's `SetDontReshapeEndIfAtSpace`).
//!
//! **A seam's room given back** (`text-autospace`). A line ending where
//! `text-autospace` puts room before the next cluster gives the room back.
//! Its last cluster becomes a piece copied from the paragraph's glyphs, with
//! the room taken off its advance. A reshaped end simply adds no room after
//! it. This matches Chrome's `UnapplyAutoSpacing` and
//! `AdjustOffsetForAutoSpacing`.
//!
//! **Punctuation trimmed at a line's edges** (`text-spacing-trim`). A line
//! reshaped at its start is shaped as a start, so an opening mark is not
//! kerned against the line before. Under `space-first` and `trim-start`, a
//! line wrapping to an opening mark is always reshaped there, with the mark
//! trimmed, as Chrome's `FirstSafeOffset` does. A closing mark that fits
//! only trimmed ends the line trimmed, where a break after it is allowed, as
//! Chrome's `ShapeLine` does.

use core::ops::Range;

use super::fit::ReshapedStart;
use super::{
    Breaker, BreakerParagraph, EdgeShape, LineHead, LineStart, Lines, ReshapedPiece,
    ReshapedPieces, StartWindow,
};
use crate::data::Id;
use crate::data::IdRange;
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId};
use crate::stages::content::ContentFlags;
use crate::stages::measure::LetterWordSpacing;
use crate::stages::shape::{
    NeighbourFonts, ShapedFlags, ShapedRunId, ShapingEdges, ShapingKey, ShapingSource,
    maybe_closing_mark, maybe_opening_mark, shape_range,
};
use crate::stages::{Segments, Step};
use crate::style::TextSpacingTrim;
use crate::unit::{InlineLayoutUnit, TextUnit};

impl<'a: 'c, 'c, 'm> Breaker<'a, 'c, 'm> {
    /// Reshapes the window of a line starting at the unsafe `start`, up to
    /// the next safe-to-break cluster within its shaping run.
    ///
    /// The window keeps the run, which the line's later reshapes from its
    /// start are shaped in.
    ///
    /// Chrome's `ShapingLineBreaker` reshapes a line's start the same way.
    /// The piece stays in the edge tables, first on the line, for the
    /// candidates to share.
    ///
    /// Nothing checks that the piece joins the paragraph's glyphs at its end,
    /// as in Chrome. HarfBuzz promises the glyphs either side of a safe break.
    /// A check would cost a reshape per window and changes no test, page,
    /// bench document or sample.
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
            after: out.edges.mark(),
        };
        Some((window, head))
    }

    /// Returns the reshape window of a line starting at `start`, and the
    /// line's head in the edge tables, with the window's piece.
    ///
    /// A start the font shaped across is reshaped. So is a wrapped line's
    /// opening mark its style trims, even where the font did not shape
    /// across it. Every fit of the line shares the piece.
    ///
    /// It is inlined into each line's layout. Out of line, the copy of what
    /// it returns stalls on the stores just made, and relayout is up to 8%
    /// slower.
    #[inline(always)]
    pub(super) fn line_head(
        &mut self,
        out: &mut Lines,
        para: &BreakerParagraph,
        start: ClusterId,
    ) -> (Option<StartWindow>, LineHead) {
        let mark = out.edges.mark();
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
        let (window, head) = if shaped_across || trims {
            self.start_window(out, para, start, trims).unzip()
        } else {
            (None, None)
        };
        (window, LineHead { mark, shaped: head })
    }

    /// Returns the piece of the start window, `window`, of a line from
    /// `start`, shaping it again if a one-piece line took it back from
    /// `head`.
    ///
    /// A newly shaped piece stays first in the edge tables for the later
    /// candidates.
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
        out.edges.rewind(head.mark);
        let range = start..window.end;
        let edges = StartWindow::edges(window.trims);
        let piece = self.reshape(out, para, window.run, range, edges, false);
        head.shaped = Some(ReshapedStart {
            piece,
            after: out.edges.mark(),
        });
        piece
    }

    /// Reshapes the edges of `line`, a line's clusters from its start to
    /// its shaped end, and returns its pieces.
    ///
    /// The start's `window` has its piece in the edge tables after `head`'s
    /// mark. A line ending inside the window is one piece, in place of the
    /// window's. Otherwise the window's piece leads, and the end is
    /// reshaped as `end` says. Where the end's piece may not start after
    /// the window, the line is one piece. A line with no window reshapes
    /// only its end.
    ///
    /// It is inlined into each candidate's measure. Out of line, the copy of
    /// the pieces it returns stalls on the stores just made, and relayout
    /// is up to 8% slower.
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
            out.edges.rewind(head.mark);
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
                out.edges.rewind(head.mark);
                head.shaped = None;
                pieces = ReshapedPieces::default();
                pieces.push(self.reshape(out, para, run, line, end.whole, true));
            }
        }
        pieces
    }

    /// Reshapes the end of a line ending at the unsafe `end`, from the last
    /// safe-to-break cluster before it.
    ///
    /// The piece starts no earlier than `floor`, nor before the shaping run
    /// of its last cluster. It widens where the shaper says its own start
    /// may not join what is before it.
    ///
    /// `floor` is the line's start, where a piece may start however it
    /// shapes, or the end of the start window. There, a piece that cannot
    /// start makes the line one piece ([`EndPiece::Whole`]).
    ///
    /// A run's start is where the paragraph was shaped apart, so a piece
    /// starting there joins exactly as the run did, whatever the shaper
    /// says. A piece reaching past it would shape earlier runs in this
    /// run's font and style.
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
            let mark = out.edges.mark();
            // A piece at the line's start is shaped as a start. Its end trims
            // its closing mark where asked.
            let edges = ShapingEdges {
                line_start: floor_is_start && from == floor && from > para.start,
                trim_start: false,
                trim_end: trims_end,
            };
            let Some(piece) = self.reshape(out, para, run, from..end, edges, true) else {
                return EndPiece::None;
            };
            if !piece.start_unsafe || (from == floor && floor_is_start) || run_start == from {
                return EndPiece::Piece(piece);
            }
            out.edges.rewind(mark);
            if from == floor {
                return EndPiece::Whole(run);
            }
            from = glyphs.prev_safe_to_break(from, lowest);
        }
    }

    /// Shapes `range` of `para` on its own into the edge tables, as its run
    /// was shaped, with the paragraph either side as context.
    ///
    /// `id` names the shaping run holding the whole range. `edges` says how
    /// its ends are shaped. `ends_line` says it ends the line, which gives
    /// back a seam's room after it.
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
        // The fonts of the runs beside, where the piece reaches its run's
        // edges.
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
        let mark = out.edges.mark();
        let (start_unsafe, written) = {
            let mut sink = out.edges.sink();
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
        // The piece's entries in the edge tables: a word and an advance per
        // cluster.
        let entries = mark.clusters..out.edges.words.next_id();
        if written != count {
            // A table could not take the piece, so the line is measured with
            // the paragraph's glyphs.
            out.edges.rewind(mark);
            return None;
        }
        let seams = para.seams;
        let letter_or_word = content.flags.contains(ContentFlags::NONZERO_SPACING);
        if letter_or_word || seams {
            // Add the prefix's spacing to the piece's own advances, as Chrome
            // spaces a reshaped result. A ligature that differs between the
            // paragraph and the piece is spaced as the piece has it.
            // Add a seam's room too, except after the last cluster of a piece
            // that ends a line. The piece stays in one run, so one spacing
            // rule covers it.
            let spaced = LetterWordSpacing::from_shaping(&content.facts, run.shaping, self.words);
            let paragraph = analysis.paragraphs.get(para.id);
            let source = self.stages.text();
            let words = out.edges.words.get_slice(entries.clone());
            let advances = out.edges.advances.get_slice_mut(entries.clone());
            // Where the walk over the piece stands, for its seams.
            let mut near = None;
            if let (Some(words), Some(advances)) = (words, advances) {
                for (i, (word, advance)) in words.iter().zip(advances.iter_mut()).enumerate() {
                    let cluster = ClusterId::new(range.start.get() + i);
                    let mut spacing = spaced
                        .after_cluster(analysis, source, cluster, word.is_continuation())
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
        let advance = out.edges.advance_sum(entries);
        // No box edge with room stands inside a shaping run, so the
        // paragraph's advance over the piece is the prefix's step, spacing
        // included.
        let paragraph = stages.measured.prefix.advance(range.start, range.end);
        Some(ReshapedPiece {
            shape: EdgeShape::new(range, mark.clusters),
            delta: advance - paragraph,
            start_unsafe,
        })
    }

    /// Returns the `text-spacing-trim` of the text holding `cluster`, from
    /// its shaping run's facts.
    fn trim(&self, cluster: ClusterId) -> Option<TextSpacingTrim> {
        let stages = &self.stages;
        let run = stages.shaped.runs.run_containing(cluster)?;
        Some(stages.content.facts.shaping(run.shaping).trim)
    }

    /// Returns whether a wrapped line starting at `start` may trim its
    /// opening mark.
    ///
    /// The text must say `space-first` or `trim-start` (Blink's
    /// `ShouldTrimStartOfWrappedLine`). The character must be a possible
    /// opening mark (`MaybeHanKerningOpen`). The reshape finds whether the
    /// font trims it.
    pub(super) fn trims_start(&self, start: ClusterId) -> bool {
        self.trim(start)
            .is_some_and(TextSpacingTrim::trims_wrapped_start)
            && self
                .clusters
                .first_char(self.stages.text(), start)
                .is_some_and(maybe_opening_mark)
    }

    /// Returns whether a line may end after the closing mark `mark`,
    /// trimmed.
    ///
    /// The text must trim (Blink's `ShouldTrimEnd`), the character must be
    /// a possible closing mark (`MaybeHanKerningClose`), and a break after
    /// it must be allowed. The break is checked first, since it is one byte
    /// and prose rarely offers one there.
    pub(super) fn trims_end(&self, para: &BreakerParagraph, mark: ClusterId) -> bool {
        mark < para.end
            && self
                .clusters
                .attrs(mark)
                .is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER))
            && self
                .stages
                .measured
                .ruby_columns()
                .interior(ClusterId::new(mark.get() + 1), &self.column)
                .is_none()
            && self
                .clusters
                .first_char(self.stages.text(), mark)
                .is_some_and(maybe_closing_mark)
            && self
                .trim(mark)
                .is_some_and(TextSpacingTrim::trims_punctuation)
    }

    /// Returns the room `text-autospace` puts before boundary `end`, which a
    /// line ending there gives back.
    ///
    /// It is zero at the paragraph's ends.
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

    /// Returns the last cluster of a line ending at `end` as its own piece,
    /// with a seam's `room` taken off its advance.
    ///
    /// The glyphs are copied, not reshaped, as Chrome's `UnapplyAutoSpacing`
    /// copies its line's last glyph.
    fn given_back(
        &mut self,
        out: &mut Lines,
        end: ClusterId,
        room: InlineLayoutUnit,
    ) -> Option<ReshapedPiece> {
        let cluster = ClusterId::new(end.get().checked_sub(1)?);
        let mark = out.edges.mark();
        let own = self.stages.cluster_advance(cluster);
        let advance = own - room;
        let copied = {
            let mut sink = out.edges.sink();
            sink.copy(&self.stages.shaped.glyphs, cluster, advance.raw());
            sink.dropped() == 0
        };
        if !copied || out.edges.words.len() != mark.clusters.get() + 1 {
            out.edges.rewind(mark);
            return None;
        }
        Some(ReshapedPiece {
            shape: EdgeShape::new(cluster..end, mark.clusters),
            delta: InlineLayoutUnit::ZERO - room,
            start_unsafe: false,
        })
    }

    /// Returns where a line's text ends without its trailing collapsible
    /// spaces, where the paragraph shaped across them.
    ///
    /// The line is `line`, ending at `end`. Returns `None` where it ends with
    /// no such space, or the text was not shaped across it. Then the line
    /// keeps the paragraph's glyphs.
    ///
    /// Chrome's `TruncateLineEndResult` reshapes here only where the line
    /// needs its exact end, for alignment, a background or a decoration.
    /// Elsewhere its last letter stays kerned against a removed space. CSS
    /// removes the space, and so does this, everywhere.
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
        // Checked cheapest first. Most lines end at a space their text was
        // not shaped across, and finding a space's text takes a seek.
        let removed = spaces
            && at > start
            && self.stages.shaped.glyphs.word(at).is_unsafe_to_break()
            && self.spaces_collapse(at..end);
        removed.then_some(at)
    }

    /// Returns whether every space in `clusters` collapses away at a break
    /// after it.
    ///
    /// Such a space hangs, and its text collapses white space. Returns
    /// `false` where the walk ends short, which happens only where a row is
    /// missing. The walk seeks the clusters' start.
    pub(super) fn spaces_collapse(&self, clusters: Range<ClusterId>) -> bool {
        let stages = &self.stages;
        let facts = &stages.content.facts;
        let mut walked = clusters.start;
        for step in Segments::new(&self.stages, clusters.clone()) {
            let Step::Segment(segment) = step else {
                continue;
            };
            walked = segment.end;
            let Some(text) = segment.text_facts(stages) else {
                return false;
            };
            let collapses = facts.text(text).collapse.collapses_spaces();
            let spaces = (segment.start..segment.end).ids().all(|cluster| {
                self.clusters.attrs(cluster).is_some_and(|attrs| {
                    attrs.class() != ClusterClass::Space
                        || (collapses && attrs.has(ClusterAttrs::HANGS))
                })
            });
            if !spaces {
                return false;
            }
        }
        walked >= clusters.end
    }
}

/// How [`Breaker::edge_pieces`] reshapes a line's end.
#[derive(Copy, Clone, Debug)]
pub(super) struct EdgeEnd {
    /// How the line is shaped where it becomes one piece.
    pub(super) whole: ShapingEdges,
    /// The end may not keep the paragraph's glyphs: the font shaped across
    /// it, its spaces are removed, or its closing mark is trimmed.
    pub(super) reshaped: bool,
    /// It trims its closing mark.
    pub(super) trims: bool,
    /// The room of a seam it gives back, zero where it gives none.
    pub(super) seam: InlineLayoutUnit,
    /// A line ending at its start window's end is one piece too.
    pub(super) whole_at_window_end: bool,
}

/// A line ending's reshape, as [`Breaker::end_piece`] finds it.
enum EndPiece {
    /// The piece, in the edge tables.
    Piece(ReshapedPiece),
    /// Its window reaches the line's start window and still may not join
    /// there, so the line becomes one piece, shaped in this run: the run
    /// holding the line's end, which holds its start too.
    Whole(ShapedRunId),
    /// Nothing could be shaped: the line keeps the paragraph's glyphs.
    None,
}
