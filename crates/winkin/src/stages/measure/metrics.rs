//! Primary-font metrics resolved once per text-facts row, shared by variants.

use super::tabs::TabStops;
use super::{Extent, MeasureInput, TextMetrics};
use crate::data::Table;
use crate::stages::content::{NodeId, TextFactsId, TextFlags, TextLineHeight};
use crate::stages::fonts::FontLineMetrics;
use crate::style::FirstLineVariant;
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::{unit, work};

/// Writes into `out` the [`TextMetrics`] of each text facts row in its
/// primary font.
///
/// `out` has one row per [`TextFactsId`], the first line's included, so both
/// variants read one table. A document has few rows, and each looks up its
/// primary font once. `block_space` is the space the block's tabs count
/// where its primary font maps none.
pub(super) fn text_metrics(
    input: &MeasureInput<'_>,
    block_space: Option<TextUnit>,
    out: &mut Table<TextFactsId, TextMetrics>,
) {
    let facts = &input.content.facts;
    out.reserve(facts.text_ids().len());
    // A tab in spaces counts the block container's space: its primary font's
    // U+0020 at the used size with its letter-spacing and word-spacing, as
    // Chrome sizes a tab in `InlineNode::FontForTab`. Where the primary maps
    // no space, it is the space of the listed font a space falls back to, as
    // in Chrome.
    let block = input
        .content
        .nodes
        .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
    let block_shaping = facts.shaping(facts.text(block).shaping);
    let space = block_space.unwrap_or_else(|| {
        input
            .primary(block)
            .map_or(TextUnit::from_px(0.0), |metrics| metrics.space)
    });
    for text in facts.text_ids() {
        work::step();
        let row = facts.text(text);
        let primary = input.primary(text);
        let (strut, line_height) = primary.map_or((Extent::NONE, LayoutUnit::ZERO), |metrics| {
            let height = line_height(row.line_height, metrics);
            (leaded(metrics, height), height)
        });
        let tab = TabStops::new(
            row.tab_size,
            InlineLayoutUnit::from_text(space),
            block_shaping.letter,
            block_shaping.word,
        );
        let mark = if row.has(TextFlags::EMPHASIS) {
            mark(primary, input.size(text))
        } else {
            Extent::NONE
        };
        out.push_bounded(
            TextMetrics {
                strut,
                line_height,
                tab,
                mark,
            },
            "a row a text's facts, which a TextFactsId names",
        );
    }
}

/// Returns the used line height, as Chrome's `ComputedLineHeightAsFixed`
/// makes it.
///
/// `normal` takes the font's own line spacing. The builder already resolves
/// a factor against the computed size, truncated onto the grid, as Blink
/// keeps a number as a percentage of the size. A length rounds onto the grid
/// and saturates.
fn line_height(height: TextLineHeight, metrics: &FontLineMetrics) -> LayoutUnit {
    match height {
        TextLineHeight::Normal => metrics.normal_line_height(),
        TextLineHeight::Factor(height) => height,
        TextLineHeight::Length(px) => LayoutUnit::from_px(px),
    }
}

/// Returns a font's ascent and descent with the leading that `height` adds.
///
/// Half the leading, floored to a whole pixel, goes over the baseline and
/// the rest under, as Blink's `CalculateLeadingSpace` splits it. Chrome puts
/// a baseline at 87 where splitting evenly would put it at 87.6.
pub(super) fn leaded(metrics: &FontLineMetrics, height: LayoutUnit) -> Extent {
    let (ascent, descent) = (metrics.ascent, metrics.descent);
    let leading = height - (ascent + descent);
    let over = leading.half().floor_px();
    Extent::new(ascent + over, descent + (leading - over))
}

/// Returns a font's ascent and descent with the leading of its own line
/// height, which widens text under `line-height: normal`.
pub(crate) fn normal_extent(metrics: &FontLineMetrics) -> Extent {
    leaded(metrics, metrics.normal_line_height())
}

/// Returns the em box an emphasis mark is drawn in.
///
/// The box is the primary font's em box at half of `size`, rounded to a
/// whole pixel (`lroundf`), as Blink normalizes it. The ascent rounds to the
/// nearest 1/64 and the descent takes the rest of the em. Without font
/// metrics, four fifths of the em go over.
fn mark(primary: Option<&FontLineMetrics>, size: f32) -> Extent {
    let size = unit::round_to_whole(size * 0.5).max(0.0);
    let em = primary.map(|metrics| (metrics.em_over.to_px(), metrics.em_under.to_px()));
    let ratio = match em {
        Some((over, under)) if over + under > 0.0 => over / (over + under),
        _ => 0.8,
    };
    let ascent = LayoutUnit::from_px(ratio * size);
    Extent::new(ascent, LayoutUnit::from_px(size) - ascent)
}
