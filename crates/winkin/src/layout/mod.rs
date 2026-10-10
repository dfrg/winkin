//! A layout, the two drivers that fill it, and the views it is read through.
//!
//! In: the builder's calls and a [`Context`]. Out: lines and fragment items,
//! read back through views.
//! Start at: [`PreparedStages::prepare`], which the builder's `finish` runs,
//! then [`Layout::break_lines`].
//!
//! Preparing runs every stage that does not depend on the width: analysis,
//! font selection, shaping and measurement. It runs once per content change.
//! Breaking runs once per width: line breaking, then line layout. Breaking
//! borrows what preparing made immutably, so the borrow checker keeps it
//! frozen.
//!
//! The views, one module each:
//! - `line`: the lines and the items on each, in visual order;
//! - `run`: the text runs, with their fonts, glyphs and clusters;
//! - `boxes`: the inline boxes and atomic inlines;
//! - `ruby`: the ruby annotations;
//! - `floats`: the floats;
//! - `positioned`: the absolutely positioned boxes' static positions;
//! - `paint`: what a line paints, in Chrome's order.
//!
//! Invariants:
//! - Readers borrow the frozen stages, write nothing and allocate nothing.
//!   Each walk holds a few words of state. Fragment items carry their final
//!   positions, so reading them back is a scan.
//! - Positions leave as `f32` pixels, converted here and nowhere else: along
//!   the line from the exact 48.16 sums, across it from 26.6.
//! - Positions are line-relative: along the line from the line box's left
//!   edge, across it from its top. [`LineMetrics`] places the line box in the
//!   [`Area`]'s coordinates.
//! - A culled box keeps no fragment. [`Layout::box_fragments`] finds its
//!   pieces from its descendants' items, as Blink answers a culled inline,
//!   so `getClientRects` and hover work for any box without a relayout.
//! - [`Line::paints`] yields the background, boxes, decorations and text in
//!   the phases Chrome paints them, each decoration cut where Chrome cuts it.
//!   It draws nothing: the caller looks each paint up by key.
//!
//! [`Layout::box_fragments`]: crate::Layout::box_fragments

mod boxes;
mod emphasis;
mod floats;
mod line;
pub mod paint;
mod positioned;
mod ruby;
mod run;
#[cfg(test)]
mod tests;

#[cfg(test)]
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use fontwich::FaceId;

use crate::build::{BoxSize, BuildOptions, BuildReport, LayoutBuilder};
use crate::config::PastLines;
use crate::context::Context;
use crate::data::{HeapBytes, Id};
use crate::font::FontMetricsProvider;
use crate::selection::{
    self, Affinity, Caret, Carets, CopyKind, NodePosition, Position, SelectedText, SelectionRect,
    SelectionRects,
};
use crate::stages::Stages;
use crate::stages::analysis::{self, Analysis, AnalysisInput};
#[cfg(test)]
use crate::stages::content::TextFactsId;
use crate::stages::content::{AtomicId, Content, FloatId, NodeKey, SizeChange};
use crate::stages::content::{ContentLimits, ContentScratch, ContentWriter};
use crate::stages::fonts::{self, FontInput, Fonts};
use crate::stages::fragments::{self, Fragments, PlaceInput, ReadInput};
use crate::stages::lines::{self, Area, BreakInput, Exclusions, LineFloatId, LineId, TextBoxTrims};
use crate::stages::measure::{self, IntrinsicSizes, MeasureInput, Measured};
use crate::stages::shape::{self, ShapeInput, ShapeSession, Shaped};
#[cfg(test)]
use crate::style::FirstLineVariant;
use crate::style::{ComputedBlockStyle, Direction};
use crate::unit::LayoutUnit;

use boxes::BoxFragments;
pub use boxes::{Atomic, BoxFragment, InlineEdges};
pub use floats::FloatPlacement;
use floats::Floats;
pub(crate) use line::LineItems;
use line::Lines;
pub use line::{CrossExtents, Item, Line, LineMetrics};
pub use positioned::StaticPosition;
use positioned::StaticPositions;
pub use ruby::Annotation;
pub(crate) use ruby::AnnotationRuns;
pub use run::{Cluster, FontInstance, Glyph, TextRun};
// The walk a text run's carets and marks read, and the walks of a run's
// glyphs and clusters, which the path and the selection hold as they read.
#[cfg(test)]
use crate::unit::InlineLayoutUnit;
use run::Places;
pub(crate) use run::{RunClusters, RunGlyphs};

/// A reusable inline formatting context.
///
/// Stores content and layout results. Rebuilding retains allocations,
/// so similar content can be rebuilt without allocating.
pub struct Layout {
    /// What does not depend on the width: the content, and the stages
    /// preparing makes from it.
    content: Content,
    stages: PreparedStages,
    /// The lines breaking makes at a width, with every decision about their
    /// ends and the block's result.
    lines: lines::Lines,
    /// The fragment items, with their final positions.
    fragments: Fragments,
    /// The builder's working memory: the boxes open while it runs, and where
    /// capitalize reads the text before a call.
    ///
    /// It lives here, not in the context, because building takes no context.
    /// So a host can build a nested layout, an inline-block's, while this
    /// one's builder is open.
    content_scratch: ContentScratch,
}

/// The stages preparing makes after the content, from analysis to
/// measurement.
///
/// The first line's measure is part of [`Measured`], as the first line's
/// shaping is part of [`Shaped`]. A builder holds them mutably beside its
/// writer, and its [`finish`](LayoutBuilder::finish) fills them with
/// [`prepare`](Self::prepare).
pub(crate) struct PreparedStages {
    analysis: Analysis,
    fonts: Fonts,
    shaped: Shaped,
    /// The measurements. Between builds it also holds the advance buffers:
    /// shaping fills them, and measuring turns them into prefix sums in
    /// place.
    measured: Measured,
}

impl PreparedStages {
    /// Returns empty stages, allocating nothing.
    fn new() -> Self {
        Self {
            analysis: Analysis::new(),
            fonts: Fonts::new(),
            shaped: Shaped::new(),
            measured: Measured::new(),
        }
    }

    /// Borrows the stages whole, with `content`, the content they were
    /// prepared from.
    fn view<'a>(&'a self, content: &'a Content) -> Stages<'a> {
        Stages {
            content,
            analysis: &self.analysis,
            fonts: &self.fonts,
            shaped: &self.shaped,
            measured: &self.measured,
        }
    }

    /// Runs the preparation stages in order: analysis, font selection, shaping
    /// and measurement.
    ///
    /// Each uses the caches, scratch and config of `cx`. Shaping's advance
    /// buffers pass to measurement, which turns them into prefix sums.
    /// Records in `report` how many fonts and glyphs were dropped because a
    /// table was full.
    pub(crate) fn prepare(
        &mut self,
        content: &Content,
        cx: &mut Context,
        provider: Option<&dyn FontMetricsProvider>,
        report: &mut BuildReport,
    ) {
        cx.start_build(content.size());
        let config = *cx.config();
        let input = AnalysisInput {
            content,
            small_kana: config.small_kana,
        };
        let (segmenters, scratch) = cx.analyzing();
        analysis::analyze(&input, segmenters, scratch, &mut self.analysis);
        let input = FontInput {
            content,
            analysis: &self.analysis,
            default_language: config.default_language,
            line_metrics: config.line_metrics,
            platform_font_variations: config.platform_font_variations,
            position_synthesis: config.position_synthesis,
        };
        let (caches, scratch) = cx.selecting_fonts();
        report.replaced_fonts = fonts::select_fonts(&input, caches, scratch, &mut self.fonts);
        let input = ShapeInput {
            content,
            analysis: &self.analysis,
            fonts: &self.fonts,
            punctuation_trim: config.punctuation_trim,
        };
        report.dropped_glyphs = shape::shape_runs(
            &input,
            &mut ShapeSession::new(cx.shaping(), provider),
            &mut self.shaped,
        );
        self.measure(content, cx, provider);
    }

    /// Runs measurement again from the shaped advances, with the boxes'
    /// sizes and edges as `content` holds them now.
    pub(crate) fn measure(
        &mut self,
        content: &Content,
        cx: &mut Context,
        provider: Option<&dyn FontMetricsProvider>,
    ) {
        let config = *cx.config();
        // The buffers the last prefix sums were kept in, the text's and the
        // first line's, holding a copy of the advances to sum.
        let advances = self.shaped.advances_into(self.measured.recycle());
        let input = MeasureInput {
            content,
            analysis: &self.analysis,
            fonts: &self.fonts,
            shaped: &self.shaped,
            word_spacing: config.word_spacing,
            super_sub: config.super_sub,
            dominant_baseline: config.dominant_baseline,
            ruby_overhang: config.ruby_overhang,
            ruby_break_within: config.ruby_break_within,
        };
        let (shaping, scratch) = cx.measuring();
        measure::measure_text(
            &input,
            &mut ShapeSession::new(shaping, provider),
            scratch,
            advances,
            &mut self.measured,
        );
    }

    /// Empties every stage, keeping every allocation.
    ///
    /// A layout holds this when its builder was dropped without finishing.
    pub(crate) fn clear(&mut self) {
        self.analysis.clear();
        self.fonts.clear();
        self.shaped.clear();
        self.measured.clear();
    }
}

impl Layout {
    /// Creates an empty layout without allocating.
    pub fn new() -> Self {
        Self {
            content: Content::new(),
            stages: PreparedStages::new(),
            lines: lines::Lines::new(),
            fragments: Fragments::new(),
            content_scratch: ContentScratch::new(),
        }
    }

    /// Starts building a block identified by `key` with style `block`.
    ///
    /// `key` identifies the block in layout views and for break opportunities
    /// between source nodes. `block` supplies the inherited text style,
    /// first-line style and block-container properties.
    ///
    /// Clears previous content and results, retaining allocations.
    /// Call [`finish`](LayoutBuilder::finish) to prepare the new content.
    pub fn builder<'a>(
        &'a mut self,
        key: NodeKey,
        block: &ComputedBlockStyle<'_>,
        options: BuildOptions,
    ) -> LayoutBuilder<'a> {
        self.builder_within(key, block, options, ContentLimits::MAX)
    }

    /// Measures the content again with new box geometry, keeping its
    /// analysis, fonts and shaping, and returns whether anything changed.
    ///
    /// `basis` is the width, in pixels, that percentage margins and padding
    /// are taken of: the containing block's inline size, as
    /// [`BuildOptions::percentage_basis`] gives it at build time. Each pair in
    /// `sizes` gives every atomic inline and float with its key the border
    /// box, and an atomic inline the baseline, of its size, as
    /// [`LayoutBuilder::atomic`] and [`LayoutBuilder::float`] take them. Keys
    /// need not be unique, and a key no box has is skipped. Boxes left out
    /// keep their sizes.
    ///
    /// The result equals building the content again with the new geometry.
    /// Where nothing changes, it returns `false` and keeps the lines. Where
    /// only atomic inlines' block sizes and baselines change, only their
    /// extents are measured again; otherwise measurement runs again from the
    /// shaped advances. Either way the lines are cleared: lines, box
    /// fragments, static positions, hit tests and paints see no lines until
    /// the next [`break_lines`](Self::break_lines).
    ///
    /// Where no two atomic inlines share a key, pairs in document order take
    /// time linear in the number of atomic inlines. Pairs in any other order
    /// give the same result. A warm call does not allocate.
    ///
    /// A `basis` that is no length counts as zero.
    ///
    /// [`BuildOptions::percentage_basis`]: crate::BuildOptions::percentage_basis
    pub fn measure<I>(&mut self, cx: &mut Context, basis: f32, sizes: I) -> bool
    where
        I: IntoIterator<Item = (NodeKey, BoxSize)>,
        I::IntoIter: Clone,
    {
        self.measure_with_provider(cx, basis, sizes, None)
    }

    /// Measures the content again with new box geometry and the host's
    /// glyph metrics.
    ///
    /// Behaves like [`measure`](Self::measure). Use the provider and strike
    /// settings the layout was built with.
    pub fn measure_with_metrics<I>(
        &mut self,
        cx: &mut Context,
        basis: f32,
        sizes: I,
        provider: &dyn FontMetricsProvider,
    ) -> bool
    where
        I: IntoIterator<Item = (NodeKey, BoxSize)>,
        I::IntoIter: Clone,
    {
        self.measure_with_provider(cx, basis, sizes, Some(provider))
    }

    fn measure_with_provider<I>(
        &mut self,
        cx: &mut Context,
        basis: f32,
        sizes: I,
        provider: Option<&dyn FontMetricsProvider>,
    ) -> bool
    where
        I: IntoIterator<Item = (NodeKey, BoxSize)>,
        I::IntoIter: Clone,
    {
        let basis = if basis.is_finite() { basis } else { 0.0 };
        let sizes = sizes.into_iter();
        // Whether measurement runs again whole, and whether some atomic
        // inline's extent alone changed.
        let mut again = self.content.set_basis(basis);
        let mut across = false;
        let in_place = self.content.can_resize_atomics();
        let mut cursor = AtomicId::new(0);
        for (key, size) in sizes.clone() {
            let size = size.sanitized();
            let mut search = self.content.keyed_atomics(key, cursor);
            let mut found = false;
            while let Some(atomic) = search.next_match(&self.content) {
                found = true;
                match self.content.set_atomic_size(atomic, size) {
                    SizeChange::None => {}
                    SizeChange::Across if in_place => across = true,
                    SizeChange::Across | SizeChange::Along => again = true,
                }
            }
            cursor = search.cursor();
            if !found {
                again |= self.set_float_sizes(key, size);
            }
        }
        if again {
            self.stages.measure(&self.content, cx, provider);
        } else if across {
            let mut cursor = AtomicId::new(0);
            for (key, _) in sizes {
                let mut search = self.content.keyed_atomics(key, cursor);
                while let Some(atomic) = search.next_match(&self.content) {
                    self.stages.measured.remeasure_atomic(&self.content, atomic);
                }
                cursor = search.cursor();
            }
        } else {
            return false;
        }
        self.clear_lines();
        true
    }

    /// Gives every float keyed `key` the border box of `size`, a sanitized
    /// size, and returns whether any size changed.
    fn set_float_sizes(&mut self, key: NodeKey, size: BoxSize) -> bool {
        let mut changed = false;
        let floats = self.content.floats().len();
        for float in (0..floats).map(FloatId::new) {
            let keyed = self
                .content
                .floats()
                .get(float)
                .is_some_and(|row| self.content.float_key(row) == key);
            if keyed {
                changed |= self.content.set_float_size(float, size);
            }
        }
        changed
    }

    /// Breaks and positions the content in `area`.
    ///
    /// Lines flow around floats placed by `exclusions`. Unsafe shaping edges
    /// are reshaped using `cx`, then items are aligned, reordered and positioned.
    ///
    /// Repeated calls reuse prepared content and retained allocations.
    /// A warm relayout does not allocate. Any width produces valid lines.
    /// An unfinished or unbuilt layout produces no lines.
    ///
    /// `cx` may differ from the context used to build the layout; the layout
    /// retains its fonts. Equivalent configuration produces the same results.
    /// For layouts built with host glyph metrics, use
    /// [`break_lines_with_metrics`](Self::break_lines_with_metrics) with the
    /// same provider and strike settings.
    pub fn break_lines(&mut self, cx: &mut Context, area: Area, exclusions: &mut dyn Exclusions) {
        self.break_lines_with_provider(cx, area, exclusions, None);
    }

    /// Breaks and positions the content using host glyph metrics.
    ///
    /// Behaves like [`break_lines`](Self::break_lines), but uses `provider`
    /// when reshaping line edges. Use the same provider and strike settings
    /// as at build time. Prepared measurements are reused.
    pub fn break_lines_with_metrics(
        &mut self,
        cx: &mut Context,
        area: Area,
        exclusions: &mut dyn Exclusions,
        provider: &dyn FontMetricsProvider,
    ) {
        self.break_lines_with_provider(cx, area, exclusions, Some(provider));
    }

    /// Breaks the content in `area` to size it, without positioning items.
    ///
    /// The lines are those [`break_lines`](Self::break_lines) makes, and
    /// floats are placed through `exclusions` alike. [`metrics`](Self::metrics),
    /// [`room_below`](Self::room_below), and each line's count, width, extent
    /// and band are as `break_lines` gives them; a line's left edge is its
    /// band's and its top zero, before alignment and positioning. Items,
    /// paints, box fragments, static positions, hit tests and carets see
    /// no items until the next `break_lines`.
    ///
    /// A host that sizes a block before it places it calls this first: it
    /// skips the positioning that `break_lines` does again.
    pub fn size_lines(&mut self, cx: &mut Context, area: Area, exclusions: &mut dyn Exclusions) {
        self.break_with_provider(cx, area, exclusions, None, false);
    }

    /// Breaks the content to size it, using host glyph metrics.
    ///
    /// Behaves like [`size_lines`](Self::size_lines), but uses `provider`
    /// when reshaping line edges, as
    /// [`break_lines_with_metrics`](Self::break_lines_with_metrics) does.
    pub fn size_lines_with_metrics(
        &mut self,
        cx: &mut Context,
        area: Area,
        exclusions: &mut dyn Exclusions,
        provider: &dyn FontMetricsProvider,
    ) {
        self.break_with_provider(cx, area, exclusions, Some(provider), false);
    }

    /// Returns pending `@font-face` faces needed by this content, without duplicates.
    ///
    /// After loading a face, update the context collection and rebuild layouts
    /// that requested it. Until then, text uses available fallback fonts.
    pub fn wanted_faces(&self) -> &[FaceId] {
        &self.stages.fonts.wanted
    }

    /// Returns lines from the last [`break_lines`](Self::break_lines) call.
    ///
    /// Lines are in block order. The iterator is empty before line breaking
    /// or after a rebuild. Reading lines, items, glyphs and clusters does
    /// not allocate.
    #[inline]
    pub fn lines(&self) -> impl ExactSizeIterator<Item = Line<'_>> + DoubleEndedIterator + Clone {
        Lines::new(self)
    }

    /// Returns the line at `index`, or `None` if out of bounds.
    #[inline]
    pub fn line(&self, index: usize) -> Option<Line<'_>> {
        LineId::try_new(index).and_then(|id| Line::new(self, id))
    }

    /// Returns fragments of inline boxes identified by `key`.
    ///
    /// Fragments are in tree order, then line order. Ruby containers and
    /// annotations always retain fragments. A base fragment uses the base
    /// font ascent and descent; an annotation fragment spans the column
    /// width on its side of the base.
    ///
    /// A box inside a ruby annotation has a fragment on each line that sets
    /// a part of it, as a box in the line's own text does.
    ///
    /// Keys need not be unique; results include all boxes with the key.
    /// Retained boxes use their stored fragments. Culled boxes derive a
    /// fragment from each group of descendant items on a line, matching
    /// Chrome. This supports client rectangles and hover without relayout.
    /// Only relevant lines are inspected. Does not allocate.
    #[inline]
    pub fn box_fragments(&self, key: NodeKey) -> impl Iterator<Item = BoxFragment<'_>> + Clone {
        BoxFragments::new(self, key)
    }

    /// Returns floats placed by the last [`break_lines`](Self::break_lines) call.
    ///
    /// Results follow placement order: floats on each line first, then floats
    /// anchored after the final forced break or in content with no lines.
    #[inline]
    pub fn floats(&self) -> impl Iterator<Item = FloatPlacement> + Clone {
        let placed = LineFloatId::new(0)..self.line_records().floats.next_id();
        Floats::new(self, placed)
    }

    /// Returns static positions for absolutely positioned boxes, in content order.
    ///
    /// After [`break_lines`](Self::break_lines), yields one position per
    /// [`LayoutBuilder::absolute`] call. The iterator is empty before line
    /// breaking or after a rebuild. Does not allocate.
    ///
    /// The host uses these positions to resolve `auto` insets.
    #[inline]
    pub fn static_positions(&self) -> impl ExactSizeIterator<Item = StaticPosition> + Clone {
        StaticPositions::new(self)
    }

    /// Returns the collapsed and transformed layout text.
    ///
    /// Contains U+FFFC for atomic inlines, `\n` for forced breaks, and U+200B
    /// for retained break opportunities. Content offsets index this string.
    #[inline]
    pub fn text(&self) -> &str {
        &self.content.text
    }

    /// Returns block extents, baselines and trim amounts.
    ///
    /// The host uses these for block layout and [`Line::metrics`](crate::Line::metrics)
    /// for individual lines. An unbroken or rebuilt layout has no lines and
    /// ends at zero.
    pub fn metrics(&self) -> LayoutMetrics {
        let block = self.lines.block;
        LayoutMetrics {
            block_end: block.block_end.to_px(),
            first_baseline: block.first_baseline.map(LayoutUnit::to_px),
            last_baseline: block.last_baseline.map(LayoutUnit::to_px),
            trim_start: block.trim_start.to_px(),
            trim_end: block.trim_end.to_px(),
        }
    }

    /// Returns the intrinsic widths, independent of the available width.
    ///
    /// Available after the builder finishes. Includes the widest unbreakable
    /// piece and the widest paragraph. Returns zero for an unbuilt or
    /// unfinished layout.
    pub fn intrinsic_sizes(&self) -> IntrinsicSizes {
        self.stages.measured.intrinsic.into()
    }

    /// Returns whether the lines may carry ruby annotations or emphasis
    /// marks.
    ///
    /// Only then does [`Area::room_above`](crate::Area::room_above) change
    /// the lines, and can [`room_below`](Self::room_below) be negative. A
    /// host need not work out the room above for content without them.
    /// Available after the builder finishes.
    pub fn has_annotations(&self) -> bool {
        lines::annotated(self.stages())
    }

    /// Returns space below the last line that the next block can use.
    ///
    /// The next block can include this space in
    /// [`Area::room_above`](crate::Area::room_above), along with its start
    /// padding and, if no border intervenes, collapsed margins and the end
    /// padding of the preceding block.
    ///
    /// Negative values indicate ruby annotations or emphasis marks extending
    /// below the line box. A final clearing break moves the block end below
    /// floats; only content extending beyond that end affects this value.
    /// [`LayoutMetrics::block_end`] already includes the extension and clearance.
    /// The host may allow annotations into end padding, matching Chrome.
    ///
    /// Returns zero when `text-box-trim` trims the end or there are no lines.
    /// For content without annotations or emphasis marks, computes the value
    /// on demand from the last line rather than during relayout.
    pub fn room_below(&self) -> f32 {
        let stages = self.stages();
        // Chrome trims what the annotations push down with the last line, and
        // lends nothing past a trimmed end (`trim_block_end_by`).
        let trims_end =
            TextBoxTrims::new(stages.content, stages.fonts).is_some_and(|trims| trims.ends);
        if trims_end {
            return 0.0;
        }
        self.lines.room_below(stages).to_px()
    }

    /// Returns the nearest caret position to a point.
    ///
    /// Returns `None` if the layout has no lines. `inline` measures from
    /// line-left along the line; `block` measures from block-start across
    /// lines. See [`LineMetrics`].
    ///
    /// [`PastLines`] controls points outside the line boxes. Within a line,
    /// character halves determine the nearest caret boundary. Points outside
    /// items use the nearest item edge.
    ///
    /// Hyphens and ellipses map to their underlying text; box edges map to
    /// adjacent text. Atomic inlines use the nearer side. Ruby annotations
    /// are hit-tested in their own positions. At bidi boundaries, the hit
    /// uses the position with a caret at that edge, matching Chrome.
    #[inline]
    pub fn hit_test(&self, inline: f32, block: f32, past_lines: PastLines) -> Option<Position> {
        selection::hit_test(self, inline, block, past_lines)
    }

    /// Returns caret geometry for `position`, or `None` if there are no lines.
    ///
    /// At a wrap, affinity selects the line. At bidi boundaries, the caret
    /// uses the paragraph direction; [`carets`](Self::carets) also returns
    /// the alternate position. Text hidden by an ellipsis retains its caret
    /// position beyond the ellipsis, matching Chrome.
    #[inline]
    pub fn caret(&self, position: Position) -> Option<Caret> {
        self.carets(position).map(|carets| carets.strong)
    }

    /// Returns both possible caret positions at `position`.
    ///
    /// The second position supports split carets at bidi boundaries.
    /// Chrome draws only the first.
    #[inline]
    pub fn carets(&self, position: Position) -> Option<Carets> {
        selection::carets(self, position)
    }

    /// Returns the base direction of the paragraph containing `position`.
    ///
    /// Uses the block direction, or the first strong character under
    /// `unicode-bidi: plaintext`. After a forced break, uses the new paragraph.
    ///
    /// To match Chrome arrow-key motion, map arrows to
    /// [`Forward`](crate::selection::MotionDirection::Forward) and
    /// [`Backward`](crate::selection::MotionDirection::Backward) using this
    /// direction rather than using physical left and right motion.
    pub fn paragraph_direction(&self, position: Position) -> Direction {
        selection::paragraph_direction(self, position)
    }

    /// Returns selection rectangles for `range` in line order.
    ///
    /// `range` contains byte offsets into [`Layout::text`]. Rectangles cover
    /// selected text in each item and span the line-box height. Selections
    /// continuing past a line end include an extra space width, matching Chrome.
    pub fn selection_rects(&self, range: Range<usize>) -> impl Iterator<Item = SelectionRect> {
        SelectionRects::new(self, range.start, range.end)
    }

    /// Returns source text slices for copying the selection `range`.
    ///
    /// Slices borrow from [`Layout::text`] and omit inserted U+FFFC atomic
    /// inline placeholders and U+200B break opportunities. Forced breaks
    /// copy as newlines. `kind` controls whether no-break spaces are preserved
    /// (as in `Selection.toString()`) or converted to spaces (as on the
    /// Chrome clipboard).
    pub fn selected_text(
        &self,
        range: Range<usize>,
        kind: CopyKind,
    ) -> impl Iterator<Item = &str> + Clone + fmt::Display {
        SelectedText::new(self, range.start, range.end, kind)
    }

    /// Maps `position` to a node and offset in the source text.
    ///
    /// Returns `None` if [`BuildOptions::map_source`](crate::BuildOptions::map_source)
    /// was disabled. Collapsed whitespace, inserted break opportunities and
    /// node boundaries may have multiple source positions. Upstream affinity
    /// selects the first; downstream selects the last, matching Chrome.
    pub fn node_position(&self, position: Position) -> Option<NodePosition> {
        selection::node_position(self, position)
    }

    /// Maps a source byte offset in node `key` to a layout position.
    ///
    /// Collapsed whitespace and inserted content may have multiple layout
    /// positions. Upstream affinity selects the first; downstream selects
    /// the last. Offsets past the source text are clamped to its end.
    ///
    /// Returns `None` if source mapping was disabled or `key` identifies no
    /// text node.
    pub fn position(&self, key: NodeKey, offset: usize, affinity: Affinity) -> Option<Position> {
        selection::position(self, key, offset, affinity)
    }

    /// Returns estimated layout heap usage by component, in bytes.
    ///
    /// Counts table capacities, including memory retained for later builds
    /// and line breaking. Excludes font bytes shared with context instances.
    pub fn heap_bytes(&self) -> LayoutHeap {
        let Self {
            content,
            stages,
            lines,
            fragments,
            content_scratch,
        } = self;
        let PreparedStages {
            analysis,
            fonts,
            shaped,
            measured,
        } = stages;
        // The first line's variant, which the fonts, the shaping and the
        // measurements each count among their own tables.
        let first_fonts = fonts.first_line_heap_bytes();
        let first_shaped = shaped.first_line_heap_bytes();
        let first_measured = measured.first_line_heap_bytes();
        LayoutHeap {
            content: content.heap_bytes(),
            analysis: analysis.heap_bytes(),
            fonts: fonts.heap_bytes().saturating_sub(first_fonts),
            shaped: shaped.heap_bytes().saturating_sub(first_shaped),
            measured: measured.heap_bytes().saturating_sub(first_measured),
            first_line: first_fonts + first_shaped + first_measured,
            lines: lines.heap_bytes() + fragments.heap_bytes(),
            builder: content_scratch.heap_bytes(),
        }
    }
}

// What the crate reaches inside a layout: the building and breaking behind
// the public calls, and the prepared stages the views read.
impl Layout {
    /// Starts building as [`builder`](Self::builder) does, within `limits`.
    ///
    /// Tests pass limits below what the ids and the text allow to reach
    /// every limit.
    pub(crate) fn builder_within<'a>(
        &'a mut self,
        key: NodeKey,
        block: &ComputedBlockStyle<'_>,
        options: BuildOptions,
        limits: ContentLimits,
    ) -> LayoutBuilder<'a> {
        // Lines broken from the content before would name its clusters.
        self.clear_lines();
        LayoutBuilder::new(
            ContentWriter::new(
                &mut self.content,
                &mut self.content_scratch,
                key,
                block,
                options,
                limits,
            ),
            &mut self.stages,
        )
    }

    /// Clears line layout's outputs: the lines and their fragments.
    fn clear_lines(&mut self) {
        self.lines.clear();
        self.fragments.clear();
    }

    fn break_lines_with_provider(
        &mut self,
        cx: &mut Context,
        area: Area,
        exclusions: &mut dyn Exclusions,
        provider: Option<&dyn FontMetricsProvider>,
    ) {
        self.break_with_provider(cx, area, exclusions, provider, true);
    }

    /// Breaks the content in `area`, and positions its items where `place`
    /// says.
    fn break_with_provider(
        &mut self,
        cx: &mut Context,
        area: Area,
        exclusions: &mut dyn Exclusions,
        provider: Option<&dyn FontMetricsProvider>,
        place: bool,
    ) {
        cx.start_break(self.content.size());
        self.clear_lines();
        // What preparing made, borrowed immutably, so it stays frozen.
        let stages = self.stages.view(&self.content);
        let (lines, fragments) = (&mut self.lines, &mut self.fragments);
        let config = *cx.config();
        // The lines.
        let input = BreakInput {
            stages,
            area,
            emphasis_room: config.emphasis_room,
            pretty: config.pretty,
            ruby_break_within: config.ruby_break_within,
        };
        let (shaping, scratch) = cx.breaking();
        lines::break_lines(
            &input,
            &mut ShapeSession::new(shaping, provider),
            scratch,
            exclusions,
            lines,
        );
        // Line layout: place items using breaking's block result, their tabs
        // sized in the area the breaker sized them in.
        let (placements, scratch) = cx.placing();
        if !place {
            placements.clear();
            return;
        }
        let input = PlaceInput {
            stages,
            lines,
            placements,
            area,
            tab_justification: config.tab_justification,
            ellipsis_space: config.ellipsis_space,
        };
        fragments::place_fragments(&input, scratch, fragments);
        placements.clear();
    }

    /// Borrows the prepared stages whole.
    pub(crate) fn stages(&self) -> Stages<'_> {
        self.stages.view(&self.content)
    }

    /// Returns the content, for the views to read.
    pub(crate) fn content(&self) -> &Content {
        &self.content
    }

    /// Returns the analysis.
    pub(crate) fn analysis(&self) -> &Analysis {
        &self.stages.analysis
    }

    /// Returns the fonts.
    pub(crate) fn fonts(&self) -> &Fonts {
        &self.stages.fonts
    }

    /// Returns the primary font of text with the text facts `text`, through
    /// its font request.
    #[cfg(test)]
    pub(crate) fn primary(&self, text: TextFactsId) -> Option<&fonts::UsedFont> {
        let request = self.content.facts.text_request(text);
        self.stages.fonts.primary_font(request)
    }

    /// Returns the id of [`primary`](Self::primary).
    #[cfg(test)]
    pub(crate) fn primary_id(&self, text: TextFactsId) -> Option<fonts::UsedFontId> {
        let request = self.content.facts.text_request(text);
        Some(self.stages.fonts.resolution(request)?.primary)
    }

    /// Returns the shaped glyphs.
    pub(crate) fn shaped(&self) -> &Shaped {
        &self.stages.shaped
    }

    /// Returns the measurements.
    pub(crate) fn measured(&self) -> &Measured {
        &self.stages.measured
    }

    /// Returns the lines as breaking recorded them.
    pub(crate) fn line_records(&self) -> &lines::Lines {
        &self.lines
    }

    /// Returns the fragment items and the block's result.
    pub(crate) fn fragments(&self) -> &Fragments {
        &self.fragments
    }

    /// Returns what line layout read and wrote, which a line's views read a
    /// text item back through, as a painter does.
    pub(crate) fn read_input(&self) -> ReadInput<'_> {
        ReadInput::new(self.stages(), &self.lines, &self.fragments)
    }

    /// Returns each cluster's advance as shaping hands it on, for tests.
    #[cfg(test)]
    pub(crate) fn shaped_advances(&self, _cx: &mut Context) -> Vec<InlineLayoutUnit> {
        self.stages
            .shaped
            .text(FirstLineVariant::Standard)
            .advances
            .clone()
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

/// Block extents, baselines and trim amounts, in pixels.
///
/// Returned by [`Layout::metrics`] after line layout. Individual line
/// geometry is available through [`LineMetrics`].
#[derive(Copy, Clone, PartialEq, Debug, Default)]
#[non_exhaustive]
pub struct LayoutMetrics {
    /// The block-end content position after `text-box-trim`.
    ///
    /// Uses the bottom of the last line box, or the area block-start if
    /// there are no lines. The host accounts for floats separately.
    pub block_end: f32,
    /// The first baseline offset from block-start.
    ///
    /// Used for inline-block and flex-item alignment. `None` if there are
    /// no lines.
    pub first_baseline: Option<f32>,
    /// The last baseline offset from block-start.
    ///
    /// `None` if there are no lines.
    pub last_baseline: Option<f32>,
    /// The upward shift applied to all lines by start trimming.
    ///
    /// Positive for excess above the trim edge; negative if the line box
    /// falls short. Zero if start trimming is disabled.
    pub trim_start: f32,
    /// The upward shift of the block end from end trimming.
    pub trim_end: f32,
}

/// Estimated layout heap usage by component, in bytes.
///
/// Returned by [`Layout::heap_bytes`]. Counts table capacities retained
/// across builds and line breaking. Excludes shared font bytes. Block
/// metrics require no heap allocation.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct LayoutHeap {
    /// The content tables.
    ///
    /// Text, nodes, items, interned styles and their lists, atomic inlines,
    /// floats, and the source offset map when the build records one.
    pub content: usize,
    /// Analysis tables: clusters, paragraphs and script/bidi runs.
    pub analysis: usize,
    /// The font selection's tables.
    ///
    /// The used fonts, the text's font runs, each style's primary font and
    /// the fonts of the generated text.
    pub fonts: usize,
    /// Shaped glyphs and shaping runs.
    pub shaped: usize,
    /// The measurement tables.
    ///
    /// Prefix advances, extra advances at line starts and ends, item
    /// extents, baseline shifts, retained boxes, ruby columns, and shaped
    /// hyphens, ellipses and emphasis marks.
    pub measured: usize,
    /// Additional font runs, glyphs and measurements for `::first-line`.
    ///
    /// Zero if no first-line variant was prepared. First-line styles and
    /// transformed text are counted in `content`.
    pub first_line: usize,
    /// Line records and positioned items at the last layout width.
    ///
    /// Includes reshaped edges, floats, baseline shifts, fragment items,
    /// line headers and justification records. Temporary working memory
    /// is counted in [`ContextHeap::scratch`](crate::ContextHeap::scratch).
    pub lines: usize,
    /// The builder's working state: the containers open while it runs.
    pub builder: usize,
}

impl LayoutHeap {
    /// Returns the total estimated heap usage, in bytes.
    pub fn total(&self) -> usize {
        let Self {
            content,
            analysis,
            fonts,
            shaped,
            measured,
            first_line,
            lines,
            builder,
        } = *self;
        content + analysis + fonts + shaped + measured + first_line + lines + builder
    }
}
