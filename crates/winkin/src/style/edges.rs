//! Physical box edges and `box-decoration-break` values.

use core::hash::{Hash, Hasher};

use super::{LengthPercentage, WritingMode};
use crate::style::same::{Same, same_by_value};
use crate::unit::LayoutUnit;

/// A value for each physical side of a box.
#[derive(Copy, Clone, Debug, Default)]
pub struct Sides<T> {
    /// The top side.
    pub top: T,
    /// The right side.
    pub right: T,
    /// The bottom side.
    pub bottom: T,
    /// The left side.
    pub left: T,
}

impl<T: Copy> Sides<T> {
    /// Returns sides with `value` on every side.
    pub const fn all(value: T) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// Lengths in pixels: the margin, border and padding of a box.
impl Sides<f32> {
    /// Nothing on any side.
    pub const ZERO: Self = Self::all(0.0);

    /// Returns `true` if any side is nonzero.
    pub fn any(self) -> bool {
        self.top != 0.0 || self.right != 0.0 || self.bottom != 0.0 || self.left != 0.0
    }
}

/// Lengths and percentages of the containing block's inline size: the
/// margin and padding of a box.
impl Sides<LengthPercentage> {
    /// Returns sides each `px` pixels long, with no percentage.
    pub const fn from_px(px: f32) -> Self {
        Self::all(LengthPercentage::from_px(px))
    }

    /// Returns `true` if any side is nonzero, at any basis.
    pub fn any(self) -> bool {
        !(self.top.is_zero()
            && self.right.is_zero()
            && self.bottom.is_zero()
            && self.left.is_zero())
    }

    /// Whether any side has a percentage, so that its length depends on
    /// the basis.
    pub(crate) fn has_percentage(self) -> bool {
        [self.top, self.right, self.bottom, self.left]
            .iter()
            .any(|side| side.fraction != 0.0)
    }

    /// Returns the lengths in pixels, each percentage taken of `basis`
    /// pixels.
    pub(crate) fn resolve(self, basis: f32) -> Sides<f32> {
        Sides {
            top: self.top.resolve(basis),
            right: self.right.resolve(basis),
            bottom: self.bottom.resolve(basis),
            left: self.left.resolve(basis),
        }
    }
}

impl From<Sides<f32>> for Sides<LengthPercentage> {
    /// Takes each side's length in pixels, with no percentage.
    fn from(px: Sides<f32>) -> Self {
        Self {
            top: LengthPercentage::from_px(px.top),
            right: LengthPercentage::from_px(px.right),
            bottom: LengthPercentage::from_px(px.bottom),
            left: LengthPercentage::from_px(px.left),
        }
    }
}

impl<T: Copy> Sides<T> {
    /// Returns the values on the sides a line runs between, as `(line-left, line-right)`.
    ///
    /// Horizontal lines run from left to right, and vertical ones from top to
    /// bottom. `sideways-lr` lines run upward, from bottom to top.
    pub(crate) fn along_line(self, writing_mode: WritingMode) -> (T, T) {
        match writing_mode {
            WritingMode::HorizontalTb => (self.left, self.right),
            WritingMode::VerticalRl | WritingMode::VerticalLr | WritingMode::SidewaysRl => {
                (self.top, self.bottom)
            }
            WritingMode::SidewaysLr => (self.bottom, self.top),
        }
    }

    /// Returns the values on the sides across a line, as `(over, under)`.
    ///
    /// Horizontal lines are over at the top, and vertical ones on the right.
    /// `sideways-lr` lines are over on the left.
    pub(crate) fn across_line(self, writing_mode: WritingMode) -> (T, T) {
        match writing_mode {
            WritingMode::HorizontalTb => (self.top, self.bottom),
            WritingMode::VerticalRl | WritingMode::VerticalLr | WritingMode::SidewaysRl => {
                (self.right, self.left)
            }
            WritingMode::SidewaysLr => (self.left, self.right),
        }
    }
}

impl Sides<f32> {
    /// Returns [`along_line`](Self::along_line)'s lengths truncated onto layout's grid.
    ///
    /// Chrome holds a box's margin, border and padding so.
    pub(crate) fn along_line_on_grid(self, writing_mode: WritingMode) -> (LayoutUnit, LayoutUnit) {
        let (start, end) = self.along_line(writing_mode);
        (
            LayoutUnit::from_px_truncated(start),
            LayoutUnit::from_px_truncated(end),
        )
    }

    /// Returns [`across_line`](Self::across_line)'s lengths truncated onto layout's grid.
    ///
    /// Chrome holds a box's margin, border and padding so.
    pub(crate) fn across_line_on_grid(self, writing_mode: WritingMode) -> (LayoutUnit, LayoutUnit) {
        let (over, under) = self.across_line(writing_mode);
        (
            LayoutUnit::from_px_truncated(over),
            LayoutUnit::from_px_truncated(under),
        )
    }
}

impl<T: Same> Same for Sides<T> {
    fn same(&self, other: &Self) -> bool {
        self.top.same(&other.top)
            && self.right.same(&other.right)
            && self.bottom.same(&other.bottom)
            && self.left.same(&other.left)
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        self.top.feed(state);
        self.right.feed(state);
        self.bottom.feed(state);
        self.left.feed(state);
    }
}

impl<T: Same> PartialEq for Sides<T> {
    fn eq(&self, other: &Self) -> bool {
        self.same(other)
    }
}

impl<T: Same> Eq for Sides<T> {}

impl<T: Same> Hash for Sides<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.feed(state);
    }
}

/// `box-decoration-break`. Not inherited.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum BoxDecorationBreak {
    /// The first fragment takes the start edge and the last the end edge.
    ///
    /// The initial value.
    #[default]
    Slice,
    /// Every fragment takes both edges.
    Clone,
}

same_by_value!(BoxDecorationBreak);
