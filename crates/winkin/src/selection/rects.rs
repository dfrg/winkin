//! The rectangles a selection paints, as Blink's `LayoutSelection` paints them.
//!
//! - One rectangle per item, over what it selects. A run's spans from its
//!   first selected cluster's edge to its last's, sharing a ligature evenly,
//!   as `InlineCursor::CurrentLocalSelectionRectForText` does.
//! - An atomic inline is selected whole or not at all. A line's hyphen is
//!   selected with the soft hyphen before it.
//! - An ellipsis and the text it hides paint nothing, as in Chrome. Neither
//!   does a box's padding. Text in each direction is its own run, so its own
//!   rectangle.
//! - A rectangle is as tall as the line box, or the text's own box where that
//!   is taller, as `ExpandSelectionRectToLineHeight` unites them.
//! - Where the selection starts before a line's placed end and goes on past
//!   it, a mark one space of the last run's font wide follows the line, as
//!   `ExpandedSelectionRectForSoftLineBreakIfNeeded` adds. There is no mark
//!   in a block that cuts lines for an ellipsis or clamp, or where the last
//!   run reads against the paragraph.
//!
//! The rectangles are line-relative. A host unites or paints them as it
//! likes, typically tinting an atomic inline's, as Chrome tints a replaced
//! element.

use core::iter::once;
use core::ops::Range;

use super::place::{self, ClusteredPosition, Leaf};
use super::{Affinity, Position};
use crate::data::{Id, TextOffset};
use crate::layout::Layout;
use crate::layout::{AnnotationRuns, CrossExtents, Item, Line, LineItems};
use crate::stages::analysis::ClusterId;
use crate::stages::content::NodeId;
use crate::stages::fonts::Generated;
use crate::stages::lines::{InlineExtents, LineId};
use crate::style::FirstLineVariant;
use crate::unit::InlineLayoutUnit;

/// A selection rectangle relative to a line box.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct SelectionRect {
    /// The zero-based line index.
    pub line: usize,
    /// Inline extents relative to line-box left.
    pub inline: InlineExtents,
    /// Cross-line extents relative to the line-over edge.
    pub block: CrossExtents,
    /// The selected content kind.
    pub kind: SelectionRectKind,
}

/// The content covered by a selection rectangle.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SelectionRectKind {
    /// Text of a run, or a line's hyphen.
    Text,
    /// The mark at the end of a line, where the selection continues past it.
    ///
    /// One space wide in the last font of the line.
    LineEnd,
    /// An atomic inline, whole.
    ///
    /// Chrome tints the box of a replaced element. Chrome does not tint
    /// inline-blocks; their text is selected instead.
    Atomic,
}

/// The rectangles a selection paints, for [`Layout::selection_rects`](crate::Layout::selection_rects).
///
/// They come line by line, in visual order on each, with the line-end mark
/// last.
pub(crate) struct SelectionRects<'a> {
    layout: &'a Layout,
    /// Where the selection starts and ends, as caret stops.
    start: usize,
    end: usize,
    /// The next line to read, and the last.
    next: LineId,
    last: Option<LineId>,
    /// The line being read, and its items still to read.
    current: Option<Reading<'a>>,
    annotation: Option<AnnotationRuns<'a>>,
}

/// A line the rectangles are being read from.
struct Reading<'a> {
    line: Line<'a>,
    /// The line box's height, which each rectangle is at least as tall as.
    height: f32,
    /// Its items still to read.
    items: LineItems<'a>,
}

impl<'a> SelectionRects<'a> {
    /// Starts the rectangles of a selection from byte `start` to `end`.
    ///
    /// It snaps both, and covers the first line the selection touches to the
    /// last, including the lines of selected ruby pieces.
    pub(crate) fn new(layout: &'a Layout, start: usize, end: usize) -> Self {
        let clusters = &layout.analysis().clusters;
        let first = place::snap(
            layout,
            ClusteredPosition::new(layout, Position::from(start)),
        );
        let start = first.offset();
        // The end walks from the start, as the rectangles walk the lines
        // between them.
        let end = Position::from(end.max(start));
        let ending = place::snap(
            layout,
            ClusteredPosition::from_walk(layout, first.cluster, end),
        );
        let end = ending.offset();
        let (next, last) = if start < end {
            let first = place::line(layout, first, None);
            let upstream = ClusteredPosition {
                position: Position::new(end, Affinity::Upstream),
                cluster: ending.cluster,
            };
            let last = place::line(layout, upstream, first);
            (first.unwrap_or(LineId::new(0)), last)
        } else {
            (LineId::new(0), None)
        };
        let (mut next, mut last) = (next, last);
        let lines = layout.line_records();
        if let Some(ruby) = &lines.ruby
            && end > 0
        {
            // A piece is selected where one of its ranges ends past the
            // cluster holding `start` and starts before `end`.
            let from = first.cluster;
            let to = ClusteredPosition::from_walk(layout, ending.cluster, Position::from(end - 1))
                .cluster;
            let columns = layout
                .measured()
                .text(FirstLineVariant::Standard)
                .ruby_columns();
            let around = ruby.around(lines, columns, from..ClusterId::new(to.get() + 1));
            for piece in around {
                let selected = once(piece.base.clone())
                    .chain(
                        ruby.levels
                            .get_slice(piece.levels.clone())
                            .unwrap_or_default()
                            .iter()
                            .map(|level| level.clusters.clone()),
                    )
                    .any(|range| {
                        start < clusters.start(range.end).get()
                            && clusters.start(range.start).get() < end
                    });
                if selected {
                    next = next.min(piece.line);
                    last = Some(last.map_or(piece.line, |last| last.max(piece.line)));
                }
            }
        }
        Self {
            layout,
            start,
            end,
            next,
            last,
            current: None,
            annotation: None,
        }
    }

    /// Returns the rectangle `item` of `line`, a line box `height` tall, paints, where it paints one.
    fn item_rect(&self, line: &Line<'a>, height: f32, item: Item<'a>) -> Option<SelectionRect> {
        let (start, end) = (self.start, self.end);
        let line_box = CrossExtents::from_line_height(height);
        let rect = |inline: InlineExtents, block: CrossExtents, kind| SelectionRect {
            line: line.index(),
            inline,
            block,
            kind,
        };
        match item {
            Item::Text(run) if !run.is_hidden() => {
                let range = run.text_range();
                let (from, to) = (start.max(range.start), end.min(range.end));
                if from >= to {
                    return None;
                }
                let clusters = &self.layout.analysis().clusters;
                let selected = |cluster: ClusterId| {
                    let range = clusters.range(cluster);
                    from <= range.start.get() && range.end.get() <= to
                };
                // Combined text spans the unit's em along the line and the
                // selected characters across it, not the line box. Chrome
                // paints a unit's horizontal text so.
                if let Some(places) = run.across_places() {
                    let (over, under) = union(
                        places
                            .filter(|&(cluster, _, _)| selected(cluster))
                            .map(|(_, start, finish)| (start.min(finish), start.max(finish))),
                    )?;
                    return Some(rect(
                        run.inline(),
                        CrossExtents { over, under },
                        SelectionRectKind::Text,
                    ));
                }
                let (left, right) = union(
                    run.places()
                        .filter(|&(cluster, _, _)| selected(cluster))
                        .map(|(_, left, right)| (left, right)),
                )?;
                let text = run.block();
                Some(rect(
                    InlineExtents::from_exact(left, right),
                    CrossExtents {
                        over: text.over.min(0.0),
                        under: text.under.max(height),
                    },
                    SelectionRectKind::Text,
                ))
            }
            // A hyphen is selected with the soft hyphen that ends at its
            // boundary. An ellipsis never is.
            Item::Generated(run) if run.generated() == Some(Generated::Hyphen) => {
                let at = run.text_range().start;
                (start < at && at <= end)
                    .then(|| rect(run.inline(), line_box, SelectionRectKind::Text))
            }
            Item::Atomic(atomic) => {
                let range = atomic.text_range();
                (start <= range.start && range.end <= end)
                    .then(|| rect(atomic.inline(), line_box, SelectionRectKind::Atomic))
            }
            Item::Text(_) | Item::Generated(_) | Item::Box(_) => None,
        }
    }

    /// Returns the mark at `line`'s end, where the selection starts before it and goes on past.
    fn line_end(&self, line: &Line<'a>) -> Option<SelectionRect> {
        if self.layout.content().block.may_cut_lines() {
            return None;
        }
        let (start, end) = (self.start, self.end);
        // A line ending in a forced break is marked where the break is
        // selected. A wrapped line is marked where the selection covers its
        // end and goes on past it.
        let breaks = self.forced_break(line);
        let marked = |at: usize| match breaks {
            Some(separator) => start <= separator.start.get() && separator.end.get() <= end,
            None => start < at && at < end,
        };
        // The logically last leaf the line placed.
        let last = place::leaves(*line).max_by_key(|leaf| leaf.end);
        let metrics = line.metrics();
        let rtl = line.level().is_rtl();
        let (space, edge) = match last {
            Some(leaf) => {
                let at = leaf.end;
                if leaf.is_rtl() != rtl || !marked(at) {
                    return None;
                }
                (
                    space_width(self.layout, &leaf),
                    leaf.caret_x(self.layout, at),
                )
            }
            // An empty line ends in its break.
            None => {
                if !marked(line.text_range().start) {
                    return None;
                }
                (
                    block_space(self.layout),
                    if rtl { metrics.width } else { 0.0 },
                )
            }
        };
        let inline = if rtl {
            InlineExtents {
                left: edge - space,
                right: edge,
            }
        } else {
            InlineExtents {
                left: edge,
                right: edge + space,
            }
        };
        Some(SelectionRect {
            line: line.index(),
            inline,
            block: CrossExtents::from_line_height(metrics.height()),
            kind: SelectionRectKind::LineEnd,
        })
    }
}

impl SelectionRects<'_> {
    /// Returns the forced break `line` ends in, as byte offsets into the layout's text.
    ///
    /// It is the line's last cluster, where that is a forced break.
    fn forced_break(&self, line: &Line<'_>) -> Option<Range<TextOffset>> {
        let clusters = &self.layout.analysis().clusters;
        let end = line.record().clusters().end;
        let last = ClusterId::new(end.get().checked_sub(1)?);
        let class = clusters.class(last)?;
        class.is_forced_break().then(|| clusters.range(last))
    }
}

/// Returns the union of `extents`, from the least start to the greatest end.
///
/// `None` where the union is empty.
fn union<T: Copy + PartialOrd>(extents: impl Iterator<Item = (T, T)>) -> Option<(T, T)> {
    let (low, high) = extents.reduce(|(low, high), (start, end)| {
        (
            if start < low { start } else { low },
            if end > high { end } else { high },
        )
    })?;
    (low < high).then_some((low, high))
}

/// Returns the width of a space in `leaf`'s font, or the block's for an atomic inline.
fn space_width(layout: &Layout, leaf: &Leaf<'_>) -> f32 {
    match leaf.run() {
        Some(run) => run.space_width().to_px(),
        None => block_space(layout),
    }
}

/// Returns the width of a space in the block's primary font.
fn block_space(layout: &Layout) -> f32 {
    let content = layout.content();
    let text = content
        .nodes
        .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
    layout
        .fonts()
        .primary_font(content.facts.text_request(text))
        .map_or(InlineLayoutUnit::ZERO, |used| {
            InlineLayoutUnit::from_text(used.metrics.space)
        })
        .to_px()
}

impl<'a> Iterator for SelectionRects<'a> {
    type Item = SelectionRect;

    fn next(&mut self) -> Option<SelectionRect> {
        loop {
            if let Some(mut reading) = self.current.take() {
                let (line, height) = (reading.line, reading.height);
                while let Some(item) = reading.items.next() {
                    if let Some(rect) = self.item_rect(&line, height, item) {
                        self.current = Some(reading);
                        return Some(rect);
                    }
                }
                if self.annotation.is_none() {
                    self.annotation = Some(line.annotation_runs());
                }
                while let Some(run) = self.annotation.as_mut().and_then(Iterator::next) {
                    if let Some(rect) = self.item_rect(&line, height, Item::Text(run)) {
                        self.current = Some(reading);
                        return Some(rect);
                    }
                }
                if let Some(mark) = self.line_end(&line) {
                    return Some(mark);
                }
                continue;
            }
            let last = self.last?;
            if self.next > last {
                return None;
            }
            let line = Line::new(self.layout, self.next)?;
            self.next = LineId::new(self.next.get() + 1);
            self.annotation = None;
            self.current = Some(Reading {
                line,
                height: line.metrics().height(),
                items: LineItems::new(&line, true),
            });
        }
    }
}
