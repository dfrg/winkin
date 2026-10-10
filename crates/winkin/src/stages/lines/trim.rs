//! What `text-box-trim` takes off a block's first and last lines.
//!
//! Breaking stacks the lines by these amounts, and the block result reports
//! them. The rules follow Chrome's `InlineLayoutAlgorithm::ApplyTextBoxTrim`
//! and `InlineBoxState::AdjustEdges`:
//! - The first line is trimmed by its ascent less the block's over edge.
//! - The last line is trimmed by its descent less the under edge.
//! - Each edge is measured from the baseline in the block's primary font.
//! - An amount may be negative. A line box shorter than its text, under a
//!   small `line-height`, moves the first line down and the block's end out.
//! - In `vertical-lr`, where lines flip, the start is a line's under side,
//!   as Chrome's `is_flipped_line` swaps them. `sideways-lr` trims as a
//!   horizontal block does, as Chrome 153 does.

use crate::stages::content::{Content, NodeId};
use crate::stages::fonts::{FontLineMetrics, Fonts};
use crate::stages::measure::Extent;
use crate::style::{FirstLineVariant, TextBoxEdge, TextEdgeOver, TextEdgeUnder};
use crate::unit::LayoutUnit;

/// A block's `text-box-trim`: its trim edges and which ends it trims.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct TextBoxTrims {
    /// How far the over edge stands over the baseline, and the under edge
    /// under it.
    over: LayoutUnit,
    under: LayoutUnit,
    starts: bool,
    /// Whether it trims the block's end.
    pub(super) ends: bool,
    /// Whether the lines flip, so the start is their under side.
    flipped: bool,
}

impl TextBoxTrims {
    /// Returns the trims of `content`'s block in `fonts`.
    ///
    /// Returns `None` where the block trims nothing or has no font, as in
    /// content never built.
    pub(super) fn new(content: &Content, fonts: &Fonts) -> Option<Self> {
        let block = content.block;
        let (starts, ends) = block.text_box_trim.sides();
        if !starts && !ends {
            return None;
        }
        // The block's own font: the primary of node 0's text.
        let root = NodeId::BLOCK;
        content.nodes.kind(root)?;
        let text = content.nodes.text_facts(root, FirstLineVariant::Standard);
        let request = content.facts.text_request(text);
        let metrics = &fonts.primary_font(request)?.metrics;
        let edge = block.text_box_edge;
        Some(Self {
            over: over_edge(edge, metrics),
            under: under_edge(edge, metrics),
            starts,
            ends,
            flipped: block.writing_mode.flips_lines(),
        })
    }

    /// Returns how far every line moves up, given the first line's extent.
    ///
    /// It is what the line box reaches past the start edge, or zero where
    /// the block does not trim its start.
    pub(super) fn start(self, first: Extent) -> LayoutUnit {
        match (self.starts, self.flipped) {
            (false, _) => LayoutUnit::ZERO,
            (true, false) => first.zero_if_none().ascent() - self.over,
            (true, true) => first.zero_if_none().descent() - self.under,
        }
    }

    /// Returns how far the block's end moves up, given the last line's
    /// extent.
    ///
    /// It is what the line box reaches past the end edge, or zero where the
    /// block does not trim its end.
    pub(super) fn end(self, last: Extent) -> LayoutUnit {
        match (self.ends, self.flipped) {
            (false, _) => LayoutUnit::ZERO,
            (true, false) => last.zero_if_none().descent() - self.under,
            (true, true) => last.zero_if_none().ascent() - self.over,
        }
    }
}

/// Returns how far `edge`'s over edge stands over the lines' baseline.
///
/// - `text`, `cap` and `ex` give the ascent, cap height and x-height, as
///   Chrome's `AdjustEdges` reads `FixedAscent`, `FixedCapHeight` and
///   `FixedXHeight`.
/// - Beyond Chrome, which parses neither, `ideographic` gives the top of the
///   ideographic em box. `ideographic-ink` gives the same, since font
///   metrics carry no character face.
///
/// Cap height and x-height stand over the alphabetic baseline. In a
/// vertical line set on its central baseline, the alphabetic offset comes
/// off them. Chrome 153 trims a 40 px line of 20 px Ahem to 30 at its start
/// and 26 at its end under `cap alphabetic` in `vertical-rl`.
fn over_edge(edge: TextBoxEdge, metrics: &FontLineMetrics) -> LayoutUnit {
    let alphabetic = metrics.alphabetic;
    match edge.over {
        TextEdgeOver::Text => metrics.ascent,
        TextEdgeOver::Cap => metrics.cap_height - alphabetic,
        TextEdgeOver::Ex => metrics.x_height - alphabetic,
        // The ideographic baseline is already an offset from the lines'.
        TextEdgeOver::Ideographic | TextEdgeOver::IdeographicInk => {
            metrics.em_over + metrics.em_under - metrics.ideographic
        }
    }
}

/// Returns how far `edge`'s under edge stands under the lines' baseline.
///
/// `text` gives the descent and `alphabetic` the alphabetic baseline, as
/// Chrome's `AdjustEdges` reads them. Beyond Chrome, `ideographic` and
/// `ideographic-ink` give the bottom of the ideographic em box.
fn under_edge(edge: TextBoxEdge, metrics: &FontLineMetrics) -> LayoutUnit {
    match edge.under {
        TextEdgeUnder::Text => metrics.descent,
        TextEdgeUnder::Alphabetic => metrics.alphabetic,
        TextEdgeUnder::Ideographic | TextEdgeUnder::IdeographicInk => metrics.ideographic,
    }
}
