//! Primary-font metrics, one row per text facts row.

use super::tabs::TabStops;
use super::{Extent, MeasureInput, TextMetrics};
use crate::data::Table;
use crate::stages::content::{NodeId, TextFactsId, TextFlags, TextLineHeight};
use crate::stages::fonts::FontLineMetrics;
use crate::style::FirstLineVariant;
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::{unit, work};

/// Appends to `out` the [`TextMetrics`] of each text facts row, from its
/// primary font.
///
/// Both variants read the one table. `block_space` is the space advance
/// tabs use where the block's primary font maps no U+0020.
pub(super) fn text_metrics(
    input: &MeasureInput<'_>,
    block_space: Option<TextUnit>,
    out: &mut Table<TextFactsId, TextMetrics>,
) {
    let facts = &input.content.facts;
    out.reserve(facts.text_ids().len());
    // Tabs count the block's space, as Blink's `InlineNode::FontForTab`.
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

/// Returns the used line height, as Blink's `ComputedLineHeightAsFixed`
/// makes it.
///
/// `normal` takes the font's line spacing. A factor arrives resolved and
/// truncated by the builder. A length rounds to the 26.6 grid and saturates.
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
/// the rest under (Blink's `CalculateLeadingSpace`): a baseline lands at 87
/// where an even split gives 87.6.
pub(super) fn leaded(metrics: &FontLineMetrics, height: LayoutUnit) -> Extent {
    let (ascent, descent) = (metrics.ascent, metrics.descent);
    let leading = height - (ascent + descent);
    let over = leading.half().floor_px();
    Extent::new(ascent + over, descent + (leading - over))
}

/// Returns a font's ascent and descent with its `line-height: normal`
/// leading.
pub(crate) fn normal_extent(metrics: &FontLineMetrics) -> Extent {
    leaded(metrics, metrics.normal_line_height())
}

/// Returns the em box an emphasis mark is drawn in.
///
/// It is the primary font's em box at half of `size`, rounded to a whole
/// pixel (`lroundf`); the ascent rounds to 1/64. Without font
/// metrics, four fifths go over the baseline.
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
