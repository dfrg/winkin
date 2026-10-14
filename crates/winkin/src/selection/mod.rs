//! Caret positioning, hit testing, selection motion and copying.
//!
//! [`Position`] identifies an offset in layout text and remains valid across
//! relayout. [`Layout::node_position`] and [`Layout::position`] convert
//! between layout and source offsets when source mapping is enabled.
//! [`Selection`] retains an anchor, focus and target column for line motion.
//!
//! Physical left/right motion follows visual order. Forward/backward motion
//! follows logical order. Choose [`WordMotion`] to match platform word stops.
//! Caret and selection geometry is relative to each line box.
//!
//! A position keeps the side it was reached from in its [`Affinity`]. Motion
//! forward lands upstream, with the text it passed; motion backward lands
//! downstream; left and right land on the side they came from. A click lands
//! upstream at the logical end of the text it hits and downstream elsewhere.
//! At a boundary between nodes, [`Layout::node_position`] follows the side,
//! so typed text goes into the node the caret came from. Chrome instead
//! lands downstream after every motion.
//!
//! An offset drawn at two places has a character stop at each; Chrome stops
//! once. At a wrap the places are the end of the line before, upstream, and
//! the start of the next, downstream. After white space the wrap removes,
//! the line's end, and the end [`Granularity::LineBoundary`] reaches, are
//! past the space, so typed text follows it. Where the text changes
//! direction, the places are the edges of the text on each side: upstream
//! the text before, downstream the text after.
//!
//! An inline box edge with margin, border or padding between two pieces of
//! text on one line draws its two sides apart, each by its own text. A click
//! in the room selects the nearer side. Character motion stops once at such
//! an edge in each direction, and the next step crosses the room and the
//! following character together. Elsewhere both sides draw at one place.
//!
//! ```
//! use winkin::config::PastLines;
//! use winkin::selection::{
//!     CopyKind, Granularity, MotionDirection, Position, Selection, WordMotion,
//! };
//! use winkin::style::Direction;
//! use winkin::{
//!     Area, BuildOptions, ComputedBlockStyle, ComputedStyle, Context, Layout, NoExclusions, NodeKey,
//! };
//! # use fontwich::{Collection, LayerBuilder, Role};
//! # let mut fonts = LayerBuilder::new(Role::Application);
//! # fonts.add_data(include_bytes!("../../../../support/testing/fonts/Ahem.ttf").as_slice()).unwrap();
//! # let fonts = Collection::new().with_layer(fonts.snapshot());
//!
//! let mut cx = Context::new(fonts);
//! let mut layout = Layout::new();
//! let style = ComputedStyle::initial();
//! let mut builder = layout.builder(&mut cx, NodeKey(0), &ComputedBlockStyle::new(&style), BuildOptions::default());
//! builder.text(NodeKey(1), "Hello big world");
//! builder.finish();
//! layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
//!
//! // A click in the third letter's right half, and the caret after it.
//! let click = layout.hit_test(41.0, 5.0, PastLines::Column).unwrap();
//! assert_eq!(click, Position::from(3));
//! let caret = layout.caret(click).unwrap();
//! assert_eq!((caret.line, caret.inline.left, caret.inline.right), (0, 48.0, 48.0));
//!
//! // A word to the right, extending: as on Windows, to the next word's start.
//! let mut selection = Selection::from(click);
//! let motion = MotionDirection::Right
//!     .extending(Granularity::Word)
//!     .with_word_motion(WordMotion::SkipSpaces);
//! selection.modify(&layout, motion);
//! assert_eq!(selection.range(), 3..6);
//! assert_eq!(layout.selected_text(selection.range(), CopyKind::Text).to_string(), "lo ");
//!
//! // Right as Chrome's arrow key goes: forward in a left-to-right paragraph.
//! // Moving by a character collapses the selection.
//! let right = match layout.paragraph_direction(selection.focus()) {
//!     Direction::Ltr => MotionDirection::Forward,
//!     Direction::Rtl => MotionDirection::Backward,
//! };
//! selection.modify(&layout, right.moving(Granularity::Character));
//! assert_eq!(selection.range(), 6..6);
//! ```

mod copy;
mod hit;
mod motion;
mod place;
mod rects;
#[cfg(test)]
mod tests;
mod visual;
mod words;

use core::ops::Range;

use crate::data::{TextOffset, index_to_u32, u32_to_index};
use crate::layout::CrossExtents;
use crate::layout::Layout;
use crate::stages::content::{MapSide, NodeKey};
use crate::stages::lines::InlineExtents;
use crate::style::Direction;
pub(crate) use copy::SelectedText;
pub(crate) use hit::hit_test;
pub(crate) use rects::SelectionRects;

pub use copy::CopyKind;
pub use rects::{SelectionRect, SelectionRectKind};

/// A caret position: a byte offset in [`Layout::text`] and an affinity.
///
/// Remains valid across relayout because it identifies no line or item.
/// Operations snap it to a grapheme boundary, preserving indivisible text
/// transformations. A grapheme crossing a box boundary is one caret position.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Position {
    /// A byte offset in [`Layout::text`].
    pub offset: usize,
    /// The affinity, selecting a side of the offset and a line at wraps.
    pub affinity: Affinity,
}

impl Position {
    /// Creates a position at `offset` with `affinity`.
    pub const fn new(offset: usize, affinity: Affinity) -> Self {
        Self { offset, affinity }
    }
}

impl From<usize> for Position {
    /// Returns the downstream position at byte offset `offset`.
    fn from(offset: usize) -> Self {
        Self::new(offset, Affinity::Downstream)
    }
}

/// The side of an offset used for caret placement.
///
/// At a wrap, upstream selects the preceding line end and downstream the
/// following line start. After white space the wrap removes, the line end
/// is past that space, as if it hung. Where the text changes direction,
/// upstream selects the edge of the text before the offset and downstream
/// the edge of the text after it. At combined-text boundaries, selects the
/// side before or after the offset.
///
/// At a boundary between nodes, selects the node: upstream the end of the
/// one before, downstream the start of the one after, as
/// [`Layout::node_position`] reports it. Motion keeps the side it arrives
/// from, so text typed at the caret goes into the node it came from.
///
/// At an inline box edge with margin, border or padding facing the offset,
/// and text on both sides on one line, the caret is also drawn on the
/// selected side of the box edge. Elsewhere both affinities draw the caret
/// at one place.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Affinity {
    /// With the text after it: the start of the line after a wrap, or the
    /// start of the node after a boundary.
    #[default]
    Downstream,
    /// With the text before it: the end of the line before a wrap, or the
    /// end of the node before a boundary.
    Upstream,
}

/// A selection from an anchor to a movable focus.
///
/// Equal endpoints form a caret. [`modify`](Self::modify) retains the target
/// column across line motion, including short lines, matching Chrome.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct Selection {
    anchor: Position,
    focus: Position,
    /// The column line motion aims for, from the area's line-left.
    ///
    /// Set by line motion; `None` after any other motion.
    goal: Option<f32>,
}

impl Selection {
    /// Creates a selection from `anchor` to `focus`.
    pub const fn new(anchor: Position, focus: Position) -> Self {
        Self {
            anchor,
            focus,
            goal: None,
        }
    }

    /// Returns the anchor position.
    pub fn anchor(&self) -> Position {
        self.anchor
    }

    /// Returns the focus position.
    pub fn focus(&self) -> Position {
        self.focus
    }

    /// Returns `true` if the anchor and focus offsets are equal.
    pub fn is_collapsed(&self) -> bool {
        self.anchor.offset == self.focus.offset
    }

    /// Returns the selected byte range in [`Layout::text`].
    ///
    /// Endpoints are in logical order regardless of selection direction.
    pub fn range(&self) -> Range<usize> {
        let (a, b) = (self.anchor.offset, self.focus.offset);
        a.min(b)..a.max(b)
    }

    /// Moves or extends the selection according to `motion`.
    ///
    /// Motion specifies direction, granularity and word-stop behavior. Word
    /// motion costs the distance traversed and keeps no state between calls.
    ///
    /// Character motion collapses a nonempty selection toward the requested
    /// end. Other motion moves the focus and also the anchor unless extending.
    /// Line motion preserves the target column; other motion resets it.
    pub fn modify(&mut self, layout: &Layout, motion: Motion) {
        *self = motion::modify(layout, *self, motion);
    }

    /// Returns whether the focus is before the anchor.
    fn is_backward(&self) -> bool {
        self.focus.offset < self.anchor.offset
    }

    /// Returns the endpoint first in text order.
    fn start(&self) -> Position {
        if self.is_backward() {
            self.focus
        } else {
            self.anchor
        }
    }

    /// Returns the endpoint last in text order.
    fn end(&self) -> Position {
        if self.is_backward() {
            self.anchor
        } else {
            self.focus
        }
    }
}

impl From<Position> for Selection {
    /// Returns a caret at `position`.
    fn from(position: Position) -> Self {
        Self::new(position, position)
    }
}

/// The selection motion direction.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum MotionDirection {
    /// Forward in the text: the DOM's `forward`.
    Forward,
    /// Backward in the text: `backward`.
    Backward,
    /// Leftward on the screen: `left`.
    ///
    /// Character and word motion move left on the screen: backward in
    /// left-to-right text and forward in right-to-left text, across changes
    /// of direction. They continue onto the previous line at the left edge,
    /// regardless of paragraph direction. Line-boundary motion goes to the
    /// line's left edge: its start in a left-to-right line and its end in a
    /// right-to-left one. In vertical lines, left is line-left.
    ///
    /// Line, paragraph and document motion have no screen side and follow the
    /// paragraph direction, matching Chrome. Chrome arrow keys follow the
    /// paragraph direction at every granularity; to match, map this to
    /// [`Forward`](Self::Forward) or [`Backward`](Self::Backward) according
    /// to [`Layout::paragraph_direction`].
    Left,
    /// Rightward on the screen: `right`.
    ///
    /// Character and word motion move right on the screen: forward in
    /// left-to-right text and backward in right-to-left text, across changes
    /// of direction. They continue onto the next line at the right edge,
    /// regardless of paragraph direction. Line-boundary motion goes to the
    /// line's right edge: its end in a left-to-right line and its start in a
    /// right-to-left one. In vertical lines, right is line-right.
    ///
    /// Line, paragraph and document motion have no screen side and follow the
    /// paragraph direction, matching Chrome. Chrome arrow keys follow the
    /// paragraph direction at every granularity; to match, map this to
    /// [`Forward`](Self::Forward) or [`Backward`](Self::Backward) according
    /// to [`Layout::paragraph_direction`].
    Right,
}

impl MotionDirection {
    /// Creates a caret motion in this direction with the specified granularity.
    ///
    /// Collapses the selection at the destination. Word motion defaults to
    /// [`WordMotion::PLATFORM`]; override with [`Motion::with_word_motion`].
    pub const fn moving(self, granularity: Granularity) -> Motion {
        Motion {
            direction: self,
            granularity,
            extend: false,
            word_motion: WordMotion::PLATFORM,
        }
    }

    /// Creates an extending motion in this direction with the specified granularity.
    ///
    /// Moves the focus while retaining the anchor. Otherwise behaves like
    /// [`moving`](Self::moving).
    pub const fn extending(self, granularity: Granularity) -> Motion {
        Motion {
            extend: true,
            ..self.moving(granularity)
        }
    }
}

/// The motion granularity, matching DOM `Selection.modify`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Granularity {
    /// A grapheme cluster.
    ///
    /// Includes clusters split by style boundaries and characters whose
    /// length a transform changed.
    Character,
    /// A word, by Unicode's word boundaries.
    ///
    /// Stops according to the [`WordMotion`] of the motion.
    Word,
    /// A line up or down, keeping the column.
    Line,
    /// An end of the line: Home and End.
    ///
    /// [`Forward`](MotionDirection::Forward) goes to the line's logical end,
    /// upstream, and [`Backward`](MotionDirection::Backward) to its logical
    /// start, downstream. [`Left`](MotionDirection::Left) and
    /// [`Right`](MotionDirection::Right) go to its edge on that side of the
    /// screen. At a wrap after white space, the line's end is past that space.
    LineBoundary,
    /// The paragraph's start or end.
    ParagraphBoundary,
    /// The text's start or end.
    DocumentBoundary,
}

/// A direction, granularity and selection-extension mode.
///
/// Construct with [`MotionDirection::moving`] or
/// [`MotionDirection::extending`]. Override platform word-stop behavior
/// with [`with_word_motion`](Self::with_word_motion).
///
/// ```
/// use winkin::selection::{Granularity, MotionDirection, WordMotion};
///
/// // Right by a character on the screen, and on by a word as on macOS.
/// let right = MotionDirection::Right.moving(Granularity::Character);
/// let word = MotionDirection::Forward
///     .extending(Granularity::Word)
///     .with_word_motion(WordMotion::StopAtWordEnd);
/// ```
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Motion {
    direction: MotionDirection,
    granularity: Granularity,
    extend: bool,
    word_motion: WordMotion,
}

impl Motion {
    /// Returns this motion with the specified word-stop behavior.
    ///
    /// Applies to forward motion and visual motion following the run direction.
    pub const fn with_word_motion(self, word_motion: WordMotion) -> Self {
        Self {
            word_motion,
            ..self
        }
    }
}

/// The stopping rule for forward word motion.
///
/// Backward motion always stops at a word start. Visual left/right motion
/// uses this rule when following a run direction and stops at word starts
/// when moving against it, matching Chrome.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum WordMotion {
    /// Past the spaces after the word, at the next word's start: Windows.
    SkipSpaces,
    /// At the word's end: macOS and Linux.
    StopAtWordEnd,
}

impl WordMotion {
    /// The default word-stop rule for the target platform.
    ///
    /// Uses [`SkipSpaces`](Self::SkipSpaces) on Windows and
    /// [`StopAtWordEnd`](Self::StopAtWordEnd) elsewhere, including WebAssembly
    /// and embedded targets.
    pub const PLATFORM: Self = if cfg!(windows) {
        Self::SkipSpaces
    } else {
        Self::StopAtWordEnd
    };
}

impl Default for WordMotion {
    /// Returns [`PLATFORM`](Self::PLATFORM).
    fn default() -> Self {
        Self::PLATFORM
    }
}

/// Caret geometry relative to a line box.
///
/// Normally spans across the line at equal inline endpoints.
/// Inside vertical combined text, spans along the unit em instead,
/// with equal block endpoints, matching Chrome.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct Caret {
    /// The zero-based line index.
    pub line: usize,
    /// Inline extents relative to line-box left.
    ///
    /// Normally a single position; combined text spans the unit em.
    /// Chrome centers a one-pixel bar here. Beside atomic inlines, the
    /// caret is inside the box edge.
    pub inline: InlineExtents,
    /// Cross-line extents relative to the line-over edge.
    ///
    /// Text uses font ascent and descent around the baseline. Atomic inlines
    /// and empty lines use line-box extents. Combined text uses a single
    /// position between characters.
    pub block: CrossExtents,
    /// Whether the adjacent text is RTL, for positioning a caret flag.
    pub rtl: bool,
}

/// The primary caret and an optional alternate at a bidi boundary.
///
/// At a bidi boundary the affinity selects the primary side. Chrome draws
/// one caret, on the side of the paragraph direction.
#[derive(Copy, Clone, PartialEq, Debug)]
#[non_exhaustive]
pub struct Carets {
    /// The primary caret, on the side of a bidi boundary the affinity selects.
    pub strong: Caret,
    /// The caret on the other side of a bidi boundary, for a split caret, or
    /// `None` outside bidi boundaries.
    pub weak: Option<Caret>,
}

/// A node and byte offset in source text.
///
/// Text offsets count bytes passed to [`LayoutBuilder::text`](crate::LayoutBuilder::text)
/// for that node. Atomic inlines and forced breaks use offsets 0 and 1
/// for their preceding and following positions.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct NodePosition {
    /// The node's key.
    pub key: NodeKey,
    /// A byte offset in the node's text.
    pub offset: usize,
}

/// Returns the strong and weak carets at `position`: [`Layout::carets`].
pub(crate) fn carets(layout: &Layout, position: Position) -> Option<Carets> {
    place::carets(layout, snapped(layout, position), None)
}

/// Returns `position` snapped to a caret stop, with its cluster.
///
/// This is the one cluster search a public call makes.
fn snapped(layout: &Layout, position: Position) -> place::ClusteredPosition {
    place::snap(layout, place::ClusteredPosition::new(layout, position))
}

/// Returns the base direction of the paragraph `position` is in:
/// [`Layout::paragraph_direction`].
pub(crate) fn paragraph_direction(layout: &Layout, position: Position) -> Direction {
    if motion::paragraph_level(layout, snapped(layout, position)).is_rtl() {
        Direction::Rtl
    } else {
        Direction::Ltr
    }
}

/// Returns the node and source offset of `position`:
/// [`Layout::node_position`].
pub(crate) fn node_position(layout: &Layout, position: Position) -> Option<NodePosition> {
    let position = snapped(layout, position).position;
    let map = layout.content().offset_map()?;
    let (key, offset) =
        map.source_position(TextOffset::new(position.offset), position.affinity.into())?;
    Some(NodePosition {
        key,
        offset: u32_to_index(offset),
    })
}

/// Returns the position at source offset `offset` in node `key`:
/// [`Layout::position`].
pub(crate) fn position(
    layout: &Layout,
    key: NodeKey,
    offset: usize,
    affinity: Affinity,
) -> Option<Position> {
    let map = layout.content().offset_map()?;
    let at = map.content_offset(key, index_to_u32(offset), affinity.into())?;
    let found = snapped(layout, Position::new(at.get(), affinity));
    // Where the two sides are in different nodes, takes the side in `key`.
    let side = |affinity: Affinity| {
        let side = map.source_position(TextOffset::new(found.offset()), affinity.into());
        side.is_some_and(|(side, _)| side == key)
            .then_some(Position::new(found.offset(), affinity))
    };
    let other = match affinity {
        Affinity::Upstream => Affinity::Downstream,
        Affinity::Downstream => Affinity::Upstream,
    };
    Some(
        side(affinity)
            .or_else(|| side(other))
            .unwrap_or(found.position),
    )
}

impl From<Affinity> for MapSide {
    /// Returns the offset map side an affinity reads: upstream the first
    /// place, downstream the last.
    fn from(affinity: Affinity) -> Self {
        match affinity {
            Affinity::Upstream => Self::Before,
            Affinity::Downstream => Self::After,
        }
    }
}
