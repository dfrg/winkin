//! A line's paint placed along a path.
//!
//! Each thing [`Line::paints`] yields, in Chrome's order, turns to the path
//! where its character's middle falls.
//!
//! **Read, not recomputed.** A run's start along the line is its own
//! [`inline`](TextRun::inline), and a glyph's place is its own `x` and `y`.
//! Marks and decorations go where the paint puts them. The only thing worked
//! out here is each character's middle, from its glyphs' advances, since that
//! is where it turns.
//!
//! **A character turns as one** (SVG 2, section 11.8.3, and Blink's
//! `SvgTextLayoutAlgorithm::PositionOnPath`). A cluster's glyphs share a text
//! offset and turn about the cluster's middle, so a mark keeps its offset from
//! its base across a bend. A ligature is one character. Combined text is one
//! character, turning about its em's middle.
//!
//! **Read once, nothing held.** Each glyph comes with where its cluster
//! starts along the line and how far it reaches, which the run's glyph
//! walk knows exactly as it places the glyph, so a character's middle is
//! found at its first glyph, with no look ahead, and a cluster that takes
//! room and draws nothing still moves the characters after it.
//!
//! **Each run a walk of its own.** A text run is handed out as a view
//! ([`PathTextRun`]) whose [`glyphs`](PathTextRun::glyphs) walk its placed
//! glyphs, as [`TextRun::glyphs`] walks them along a straight line, so a
//! renderer takes the run's font once and walks its glyphs in a loop of its
//! own; and a decoration as a view ([`PathDecoration`]) whose
//! [`pieces`](PathDecoration::pieces) walk it.

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
    /// A decoration line painted before the text, in pieces.
    ///
    /// Under (or over) a run of it, one piece a character, as SVG and Chrome
    /// draw a decoration along a path.
    DecorationBeforeText(PathDecoration<'a, P>),
    /// A run of a ruby annotation's text, on its annotation's baseline.
    ///
    /// Over (or under) its base.
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
    /// Text the layout made: a hyphen or an ellipsis.
    ///
    /// A hyphen or an ellipsis follows the text of its line.
    Generated(PathTextRun<'a, P>),
    /// A decoration line painted after the text, in pieces.
    ///
    /// Through a run of it, as
    /// [`DecorationBeforeText`](Self::DecorationBeforeText) is.
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

/// What a line's items are placed from along a path.
struct OnPath<'a, P: ?Sized> {
    path: &'a P,
    length: f32,
    ends: PastEnds,
    /// Where the line box's left is along the path, which the room it was
    /// broken in, and its alignment in that room, put it at.
    left: f32,
    /// Where its baseline is, from its top: what sits on the path.
    baseline: f32,
}

impl<P: ?Sized> Clone for OnPath<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: ?Sized> Copy for OnPath<'_, P> {}

impl<P: TextPath + ?Sized> OnPath<'_, P> {
    /// The path where something whose middle is `middle` along the line
    /// goes, and the way it runs there; `None` where it is not drawn.
    #[inline]
    fn frame(&self, middle: f32) -> Option<PathPoint> {
        self.ends.frame(self.path, self.length, self.left + middle)
    }

    /// Where the point `(x, y)` of the line, along it and down from its
    /// top, goes when what it belongs to has its middle `middle` along the
    /// line and is turned to `frame`: standing across the path where
    /// `standing`, narrowed by `scale`.
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
        // Along the path, its x axis is the path's way; standing, its x
        // axis is up from the path, and its y axis the path's way.
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

/// A line's paint along a path, in Chrome's order: what
/// [`paints`](super::paints) walks.
///
/// Every glyph, emphasis mark, atomic inline and piece of a decoration it
/// paints, each placed where its character's middle falls on the path and
/// turned to the way the path runs there. The line starts along the path
/// where its box's left is, [`LineMetrics::left`](crate::LineMetrics::left);
/// its baseline sits on the path, and whatever stands off its baseline
/// stands as far off the path.
pub(super) struct PathWalk<'a, P: ?Sized, I> {
    /// The line's paint along a straight line.
    paint: I,
    line: Line<'a>,
    on: OnPath<'a, P>,
    /// Whether the glyphs of the run handed out last stand across the path,
    /// as its marks, painted after it, then do.
    standing: bool,
}

impl<'a, P: TextPath + ?Sized, I> PathWalk<'a, P, I> {
    /// `line`'s paint along `path`, from `paint`, its paint along a
    /// straight line, with what is past the path's ends drawn as `ends`
    /// says.
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

    /// `run` along the path, whose marks come next.
    fn run(&mut self, run: TextRun<'a>) -> PathTextRun<'a, P> {
        self.standing = stands(&run);
        PathTextRun { run, on: self.on }
    }

    /// `decoration`'s bar along the path.
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
                // SVG paints no background behind text, and Chrome's
                // `textPath` none: the block's background and the boxes'
                // are not placed.
                Paint::Background(_) | Paint::Box(_) => continue,
            });
        }
    }
}

/// Whether `run`'s glyphs stand across a path: upright or combined in a
/// vertical line.
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
/// path are omitted when configured.
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

    /// Returns glyphs and path placements in drawing order without allocating.
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
    /// The glyph with its original straight-line position.
    ///
    /// Use its identifier, advance and text offset for rendering. `place`
    /// provides the transformed position.
    pub glyph: Glyph,
    /// The path placement.
    pub place: Placement,
}

/// The portion of a decoration line for one character on a path.
///
/// Yielded by [`PathDecoration::pieces`]. Draw the bar from the placement
/// origin across the inline extent, at its straight-line baseline offset,
/// then apply the placement transform.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct PathPiece {
    /// The inline extents relative to line-box left.
    pub inline: InlineExtents,
    /// The path placement, with its origin at the piece start on the decoration baseline.
    pub place: Placement,
}

impl<P: ?Sized> fmt::Debug for PathTextRun<'_, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PathTextRun")
            .field("run", &self.run)
            .finish_non_exhaustive()
    }
}

/// A text run's glyphs placed along a path, in drawing order, each with
/// where it is drawn: what [`PathTextRun::glyphs`] walks.
struct PathGlyphs<'a, P: ?Sized> {
    on: OnPath<'a, P>,
    /// The run's glyphs, each with where its character is along the line.
    glyphs: RunGlyphs<'a>,
    /// Whether the run is combined text, one character whatever it holds,
    /// which turns about its em's middle.
    combined: bool,
    /// The text offset the glyphs of the character being handed out share.
    character: Option<usize>,
    /// Where its middle is along the line, and the path there, `None` where
    /// it is not drawn.
    middle: f32,
    frame: Option<PathPoint>,
    /// Whether its glyphs stand across the path, and how much they are
    /// narrowed.
    standing: bool,
    scale: f32,
}

impl<'a, P: TextPath + ?Sized> PathGlyphs<'a, P> {
    /// `run`'s glyphs, to be placed as `on` places them.
    fn new(run: TextRun<'a>, on: OnPath<'a, P>) -> Self {
        let combined = run.orientation() == RunOrientation::Combined;
        let InlineExtents {
            left: start,
            right: end,
        } = run.inline();
        // Combined text is one character, its em's: every glyph of it turns
        // about the em's middle, found now. Any other run's characters are
        // found as its glyphs are read.
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
    /// A glyph, and where it is drawn.
    type Item = PathGlyph;

    #[inline]
    fn next(&mut self) -> Option<PathGlyph> {
        loop {
            if self.combined {
                // One character: all of it on the path, or none.
                self.frame?;
            }
            let (glyph, left, advance) = self.glyphs.next_placed()?;
            if !self.combined && self.character != Some(glyph.text_offset) {
                // A character's first glyph: its middle is its cluster's.
                self.character = Some(glyph.text_offset);
                self.middle = left + advance / 2.0;
                self.frame = self.on.frame(self.middle);
            }
            // Off the path, the whole character is left out.
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
/// Characters with midpoints outside the path are omitted when configured.
pub struct PathDecoration<'a, P: ?Sized> {
    decoration: Decoration,
    /// The line whose items the bar's text is among.
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

    /// Returns character pieces in visual order without allocating.
    ///
    /// Each piece includes straight-line extents and a path placement.
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

/// A decoration line's pieces along a path, one a character it runs under
/// (or through): what [`PathDecoration::pieces`] walks.
struct PathPieces<'a, P: ?Sized> {
    decoration: Decoration,
    on: OnPath<'a, P>,
    /// The text runs the bar was found over.
    runs: DecorationRuns<'a>,
    /// The clusters of the run being cut, and the extent of combined text,
    /// one piece whatever it holds.
    clusters: Option<RunClusters<'a>>,
    combined: Option<InlineExtents>,
}

impl<P: TextPath + ?Sized> PathPieces<'_, P> {
    /// The next character with an advance among the text runs the bar
    /// reaches: where it starts and ends along the line.
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
    /// A piece's start and end along the line, and where it is drawn.
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
