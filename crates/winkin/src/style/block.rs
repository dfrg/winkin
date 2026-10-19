//! Block writing mode, alignment, indentation, wrapping and clamping values.

use super::{CalcResolver, LengthPercentageOrCalc};
use crate::unit::LayoutUnit;

/// `writing-mode`: which way a block's lines run and stack.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum WritingMode {
    /// Lines run left to right and stack top to bottom. The initial value.
    #[default]
    HorizontalTb,
    /// Lines run top to bottom and stack right to left.
    VerticalRl,
    /// Lines run top to bottom and stack left to right.
    VerticalLr,
    /// As `VerticalRl`, with every glyph on its side.
    SidewaysRl,
    /// Lines run bottom to top and stack left to right.
    ///
    /// Every glyph is on its side.
    SidewaysLr,
}

impl WritingMode {
    /// Returns whether it is a vertical typographic mode: `vertical-rl` or
    /// `vertical-lr`.
    ///
    /// Only these modes read `text-orientation` and `text-combine-upright`,
    /// as CSS Writing Modes 4 specifies. The sideways modes set horizontal
    /// lines turned whole.
    pub(crate) fn is_vertical_typographic(self) -> bool {
        matches!(self, Self::VerticalRl | Self::VerticalLr)
    }

    /// Returns whether its lines flip, which only `vertical-lr` does.
    ///
    /// Its lines stack from the left with their over side on the right, so
    /// a line box's block-start side is its under side, as in Chrome's
    /// `IsFlippedLinesWritingMode`. `sideways-lr` lines also stack from the
    /// left, but run upward with their over side on the left (Blink's
    /// `LineOver`), so they do not flip.
    pub(crate) fn flips_lines(self) -> bool {
        self == Self::VerticalLr
    }
}

/// `text-align`, with `match-parent` already resolved by the cascade.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextAlign {
    /// The start side of each paragraph's direction. The initial value.
    #[default]
    Start,
    /// The end side of each paragraph's direction.
    End,
    /// The left edge.
    Left,
    /// The right edge.
    Right,
    /// Centered.
    Center,
    /// Both edges.
    Justify,
}

impl TextAlign {
    /// Returns whether every line in a block aligned `self`, with `last` on
    /// its last line, needs its exact end.
    ///
    /// This is the alignment half of Chrome's `NeedsAccurateEndPosition`,
    /// which decides whether a line's end is reshaped at a space.
    ///
    /// Chrome reads both values on every line, and reads them before it
    /// knows the line's direction. So `left` never needs it, and `right`
    /// always does. Chrome 155, `XRAY bravo` in Times New Roman at 20px, 90px
    /// wide: the `Y` is reshaped under `right`, `end`, `center` and
    /// `justify`, in either direction, and under `start` with
    /// `text-align-last: center`, but not under `left` in a right-to-left
    /// block.
    pub(crate) fn needs_exact_end(self, last: TextAlignLast) -> bool {
        let aligned = matches!(self, Self::End | Self::Right | Self::Center | Self::Justify);
        aligned
            || matches!(
                last,
                TextAlignLast::End
                    | TextAlignLast::Right
                    | TextAlignLast::Center
                    | TextAlignLast::Justify
            )
    }
}

/// `text-align-last`: the alignment of each paragraph's last line.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextAlignLast {
    /// `start` where `text-align` is `justify`, else `text-align`.
    ///
    /// The initial value.
    #[default]
    Auto,
    /// The start side.
    Start,
    /// The end side.
    End,
    /// The left edge.
    Left,
    /// The right edge.
    Right,
    /// Centered.
    Center,
    /// Both edges.
    Justify,
}

impl TextAlignLast {
    /// Returns the alignment of a paragraph's last line in a block aligned `align`.
    ///
    /// `auto` gives `align`, or `start` where `align` is `justify`, as
    /// Chrome's `GetTextAlign(is_last_line)` resolves it.
    #[inline]
    pub(crate) fn resolve(self, align: TextAlign) -> TextAlign {
        match self {
            Self::Auto if align == TextAlign::Justify => TextAlign::Start,
            Self::Auto => align,
            Self::Start => TextAlign::Start,
            Self::End => TextAlign::End,
            Self::Left => TextAlign::Left,
            Self::Right => TextAlign::Right,
            Self::Center => TextAlign::Center,
            Self::Justify => TextAlign::Justify,
        }
    }
}

/// `text-indent`.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct TextIndent {
    /// The indent: a length in CSS pixels plus a fraction of the area's
    /// width, or a `calc()` of them.
    pub amount: LengthPercentageOrCalc,
    /// `hanging`: every line but the first is indented.
    pub hanging: bool,
    /// `each-line`: the first line after each forced break is indented too.
    pub each_line: bool,
}

impl TextIndent {
    /// Returns how far it indents a line in an area `width` wide, a
    /// `calc()` resolved by `calc`.
    ///
    /// The result is truncated onto the grid, as Chrome's
    /// `MinimumValueForLength` does. The intrinsic sizes take a `width` of
    /// zero, as Chrome's breaker does in its min- and max-content modes.
    pub(crate) fn length(self, width: LayoutUnit, calc: Option<&dyn CalcResolver>) -> LayoutUnit {
        LayoutUnit::from_px_truncated(self.amount.resolve(width.to_px(), calc))
    }
}

/// `text-wrap-style`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextWrapStyle {
    /// Greedy: each line takes what fits. The initial value.
    #[default]
    Auto,
    /// Lines of near-equal length.
    Balance,
    /// No short last line.
    Pretty,
    /// As `Auto`.
    Stable,
}

/// `text-overflow`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextOverflow {
    /// Overflow extends past the edge, for the host to clip.
    ///
    /// The initial value.
    #[default]
    Clip,
    /// An ellipsis where the line is cut.
    Ellipsis,
}

/// `line-clamp` longhands from CSS Overflow 4.
///
/// Limits the lines kept. Where text follows the last line kept, that line
/// takes the block ellipsis (`block-ellipsis: auto`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum LineClamp {
    /// No limit. The initial value.
    #[default]
    None,
    /// At most this many lines: `max-lines`, with `continue: collapse`.
    ///
    /// CSS never computes `Lines(0)`, which clamps nothing.
    Lines(u32),
    /// `line-clamp: auto`: `max-lines: none` with `continue: collapse`.
    ///
    /// Keeps the lines that end within
    /// [`Area::block_end`](crate::Area::block_end), which the host sets from
    /// the block's `height` or `max-height`. Clamps nothing where
    /// `block_end` is `None`. Chrome 153 supports it only behind a flag.
    Auto,
}

impl LineClamp {
    /// Returns whether it may end the block before its text does: a nonzero
    /// count of lines, or `auto`.
    pub(crate) fn clamps(self) -> bool {
        match self {
            Self::Lines(lines) => lines > 0,
            Self::Auto => true,
            Self::None => false,
        }
    }
}

/// `text-group-align`.
///
/// Chrome does not support it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextGroupAlign {
    /// No group alignment. The initial value.
    #[default]
    None,
    /// The group at the start.
    Start,
    /// The group at the end.
    End,
    /// The group at the left.
    Left,
    /// The group at the right.
    Right,
    /// The group centered.
    Center,
}

impl TextGroupAlign {
    /// Returns how far the group of lines moves in from the left.
    ///
    /// `least` is the least room any line in the group spares, and `rtl`
    /// says whether the block reads right to left. A group at the left
    /// moves by nothing, at the right by `least`, and centered by half.
    pub(crate) fn left_inset(self, least: LayoutUnit, rtl: bool) -> LayoutUnit {
        match (self, rtl) {
            (Self::Start, false) | (Self::End, true) | (Self::Left, _) => LayoutUnit::ZERO,
            (Self::Start, true) | (Self::End, false) | (Self::Right, _) => least,
            (Self::Center, _) => least.half(),
            (Self::None, _) => LayoutUnit::ZERO,
        }
    }
}

/// `text-fit`: scales the text of each line to fill it.
///
/// Lines break at the text's own size, then each is scaled. The scale
/// multiplies glyph advances, font metrics, `normal` and `<number>` line
/// heights, and percentage spacing; lengths keep their size. A line within
/// 2 px of its room is not scaled. It applies only to a block with no
/// floats, initial letter or ruby, and does not change intrinsic sizes.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct TextFit {
    /// Whether text grows to fill a line, shrinks to fit it, or neither.
    pub kind: TextFitKind,
    /// Which lines scale, and whether they share one scale.
    pub target: TextFitTarget,
    /// The scale limit as a fraction, `1.5` for `150%`, or `None` for no
    /// limit.
    ///
    /// Growing stops at `max(limit, 1)`, and shrinking at `min(limit, 1)`.
    pub limit: Option<f32>,
}

impl TextFit {
    /// `none`: the initial value.
    pub const NONE: Self = Self {
        kind: TextFitKind::None,
        target: TextFitTarget::Consistent,
        limit: None,
    };
}

/// The first keyword of `text-fit`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextFitKind {
    /// No scaling. The initial value.
    #[default]
    None,
    /// Text grows to fill lines narrower than their room.
    Grow,
    /// Text shrinks to fit lines wider than their room.
    Shrink,
}

/// The second keyword of `text-fit`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextFitTarget {
    /// One scale for every line of the block: the least scale any line
    /// asks for. The initial value.
    #[default]
    Consistent,
    /// Each line takes its own scale, except a paragraph's last line.
    PerLine,
    /// Each line takes its own scale.
    PerLineAll,
}
