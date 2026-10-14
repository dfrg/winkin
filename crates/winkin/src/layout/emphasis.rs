//! Emphasis marks: where each mark over or under a text run goes.
//!
//! - The crate places marks; the host draws them. The host draws the shape
//!   and colour by the run's key, as Chrome draws the style's mark string in
//!   the text's font at half its size (`Font::DrawEmphasisMarks`). Each mark
//!   has its middle along the line, its baseline across it, and its size.
//!   The host centres the glyph's ink bounds on that middle, upright in
//!   vertical text too, as Blink does (`AddEmphasisMark`, `BoundsForGlyph`).
//! - Each grapheme cluster that takes a mark gets one. Its middle is the
//!   cluster's, less half the cluster's letter-spacing, as Blink centres a
//!   mark (`AddEmphasisMark`). A ligature's advance is shared evenly among
//!   the clusters it draws (`AddEmphasisMarkToBloberizer`).
//!   `text-emphasis-skip` picks the clusters by the class of each one's first
//!   character (`unicode::EmphasisClass`). Chrome reads no
//!   `text-emphasis-skip` and always skips what its initial value,
//!   `spaces punctuation`, skips.
//! - Combined text takes one mark, in the middle of its em, whatever its
//!   characters. Blink's `TextCombinePainter::PaintEmphasisMark` marks a
//!   `LayoutTextCombine` as a hiragana; CSS Writing Modes 3 marks it as a
//!   U+FFFC.
//! - The baseline is Blink's (`TextPainter::SetEmphasisMark`). The mark's
//!   em box sits against the text's em box on the mark's side, its offset
//!   from the run's baseline floored over and ceiled under. In a ruby base
//!   with an annotation on that side, it sits against the outermost such
//!   annotation's em box (`SetTextEmphasisAnnotationMetrics`). Line layout
//!   sets that box on the line, and the breaker makes room for it.
//!
//! Reading the marks allocates nothing: they come from the run's places,
//! the walk carets read too.

use crate::stages::analysis::{ClusterId, Clusters as ClusterTable, RunOrientation};
use crate::stages::content::{NodeKey, TextFlags, VariantText};
use crate::stages::fragments::FragmentItem;
use crate::stages::measure::{Extent, RubySide, em_box};
use crate::style::{EmphasisSide, EmphasisSkip};
use crate::unicode;
use crate::unit::{InlineLayoutUnit, LayoutUnit};

use crate::layout::{Line, Places, TextRun};
use crate::work;

/// The position and size of an emphasis mark.
///
/// Draw the mark specified by the style at font size `size`, on `baseline`,
/// with the center of its glyph's ink bounds on `x`. Center the ink, not the
/// advance or the font's ascent and descent, in vertical text as in
/// horizontal text: those move the mark by the font's bearings, or by where
/// its ink sits between its ascent and descent. This matches Chrome.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct EmphasisMark {
    /// The node key of the marked text.
    ///
    /// Its style determines the mark shape and color.
    pub key: NodeKey,
    /// The center of the mark's ink along the line, relative to the line
    /// box's left edge.
    pub x: f32,
    /// The baseline's offset from the line box's top.
    pub baseline: f32,
    /// The font size in pixels: half the text size, rounded to a pixel.
    pub size: f32,
    /// The cluster start, as a byte offset into
    /// [`Layout::text`](crate::Layout::text).
    pub text_offset: usize,
}

/// A text run's emphasis marks: what [`TextRun::emphasis_marks`] walks, and
/// what a line's paint yields after each run.
pub(super) struct EmphasisMarks<'a> {
    /// The run's clusters where the marks go, `None` where it has no marks.
    places: Option<Places<'a>>,
    /// The text as its line draws it.
    text: VariantText<'a>,
    clusters: &'a ClusterTable,
    key: NodeKey,
    skip: EmphasisSkip,
    /// The marks' baseline and size, in pixels.
    baseline: f32,
    size: f32,
    /// Half the letter-spacing each cluster carries, by which a mark moves
    /// back.
    half_spacing: InlineLayoutUnit,
    /// Combined text's one mark, not yet returned: its middle along the line,
    /// and the run's start in the text.
    combined: Option<(InlineLayoutUnit, usize)>,
}

impl<'a> EmphasisMarks<'a> {
    /// Creates the walk of `run`'s marks, in the logical order of its
    /// clusters.
    pub(super) fn new(run: &TextRun<'a>) -> Self {
        let line = run.line();
        let layout = line.layout();
        let item = run.item();
        let content = layout.content();
        // The block's first line is set in its first-line styles, where it
        // has its own.
        let variant = line.variant();
        let facts = &content.facts;
        let id = content.nodes.text_facts(item.node, variant);
        let text = facts.text(id);
        // The mark's em box, from the text's metrics, where it sets marks.
        let mark = layout
            .measured()
            .text_metrics(id)
            .filter(|_| text.has(TextFlags::EMPHASIS))
            .map(|metrics| {
                // On a line `text-fit` scales, the mark scales with the text.
                let scale = line.scale();
                let mark = metrics.mark;
                if scale == 1.0 {
                    mark
                } else {
                    let ascent = mark.ascent().times_truncated(scale);
                    Extent::new(ascent, mark.descent().times_truncated(scale))
                }
            });
        let combined = item.is_marked() && run.orientation() == RunOrientation::Combined;
        let (places, baseline, size) = match mark {
            Some(mark) if item.is_marked() => {
                let places = run.places();
                let offset = mark_offset(line, item, text.emphasis_side, mark);
                (
                    Some(places),
                    (item.block + offset).to_px(),
                    mark.height().to_px(),
                )
            }
            _ => (None, 0.0, 0.0),
        };
        Self {
            places,
            text: content.text(variant),
            clusters: &layout.analysis().clusters,
            key: content.nodes.key(item.node),
            skip: text.emphasis_skip,
            baseline,
            size,
            half_spacing: InlineLayoutUnit::from_text(facts.shaping(text.shaping).letter).half(),
            combined: (combined && mark.is_some()).then(|| {
                let left = item.inline;
                let right = left + item.advance();
                let start = layout.analysis().clusters.start(item.clusters().start);
                (left + (right - left).half(), start.get())
            }),
        }
    }

    /// Returns whether `cluster` takes a mark under the run's
    /// `text-emphasis-skip`, by its first character.
    fn takes(&self, cluster: ClusterId) -> bool {
        let Some(ch) = self.clusters.first_char(self.text, cluster) else {
            return false;
        };
        let rare = unicode::rare_props(ch);
        !self.skip.skips(rare.emphasis(), rare.is_wide())
    }
}

impl Iterator for EmphasisMarks<'_> {
    type Item = EmphasisMark;

    fn next(&mut self) -> Option<EmphasisMark> {
        if let Some((middle, text_offset)) = self.combined.take() {
            // No other mark: the unit is one character.
            self.places = None;
            return Some(EmphasisMark {
                key: self.key,
                x: middle.to_px(),
                baseline: self.baseline,
                size: self.size,
                text_offset,
            });
        }
        loop {
            work::step();
            // The cluster's own place, or its share of the ligature it is
            // drawn in.
            let (cluster, from, to) = self.places.as_mut()?.next()?;
            if !self.takes(cluster) {
                continue;
            }
            let middle = from + (to - from).half();
            return Some(EmphasisMark {
                key: self.key,
                x: (middle - self.half_spacing).to_px(),
                baseline: self.baseline,
                size: self.size,
                text_offset: self.clusters.start(cluster).get(),
            });
        }
    }
}

/// Returns the offset of a mark's baseline from the baseline of text `item`
/// on `line`, for a mark with em box `mark` on `side`.
///
/// The mark sits past the text's em box, or past the outermost annotation on
/// that side of the ruby column whose base holds the text, rounded out to
/// whole pixels.
fn mark_offset(
    line: Line<'_>,
    item: &FragmentItem,
    side: EmphasisSide,
    mark: Extent,
) -> LayoutUnit {
    let layout = line.layout();
    let (content, fonts, variant) = (layout.content(), layout.fonts(), line.variant());
    // The item's kept em box, where its fonts share one; otherwise the em
    // box of its font runs, found by search.
    let kept = item
        .text_item()
        .and_then(|at| line.stages().measured().em_boxes().get(at));
    let em = kept.unwrap_or_else(|| {
        let clusters = item.clusters();
        fonts
            .runs(variant)
            .containing(clusters.start)
            .map_or(Extent::NONE, |first| {
                let text = content.nodes.text_facts(item.node, variant);
                let request = content.facts.text_request(text);
                em_box(fonts, request, clusters, variant, first).0
            })
    });
    let (over, under) = (em.zero_if_none().ascent(), em.zero_if_none().descent());
    // On a line `text-fit` scales, the em box scales with the text.
    let scale = line.scale();
    let (over, under) = if scale == 1.0 {
        (over, under)
    } else {
        (over.times_truncated(scale), under.times_truncated(scale))
    };
    // Past an annotation of its column on that side, where there is one.
    let past = annotation_reach(line, item, RubySide::from(side));
    match side {
        EmphasisSide::Over => -(past.unwrap_or(over) + mark.descent()).ceil_px(),
        EmphasisSide::Under => (past.unwrap_or(under) + mark.ascent()).ceil_px(),
    }
}

/// Returns how far the outermost annotation on `side` of the ruby column
/// whose base holds `item` reaches from `item`'s baseline on `line`.
///
/// The reach runs to the far edge of the annotation's em box. Returns `None`
/// where the column has no annotation on that side there.
fn annotation_reach(line: Line<'_>, item: &FragmentItem, side: RubySide) -> Option<LayoutUnit> {
    let layout = line.layout();
    let rubies = line.stages().measured().ruby_columns();
    if rubies.is_empty() {
        return None;
    }
    let level = rubies.outermost_annotation(item.clusters().start, side)?;
    let extent = rubies.level(level)?.extent;
    let head = layout.fragments().annotation_line(line.id(), level)?;
    let baseline = item.block;
    Some(match side {
        RubySide::Over => baseline - (head.block - extent.ascent().max(LayoutUnit::ZERO)),
        RubySide::Under => head.block + extent.descent().max(LayoutUnit::ZERO) - baseline,
    })
}
