//! Text emphasis values.

use super::WritingMode;
use crate::style::same::same_by_value;
use crate::unicode::EmphasisClass;

/// The side of horizontal text `text-emphasis` marks are set on.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum EmphasisSide {
    /// Above. The initial value.
    #[default]
    Over,
    /// Below.
    Under,
}

/// The side of vertical text `text-emphasis` marks are set on.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum EmphasisVerticalSide {
    /// To the right. The initial value.
    #[default]
    Right,
    /// To the left.
    Left,
}

impl EmphasisVerticalSide {
    /// Returns the side of a vertical typographic line it names.
    ///
    /// `right` is over in both `vertical-rl` and `vertical-lr`.
    pub(crate) fn line_side(self) -> EmphasisSide {
        match self {
            Self::Right => EmphasisSide::Over,
            Self::Left => EmphasisSide::Under,
        }
    }
}

/// `text-emphasis-position`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct EmphasisPosition {
    /// The side in horizontal text.
    pub side: EmphasisSide,
    /// The side in vertical text.
    pub vertical_side: EmphasisVerticalSide,
}

impl EmphasisPosition {
    /// Returns the side of the line the marks take in a block of `writing_mode`.
    ///
    /// `vertical-rl` and `vertical-lr` read the vertical side, and
    /// `horizontal-tb` and the sideways modes read the horizontal one, as
    /// CSS Text Decoration 4 specifies. Chrome 153 reads the vertical side
    /// in the sideways modes too.
    pub(crate) fn side(self, writing_mode: WritingMode) -> EmphasisSide {
        if writing_mode.is_vertical_typographic() {
            self.vertical_side.line_side()
        } else {
            self.side
        }
    }
}

/// `text-emphasis-skip`: the characters that take no mark.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct EmphasisSkip(u8);

impl EmphasisSkip {
    /// `spaces`: word separators.
    pub const SPACES: Self = Self(1 << 0);
    /// `punctuation`.
    pub const PUNCTUATION: Self = Self(1 << 1);
    /// `symbols`.
    pub const SYMBOLS: Self = Self(1 << 2);
    /// `narrow`: characters narrower than the mark.
    pub const NARROW: Self = Self(1 << 3);
    /// `spaces punctuation`. The initial value.
    pub const INITIAL: Self = Self(Self::SPACES.0 | Self::PUNCTUATION.0);

    /// Returns `true` if every kind in `other` is skipped.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the kinds skipped by either.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether a character of emphasis class `class` takes no mark.
    ///
    /// `wide` says whether its East Asian width is wide or fullwidth. A
    /// character takes no mark where its class is skipped, or where `narrow`
    /// is skipped and it is not wide.
    #[inline]
    pub(crate) fn skips(self, class: EmphasisClass, wide: bool) -> bool {
        let skipped = match class {
            EmphasisClass::Marked => false,
            EmphasisClass::Space => self.contains(Self::SPACES),
            EmphasisClass::Punctuation => self.contains(Self::PUNCTUATION),
            EmphasisClass::Symbol => self.contains(Self::SYMBOLS),
            EmphasisClass::Never => true,
        };
        let narrow = self.contains(Self::NARROW) && !wide;
        skipped || narrow
    }
}

impl Default for EmphasisSkip {
    fn default() -> Self {
        Self::INITIAL
    }
}

/// Layout properties for `text-emphasis`.
///
/// Sets which text takes marks and on which side. The host supplies mark
/// shapes and colors when painting.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct TextEmphasis {
    /// Whether `text-emphasis-style` sets marks.
    ///
    /// Marks take room on their side, which can make a line taller.
    pub marks: bool,
    /// `text-emphasis-position`.
    pub position: EmphasisPosition,
    /// `text-emphasis-skip`.
    pub skip: EmphasisSkip,
}

impl TextEmphasis {
    /// No marks. The initial value.
    pub const NONE: Self = Self {
        marks: false,
        position: EmphasisPosition {
            side: EmphasisSide::Over,
            vertical_side: EmphasisVerticalSide::Right,
        },
        skip: EmphasisSkip::INITIAL,
    };
}

same_by_value!(TextEmphasis);
