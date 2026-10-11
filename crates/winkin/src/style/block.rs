//! Block writing mode, alignment, indentation, wrapping and clamping values.

use super::LengthPercentage;
use crate::stages::analysis::BidiLevel;
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
    /// Whether it is a vertical typographic mode: `vertical-rl` or `vertical-lr`.
    ///
    /// In these modes `text-orientation` sets text upright or on its side,
    /// and `text-combine-upright` combines it, as CSS Writing Modes 4 says.
    /// The sideways modes set their lines as horizontal ones turned whole.
    pub(crate) fn is_vertical_typographic(self) -> bool {
        matches!(self, Self::VerticalRl | Self::VerticalLr)
    }

    /// Whether its lines flip, which only `vertical-lr` does.
    ///
    /// Its lines stack from the left and have their over side on the right.
    /// So a line box's block-start side is its under side, as Chrome's
    /// `IsFlippedLinesWritingMode` has it. `sideways-lr` also stacks lines
    /// from the left, but they run up the page with their over side on the
    /// left, as Blink's `LineOver` has it. Its block-start side is its over
    /// side, as in every other mode.
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
    /// Whether a line in a paragraph at bidi level `level` needs its exact end.
    ///
    /// It does where alignment moves the line by its width. This is Chrome's
    /// `NeedsAccurateEndPosition`, which decides whether a break at a space
    /// is reshaped.
    pub(crate) fn needs_exact_end(self, level: BidiLevel) -> bool {
        let rtl = level.is_rtl();
        match self {
            Self::Start => false,
            Self::Left => rtl,
            Self::Right => !rtl,
            Self::End | Self::Center | Self::Justify => true,
        }
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
    /// It resolves as Chrome's `GetTextAlign(is_last_line)` does. `auto` is
    /// `align`, but `start` where `align` is `justify`.
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
    /// The indent: a length plus a fraction of the line's width.
    pub amount: LengthPercentage,
    /// `hanging`: every line but the first is indented.
    pub hanging: bool,
    /// `each-line`: the first line after each forced break is indented too.
    pub each_line: bool,
}

impl TextIndent {
    /// Returns how far it indents a line in an area `width` wide.
    ///
    /// A percentage is of the area's width, truncated onto the grid as
    /// Chrome's `MinimumValueForLength` does.
    pub(crate) fn length(self, width: LayoutUnit) -> LayoutUnit {
        LayoutUnit::from_px_truncated(self.amount.resolve(width.to_px()))
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
    /// Overflow extends past the edge, for the caller to clip.
    ///
    /// The initial value.
    #[default]
    Clip,
    /// An ellipsis where the line is cut.
    Ellipsis,
}

/// `line-clamp` longhands from CSS Overflow 4.
///
/// Limits retained lines. If text follows the last retained line, that line
/// receives the block ellipsis (`block-ellipsis: auto`).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum LineClamp {
    /// No limit. The initial value.
    #[default]
    None,
    /// At most this many lines: `max-lines`, with `continue: collapse`.
    ///
    /// CSS never computes `Lines(0)`. It clamps nothing, as `None` does.
    Lines(u32),
    /// `line-clamp: auto`: `max-lines: none` with `continue: collapse`.
    ///
    /// Retains the lines that end within
    /// [`Area::block_end`](crate::Area::block_end), which the host sets from
    /// the `height` or `max-height` of the block. Clamps nothing if
    /// `block_end` is `None`. Chrome 153 supports this only behind a flag.
    Auto,
}

impl LineClamp {
    /// Whether it may end the block before its text does.
    ///
    /// A nonzero count of lines may, and so may `auto`.
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
/// Supported by winkin but not Chrome.
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
    /// moves by nothing, a group at the right by all of `least`, and a
    /// centered group by half.
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
/// Lines break at the text's own size, and each is scaled after. The scale
/// multiplies glyph advances, font metrics, `normal` and `<number>` line
/// heights, and percentage spacing; lengths keep their size. It applies only
/// to a block with no floats, initial letter or ruby, and does not change
/// intrinsic sizes.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub struct TextFit {
    /// Whether text grows to fill a line, shrinks to fit it, or neither.
    pub kind: TextFitKind,
    /// Which lines scale, and whether they share one scale.
    pub target: TextFitTarget,
    /// The scale limit as a fraction, `1.5` for `150%`, or `None` for no
    /// limit.
    ///
    /// Growing is held to at most `max(limit, 1)`, and shrinking to at least
    /// `min(limit, 1)`.
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
    /// One scale for every line of the block: the least that fills the lines
    /// it measures. The initial value.
    #[default]
    Consistent,
    /// Each line takes its own scale, except a paragraph's last line.
    PerLine,
    /// Each line takes its own scale.
    PerLineAll,
}
