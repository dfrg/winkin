//! The block result: its end, its first and last baselines, and its
//! `text-box-trim`.
//!
//! The result is worked out once when breaking finishes, before line layout
//! writes an item, so nothing is placed and then moved. Every line moves up
//! by the start trim, so the lines keep their spacing. The block's end moves
//! up by both trims. `TextBoxTrims` in `trim` works out the amounts.

use super::{LinePlacements, Lines, TextBoxTrims};
use crate::unit::LayoutUnit;

/// The block result, which the block's host reads.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct BlockResult {
    /// How far `text-box-trim` moves every line up.
    ///
    /// It is the first line's excess over its trim edge, negative where the
    /// line box falls short of the edge.
    pub(crate) trim_start: LayoutUnit,
    /// How far `text-box-trim` pulls the block's end up: the last line's
    /// excess.
    pub(crate) trim_end: LayoutUnit,
    /// Where the block's content ends, after the trim.
    ///
    /// It is the last line box's bottom, or the area's start with no lines.
    /// The host adds floats.
    pub(crate) block_end: LayoutUnit,
    /// The first line's baseline after the trim, for an inline-block's or a
    /// flex item's host.
    pub(crate) first_baseline: Option<LayoutUnit>,
    /// The last line's baseline after the trim.
    pub(crate) last_baseline: Option<LayoutUnit>,
    /// `text-box-trim` trims the block's end, which then lends no room
    /// below it.
    ///
    /// It is false where the block has no font to trim by.
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

    /// Finishes the block once breaking has chosen its final lines.
    ///
    /// `trim_start` is the amount already used to stack and fit the lines.
    /// `cleared` is where a clearing break after the last line moved the
    /// block's end, in the host's positions. Chrome adds that clearance
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
            trims.map_or(LayoutUnit::ZERO, |trims| trims.end(line.extent))
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

    /// Returns a line's final top from the breaker's untrimmed position.
    pub(crate) fn top(&self, block_start: LayoutUnit) -> LayoutUnit {
        block_start - self.trim_start
    }
}
