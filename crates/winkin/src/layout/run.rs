//! Text runs: a text item read back with its font, its glyphs and its
//! clusters.
//!
//! - A text item holds clusters of one used font and names the shaping run
//!   of its first shaped cluster, which carries that font. The used font
//!   holds what a renderer draws with: shared bytes, index, coordinates and
//!   synthesis, copied from the context's instance during font selection.
//!   So [`TextRun::font`] borrows the layout alone, with no context.
//! - `fragments::GlyphWalk` decodes glyphs on the fly: the paragraph's
//!   glyphs, and the line's own at its reshaped edges, in drawing order and
//!   placed exactly. This file converts them to pixels without allocating.

use core::fmt;
use core::ops::Range;
use core::slice::Iter;

use fontwich::{FontBytes, FontKey};

use crate::data::Id;
use crate::stages::analysis::{
    BidiLevel, ClusterId, Clusters as ClusterTable, RunOrientation, ScriptRun, ScriptRunId,
};
use crate::stages::content::{ContentFlags, NodeKey};
use crate::stages::fonts::{Generated, LineBaseline, NormalizedCoord, UsedFont, UsedFontId};
use crate::stages::fragments::{
    ClusterWalk, ExactGlyph, FragmentItem, FragmentItemFlags, FragmentItemKind, GlyphWalk,
};
use crate::stages::measure::{Extent, ShapedGenerated};
use crate::stages::shape::{ShapedRun, SidecarGlyph};
use crate::style::WritingMode;
use crate::unit::{InlineLayoutUnit, LayoutUnit};

use super::emphasis::{EmphasisMark, EmphasisMarks};
use super::places::{AcrossPlaces, Places};
use super::{Atomic, CrossExtents, Item, Line};
use crate::stages::lines::InlineExtents;
use crate::work;

/// Shaped or generated text from one node, font and bidi level on a line.
#[derive(Copy, Clone)]
pub struct TextRun<'a> {
    line: Line<'a>,
    item: &'a FragmentItem,
}

impl<'a> TextRun<'a> {
    /// Returns the node key used for styling.
    ///
    /// Hyphens use the preceding text node; ellipses use the block key.
    pub fn key(&self) -> NodeKey {
        self.line.layout().content().nodes.key(self.item.node)
    }

    /// Returns the logical byte range in [`Layout::text`](crate::Layout::text).
    ///
    /// Generated text has an empty range at its insertion position.
    pub fn text_range(&self) -> Range<usize> {
        let range = self
            .item
            .text_range(&self.line.layout().analysis().clusters);
        range.start.get()..range.end.get()
    }

    /// Returns the font and size used to draw the run.
    ///
    /// Returns `None` if no font is available, such as with an empty collection
    /// or unavailable font bytes. Clusters retain their positions but draw nothing.
    ///
    /// Borrows font bytes and coordinates from the layout. Does not allocate
    /// or require a context.
    pub fn font(&self) -> Option<FontInstance<'a>> {
        let used = self.used()?;
        let drawn = used.instance.as_ref()?;
        Some(FontInstance {
            bytes: &drawn.bytes,
            index: drawn.index,
            size: {
                let (size, scale) = (used.glyph_size().to_px(), self.line.scale());
                if scale == 1.0 { size } else { size * scale }
            },
            coords: &drawn.coords,
            embolden: drawn.embolden,
            skew: drawn.skew,
        })
    }

    /// Returns the run advance in pixels, including reshaped line edges.
    ///
    /// In a ruby base or annotation, the advance includes the room that
    /// `ruby-align` distributes beside the run's text.
    pub fn advance(&self) -> f32 {
        self.item.advance().to_px()
    }

    /// Returns inline extents relative to line-box left.
    ///
    /// In a ruby base or annotation, the extents include the room that
    /// `ruby-align` distributes beside the run's text, as do the extents of
    /// the clusters beside that room. Glyph positions exclude it.
    pub fn inline(&self) -> InlineExtents {
        self.item.inline_extents()
    }

    /// Returns text extents across the line, relative to the line-over edge.
    ///
    /// Uses font ascent and descent around the baseline. These extents also
    /// contribute to rectangles of culled boxes, matching Chrome.
    pub fn block(&self) -> CrossExtents {
        let (over, under) = self.block_extent();
        CrossExtents {
            over: over.to_px(),
            under: under.to_px(),
        }
    }

    /// Returns the baseline offset from line-box top.
    ///
    /// Includes accumulated `vertical-align` shifts from enclosing boxes.
    pub fn baseline(&self) -> f32 {
        self.item.block.to_px()
    }

    /// Returns glyphs in drawing order, positioned relative to line-box left and top.
    ///
    /// Includes reshaped edge glyphs. Cloning the iterator preserves its
    /// current position without copying glyph data.
    #[inline]
    pub fn glyphs(&self) -> impl Iterator<Item = Glyph> + Clone + use<'a> {
        RunGlyphs::new(self)
    }

    /// Returns clusters in logical order with their text ranges and positions.
    ///
    /// Used for caret positioning, selection and decorations. Generated text
    /// has no clusters. Cloning copies only the iterator state.
    pub fn clusters(&self) -> impl ExactSizeIterator<Item = Cluster> + Clone + use<'a> {
        RunClusters::new(self)
    }

    /// Returns emphasis marks in logical cluster order.
    ///
    /// Returns no marks for generated text or styles without emphasis.
    /// Does not allocate.
    pub fn emphasis_marks(&self) -> impl Iterator<Item = EmphasisMark> + use<'a> {
        EmphasisMarks::new(self)
    }

    /// Returns the bidi level: even for LTR, odd for RTL.
    pub fn level(&self) -> BidiLevel {
        self.item.level
    }

    /// Returns `true` if the run is RTL.
    ///
    /// Glyphs are drawn from the logical end.
    pub fn is_rtl(&self) -> bool {
        self.item.level.is_rtl()
    }

    /// Returns the glyph orientation within the line.
    ///
    /// Glyphs may be horizontal, upright, sideways or combined into one
    /// upright em. Positions always use the line frame: along the line from
    /// its left and across it from its top. For vertical lines, these axes
    /// map to the page top and right.
    ///
    /// The painter maps the frame to the page and rotates sideways glyphs
    /// with the line. Upright and combined glyphs remain upright. Hyphens
    /// and ellipses inherit the orientation of the preceding text.
    pub fn orientation(&self) -> RunOrientation {
        let layout = self.line.layout();
        if layout.content().block.writing_mode == WritingMode::HorizontalTb {
            return RunOrientation::Horizontal;
        }
        if let Some(generated) = self.shaped_generated() {
            return generated.orientation;
        }
        self.script_run()
            .map_or(RunOrientation::Horizontal, |(_, run)| run.orientation)
    }

    /// Returns the scale across the line for combined text, or `1.0` for
    /// other runs.
    ///
    /// Narrows combined text to one em where narrower font forms are not
    /// enough. Scale each glyph about its supplied position; positions are
    /// already scaled, matching Chrome.
    pub fn combine_scale(&self) -> f32 {
        if self.orientation() != RunOrientation::Combined {
            return 1.0;
        }
        let shaped = self.line.stages().shaped;
        self.script_run()
            .map_or(1.0, |(unit, _)| shaped.combine_fit(unit).scale())
    }

    /// Returns `true` if this run represents a tab.
    ///
    /// A tab is one cluster with no glyphs. Its advance is the distance to
    /// the next tab stop.
    pub fn is_tab(&self) -> bool {
        self.item.flags.contains(FragmentItemFlags::TAB)
    }

    /// Returns `true` for preserved whitespace hanging past the line end.
    ///
    /// Enclosing boxes cover it, but browsers do not report it as ink.
    pub fn is_hanging(&self) -> bool {
        self.item.flags.contains(FragmentItemFlags::HANGS)
    }

    /// Returns `true` for laid-out text hidden by an ellipsis.
    pub fn is_hidden(&self) -> bool {
        self.item.flags.contains(FragmentItemFlags::HIDDEN)
    }

    /// Returns the generated text kind, or `None` for source text.
    pub fn generated(&self) -> Option<Generated> {
        self.shaped_generated().map(|text| text.kind)
    }
}

// What a caret walks on a line.
impl<'a> Line<'a> {
    /// Returns the line's text runs and atomic inlines in visual order,
    /// including the tail an ellipsis hides: the leaves a caret walks.
    pub(crate) fn leaf_items(&self) -> impl DoubleEndedIterator<Item = Item<'a>> + Clone + use<'a> {
        let line = *self;
        let items = self.layout().fragments().line_items(self.id());
        items.iter().filter_map(move |item| {
            work::step();
            match item.kind() {
                FragmentItemKind::Text => Some(Item::Text(TextRun::new(line, item))),
                FragmentItemKind::Atomic => Some(Item::Atomic(Atomic::new(line, item))),
                _ => None,
            }
        })
    }
}

// What the other views and the paths read of a run.
impl<'a> TextRun<'a> {
    /// Returns text item `item` of `line` as a run.
    #[inline]
    pub(super) fn new(line: Line<'a>, item: &'a FragmentItem) -> Self {
        Self { line, item }
    }

    /// Returns the run's line.
    #[inline]
    pub(super) fn line(&self) -> Line<'a> {
        self.line
    }

    /// Returns the run's fragment item.
    #[inline]
    pub(super) fn item(&self) -> &'a FragmentItem {
        self.item
    }

    /// Returns the generated text a generated run draws, or `None` for
    /// content text.
    fn shaped_generated(&self) -> Option<&'a ShapedGenerated> {
        let at = self.item.generated()?;
        self.line.layout().measured().generated().get(at)
    }

    /// Returns the shaping run its item names.
    ///
    /// The block's first line is set in its own shaping, where it has one.
    fn shaped_run(&self) -> Option<&'a ShapedRun> {
        let runs = &self.line.stages().shaped.runs;
        runs.get(self.item.run()?)
    }

    /// Returns the analysis run of its shaping run.
    ///
    /// A shaping run never crosses an analysis run. The first line's shaping
    /// runs name the same analysis runs as the ordinary ones.
    fn script_run(&self) -> Option<(ScriptRunId, &'a ScriptRun)> {
        let id = self.shaped_run()?.script_run;
        let runs = &self.line.layout().analysis().runs;
        runs.get(id).map(|run| (id, run))
    }

    /// Returns its used font, or `None` where none draws it.
    fn used(&self) -> Option<&'a UsedFont> {
        let id = self.used_id()?;
        self.line.layout().fonts().used.get(id)
    }

    /// Returns its used font's id: its shaping run's font, or for generated
    /// text the font chosen for it.
    fn used_id(&self) -> Option<UsedFontId> {
        if let Some(generated) = self.shaped_generated() {
            return Some(generated.font);
        }
        self.shaped_run().map(|run| run.font)
    }

    /// Returns its font's extent across the line, on the 26.6 grid, as
    /// [`block`](Self::block) reports it.
    pub(super) fn block_extent(&self) -> (LayoutUnit, LayoutUnit) {
        let extent = self.used().map_or(Extent::NONE, |used| {
            let metrics = &used.metrics;
            let scale = self.line.scale();
            if scale == 1.0 {
                Extent::new(metrics.ascent, metrics.descent)
            } else {
                let ascent = metrics.ascent.times_truncated(scale);
                Extent::new(ascent, metrics.descent.times_truncated(scale))
            }
        });
        extent.around(self.item.block)
    }

    /// Returns where its glyphs' baseline is from the line box's top.
    ///
    /// This is its box's baseline less any synthesized raise. On a line with
    /// a central baseline, sideways text sits on the font's alphabetic
    /// baseline, below the central one, while upright and combined glyphs are
    /// placed about the central one.
    fn glyph_baseline(&self) -> LayoutUnit {
        // Glyphs leave the run's baseline only where a style sets
        // `font-variant-position`, which may be synthesized, or on a line
        // with a central baseline. Elsewhere the font lookup is skipped.
        let content = self.line.layout().content();
        if !content.flags.contains(ContentFlags::VARIANT_POSITION)
            && LineBaseline::from_content(content) == LineBaseline::Alphabetic
            && work::fast_paths()
        {
            return self.item.block;
        }
        let raise = self.used().map_or(LayoutUnit::ZERO, |used| {
            let alphabetic = used.metrics.alphabetic;
            let standing = alphabetic != LayoutUnit::ZERO
                && matches!(
                    self.orientation(),
                    RunOrientation::Upright | RunOrientation::Combined
                );
            let under = if standing {
                LayoutUnit::ZERO
            } else {
                alphabetic
            };
            used.synthesis.raise() - under
        });
        let scale = self.line.scale();
        if scale == 1.0 {
            self.item.block - raise
        } else {
            self.item.block - raise.times_truncated(scale)
        }
    }

    /// Returns the walk of its clusters in logical order, each with its exact
    /// start and advance: what [`clusters`](Self::clusters) reads.
    #[inline]
    pub(super) fn cluster_walk(&self) -> ClusterWalk<'a> {
        self.line.cluster_walk(self.item)
    }

    /// Returns its clusters as carets and marks see them, in logical order.
    ///
    /// A cluster's place is its own extent, or its even share of the ligature
    /// it is drawn in, as Blink divides a ligature among its graphemes.
    /// Emphasis marks and carets read these.
    pub(crate) fn places(&self) -> Places<'a> {
        Places::new(
            self.cluster_walk(),
            self.line.stages().shaped,
            self.item.clusters().end,
            self.item.level.is_rtl(),
        )
    }

    /// Returns whether it is combined text, set across its line in one em
    /// (`text-combine-upright`).
    #[inline]
    pub(crate) fn is_combined(&self) -> bool {
        self.orientation() == RunOrientation::Combined
    }

    /// Returns a combined run's clusters as carets and selections see them.
    ///
    /// They come in logical order, each with its start and end across the
    /// line from the line box's top. Returns `None` for a run that is not
    /// combined.
    ///
    /// A combined unit's text is horizontal text set across its em, from the
    /// line's under side toward its over side as it reads. Chrome 153 places a
    /// caret in it as in the horizontal text its `LayoutTextCombine` holds, at
    /// the pen between two clusters. Such a caret runs along the line over the
    /// em, while the rest of a line's carets run across it.
    ///
    /// - A cluster starts where its first glyph is drawn; a base glyph stands
    ///   at its pen.
    /// - A ligature's clusters, and any that draw nothing, share the span to
    ///   the next evenly, as a caret divides a ligature along a line.
    /// - The unit ends where it starts, mirrored about its middle. Its text is
    ///   centred there, between its font's text-over and text-under baselines.
    ///
    /// A line never breaks inside a unit, whose glyphs are the paragraph's.
    pub(crate) fn across_places(&self) -> Option<AcrossPlaces<'a>> {
        if !self.is_combined() {
            return None;
        }
        let layout = self.line.layout();
        let analysis = layout.analysis();
        let (unit, run) = self.script_run()?;
        let unit_end = analysis
            .runs
            .next_start(unit)
            .unwrap_or(analysis.clusters.end_id());
        let baseline = self.glyph_baseline().to_px();
        let shaped = self.line.stages().shaped;
        let middle = baseline - shaped.combine_fit(unit).middle.to_px();
        Some(AcrossPlaces::new(
            &shaped.glyphs,
            (baseline, middle),
            self.item.clusters(),
            run.start..unit_end,
        ))
    }

    /// Returns the exact width of a space in its font.
    ///
    /// The selection's line-end mark is this wide, as Blink draws one space
    /// in the font of the fragment ending the line.
    pub(crate) fn space_width(&self) -> InlineLayoutUnit {
        let layout = self.line.layout();
        let content = layout.content();
        let text = content
            .nodes
            .text_facts(self.item.node, self.line.variant());
        layout
            .fonts()
            .primary_font(content.facts.text_request(text))
            .map_or(InlineLayoutUnit::ZERO, |used| {
                InlineLayoutUnit::from_text(used.metrics.space)
            })
    }
}

impl fmt::Debug for TextRun<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextRun")
            .field("key", &self.key())
            .field("text_range", &self.text_range())
            .field("level", &self.level().get())
            .field("inline", &self.inline())
            .field("baseline", &self.baseline())
            .field("font", &self.font())
            .finish()
    }
}

/// A font instance for rendering a text run.
///
/// Contains font bytes, collection index, pixel size, variation coordinates
/// and synthetic styling. Returned by [`TextRun::font`] and passed to
/// [`FontMetricsProvider`](crate::font::FontMetricsProvider).
///
/// Borrows from the layout without requiring a [`Context`](crate::Context).
/// Use [`key`](Self::key) and coordinates for renderer caches; no internal
/// layout id is required.
///
/// A strike cache must include the key, `coords`, `size`, `embolden`, `skew`,
/// device scale and hinting settings. The face key alone is insufficient.
#[derive(Copy, Clone)]
#[non_exhaustive]
pub struct FontInstance<'a> {
    /// Shared font bytes and their identifier.
    ///
    /// Cloning retains the allocation without copying bytes.
    pub bytes: &'a FontBytes,
    /// The collection index, or zero for a single-font file.
    pub index: u32,
    /// The rendering size in pixels.
    ///
    /// Includes size reductions for synthesized small capitals and positions,
    /// and the scale `text-fit` sets on the line.
    pub size: f32,
    /// Normalized variation coordinates in `fvar` axis order.
    ///
    /// Reflects weight, width, style, optical sizing and face/element variation
    /// settings. Empty for the default instance.
    pub coords: &'a [NormalizedCoord],
    /// Whether to apply synthetic bold.
    pub embolden: bool,
    /// The synthetic slant angle in clockwise CSS degrees, if any.
    pub skew: Option<f32>,
}

impl<'a> FontInstance<'a> {
    /// Returns the font file bytes.
    pub fn data(&self) -> &'a [u8] {
        self.bytes.data()
    }

    /// Returns the fontwich key: the byte-source identifier and collection index.
    pub fn key(&self) -> FontKey {
        FontKey {
            source: self.bytes.id(),
            index: self.index,
        }
    }
}

impl fmt::Debug for FontInstance<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontInstance")
            .field("key", &self.key())
            .field("size", &self.size)
            .field("coords", &self.coords)
            .field("embolden", &self.embolden)
            .field("skew", &self.skew)
            .finish()
    }
}

/// A positioned glyph.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Glyph {
    /// The glyph identifier in the run font.
    pub id: u32,
    /// The glyph origin along the line, relative to line-box left.
    pub x: f32,
    /// The glyph origin across the line, relative to line-box top.
    ///
    /// Equals the run baseline minus the upward glyph offset.
    pub y: f32,
    /// The pen advance in pixels.
    ///
    /// Includes shaping advance and, on the last glyph of a cluster, any
    /// additional cluster spacing. Glyph advances sum to the cluster advance.
    pub advance: f32,
    /// The cluster start as a byte offset into [`Layout::text`](crate::Layout::text).
    pub text_offset: usize,
}

/// Where a run's glyphs come from.
///
/// One exists per run being read, on the reader's stack and never in a
/// collection, so the text walk's larger size costs nothing. Boxing it, as
/// the lint asks, would allocate on every read.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
enum Source<'a> {
    /// The content's text: the paragraph's glyphs, and the line's own at its
    /// reshaped edges.
    Text(GlyphWalk<'a>),
    /// Generated text's glyphs, shaped once during measurement, drawn from
    /// the run's start with their own advances.
    ///
    /// The text is one character, from `left`, `advance` wide, its glyphs
    /// sharing one text offset.
    Generated {
        glyphs: Iter<'a, SidecarGlyph>,
        pen: InlineLayoutUnit,
        offset: usize,
        left: InlineLayoutUnit,
        advance: InlineLayoutUnit,
    },
}

/// A text run's glyphs, in drawing order: what [`TextRun::glyphs`] walks.
#[derive(Clone)]
pub(crate) struct RunGlyphs<'a> {
    source: Source<'a>,
    clusters: &'a ClusterTable,
    /// The run's baseline from the line box's top, less any synthesized
    /// raise, in pixels.
    baseline: f32,
    /// The scale `text-fit` sets the line's text in, applied to generated
    /// text's glyphs; the text walk scales its own.
    scale: f32,
}

impl<'a> RunGlyphs<'a> {
    /// Creates the walk of `run`'s glyphs, in drawing order.
    #[inline]
    pub(crate) fn new(run: &TextRun<'a>) -> Self {
        let layout = run.line.layout();
        let baseline = run.glyph_baseline().to_px();
        let clusters = &layout.analysis().clusters;
        // Each arm builds the walk in place, so the text walk, a few hundred
        // bytes, is not copied out.
        match run.shaped_generated() {
            Some(generated) => {
                let glyphs = layout.measured().generated().glyphs(generated);
                Self {
                    source: Source::Generated {
                        glyphs: glyphs.iter(),
                        pen: run.item.inline,
                        offset: clusters.start(run.item.clusters().start).get(),
                        left: run.item.inline,
                        advance: run.item.advance(),
                    },
                    clusters,
                    baseline,
                    scale: run.line.scale(),
                }
            }
            None => Self {
                source: Source::Text(run.line.glyph_walk(run.item)),
                clusters,
                baseline,
                scale: 1.0,
            },
        }
    }
}

impl RunGlyphs<'_> {
    /// Returns the next glyph, with its character's start along the line and
    /// advance, in pixels.
    ///
    /// The character is its cluster, as the walk placed it, or generated
    /// text's whole run. Text on a path turns the character about its middle.
    #[inline]
    pub(crate) fn next_placed(&mut self) -> Option<(Glyph, f32, f32)> {
        match &mut self.source {
            Source::Text(walk) => {
                let (glyph, placed) = walk.next_placed()?;
                let drawn = drawn(glyph, self.baseline, self.clusters);
                // A compact cluster's place is its one glyph's.
                let (left, step) = placed.map_or((drawn.x, drawn.advance), |(left, step)| {
                    (left.to_px(), step.to_px())
                });
                Some((drawn, left, step))
            }
            Source::Generated { left, advance, .. } => {
                let (left, advance) = (left.to_px(), advance.to_px());
                Some((self.next()?, left, advance))
            }
        }
    }
}

/// Converts a text item's exact glyph to pixels, with the run's glyph
/// baseline `baseline` from the line box's top and its text offset from
/// `clusters`.
#[inline]
fn drawn(glyph: ExactGlyph, baseline: f32, clusters: &ClusterTable) -> Glyph {
    Glyph {
        id: glyph.id,
        x: glyph.x.to_px(),
        y: baseline - glyph.y.to_px(),
        advance: glyph.advance.to_px(),
        text_offset: clusters.start(glyph.cluster).get(),
    }
}

impl Iterator for RunGlyphs<'_> {
    type Item = Glyph;

    #[inline]
    fn next(&mut self) -> Option<Glyph> {
        match &mut self.source {
            Source::Text(walk) => {
                let glyph = walk.next()?;
                Some(drawn(glyph, self.baseline, self.clusters))
            }
            Source::Generated {
                glyphs,
                pen,
                offset,
                left,
                ..
            } => {
                // The pen runs at the text's own size from the run's left; on
                // a line `text-fit` scales, offsets scale by `scale`.
                let glyph = glyphs.next()?;
                let (dx, dy) = (glyph.x_offset, glyph.y_offset);
                let advance = InlineLayoutUnit::from_text(glyph.advance);
                let x = *pen + InlineLayoutUnit::from_text(dx);
                *pen += advance;
                let scale = self.scale;
                Some(Glyph {
                    id: glyph.id(),
                    x: left.to_px() + (x - *left).to_px() * scale,
                    y: self.baseline - dy.to_px() * scale,
                    advance: advance.to_px() * scale,
                    text_offset: *offset,
                })
            }
        }
    }
}

/// A text cluster and its position within a run.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Cluster {
    start: usize,
    end: usize,
    left: f32,
    advance: f32,
}

impl Cluster {
    /// Returns the byte range in [`Layout::text`](crate::Layout::text).
    pub fn text_range(&self) -> Range<usize> {
        self.start..self.end
    }

    /// Returns the cluster advance in pixels.
    pub fn advance(&self) -> f32 {
        self.advance
    }

    /// Returns inline extents relative to line-box left.
    ///
    /// For RTL clusters, the logical start is on the right.
    pub fn inline(&self) -> InlineExtents {
        InlineExtents {
            left: self.left,
            right: self.left + self.advance,
        }
    }
}

/// A text run's clusters, in logical order: what [`TextRun::clusters`]
/// walks.
#[derive(Clone)]
pub(crate) struct RunClusters<'a> {
    walk: ClusterWalk<'a>,
    clusters: &'a ClusterTable,
}

impl<'a> RunClusters<'a> {
    /// Creates the walk of `run`'s clusters, in logical order.
    pub(crate) fn new(run: &TextRun<'a>) -> Self {
        Self {
            walk: run.cluster_walk(),
            clusters: &run.line.layout().analysis().clusters,
        }
    }
}

impl Iterator for RunClusters<'_> {
    type Item = Cluster;

    fn next(&mut self) -> Option<Cluster> {
        let (cluster, left, advance) = self.walk.next()?;
        let next = ClusterId::new(cluster.get() + 1);
        Some(Cluster {
            start: self.clusters.start(cluster).get(),
            end: self.clusters.start(next).get(),
            left: left.to_px(),
            advance: advance.to_px(),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.walk.size_hint()
    }
}

impl ExactSizeIterator for RunClusters<'_> {}

// The pixel extents the views return, made from exact positions.
impl InlineExtents {
    /// Returns the extents from `left` to `right`, exact positions along the
    /// line, in pixels.
    #[inline]
    pub(crate) fn from_exact(left: InlineLayoutUnit, right: InlineLayoutUnit) -> Self {
        Self {
            left: left.to_px(),
            right: right.to_px(),
        }
    }
}

impl CrossExtents {
    /// Returns the extents of a line box `height` pixels tall.
    #[inline]
    pub(crate) fn from_line_height(height: f32) -> Self {
        Self {
            over: 0.0,
            under: height,
        }
    }
}
