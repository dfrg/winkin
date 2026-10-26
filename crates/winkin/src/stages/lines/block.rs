//! The block result: its end, its first and last baselines, and its
//! `text-box-trim`.
//!
//! Breaking works the result out before line layout writes an item, so
//! nothing is placed and then moved. Every line moves up by the start trim;
//! the block's end moves up by both trims.

use super::{LinePlacements, Lines, TextBoxTrims};
use crate::unit::LayoutUnit;

/// The block's end, baselines and trims, as its host reads them.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct BlockResult {
    /// How far `text-box-trim` moves every line up: the first line's excess
    /// over its trim edge, negative where it falls short, zero with no lines.
    pub(crate) trim_start: LayoutUnit,
    /// How far `text-box-trim` pulls the block's end up: the last line's
    /// excess.
    pub(crate) trim_end: LayoutUnit,
    /// Where the block's content ends after the trims, in the host's
    /// positions: the last line box's bottom, or the area's start with no
    /// lines, at least past a clearing break. Floats are not counted.
    pub(crate) block_end: LayoutUnit,
    /// The first line's baseline after the trim, for an inline-block's or a
    /// flex item's host; `None` with no lines.
    pub(crate) first_baseline: Option<LayoutUnit>,
    /// The last line's baseline after the trim; `None` with no lines.
    pub(crate) last_baseline: Option<LayoutUnit>,
    /// Whether `text-box-trim` trims the block's end, which then lends no
    /// room below it; false where the block has no font to trim by.
    pub(crate) trims_end: bool,
}

impl BlockResult {
    /// A block with no lines, at zero.
    pub(crate) const EMPTY: Self = Self {
        trim_start: LayoutUnit::ZERO,
        trim_end: LayoutUnit::ZERO,
        block_end: LayoutUnit::ZERO,
        first_baseline: None,
        last_baseline: None,
        trims_end: false,
    };

    /// Returns the result of a block whose lines are final.
    ///
    /// `end` is where the next line box would start, in the records'
    /// positions. `trim_start` is the trim the lines were stacked and fitted
    /// with. `cleared` is where a clearing break after the last line moved
    /// the block's end, in the host's positions. That clearance applies
    /// after the trim, so the trim never pulls the end above it.
    pub(super) fn new(
        lines: &Lines,
        placements: &LinePlacements,
        end: LayoutUnit,
        trim_start: LayoutUnit,
        cleared: Option<LayoutUnit>,
        trims: Option<TextBoxTrims>,
    ) -> Self {
        let records = lines.lines.as_slice();
        let (first, last) = (records.first(), records.last());
        let trim_start = if first.is_some() {
            trim_start
        } else {
            LayoutUnit::ZERO
        };
        let trim_end = last.map_or(LayoutUnit::ZERO, |line| {
            trims.map_or(LayoutUnit::ZERO, |trims| {
                let scale = lines.lines.last_id().map_or(1.0, |id| lines.scale(id));
                trims.end(line.extent, scale, line.variant())
            })
        });
        let trimmed_end = end - trim_start - trim_end;
        let mut block = Self {
            trim_start,
            trim_end,
            block_end: cleared.map_or(trimmed_end, |cleared| trimmed_end.max(cleared)),
            first_baseline: None,
            last_baseline: None,
            trims_end: trims.is_some_and(|trims| trims.ends),
        };
        block.first_baseline = first
            .zip(placements.as_slice().first())
            .map(|(line, facts)| block.top(facts.block_start) + line.ascent());
        block.last_baseline = last
            .zip(placements.as_slice().last())
            .map(|(line, facts)| block.top(facts.block_start) + line.ascent());
        block
    }

    /// Returns a line's final top, given its untrimmed `block_start`.
    pub(crate) fn top(&self, block_start: LayoutUnit) -> LayoutUnit {
        block_start - self.trim_start
    }
}
