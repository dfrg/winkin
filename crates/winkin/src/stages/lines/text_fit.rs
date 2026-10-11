//! `text-fit`: the scale each line's text takes to fill its line.
//!
//! Lines break at the text's own size; a line's scale is worked out after it
//! is fitted and before its height, as Chrome's `LineFitter` does. A
//! per-line target scales each line by itself. `consistent` lays the block
//! out once, takes the least scale its lines ask for, as Chrome's
//! `MeasurePerBlockScale` does, and lays the block out again with that scale
//! on every line.
//!
//! What scales is a line's text, less the letter- and word-spacing set as
//! fixed lengths. Atomic inlines, box edges, tabs, fixed spacing and the
//! line's edge costs keep their size. A per-line scale counts `text-indent`
//! in neither part, as Chrome does.
//!
//! Only a block that sets `text-fit` and has no float, initial letter or
//! ruby gets here, as Chrome's `ShouldApplyTextFit` has it. Other content
//! pays one test a line.

use core::ops::Range;

use super::height::primary_metrics;
use super::placement::LinePlacements;
use super::{Breaker, EdgeShape, Fitted, LineBand, Lines};
use crate::data::Id;
use crate::stages::analysis::{ClusterClass, ClusterId};
use crate::stages::content::{
    Content, ContentFlags, ItemFlags, ItemId, ItemKind, NodeId, TextFactsId, TextLineHeight,
};
use crate::stages::fonts::FontLineMetrics;
use crate::stages::measure::{Extent, LetterWordSpacing, WordSpacingRule};
use crate::stages::{LineStages, Stages};
use crate::style::{TextFitKind, TextFitTarget, VerticalAlign};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// Returns how far a line may fall short of its room, or overflow it, and
/// still take no scale: 2 px, Chrome's epsilon at a device pixel ratio of 1.
fn epsilon() -> LayoutUnit {
    LayoutUnit::from_px(2.0)
}

/// Returns `scale`, or 1 where it is not a finite positive number.
///
/// A line's record holds only such a scale, so nothing a reader multiplies
/// by it goes wrong.
fn usable_scale(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// The block's `text-fit`, where it applies, and the scale `consistent`
/// sets every line in once it is measured.
#[derive(Copy, Clone, Debug)]
pub(super) struct TextFitting {
    kind: TextFitKind,
    target: TextFitTarget,
    /// The limit's bound on a scale: the most text grows, or the least it
    /// shrinks to.
    bound: f32,
    /// The scale of every line, on `consistent`'s second layout.
    block: Option<f32>,
}

impl TextFitting {
    /// Returns the fitting `content`'s block asks for, or `None` where it
    /// sets no `text-fit` or has a float, an initial letter or ruby.
    pub(super) fn new(content: &Content) -> Option<Self> {
        let fit = content.text_fit();
        let excluded = content.flags.contains(ContentFlags::FLOATS)
            || content.flags.contains(ContentFlags::RUBY)
            || content.block.initial_letter.is_some();
        (fit.kind != TextFitKind::None && !excluded).then_some(Self {
            kind: fit.kind,
            target: fit.target,
            bound: fit.bound,
            block: None,
        })
    }

    /// Returns whether the block shares one scale, still to be measured on
    /// a first layout of its lines.
    pub(super) fn measures_block(&self) -> bool {
        self.target == TextFitTarget::Consistent && self.block.is_none()
    }

    /// Returns the fitting that sets every line at `scale`.
    pub(super) fn with_block_scale(self, scale: f32) -> Self {
        Self {
            block: Some(scale),
            ..self
        }
    }

    /// Returns `scale` held to the limit, as Chrome's `RestrictScale` holds
    /// it.
    fn restrict(&self, scale: f32) -> f32 {
        match self.kind {
            TextFitKind::Shrink => scale.max(self.bound),
            TextFitKind::Grow | TextFitKind::None => scale.min(self.bound),
        }
    }

    /// Returns whether a line `missing` short of its room scales: it is not
    /// within [`epsilon`] of it, and the kind goes the way it needs.
    fn scales(&self, missing: LayoutUnit) -> bool {
        let wanted = match self.kind {
            TextFitKind::Grow => missing > LayoutUnit::ZERO,
            TextFitKind::Shrink => missing < LayoutUnit::ZERO,
            TextFitKind::None => false,
        };
        wanted && missing.max(-missing) >= epsilon()
    }
}

/// What a line's text takes along it: all of it, and the part of that set
/// by fixed letter- and word-spacing.
#[derive(Copy, Clone, Debug, Default)]
struct LineText {
    text: InlineLayoutUnit,
    fixed: InlineLayoutUnit,
}

impl LineText {
    /// Returns the part that scales.
    fn flexible(self) -> InlineLayoutUnit {
        self.text - self.fixed
    }

    /// Returns the scale that makes `room` hold the flexible part, or `None`
    /// where there is none.
    fn scale_into(self, room: InlineLayoutUnit) -> Option<f32> {
        let flexible = self.flexible();
        (flexible > InlineLayoutUnit::ZERO).then(|| room.to_px() / flexible.to_px())
    }

    /// Returns `width`, which holds this text, with its flexible part
    /// scaled by `scale`.
    fn scaled_width(self, width: LayoutUnit, scale: f32) -> LayoutUnit {
        let flexible = self.flexible();
        (InlineLayoutUnit::from_layout(width) - flexible + flexible.times(scale)).to_layout()
    }
}

impl LineStages<'_> {
    /// Returns what the text of the line holding `clusters`, from item
    /// `from` on, takes along it.
    ///
    /// A cluster `reshaped` gives an advance takes that one, as a reshaped
    /// line edge does. Tabs count as no text, as they reach their stops
    /// whatever the scale.
    fn line_text(
        &self,
        from: ItemId,
        clusters: Range<ClusterId>,
        words: WordSpacingRule,
        reshaped: impl Fn(ClusterId) -> Option<InlineLayoutUnit>,
    ) -> LineText {
        let Range { start, end } = clusters;
        let (content, analysis) = (self.content, self.analysis);
        let mut near = None;
        let mut sum = LineText::default();
        self.items_on_line(from, start, end, |id, held| {
            let Some(item) = content.items.get(id) else {
                return;
            };
            if item.kind != ItemKind::Text || item.flags.contains(ItemFlags::ANNOTATION) {
                return;
            }
            let shaping = content.facts.text(self.text_facts(item.node)).shaping;
            let fixed = LetterWordSpacing::from_fixed(&content.facts, shaping, words);
            for cluster in held.start.max(start).get()..held.end.min(end).get() {
                work::step();
                let cluster = ClusterId::new(cluster);
                if analysis.clusters.class(cluster) == Some(ClusterClass::Tab) {
                    continue;
                }
                sum.text += reshaped(cluster)
                    .unwrap_or_else(|| self.cluster_advance_near(cluster, &mut near));
                if !fixed.is_none() {
                    let continuation = self.shaped.glyphs.word(cluster).is_continuation();
                    let after = fixed.after_cluster(analysis, self.text(), cluster, continuation);
                    sum.fixed += InlineLayoutUnit::from_text(after);
                }
            }
        });
        sum
    }
}

/// What a line whose text is scaled reads of its height, as Chrome's
/// `InlineBoxState::ComputeTextMetrics` and `ComputeAlignmentBaselineShift`
/// scale them.
impl LineStages<'_> {
    /// Returns the used line height of text with the facts `text`, whose
    /// primary font has `metrics`, in a line scaled by `scale`.
    ///
    /// `normal` and a number scale with the text; a length, a percentage
    /// among them, keeps its size.
    fn scaled_line_height(
        &self,
        text: TextFactsId,
        metrics: &FontLineMetrics,
        scale: f32,
    ) -> LayoutUnit {
        match self.content.facts.text(text).line_height {
            TextLineHeight::Normal => metrics.normal_line_height().times_truncated(scale),
            TextLineHeight::Factor(height) => height.times_truncated(scale),
            TextLineHeight::Length(px) => LayoutUnit::from_px(px),
        }
    }

    /// Returns the strut of text with the facts `text` in a line scaled by
    /// `scale`: its primary font's ascent and descent scaled, around its
    /// scaled line height.
    ///
    /// The leading splits as the measurements split it.
    pub(super) fn scaled_strut(&self, text: TextFactsId, scale: f32) -> Extent {
        let Some(metrics) = primary_metrics(self, text) else {
            return Extent::NONE;
        };
        let ascent = metrics.ascent.times_truncated(scale);
        let descent = metrics.descent.times_truncated(scale);
        let leading = self.scaled_line_height(text, metrics, scale) - (ascent + descent);
        let over = leading.half().floor_px();
        Extent::new(ascent + over, descent + (leading - over))
    }

    /// Returns the extent of item `item`, `extent` as measured, in a line
    /// scaled by `scale`.
    ///
    /// Text, a forced break and a box's opening edge take their scaled
    /// strut. An atomic inline keeps its margin box.
    pub(super) fn scaled_extent(&self, item: ItemId, extent: Extent, scale: f32) -> Extent {
        let Some(found) = self.content.items.get(item) else {
            return extent;
        };
        let scales =
            found.kind == ItemKind::Text || found.kind == ItemKind::Break || found.kind.is_open();
        if extent.is_none() || !scales {
            return extent;
        }
        self.scaled_strut(self.text_facts(found.node), scale)
    }

    /// Returns how much further `vertical-align: align` raises box `node` in
    /// a line scaled by `scale` than measurement raised it.
    ///
    /// `super` and `sub` take the parent's scaled font size, and a
    /// percentage the box's scaled line height. A length and the keywords
    /// placed by the line keep theirs.
    pub(super) fn scaled_shift(
        &self,
        node: NodeId,
        align: VerticalAlign,
        scale: f32,
    ) -> LayoutUnit {
        let facts = &self.content.facts;
        let measured = self.measured.shift(node);
        match align {
            VerticalAlign::Super | VerticalAlign::Sub => {
                let parent = self.text_facts(self.content.nodes.parent(node));
                let size = facts
                    .request(facts.text_request(parent))
                    .font
                    .computed_size();
                let ratio = |size: LayoutUnit| {
                    let one = LayoutUnit::from_px(1.0);
                    if align == VerticalAlign::Super {
                        size.divided(3) + one
                    } else {
                        -(size.divided(5) + one)
                    }
                };
                let size = LayoutUnit::from_px(size);
                // Under the size rule the shift is the size's; under the
                // font's own offsets it scales with them.
                if measured == ratio(size) {
                    ratio(size.times_truncated(scale)) - measured
                } else {
                    measured.times_truncated(scale) - measured
                }
            }
            VerticalAlign::Fraction(fraction) => {
                let text = self.text_facts(node);
                let Some(metrics) = primary_metrics(self, text) else {
                    return LayoutUnit::ZERO;
                };
                let height = self.scaled_line_height(text, metrics, 1.0);
                let scaled = self.scaled_line_height(text, metrics, scale);
                LayoutUnit::from_px_truncated(scaled.to_px() * fraction)
                    - LayoutUnit::from_px_truncated(height.to_px() * fraction)
            }
            _ => LayoutUnit::ZERO,
        }
    }
}

/// The scale of a line, as more of the breaker's `impl`.
impl<'a: 'c, 'c, 'm, 'provider> Breaker<'a, 'c, 'm, 'provider> {
    /// Returns the scale the line `fitted`, from `start` and item
    /// `first_item`, takes in `band`, and scales its width and what hangs
    /// past it.
    ///
    /// `ends_paragraph` says it is its paragraph's last line, which
    /// `per-line` leaves unscaled. On `consistent`'s first layout every line
    /// keeps its size.
    #[inline(never)]
    pub(super) fn fit_text(
        &self,
        out: &Lines,
        fitting: TextFitting,
        (first_item, start): (ItemId, ClusterId),
        band: LineBand,
        fitted: &mut Fitted,
        ends_paragraph: bool,
    ) -> f32 {
        if fitting.measures_block()
            || (fitting.target == TextFitTarget::PerLine
                && ends_paragraph
                && fitting.block.is_none())
        {
            return 1.0;
        }
        let pieces = fitted.pieces;
        let clusters = start..fitted.content_end;
        let reshaped = |at| pieces.advance(out, at);
        let text = (self.stages).line_text(first_item, clusters, self.words, reshaped);
        let scale = match fitting.block {
            Some(scale) => scale,
            None => {
                let missing = band.width() - (fitted.indent + fitted.width);
                if !fitting.scales(missing) {
                    return 1.0;
                }
                // The room the text has: the band less what does not scale.
                let room =
                    InlineLayoutUnit::from_layout(band.width() - fitted.width) + text.flexible();
                match text.scale_into(room) {
                    Some(scale) => fitting.restrict(scale),
                    None => return 1.0,
                }
            }
        };
        let scale = usable_scale(scale);
        if scale != 1.0 {
            fitted.width = text.scaled_width(fitted.width, scale);
            fitted.hang.space = fitted.hang.space.times_truncated(scale);
            fitted.hang.unconditional = fitted.hang.unconditional.times_truncated(scale);
        }
        scale
    }
}

/// Returns the one scale `consistent` sets every line of `out` in, as
/// Chrome's `MeasurePerBlockScale` measures it: the least that fills a line,
/// held to the limit.
///
/// It skips lines within [`epsilon`] of their room, lines with no text, and
/// lines whose text cannot take up what they overflow by. Where the least
/// scale goes the way the kind does not, it is 1.
pub(super) fn block_scale(
    stages: Stages<'_>,
    fitting: TextFitting,
    out: &Lines,
    placements: &LinePlacements,
) -> f32 {
    let words = stages.measured.word_spacing_rule;
    let mut least = f32::INFINITY;
    for ((_, line), facts) in out.lines.iter().zip(placements.as_slice()) {
        let missing = line.band.width() - (line.indent + line.width);
        if missing.max(-missing) < epsilon() {
            continue;
        }
        let stages = stages.variant(line.variant());
        let edges = out.edges.line_edges(line);
        let reshaped = |cluster| {
            let at = edges
                .iter()
                .find_map(|edge: &EdgeShape| edge.entry(cluster))?;
            (out.edges.advances.get(at)).map(|&advance| InlineLayoutUnit::from_text(advance))
        };
        let clusters = line.clusters().start..facts.content_end;
        let text = stages.line_text(line.first_item, clusters, words, reshaped);
        let room = InlineLayoutUnit::from_layout(missing) + text.flexible();
        if room <= InlineLayoutUnit::ZERO {
            continue;
        }
        if let Some(scale) = text.scale_into(room) {
            least = least.min(scale);
        }
    }
    if !least.is_finite() {
        return 1.0;
    }
    let scale = usable_scale(fitting.restrict(least));
    let wanted = match fitting.kind {
        TextFitKind::Grow => scale > 1.0,
        TextFitKind::Shrink => scale < 1.0,
        TextFitKind::None => false,
    };
    if wanted { scale } else { 1.0 }
}
