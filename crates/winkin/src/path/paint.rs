//! A line's paint placed along a path.
//!
//! Each [`Paint`] that [`Line::paints`] yields turns to the path at its
//! character's middle.
//!
//! - Positions come from the straight-line paint: a run's
//!   [`inline`](TextRun::inline) extent, a glyph's `x` and `y`, and the
//!   places of marks and decorations. Only each character's middle is
//!   computed here, from its cluster's advance.
//! - A character turns as one (SVG 2, section 11.8.3, and Blink's
//!   `SvgTextLayoutAlgorithm::PositionOnPath`). A cluster's glyphs turn about
//!   the cluster's middle, so a mark keeps its offset from its base across a
//!   bend. A ligature is one character. Combined text is one character,
//!   turning about its em's middle.
//! - Nothing is buffered. The glyph walk gives each glyph its cluster's start
//!   and advance, so a character's middle is known at its first glyph. A
//!   cluster that draws nothing still advances the characters after it.
//! - [`PathTextRun::glyphs`] walks one run, so a renderer selects the font
//!   once per run. [`PathDecoration::pieces`] walks one decoration.

use core::fmt;

use super::{PagePoint, PastEnds, PathPoint, TextPath};
use crate::layout::{RunClusters, RunGlyphs};
use crate::paint::{Decoration, DecorationRuns, EmphasisMark, Paint};
use crate::{Atomic, Glyph, InlineExtents, Line, RunOrientation, TextRun};

/// A page position, rotation and horizontal scale for drawing on a path.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct Placement {
    /// The horizontal page coordinate of the origin.
    pub x: f32,
    /// The vertical page coordinate of the origin.
    pub y: f32,
    /// The cosine of the clockwise rotation angle.
    ///
    /// With `sin`, forms the unit direction of the local x axis.
    /// Page y increases downward.
    pub cos: f32,
    /// The sine of the clockwise rotation angle.
    pub sin: f32,
    /// The scale along the local x axis.
    ///
    /// Equals `1.0` except for combined text narrowed to one em;
    /// see [`TextRun::combine_scale`].
    pub scale: f32,
}

impl Placement {
    /// Returns the affine transform `[a, b, c, d, e, f]`.
    ///
    /// Maps `(x, y)` to `(a*x + c*y + e, b*x + d*y + f)`, applying
    /// scale, rotation and translation to drawing at the local origin.
    pub fn transform(&self) -> [f32; 6] {
        [
            self.cos * self.scale,
            self.sin * self.scale,
            -self.sin,
            self.cos,
            self.x,
            self.y,
        ]
    }
}

/// A paint operation positioned along a path.
///
/// Yielded by [`paints`](super::paints) in [`Line::paints`] order.
/// Each character rotates at its midpoint. Draw in the local straight-line
/// frame, then apply the placement transform.
///
/// Text and decoration views provide [`PathTextRun::glyphs`] and
/// [`PathDecoration::pieces`] for individual placements.
#[non_exhaustive]
pub enum PathPaint<'a, P: ?Sized> {
    /// An underline or overline painted before the text.
    ///
    /// Covers one text run, split into one piece per character, matching SVG
    /// and Chrome.
    DecorationBeforeText(PathDecoration<'a, P>),
    /// A run of ruby annotation text, on the annotation's baseline.
    ///
    /// Annotations paint after the rest of the line.
    Annotation(PathTextRun<'a, P>),
    /// A run of text.
    Text(PathTextRun<'a, P>),
    /// An emphasis mark, after the run whose text it marks.
    ///
    /// The placement origin is the center of the mark on its baseline. The
    /// mark rotates with the glyphs of the marked run.
    Emphasis {
        /// The mark as placed on the straight line.
        mark: EmphasisMark,
        /// The position of the mark center on the path, and its rotation.
        place: Placement,
    },
    /// An atomic inline, which the host paints.
    ///
    /// The placement origin is the line-left, line-over corner of the margin
    /// box; the box extends along the path and toward line-under.
    Atomic {
        /// The atomic inline as placed on the straight line.
        atomic: Atomic<'a>,
        /// The position of the margin-box corner on the path, and its rotation.
        place: Placement,
    },
    /// Text the layout generates: a hyphen or an ellipsis, where it stands on
    /// the line.
    Generated(PathTextRun<'a, P>),
    /// A line through the text, painted after it.
    ///
    /// Covers one text run, split into one piece per character.
    DecorationAfterText(PathDecoration<'a, P>),
}

impl<P: ?Sized> Clone for PathPaint<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ?Sized> Copy for PathPaint<'_, P> {}

impl<P: ?Sized> fmt::Debug for PathPaint<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DecorationBeforeText(decoration) => f
                .debug_tuple("DecorationBeforeText")
                .field(decoration)
                .finish(),
            Self::Annotation(run) => f.debug_tuple("Annotation").field(run).finish(),
            Self::Text(run) => f.debug_tuple("Text").field(run).finish(),
            Self::Emphasis { mark, place } => f
                .debug_struct("Emphasis")
                .field("mark", mark)
                .field("place", place)
                .finish(),
            Self::Atomic { atomic, place } => f
                .debug_struct("Atomic")
                .field("atomic", atomic)
                .field("place", place)
                .finish(),
            Self::Generated(run) => f.debug_tuple("Generated").field(run).finish(),
            Self::DecorationAfterText(decoration) => f
                .debug_tuple("DecorationAfterText")
                .field(decoration)
                .finish(),
        }
    }
}

/// A path and where one line lies on it.
struct OnPath<'a, P: ?Sized> {
    path: &'a P,
    length: f32,
    ends: PastEnds,
    /// The distance along the path of the line box's left edge, including
    /// the start offset and alignment.
    left: f32,
    /// The baseline's distance below the line box's top. The baseline lies
    /// on the path.
    baseline: f32,
}

impl<P: ?Sized> Clone for OnPath<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ?Sized> Copy for OnPath<'_, P> {}

impl<P: TextPath + ?Sized> OnPath<'_, P> {
    /// Returns the path point and tangent for something centred `middle`
    /// along the line, or `None` where it is not drawn.
    #[inline]
    fn frame(&self, middle: f32) -> Option<PathPoint> {
        self.ends.frame(self.path, self.length, self.left + middle)
    }

    /// Places the line point `(x, y)`, along the line and down from its top,
    /// of something centred `middle` along the line and turned to `frame`.
    ///
    /// `standing` turns it to stand across the path; `scale` narrows its x
    /// axis.
    #[inline]
    fn place(
        &self,
        frame: PathPoint,
        middle: f32,
        (x, y): (f32, f32),
        standing: bool,
        scale: f32,
    ) -> Placement {
        let PagePoint { x, y } = frame.offset(x - middle, self.baseline - y);
        // The local x axis follows the tangent. Standing, it points up from
        // the path and the local y axis follows the tangent.
        let (cos, sin) = if standing {
            (frame.dy, -frame.dx)
        } else {
            (frame.dx, frame.dy)
        };
        Placement {
            x,
            y,
            cos,
            sin,
            scale,
        }
    }
}

/// The iterator behind [`paints`](super::paints).
///
/// Places each glyph, emphasis mark, atomic inline and decoration piece at
/// its character's middle on the path, turned to the tangent there. The line
/// starts at [`LineMetrics::left`](crate::LineMetrics::left) along the path.
/// Its baseline lies on the path, and offsets from the baseline become
/// offsets from the path.
pub(super) struct PathWalk<'a, P: ?Sized, I> {
    /// The line's straight-line paint.
    paint: I,
    line: Line<'a>,
    on: OnPath<'a, P>,
    /// Whether the last run's glyphs stand across the path. Its emphasis
    /// marks, which follow it, stand too.
    standing: bool,
}

impl<'a, P: TextPath + ?Sized, I> PathWalk<'a, P, I> {
    /// Creates the walk of `line` along `path` from its straight-line
    /// `paint`.
    pub(super) fn new(line: &Line<'a>, path: &'a P, ends: PastEnds, paint: I) -> Self {
        let metrics = line.metrics();
        Self {
            paint,
            line: *line,
            on: OnPath {
                path,
                length: path.length(),
                ends,
                left: metrics.left,
                baseline: metrics.ascent,
            },
            standing: false,
        }
    }

    /// Wraps `run` and records whether its marks stand.
    fn run(&mut self, run: TextRun<'a>) -> PathTextRun<'a, P> {
        self.standing = stands(&run);
        PathTextRun { run, on: self.on }
    }

    /// Wraps `decoration` for placement along the path.
    fn decoration(&self, decoration: Decoration) -> PathDecoration<'a, P> {
        PathDecoration {
            decoration,
            line: self.line,
            on: self.on,
        }
    }
}

impl<'a, P, I> Iterator for PathWalk<'a, P, I>
where
    P: TextPath + ?Sized,
    I: Iterator<Item = Paint<'a>>,
{
    type Item = PathPaint<'a, P>;

    fn next(&mut self) -> Option<PathPaint<'a, P>> {
        loop {
            return Some(match self.paint.next()? {
                Paint::Text(run) => PathPaint::Text(self.run(run)),
                Paint::Annotation(run) => PathPaint::Annotation(self.run(run)),
                Paint::Generated(run) => PathPaint::Generated(self.run(run)),
                Paint::DecorationBeforeText(bar) => {
                    PathPaint::DecorationBeforeText(self.decoration(bar))
                }
                Paint::DecorationAfterText(bar) => {
                    PathPaint::DecorationAfterText(self.decoration(bar))
                }
                Paint::Emphasis(mark) => {
                    let Some(frame) = self.on.frame(mark.x) else {
                        continue;
                    };
                    let at = (mark.x, mark.baseline);
                    let place = self.on.place(frame, mark.x, at, self.standing, 1.0);
                    PathPaint::Emphasis { mark, place }
                }
                Paint::Atomic(atomic) => {
                    let InlineExtents { left, right } = atomic.inline();
                    let middle = (left + right) / 2.0;
                    let Some(frame) = self.on.frame(middle) else {
                        continue;
                    };
                    let at = (left, atomic.block().over);
                    let place = self.on.place(frame, middle, at, false, 1.0);
                    PathPaint::Atomic { atomic, place }
                }
                // SVG paints no backgrounds behind text, nor does Chrome on a
                // `textPath`.
                Paint::Background(_) | Paint::Box(_) => continue,
            });
        }
    }
}

/// Returns whether `run`'s glyphs stand across a path: upright or combined
/// in a vertical line.
fn stands(run: &TextRun<'_>) -> bool {
    matches!(
        run.orientation(),
        RunOrientation::Upright | RunOrientation::Combined
    )
}

/// A text run with glyph placements along a path.
///
/// Yielded as [`PathPaint::Text`], [`PathPaint::Annotation`] or
/// [`PathPaint::Generated`]. Placements include glyph offsets; draw at the
/// local origin using the font from [`run`](Self::run).
///
/// Horizontal and sideways glyphs follow the path tangent. Upright and
/// combined vertical glyphs rotate a quarter turn clockwise, with their
/// top toward the path start. Characters with midpoints outside the
/// path are omitted under [`PastEnds::Hidden`].
pub struct PathTextRun<'a, P: ?Sized> {
    run: TextRun<'a>,
    on: OnPath<'a, P>,
}

impl<P: ?Sized> Clone for PathTextRun<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ?Sized> Copy for PathTextRun<'_, P> {}

impl<'a, P: TextPath + ?Sized> PathTextRun<'a, P> {
    /// Returns the underlying text run, including its node, font and orientation.
    pub fn run(&self) -> TextRun<'a> {
        self.run
    }

    /// Returns the glyphs with their path placements, in drawing order.
    ///
    /// Does not allocate.
    pub fn glyphs(&self) -> impl Iterator<Item = PathGlyph> + use<'a, P> {
        PathGlyphs::new(self.run, self.on)
    }
}

/// A glyph and its path placement.
///
/// Yielded by [`PathTextRun::glyphs`]. Draw at the local origin and apply
/// `place`. Glyph offsets are already included in the placement.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct PathGlyph {
    /// The glyph, with its straight-line position.
    ///
    /// Its id, advance and text offset are unchanged; `place` gives its
    /// position on the path.
    pub glyph: Glyph,
    /// The path placement.
    pub place: Placement,
}

/// The part of a decoration line under one character on a path.
///
/// Yielded by [`PathDecoration::pieces`]. Draw the bar from the placement
/// origin across the inline extent, at its straight-line baseline offset,
/// then apply the placement transform.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct PathPiece {
    /// The extent along the line, from the line box's left edge.
    pub inline: InlineExtents,
    /// The path placement, with its origin at the piece start on the
    /// decoration's baseline.
    pub place: Placement,
}

impl<P: ?Sized> fmt::Debug for PathTextRun<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathTextRun")
            .field("run", &self.run)
            .finish_non_exhaustive()
    }
}

/// The iterator behind [`PathTextRun::glyphs`].
struct PathGlyphs<'a, P: ?Sized> {
    on: OnPath<'a, P>,
    /// The run's glyphs, each with its cluster's start and advance.
    glyphs: RunGlyphs<'a>,
    /// Whether the run is combined text, which turns as one character about
    /// its em's middle.
    combined: bool,
    /// The text offset of the character being placed.
    character: Option<usize>,
    /// The character's middle along the line.
    middle: f32,
    /// The path point at `middle`, or `None` where the character is not
    /// drawn.
    frame: Option<PathPoint>,
    /// Whether the glyphs stand across the path.
    standing: bool,
    /// The narrowing of combined text; `1.0` for other runs.
    scale: f32,
}

impl<'a, P: TextPath + ?Sized> PathGlyphs<'a, P> {
    /// Creates the walk of `run`'s glyphs, placed by `on`.
    fn new(run: TextRun<'a>, on: OnPath<'a, P>) -> Self {
        let combined = run.orientation() == RunOrientation::Combined;
        let InlineExtents {
            left: start,
            right: end,
        } = run.inline();
        // Combined text is one character turning about its em's middle.
        // Other runs find each character's middle as its glyphs arrive.
        let middle = (start + end) / 2.0;
        Self {
            on,
            glyphs: RunGlyphs::new(&run),
            combined,
            character: None,
            middle,
            frame: if combined { on.frame(middle) } else { None },
            standing: stands(&run),
            scale: run.combine_scale(),
        }
    }
}

impl<P: TextPath + ?Sized> Iterator for PathGlyphs<'_, P> {
    type Item = PathGlyph;

    #[inline]
    fn next(&mut self) -> Option<PathGlyph> {
        loop {
            if self.combined {
                // Combined text is drawn whole or not at all.
                self.frame?;
            }
            let (glyph, left, advance) = self.glyphs.next_placed()?;
            if !self.combined && self.character != Some(glyph.text_offset) {
                // A character's first glyph sets its middle from the cluster.
                self.character = Some(glyph.text_offset);
                self.middle = left + advance / 2.0;
                self.frame = self.on.frame(self.middle);
            }
            // A character off the path is dropped whole.
            let Some(frame) = self.frame else {
                continue;
            };
            let at = (glyph.x, glyph.y);
            let place = self
                .on
                .place(frame, self.middle, at, self.standing, self.scale);
            return Some(PathGlyph { glyph, place });
        }
    }
}

/// A decoration line split into character-sized path pieces.
///
/// Yielded as [`PathPaint::DecorationBeforeText`] or
/// [`PathPaint::DecorationAfterText`]. Each piece rotates with its character,
/// matching SVG and Chrome. Draw it from the placement origin at the
/// straight-line baseline offset, then apply the transform.
///
/// Characters with midpoints outside the path are omitted under
/// [`PastEnds::Hidden`].
pub struct PathDecoration<'a, P: ?Sized> {
    decoration: Decoration,
    /// The line holding the decorated text.
    line: Line<'a>,
    on: OnPath<'a, P>,
}

impl<P: ?Sized> Clone for PathDecoration<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ?Sized> Copy for PathDecoration<'_, P> {}

impl<'a, P: TextPath + ?Sized> PathDecoration<'a, P> {
    /// Returns the underlying decoration, including its node, extents and font metrics.
    pub fn decoration(&self) -> Decoration {
        self.decoration
    }

    /// Returns the per-character pieces in visual order.
    ///
    /// Each piece has its straight-line extent and a path placement. Does not
    /// allocate.
    pub fn pieces(&self) -> impl Iterator<Item = PathPiece> + use<'a, P> {
        PathPieces {
            decoration: self.decoration,
            on: self.on,
            runs: self.decoration.runs(self.line),
            clusters: None,
            combined: None,
        }
    }
}

impl<P: ?Sized> fmt::Debug for PathDecoration<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathDecoration")
            .field("decoration", &self.decoration)
            .finish_non_exhaustive()
    }
}

/// The iterator behind [`PathDecoration::pieces`]: one piece per character
/// under the bar.
struct PathPieces<'a, P: ?Sized> {
    decoration: Decoration,
    on: OnPath<'a, P>,
    /// The text runs under the bar.
    runs: DecorationRuns<'a>,
    /// The clusters of the run being split.
    clusters: Option<RunClusters<'a>>,
    /// The extent of a combined run, which makes one piece.
    combined: Option<InlineExtents>,
}

impl<P: TextPath + ?Sized> PathPieces<'_, P> {
    /// Returns the extent along the line of the next character with an
    /// advance, among the runs the bar overlaps.
    fn next_character(&mut self) -> Option<InlineExtents> {
        let bar = self.decoration.inline();
        loop {
            if let Some(extent) = self.combined.take() {
                return Some(extent);
            }
            if let Some(clusters) = self.clusters.as_mut() {
                match clusters.next() {
                    Some(cluster) => {
                        let extent = cluster.inline();
                        if extent.right > extent.left {
                            return Some(extent);
                        }
                        continue;
                    }
                    None => self.clusters = None,
                }
            }
            let run = self.runs.next()?;
            let extent = run.inline();
            if extent.right <= bar.left || extent.left >= bar.right {
                continue;
            }
            if run.orientation() == RunOrientation::Combined {
                self.combined = Some(extent);
            } else {
                self.clusters = Some(RunClusters::new(&run));
            }
        }
    }
}

impl<P: TextPath + ?Sized> Iterator for PathPieces<'_, P> {
    type Item = PathPiece;

    fn next(&mut self) -> Option<PathPiece> {
        let bar = self.decoration.inline();
        loop {
            let InlineExtents { left, right } = self.next_character()?;
            let (from, to) = (left.max(bar.left), right.min(bar.right));
            if to <= from {
                continue;
            }
            let middle = (left + right) / 2.0;
            let Some(frame) = self.on.frame(middle) else {
                continue;
            };
            let at = (from, self.decoration.baseline());
            return Some(PathPiece {
                inline: InlineExtents {
                    left: from,
                    right: to,
                },
                place: self.on.place(frame, middle, at, false, 1.0),
            });
        }
    }
}
