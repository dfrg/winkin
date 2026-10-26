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
    /// Returns whether spaces and tabs are kept, as under `preserve`,
    /// `break-spaces` and `preserve-spaces`.
    pub(crate) fn keeps_spaces(self) -> bool {
        matches!(
            self,
            Self::Preserve | Self::BreakSpaces | Self::PreserveSpaces
        )
    }

    /// Returns whether spaces and tabs collapse, as under `collapse` and
    /// `preserve-breaks`.
    ///
    /// Under `discard`, neither this nor [`keeps_spaces`](Self::keeps_spaces)
    /// holds.
    pub(crate) fn collapses_spaces(self) -> bool {
        matches!(self, Self::Collapse | Self::PreserveBreaks)
    }
}

/// `white-space-trim`: removal of white space at an element's edges.
///
/// Not inherited.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct WhiteSpaceTrim {
    /// `discard-before`: the white space just before the element's start.
    pub discard_before: bool,
    /// `discard-after`: the white space just after the element's end.
    pub discard_after: bool,
    /// `discard-inner`: the white space just inside the element's start and
    /// end.
    ///
    /// On a block, also removes preserved white space up to the first
    /// segment break and from the last one, as for `<pre>`.
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
    /// `math-auto`: a text node of one character in mathematical italic.
    ///
    /// Excludes `full-width` and `full-size-kana`.
    MathAuto,
}

/// `text-transform`.
///
/// Applied while building, with the text's language and full case mappings.
/// Layout and its positions use the transformed text. `capitalize` finds
/// words by Unicode word boundaries.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct TextTransform {
    /// The case or exclusive mathematical transform.
    pub case: TextCase,
    /// `full-width`: converts characters to full-width forms.
    ///
    /// Covers printable ASCII, half-width katakana and Hangul, and some
    /// symbols. A preserved space becomes U+3000. Applied after case mapping.
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

    /// Returns whether the case is `math-auto`, which excludes the other
    /// transforms.
    pub(crate) fn is_math_auto(self) -> bool {
        matches!(self.case, TextCase::MathAuto)
    }
}

/// `-webkit-text-security`: the character each grapheme of text is drawn as.
///
/// Masked text is laid out as one mask character per extended grapheme
/// cluster, after `text-transform`. Spaces, tabs and segment breaks are
/// masked too, so none of them collapses or breaks a line. Offsets, carets
/// and selections refer to the unmasked text, and each mask is one caret
/// stop.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextSecurity {
    /// The text as written. The initial value.
    #[default]
    None,
    /// U+2022 BULLET.
    Disc,
    /// U+25E6 WHITE BULLET.
    Circle,
    /// U+25A0 BLACK SQUARE.
    Square,
}

impl TextSecurity {
    /// Returns the character text is masked with, or `None` for `none`.
    pub const fn mask(self) -> Option<char> {
        match self {
            Self::None => None,
            Self::Disc => Some('\u{2022}'),
            Self::Circle => Some('\u{25E6}'),
            Self::Square => Some('\u{25A0}'),
        }
    }
}

/// `line-break`: how strictly CJK line-breaking rules apply.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum LineBreak {
    /// The rules chosen by the content language. The initial value.
    ///
    /// Breaks as `normal`, but tailors the rules for Chinese text only, not
    /// for Japanese or Korean: those may then break before neither U+301C
    /// `〜` nor a curly quotation mark beside an ideograph.
    #[default]
    Auto,
    /// The least restrictive rules.
    Loose,
    /// The common rules.
    ///
    /// Tailored for Chinese, Japanese and Korean text.
    Normal,
    /// The most restrictive rules.
    Strict,
    /// An opportunity around every typographic character unit.
    Anywhere,
}

/// `word-break`: where lines may break between letters.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum WordBreak {
    /// The customary rules for each script. The initial value.
    #[default]
    Normal,
    /// An opportunity between any two letters, as for ideographs.
    BreakAll,
    /// No opportunity between letters, ideographs included.
    KeepAll,
    /// Japanese text breaks only between phrases, where a
    /// [`PhraseSegmenter`](crate::segment::PhraseSegmenter) finds them.
    ///
    /// A line that no phrase boundary lets fit breaks as under `normal`.
    /// Text in another language, or with no segmenter on the
    /// [`Context`](crate::Context), breaks as under `normal`.
    AutoPhrase,
}

/// `hyphens`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum Hyphens {
    /// No hyphenation, not even at soft hyphens.
    None,
    /// Breaks at soft hyphens only. The initial value.
    #[default]
    Manual,
    /// Breaks at soft hyphens and where a
    /// [`Hyphenator`](crate::segment::Hyphenator) finds a break in a word of
    /// the text's language.
    ///
    /// With no hyphenator on the [`Context`](crate::Context), or text with no
    /// language, behaves as `manual`.
    Auto,
}

/// `tab-size`.
#[derive(Copy, Clone, Debug)]
pub enum TabSize {
    /// A multiple of the advance of a space in the element's font.
    Spaces(f32),
    /// A distance in CSS pixels.
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
    /// A length plus a percentage of a basis.
    ///
    /// The basis is the computed font size for `letter-spacing` and
    /// `word-spacing`, the line width for `text-indent`, and the containing
    /// block's inline size for margins and padding.
    ///
    /// A `calc()` that mixes the two linearly, such as `calc(10px + 5%)`,
    /// is one too. [`LengthPercentageOrCalc`](crate::style::LengthPercentageOrCalc)
    /// carries any other `calc()`.
    pub struct LengthPercentage {
        /// The length, in CSS pixels.
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

    /// Returns a length of `px` pixels, with no percentage.
    pub const fn from_px(px: f32) -> Self {
        Self { px, fraction: 0.0 }
    }

    /// Returns whether it is zero at any basis.
    pub(crate) fn is_zero(self) -> bool {
        self.px == 0.0 && self.fraction == 0.0
    }

    /// Returns the length in pixels, with the percentage taken of `basis` pixels.
    ///
    /// A NaN in either part or in `basis` gives a NaN, which the layout
    /// grid takes as zero.
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
    /// The initial value. CSS gives [`NORMAL`](Self::NORMAL).
    pub const NO_AUTOSPACE: Self = Self {
        ideograph_alpha: false,
        ideograph_numeric: false,
    };

    /// `normal`: both kinds of space.
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
    /// As `Normal`, but an opening mark keeps its blank only at the start of
    /// the first line and of a line after a forced break.
    SpaceFirst,
    /// As `Normal`, and an opening mark loses its blank at every line start.
    TrimStart,
}

impl TextSpacingTrim {
    /// Returns whether text trims any punctuation: all but `space-all`.
    pub(crate) fn trims_punctuation(self) -> bool {
        self != Self::SpaceAll
    }

    /// Returns whether a wrapped line's opening mark is trimmed: under
    /// `space-first` and `trim-start`, as Blink's `ShouldTrimStartOfWrappedLine`.
    pub(crate) fn trims_wrapped_start(self) -> bool {
        matches!(self, Self::SpaceFirst | Self::TrimStart)
    }

    /// Returns whether a paragraph's opening mark is trimmed: under
    /// `trim-start`, as Blink's `ShouldTrimStartOfParagraph`.
    pub(crate) fn trims_paragraph_start(self) -> bool {
        self == Self::TrimStart
    }
}

/// Whether a stop or comma at a line's end hangs: the end part of
/// `hanging-punctuation`.
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

/// `text-decoration-skip-spaces`: the spaces that underlines, overlines and
/// lines through leave out.
///
/// The spaces are U+0020, the tab, the no-break spaces and the other space
/// separators (Unicode category Zs), with any word-spacing after them. A tab
/// has no decoration under any value.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextDecorationSkipSpaces {
    /// `none`: decorations run under every space.
    None,
    /// `all`: decorations leave out the spaces at a line's ends and the
    /// spaces between two characters of one text run.
    ///
    /// They also leave out the letter-spacing at the right end of the line's
    /// content. Spaces at either end of a text run inside the line keep their
    /// decoration.
    All,
    /// `start`: decorations leave out the spaces at a line's start.
    Start,
    /// `end`: decorations leave out the spaces at a line's end.
    End,
    /// `start end`: decorations leave out the spaces at both ends of a line.
    ///
    /// The initial value.
    #[default]
    StartEnd,
}

impl TextDecorationSkipSpaces {
    /// Returns whether the spaces at a line's start are left out.
    pub(crate) fn skips_start(self) -> bool {
        matches!(self, Self::All | Self::Start | Self::StartEnd)
    }

    /// Returns whether the spaces at a line's end are left out.
    pub(crate) fn skips_end(self) -> bool {
        matches!(self, Self::All | Self::End | Self::StartEnd)
    }
}

same_by_value!(
    TextDecorationSkipSpaces,
    WhiteSpaceCollapse,
    WhiteSpaceTrim,
    TextCase,
    TextTransform,
    TextSecurity,
    LineBreak,
    WordBreak,
    Hyphens,
    TextAutospace,
    TextSpacingTrim,
    HangEnd,
    HangingPunctuation,
    TextJustify,
);

eq_and_hash_by_bits!(TabSize);
