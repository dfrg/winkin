//! The host's side of line breaking: the area lines fill and the floats they
//! flow around.
//!
//! Positions are line-relative: inline from a line's line-left end, block
//! from the block-start edge. Lengths are `f32` pixels, rounded onto the
//! 1/64 px `LayoutUnit` grid as the breaker reads them.

use crate::build::FloatSide;
use crate::stages::content::NodeKey;

/// Extents along a line, from line-left to line-right.
///
/// In vertical writing these are the column top and bottom. Bands and floats
/// are relative to the area's line-left edge; runs, boxes, carets and
/// selection rectangles to the line box's left edge.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct InlineExtents {
    /// The line-left end.
    pub left: f32,
    /// The line-right end.
    pub right: f32,
}

impl InlineExtents {
    /// Unbounded inline space, clipped to [`Area`] during line breaking.
    pub const EVERYTHING: Self = Self {
        left: f32::NEG_INFINITY,
        right: f32::INFINITY,
    };

    /// Returns the inline size, or zero if the endpoints are reversed.
    pub fn size(&self) -> f32 {
        let size = self.right - self.left;
        if size > 0.0 { size } else { 0.0 }
    }
}

/// Flow-relative extents from block-start to block-end.
///
/// [`CrossExtents`](crate::CrossExtents) measures from line-over instead.
/// The two differ only in `vertical-lr`, where lines stack from the left but
/// line-over is on the right.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct BlockExtents {
    /// The block-start end.
    pub start: f32,
    /// The block-end end.
    pub end: f32,
}

impl BlockExtents {
    /// Returns the block size, or zero if the endpoints are reversed.
    pub fn size(&self) -> f32 {
        let size = self.end - self.start;
        if size > 0.0 { size } else { 0.0 }
    }
}

/// The available area for line breaking.
///
/// Change it and call [`break_lines`](crate::Layout::break_lines) to relayout
/// without repeating preparation.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Area {
    /// The inline extents of the block's content box.
    ///
    /// These set the tab-stop origin and the basis of a percentage `text-indent`.
    pub inline: InlineExtents,
    /// The block position where the first line box begins.
    pub block_start: f32,
    /// The block-end limit from `height` or `max-height`, if any.
    ///
    /// Only `line-clamp: auto` reads it, to keep the lines that fit. `None` lets
    /// the content set the block size.
    pub block_end: Option<f32>,
    /// The space above block-start that first-line annotations may use.
    ///
    /// Ruby and emphasis marks fill it before the first line moves down. It
    /// includes the start padding and, where no border intervenes, the
    /// collapsed start margin and the preceding block's
    /// [`Layout::room_below`](crate::Layout::room_below). Defaults to zero;
    /// negative and NaN values count as zero.
    pub room_above: f32,
}

impl Area {
    /// Creates an area with `width` pixels of inline space starting at zero.
    ///
    /// Block-start is zero, with no room above and no block-end limit. Zero,
    /// negative and NaN widths give no space, so each line overflows with the
    /// content it must keep. An infinite width saturates the 1/64 px grid at
    /// about 33.5 million pixels.
    pub fn new(width: f32) -> Self {
        Self {
            inline: InlineExtents {
                left: 0.0,
                right: width,
            },
            block_start: 0.0,
            block_end: None,
            room_above: 0.0,
        }
    }

    /// Returns the available inline size, as [`inline.size()`](InlineExtents::size).
    pub fn inline_size(&self) -> f32 {
        self.inline.size()
    }
}

/// A float or initial letter to place through [`Exclusions::place`].
///
/// The host looks up properties such as `clear` and the float shape by key.
///
/// An initial letter uses the key of the box with `initial-letter`. Its
/// margin box runs from the line top, or above it for a raised letter, to the
/// bottom of the fitted ink. A provider should:
/// - place it at `block_start`, even above an earlier float;
/// - place later same-side floats and clearing blocks below it, whatever
///   their `clear`;
/// - place a block that begins with an initial letter below earlier initial
///   letters on either side.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct FloatRequest {
    /// The node key of the float.
    pub key: NodeKey,
    /// The side the float floats to.
    pub side: FloatSide,
    /// The nonnegative margin-box size along the line.
    ///
    /// The supplied border box plus the style margins, each truncated to the
    /// 1/64 px grid.
    pub inline_size: f32,
    /// The nonnegative margin-box size across the lines, on the same grid.
    pub block_size: f32,
    /// The lowest block-start position the float may take.
    ///
    /// This is the line top where the float fits beside the line, and the line
    /// bottom otherwise. The host may place the float lower.
    pub block_start: f32,
}

/// The margin-box position of a float, as [`Exclusions::place`] returns it.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct PlacedFloat {
    /// The margin box along the line.
    pub inline: InlineExtents,
    /// The margin box across the block.
    pub block: BlockExtents,
}

/// A host-defined checkpoint for speculative float placement.
///
/// A host that only appends floats can store the float count.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct ExclusionsCheckpoint(pub u64);

/// Float placement and available inline space for each line.
///
/// Implement it on the enclosing block formatting context so that floats
/// from adjacent blocks affect this layout.
///
/// A line's band is first queried at the block's strut height, then again at
/// the line's height, refitting at most twice if the band narrows. Bands are
/// clipped to [`Area`]. A NaN band gives no space.
///
/// Queries identify a line by [`Line::index`](crate::Line::index), and may
/// repeat or come out of order while balancing or pretty wrapping. A
/// provider such as [`path::PathRoom`](crate::path::PathRoom) should key on
/// the index, not count queries.
///
/// Floats are placed in logical order, at the line top or below the line,
/// and must stay put so that refitting converges. A rejected trial break
/// restores them through [`checkpoint`](Self::checkpoint) and
/// [`rewind`](Self::rewind).
pub trait Exclusions {
    /// Returns the inline space for line `line` over the block extents `block`.
    fn band(&self, line: usize, block: BlockExtents) -> InlineExtents;

    /// Returns the next block position below `top` where the band changes.
    ///
    /// An overflowing line that floats narrow may move down to it, at most 256
    /// times. Returns `None` if the band never changes; a result not below `top`
    /// is ignored.
    fn below(&self, top: f32) -> Option<f32>;

    /// Places a float and returns its margin-box position.
    ///
    /// Keep the placement so that later bands account for it. A placement
    /// undone by [`rewind`](Self::rewind) may be requested again.
    fn place(&mut self, float: FloatRequest) -> PlacedFloat;

    /// Returns a checkpoint for speculative placement.
    fn checkpoint(&self) -> ExclusionsCheckpoint;

    /// Discards the placements made after `to`.
    fn rewind(&mut self, to: ExclusionsCheckpoint);
}

/// An exclusion provider that gives each line the full [`Area`].
///
/// It places each float at line-left from its `block_start` and keeps none.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct NoExclusions;

impl Exclusions for NoExclusions {
    fn band(&self, _line: usize, _block: BlockExtents) -> InlineExtents {
        InlineExtents::EVERYTHING
    }

    fn below(&self, _top: f32) -> Option<f32> {
        None
    }

    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        PlacedFloat {
            inline: InlineExtents {
                left: 0.0,
                right: float.inline_size,
            },
            block: BlockExtents {
                start: float.block_start,
                end: float.block_start + float.block_size,
            },
        }
    }

    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(0)
    }

    fn rewind(&mut self, _to: ExclusionsCheckpoint) {}
}
