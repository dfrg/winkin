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
use crate::stages::fonts::Fonts;
use crate::stages::measure::Extent;
use crate::style::FirstLineVariant;
use crate::unit::LayoutUnit;

/// A block's `text-box-trim`: its trim edges and which ends it trims.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct TextBoxTrims {
    /// The over edge's distance over the baseline and the under edge's
    /// under it, in the standard variant's font and the first line's.
    edges: [(LayoutUnit, LayoutUnit); 2],
    /// Whether it trims the block's start.
    starts: bool,
    /// Whether it trims the block's end.
    pub(super) ends: bool,
    /// Whether the lines flip, so the start is their under side.
    flipped: bool,
}

impl TextBoxTrims {
    /// Returns the trims of `content`'s block in `fonts`.
    ///
    /// The edges are measured in the block's font, and on the first line in
    /// its `::first-line` font, as Chrome's `ApplyTextBoxTrim` reads the
    /// line's style. Returns `None` where the block trims nothing or has no
    /// font, as in content never built.
    pub(super) fn new(content: &Content, fonts: &Fonts) -> Option<Self> {
        let block = content.block;
        let (starts, ends) = block.text_box_trim.sides();
        if !starts && !ends {
            return None;
        }
        // The block's font: the primary font of the block node's text.
        let root = NodeId::BLOCK;
        content.nodes.kind(root)?;
        let edge = block.text_box_edge;
        let edges = |variant| {
            let text = content.nodes.text_facts(root, variant);
            let request = content.facts.text_request(text);
            let metrics = &fonts.primary_font(request)?.metrics;
            Some((metrics.over_edge(edge), metrics.under_edge(edge)))
        };
        let standard = edges(FirstLineVariant::Standard)?;
        let first = edges(FirstLineVariant::FirstLine).unwrap_or(standard);
        Some(Self {
            edges: [standard, first],
            starts,
            ends,
            flipped: block.writing_mode.flips_lines(),
        })
    }

    /// Returns how far every line moves up, given the first line's extent,
    /// `text-fit` scale and variant.
    ///
    /// It is what the line box reaches past the start edge, or zero where
    /// the block does not trim its start. The edges scale with the text, as
    /// Chrome scales the line's edge metrics.
    pub(super) fn start(self, first: Extent, scale: f32, variant: FirstLineVariant) -> LayoutUnit {
        let (over, under) = self.edges(variant);
        match (self.starts, self.flipped) {
            (false, _) => LayoutUnit::ZERO,
            (true, false) => first.zero_if_none().ascent() - scaled(over, scale),
            (true, true) => first.zero_if_none().descent() - scaled(under, scale),
        }
    }

    /// Returns how far the block's end moves up, given the last line's
    /// extent, `text-fit` scale and variant.
    ///
    /// It is what the line box reaches past the end edge, or zero where the
    /// block does not trim its end.
    pub(super) fn end(self, last: Extent, scale: f32, variant: FirstLineVariant) -> LayoutUnit {
        let (over, under) = self.edges(variant);
        match (self.ends, self.flipped) {
            (false, _) => LayoutUnit::ZERO,
            (true, false) => last.zero_if_none().descent() - scaled(under, scale),
            (true, true) => last.zero_if_none().ascent() - scaled(over, scale),
        }
    }

    /// Returns the over and under edges in `variant`'s font.
    fn edges(self, variant: FirstLineVariant) -> (LayoutUnit, LayoutUnit) {
        match variant {
            FirstLineVariant::Standard => self.edges[0],
            FirstLineVariant::FirstLine => self.edges[1],
        }
    }
}

/// Returns `edge`, an edge's distance from the baseline, scaled by the
/// `text-fit` scale `scale` and truncated onto the grid.
fn scaled(edge: LayoutUnit, scale: f32) -> LayoutUnit {
    if scale == 1.0 {
        edge
    } else {
        edge.times_truncated(scale)
    }
}
