//! Text placement along curves, matching SVG `textPath`.
//!
//! Break lines normally, then map their paint operations onto a path.
//! Wrapping, justification, hyphenation, ellipses, bidi order and annotations
//! use the same layout as straight lines. The mapping does not modify layout.
//!
//! [`PathRoom`] supplies each line with its path length minus a start offset.
//! It uses line indices, so repeated fitting or balancing selects the same
//! path. [`paints`] positions characters at their advance midpoints and
//! rotates each character as a unit, preserving offsets between base glyphs
//! and marks (SVG 2, section 11.8.3).
//!
//! The path is the alphabetic baseline for horizontal lines and the central
//! baseline for vertical lines. Vertical upright and combined glyphs stand
//! across the path, with their top toward its start. Combined text retains
//! [`TextRun::combine_scale`](crate::TextRun::combine_scale).
//!
//! Hyphens, ellipses, ruby, emphasis and atomic inlines follow the path.
//! Decorations are split per character, matching SVG and Chrome.
//! Backgrounds and borders are omitted. The host places floats.
//!
//! [`PastEnds::Hidden`] omits characters with midpoints outside the path;
//! [`PastEnds::Straight`] extends the endpoint tangents. Closed paths do not
//! wrap back to the start, matching current browsers rather than SVG 2.
//!
//! SVG `side="right"` maps to [`ReversedPath`], `startOffset` to [`PathRoom`],
//! and `text-anchor` to line alignment. Placement uses `method="align"` and
//! `spacing="exact"`. Renderers can implement stretched outlines or smooth
//! decorations using [`TextPath::point`].
//!
//! Implement [`TextPath`] with an existing curve library, or use [`Polyline`]
//! and [`BezierPath`] with supplied measurement buffers. These operations
//! do not allocate.
//!
//! ```
//! use winkin::paint::Decorates;
//! use winkin::path::{self, PagePoint, PastEnds, PathPaint, PathRoom, Polyline};
//! use winkin::{BuildOptions, ComputedBlockStyle, ComputedStyle, Context, Layout, NodeKey};
//! # use fontwich::{Collection, LayerBuilder, Role};
//! # let mut fonts = LayerBuilder::new(Role::Application);
//! # fonts.add_data(include_bytes!("../../../../support/testing/fonts/Ahem.ttf").as_slice()).unwrap();
//! # let fonts = Collection::new().with_layer(fonts.snapshot());
//!
//! let mut cx = Context::new(fonts);
//! let mut layout = Layout::new();
//! let style = ComputedStyle::initial();
//! let mut builder = layout.builder(&mut cx, NodeKey(0), &ComputedBlockStyle::new(&style), BuildOptions::default());
//! builder.text(NodeKey(1), "Up and over the hill");
//! builder.finish();
//!
//! // Two paths, a roof and a flat line after it, measured into a buffer
//! // each; each line is broken to its own path's length.
//! let roof = [
//!     PagePoint::new(0.0, 100.0),
//!     PagePoint::new(60.0, 40.0),
//!     PagePoint::new(120.0, 100.0),
//! ];
//! let flat = [PagePoint::new(0.0, 150.0), PagePoint::new(400.0, 150.0)];
//! let (mut roof_ends, mut flat_ends) = ([0.0; 3], [0.0; 2]);
//! let paths = [Polyline::new(&roof, &mut roof_ends), Polyline::new(&flat, &mut flat_ends)];
//! let mut room = PathRoom::new(&paths, 0.0);
//! layout.break_lines(&mut cx, room.area(), &mut room);
//! assert_eq!(layout.lines().len(), 2);
//!
//! // Each line along its path: every glyph where its character's middle
//! // falls on it, turned to the path there.
//! for (line, path) in layout.lines().zip(&paths) {
//!     for paint in path::paints(&line, path, PastEnds::Hidden, |_| Decorates::None) {
//!         if let PathPaint::Text(run) = paint {
//!             let font = run.run().font().expect("Ahem draws it");
//!             for placed in run.glyphs() {
//!                 // Draw `placed.glyph.id` in `font` at the origin, through
//!                 // `placed.place.transform()`.
//!                 let [a, b, _, _, x, y] = placed.place.transform();
//!                 assert!(x.is_finite() && y.is_finite() && (a * a + b * b - 1.0).abs() < 1e-3);
//!                 # let _ = (&font, placed.glyph);
//!             }
//!         }
//!     }
//! }
//! ```

mod curves;
mod paint;
mod room;
#[cfg(test)]
mod tests;

pub use curves::{BezierPath, Polyline};
pub use paint::{PathDecoration, PathGlyph, PathPaint, PathPiece, PathTextRun, Placement};
pub use room::PathRoom;

use crate::paint::Decorates;
use crate::{Line, NodeKey};
use paint::PathWalk;

/// Returns paint operations positioned along `path` in Chrome paint order.
///
/// Uses [`Line::paints`] and the supplied `decorates` function. Characters
/// rotate at their advance midpoints; glyph offsets remain relative to
/// the character. The line starts at [`LineMetrics::left`](crate::LineMetrics::left),
/// including path offset and alignment. Baseline offsets become path offsets.
///
/// `past_ends` controls characters whose midpoints lie outside the path.
/// Does not allocate.
pub fn paints<'a, P, F>(
    line: &Line<'a>,
    path: &'a P,
    past_ends: PastEnds,
    decorates: F,
) -> impl Iterator<Item = PathPaint<'a, P>> + use<'a, P, F>
where
    P: TextPath + ?Sized,
    F: Fn(NodeKey) -> Decorates,
{
    PathWalk::new(line, path, past_ends, line.paints(decorates))
}

/// A curve for positioning text in page coordinates.
///
/// Positions beside the curve use distance along the path and distance above
/// it. Above is a quarter turn counterclockwise from the tangent, with page
/// y increasing downward. For a downward path, above is page-right.
///
/// Nonfinite lengths or points omit affected placements without panicking.
pub trait TextPath {
    /// Returns the path length.
    fn length(&self) -> f32;

    /// Returns the point and unit tangent at `distance` along the path.
    ///
    /// Queried only between zero and [`length`](Self::length).
    /// [`PastEnds`] handles out-of-range placement using the endpoints.
    fn at(&self, distance: f32) -> PathPoint;

    /// Returns a point at distances `along` the path and `up` above it.
    ///
    /// Extends endpoint tangents outside the path. Can be used to bend
    /// outlines or decorations point by point, as in SVG `method="stretch"`.
    /// Returns NaN coordinates if the path provides no valid placement.
    fn point(&self, along: f32, up: f32) -> PagePoint {
        let length = self.length();
        PastEnds::Straight
            .frame(self, length, along)
            .map_or(PagePoint::new(f32::NAN, f32::NAN), |at| at.offset(0.0, up))
    }
}

impl<P: TextPath + ?Sized> TextPath for &P {
    fn length(&self) -> f32 {
        (**self).length()
    }

    fn at(&self, distance: f32) -> PathPoint {
        (**self).at(distance)
    }
}

/// A point in page coordinates, with y increasing downward.
///
/// Used by [`Polyline`], [`BezierPath`], [`TextPath::point`] and
/// [`PathPoint::offset`].
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct PagePoint {
    /// The horizontal page coordinate.
    pub x: f32,
    /// The vertical page coordinate.
    pub y: f32,
}

impl PagePoint {
    /// Creates a point at `(x, y)`.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A point on a [`TextPath`] with a unit tangent.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct PathPoint {
    /// The horizontal page coordinate.
    pub x: f32,
    /// The vertical page coordinate.
    pub y: f32,
    /// The horizontal component of the unit tangent.
    pub dx: f32,
    /// The vertical component of the unit tangent.
    pub dy: f32,
}

impl PathPoint {
    /// Returns a point offset by `along` along the tangent and `up` above it.
    ///
    /// Above is a quarter turn counterclockwise from the tangent.
    pub fn offset(&self, along: f32, up: f32) -> PagePoint {
        PagePoint {
            x: self.x + self.dx * along + self.dy * up,
            y: self.y + self.dy * along - self.dx * up,
        }
    }

    /// Whether every part of it is a number and finite: what a path that
    /// answers otherwise leaves out.
    fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.dx.is_finite() && self.dy.is_finite()
    }
}

/// A path traversed from end to start, matching SVG `side="right"`.
///
/// Reverses text direction along the path and positions it on the opposite side.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct ReversedPath<P>(pub P);

impl<P: TextPath> TextPath for ReversedPath<P> {
    fn length(&self) -> f32 {
        self.0.length()
    }

    fn at(&self, distance: f32) -> PathPoint {
        let at = self.0.at(self.0.length() - distance);
        PathPoint {
            dx: -at.dx,
            dy: -at.dy,
            ..at
        }
    }
}

/// Placement behavior for characters with midpoints outside a path.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum PastEnds {
    /// Omits characters with midpoints outside the path.
    ///
    /// Matches SVG 2, section 11.8.3, and Chrome.
    #[default]
    Hidden,
    /// Extends each endpoint tangent to draw out-of-range characters.
    ///
    /// Allows the full line to be drawn beyond the path. SVG and Chrome
    /// omit these characters instead.
    Straight,
}

impl PastEnds {
    /// Where `path`, `length` long, is `along` it, and the way it runs
    /// there: `None` where that is past its ends and they hide what is
    /// there, or where the path answers something that is not a finite
    /// number.
    fn frame<P: TextPath + ?Sized>(self, path: &P, length: f32, along: f32) -> Option<PathPoint> {
        let at = if (0.0..=length).contains(&along) {
            path.at(along)
        } else {
            match self {
                Self::Hidden => return None,
                Self::Straight => {
                    // From the nearer end, straight on: NaN goes nowhere,
                    // as the point it makes is no number.
                    let end = if along < 0.0 { 0.0 } else { length.max(0.0) };
                    let at = path.at(end);
                    let past = along - end;
                    PathPoint {
                        x: at.x + at.dx * past,
                        y: at.y + at.dy * past,
                        ..at
                    }
                }
            }
        };
        at.is_finite().then_some(at)
    }
}
