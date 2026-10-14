//! Exclusions that give each line the length of its own path.
//!
//! [`Exclusions::band`] receives the line index, so line `i` gets path `i`
//! however often and in whatever order the breaker asks: at a new height,
//! when balancing or `pretty` relays a paragraph, or on relayout. Counting
//! calls instead would give a relaid paragraph the wrong paths.

use super::TextPath;
use crate::stages::lines::{
    Area, BlockExtents, Exclusions, ExclusionsCheckpoint, FloatRequest, InlineExtents,
    NoExclusions, PlacedFloat,
};

/// Available space for line `i` along `paths[i]`.
///
/// Implements [`Exclusions`] for path layout. Each line starts at distance
/// `start`, matching SVG `startOffset`. Alignment uses the remaining length;
/// [`paints`](super::paints) maps the result to the path.
///
/// Lines beyond the supplied paths reuse the final path length; the host
/// chooses where to draw them. With no paths, lines have zero available width
/// and may overflow.
///
/// Break lines using [`area`](Self::area), which covers all path bands.
/// Floats are placed as in [`NoExclusions`] and do not reduce line width.
#[derive(Copy, Clone, Debug)]
pub struct PathRoom<'a, P> {
    paths: &'a [P],
    start: f32,
}

impl<'a, P: TextPath> PathRoom<'a, P> {
    /// Creates path space with each line starting at distance `start`.
    pub fn new(paths: &'a [P], start: f32) -> Self {
        Self { paths, start }
    }

    /// Returns an area covering all path bands.
    ///
    /// Extends from the smaller of zero and the start offset to the longest
    /// path end. Supplies no annotation space above the first line.
    pub fn area(&self) -> Area {
        let left = self.start.min(0.0);
        let right = self
            .paths
            .iter()
            .map(TextPath::length)
            .filter(|length| length.is_finite())
            .fold(self.start.max(0.0), f32::max);
        Area {
            inline: InlineExtents { left, right },
            ..Area::new(0.0)
        }
    }
}

impl<P: TextPath> Exclusions for PathRoom<'_, P> {
    fn band(&self, line: usize, _block: BlockExtents) -> InlineExtents {
        let length = self
            .paths
            .get(line)
            .or(self.paths.last())
            .map_or(self.start, TextPath::length);
        // A path shorter than the start, or with a NaN length, leaves no
        // room.
        InlineExtents {
            left: self.start,
            right: length.max(self.start),
        }
    }

    fn below(&self, _top: f32) -> Option<f32> {
        None
    }

    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        NoExclusions.place(float)
    }

    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(0)
    }

    fn rewind(&mut self, _to: ExclusionsCheckpoint) {}
}
