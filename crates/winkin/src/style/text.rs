//! Whitespace, transformation, breaking, spacing and justification values.

use core::hash::{Hash, Hasher};
use core::mem;

use crate::style::same::{Same, eq_and_hash_by_bits, same_by_value, style_struct};

/// `white-space-collapse`: what happens to spaces, tabs and segment breaks.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum WhiteSpaceCollapse {
    /// A run of spaces, tabs and segment breaks becomes one space.
    ///
    /// The initial value.
    #[default]
    Collapse,
    /// Everything is kept; a segment break forces a line break.
    Preserve,
    /// As `Preserve`, and a space at a line's end takes room.
    ///
    /// Such a space may wrap rather than hang.
    BreakSpaces,
    /// Segment breaks are kept and force line breaks: `pre-line`.
    ///
    /// Spaces and tabs collapse, and the spaces around a kept break go.
    PreserveBreaks,
    /// Spaces and tabs are kept; a segment break becomes a space.
    PreserveSpaces,
    /// Every space, tab and segment break is removed.
    Discard,
}

impl WhiteSpaceCollapse {
    /// Whether spaces and tabs are kept as content.
    ///
    /// They are under `preserve`, `break-spaces` and `preserve-spaces`.
    pub(crate) fn keeps_spaces(self) -> bool {
        matches!(
            self,
            Self::Preserve | Self::BreakSpaces | Self::PreserveSpaces
        )
    }

    /// Whether spaces and tabs collapse, as under `collapse` and `preserve-breaks`.
    ///
    /// `discard` removes them, so neither this nor
    /// [`keeps_spaces`](Self::keeps_spaces) holds there.
    pub(crate) fn collapses_spaces(self) -> bool {
        matches!(self, Self::Collapse | Self::PreserveBreaks)
    }
}

/// `white-space-trim`: removal of collapsible whitespace at element edges.
///
/// Not inherited. Supported by winkin but not Chrome.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct WhiteSpaceTrim {
    /// `discard-before`: the white space just before the element's start.
    pub discard_before: bool,
    /// `discard-after`: the white space just after the element's end.
    pub discard_after: bool,
    /// `discard-inner`: removes leading and trailing whitespace.
    ///
    /// For blocks, also removes preserved whitespace through the first
    /// segment break and from the last segment break, as for `<pre>`.
    pub discard_inner: bool,
}

impl WhiteSpaceTrim {
    /// Nothing removed. The initial value.
    pub const NONE: Self = Self {
        discard_before: false,
        discard_after: false,
        discard_inner: false,
    };
}

/// The case or exclusive mathematical part of `text-transform`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextCase {
    /// As written. The initial value.
    #[default]
    None,
    /// Every letter uppercase.
    Uppercase,
    /// Every letter lowercase.
    Lowercase,
    /// The first letter of each word uppercase.
    Capitalize,
    /// A single-character text node mapped to mathematical italic.
    /// Ignores `full-width` and `full-size-kana`.
    MathAuto,
}

/// `text-transform`.
///
/// Applied while building, using the text language and full CSS case mappings.
/// All layout operations use the transformed text. `capitalize` uses Unicode
/// word boundaries.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct TextTransform {
    /// The case or exclusive mathematical transform.
    pub case: TextCase,
    /// `full-width`: converts characters to full-width forms.
    ///
    /// Includes printable ASCII, half-width katakana and Hangul, and selected
    /// symbols. Preserved spaces become U+3000. Applied after case mapping.
    /// Supported even where Chrome requires a flag.
    pub full_width: bool,
    /// `full-size-kana`: converts small kana to full-size forms.
    ///
    /// Uses the CSS Text 3 table for hiragana, katakana and half-width katakana.
    /// Applied after case mapping and `full-width`.
    pub full_size_kana: bool,
}

impl TextTransform {
    /// No transform. The initial value.
    pub const NONE: Self = Self {
        case: TextCase::None,
        full_width: false,
        full_size_kana: false,
    };

    /// `math-auto`: single-character text nodes in mathematical italic.
    pub const MATH_AUTO: Self = Self {
        case: TextCase::MathAuto,
        ..Self::NONE
    };
}

/// `line-break`: how strictly CJK line-breaking rules apply.
///
/// Resolve `auto` to `normal` before building, as browsers do.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum LineBreak {
    /// The least restrictive rules.
    Loose,
    /// The common rules. The initial value, as `auto` resolves.
    #[default]
    Normal,
    /// The most restrictive rules.
    Strict,
    /// An opportunity around every typographic character unit.
    Anywhere,
}

/// `hyphens`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Hyphens {
    /// No hyphenation: soft hyphens are no opportunity.
    None,
    /// Breaks at soft hyphens only. The initial value.
    #[default]
    Manual,
    /// Breaks at soft hyphens and where a dictionary for the language allows.
    Auto,
}

/// `tab-size`.
#[derive(Copy, Clone, Debug)]
pub enum TabSize {
    /// A multiple of the advance of a space in the element's font.
    Spaces(f32),
    /// A distance in pixels.
    Px(f32),
}

impl TabSize {
    /// Eight spaces. The initial value.
    pub const INITIAL: Self = Self::Spaces(8.0);
}

impl Default for TabSize {
    fn default() -> Self {
        Self::INITIAL
    }
}

impl Same for TabSize {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Spaces(a), Self::Spaces(b)) | (Self::Px(a), Self::Px(b)) => a.same(b),
            _ => false,
        }
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        match self {
            Self::Spaces(n) | Self::Px(n) => n.feed(state),
        }
    }
}

style_struct! {
    /// A length plus a percentage of the applicable basis.
    ///
    /// The basis is computed font size for `word-spacing` and line width
    /// for `text-indent`.
    pub struct LengthPercentage {
        /// The length, in pixels.
        pub px: f32,
        /// The percentage, as a fraction: 1.0 is `100%`.
        pub fraction: f32,
    }
}

impl LengthPercentage {
    /// Zero.
    pub const ZERO: Self = Self {
        px: 0.0,
        fraction: 0.0,
    };

    /// Returns the length in pixels, with the percentage taken of `basis` pixels.
    ///
    /// A NaN in either part or in `basis` gives a NaN. The grid it goes onto
    /// takes a NaN as zero.
    pub(crate) fn resolve(self, basis: f32) -> f32 {
        self.px + self.fraction * basis
    }
}

impl Default for LengthPercentage {
    fn default() -> Self {
        Self::ZERO
    }
}

/// `text-autospace`: space added where ideographs meet letters and digits.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct TextAutospace {
    /// `ideograph-alpha`: between an ideograph and another script's letter.
    pub ideograph_alpha: bool,
    /// `ideograph-numeric`: between an ideograph and a digit.
    pub ideograph_numeric: bool,
}

impl TextAutospace {
    /// `no-autospace`: nothing added.
    ///
    /// This is the initial value as Chrome computes it.
    pub const NO_AUTOSPACE: Self = Self {
        ideograph_alpha: false,
        ideograph_numeric: false,
    };

    /// `normal`: both.
    pub const NORMAL: Self = Self {
        ideograph_alpha: true,
        ideograph_numeric: true,
    };
}

/// `text-spacing-trim`: removal of the blank half of fullwidth punctuation.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextSpacingTrim {
    /// Adjacent marks are trimmed.
    ///
    /// An opening mark keeps its blank at a line's start. The initial value.
    #[default]
    Normal,
    /// No trimming.
    SpaceAll,
    /// As `Normal`, but fewer line starts keep an opening mark's blank.
    ///
    /// An opening mark keeps its blank only at the start of the first line and
    /// of a line after a forced break.
    SpaceFirst,
    /// As `Normal`, and an opening mark loses its blank at every line's start.
    TrimStart,
}

impl TextSpacingTrim {
    /// Whether text set so trims any punctuation: all but `space-all`.
    pub(crate) fn trims_punctuation(self) -> bool {
        self != Self::SpaceAll
    }

    /// Whether text set so trims a wrapped line's opening mark.
    ///
    /// It does under `space-first` and `trim-start`, as Blink's
    /// `ShouldTrimStartOfWrappedLine` says.
    pub(crate) fn trims_wrapped_start(self) -> bool {
        matches!(self, Self::SpaceFirst | Self::TrimStart)
    }

    /// Whether text set so trims a paragraph's opening mark.
    ///
    /// It does under `trim-start`, as Blink's `ShouldTrimStartOfParagraph`
    /// says.
    pub(crate) fn trims_paragraph_start(self) -> bool {
        self == Self::TrimStart
    }
}

/// Whether a stop or comma at a line's end hangs.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum HangEnd {
    /// It stays inside the line.
    #[default]
    None,
    /// `allow-end`: it hangs when it would not otherwise fit.
    Allow,
    /// `force-end`: it always hangs.
    Force,
}

/// `hanging-punctuation`.
///
/// Supported by winkin. Chrome parses it but does not apply it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct HangingPunctuation {
    /// `first`: an opening bracket or quote at the first line's start.
    pub first: bool,
    /// `last`: a closing bracket or quote at the last line's end.
    pub last: bool,
    /// A stop or comma at any line's end.
    pub end: HangEnd,
}

impl HangingPunctuation {
    /// Nothing hangs. The initial value.
    pub const NONE: Self = Self {
        first: false,
        last: false,
        end: HangEnd::None,
    };
}

/// `text-justify`: where a justified line takes its extra room.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextJustify {
    /// Spaces, and the gaps between ideographs. The initial value.
    #[default]
    Auto,
    /// Nowhere: justification is off.
    None,
    /// Spaces only.
    InterWord,
    /// Every gap between characters.
    InterCharacter,
}

same_by_value!(
    WhiteSpaceCollapse,
    WhiteSpaceTrim,
    TextCase,
    TextTransform,
    LineBreak,
    Hyphens,
    TextAutospace,
    TextSpacingTrim,
    HangEnd,
    HangingPunctuation,
    TextJustify,
);

eq_and_hash_by_bits!(TabSize);
