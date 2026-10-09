//! The pipeline, in the order it runs, and borrowed views of the prepared
//! data.
//!
//! Building writes the content. `LayoutBuilder::finish` then runs the
//! preparation stages, analysis to measurement, which do not depend on the
//! width. `Layout::break_lines` runs breaking and line layout for one width.
//! `layout/mod.rs` holds both orchestrators.
//!
//! [`Stages`] borrows every prepared table. [`LineStages`] picks the ordinary
//! or first-line version for one line.
//!
//! A line is set in the text's styles or, for the block's first line, in
//! those `::first-line` gives. The variant decides the line's shaping,
//! measurements and styles. Its clusters, items and fonts are the text's in
//! either case. So the variant is resolved once per line, not at every
//! question about the line.
//!
//! `segments` holds [`Segments`], the walk over the clusters in reading
//! order, segment by segment.

// The text, nodes, items and style facts the builder writes.
pub(crate) mod content;
// Grapheme clusters, break opportunities, paragraphs, and script, language,
// bidi and orientation runs.
pub(crate) mod analysis;
// The font each cluster is drawn in, and the used fonts.
pub(crate) mod fonts;
// Each run's shaped glyphs and advances.
pub(crate) mod shape;
// Measurements that hold at every width: advance prefixes, intrinsic sizes,
// item extents and line-edge costs.
pub(crate) mod measure;
// The lines chosen for one width, and the block they make.
pub(crate) mod lines;
// Line layout: each line's items, placed and in visual order.
pub(crate) mod fragments;

// The walk over clusters most of the stages after measurement make.
mod segments;

pub(crate) use segments::{Segment, Segments, Slot, Step};

use crate::style::FirstLineVariant;
use analysis::Analysis;
use content::{Content, NodeId, TextFactsId, VariantText};
use fonts::Fonts;
use measure::{Measured, MeasuredText};
use shape::{Shaped, ShapedText};

/// Every prepared table, borrowed whole.
///
/// The tables are frozen once built, so a copy reads what the layout holds.
#[derive(Copy, Clone)]
pub(crate) struct Stages<'a> {
    pub(crate) content: &'a Content,
    pub(crate) analysis: &'a Analysis,
    pub(crate) fonts: &'a Fonts,
    pub(crate) shaped: &'a Shaped,
    pub(crate) measured: &'a Measured,
}

impl<'a> Stages<'a> {
    /// Returns the view a line set in `variant` reads.
    ///
    /// It holds the first line's shaping and measurements where the build
    /// made them, and the text's otherwise.
    #[inline]
    pub(crate) fn variant(self, variant: FirstLineVariant) -> LineStages<'a> {
        LineStages {
            content: self.content,
            analysis: self.analysis,
            fonts: self.fonts,
            variant,
            shaped: self.shaped.text(variant),
            measured: self.measured.text(variant),
        }
    }

    /// Returns the view the block's first line reads, where `::first-line`
    /// restyles something it is measured by.
    ///
    /// Returns `None` where it restyles nothing, and the first line reads the
    /// text's view.
    pub(crate) fn first_line(self) -> Option<LineStages<'a>> {
        self.measured
            .first_line()
            .map(|_| self.variant(FirstLineVariant::FirstLine))
    }
}

/// The prepared data as a line set in one first-line variant reads it.
///
/// The clusters, items and fonts are shared by every variant. The styles,
/// shaping and measurements are the variant's own.
#[derive(Copy, Clone)]
pub(crate) struct LineStages<'a> {
    pub(crate) content: &'a Content,
    pub(crate) analysis: &'a Analysis,
    fonts: &'a Fonts,
    /// Whose styles the line is set in: the first line's, or the text's.
    variant: FirstLineVariant,
    pub(crate) shaped: &'a ShapedText,
    measured: &'a MeasuredText,
}

impl<'a> LineStages<'a> {
    /// Whose styles the line is set in.
    #[inline]
    pub(crate) fn variant(&self) -> FirstLineVariant {
        self.variant
    }

    /// `node`'s text facts in the variant.
    #[inline]
    pub(crate) fn text_facts(&self, node: NodeId) -> TextFactsId {
        self.content.nodes.text_facts(node, self.variant)
    }

    /// The text as the variant reads it.
    #[inline]
    fn text(&self) -> VariantText<'a> {
        self.content.text(self.variant)
    }
}
