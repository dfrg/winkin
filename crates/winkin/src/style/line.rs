//! Line height, alignment, initial-letter and trimming values.

use core::hash::{Hash, Hasher};
use core::mem;

use crate::style::same::{Same, eq_and_hash_by_bits, same_by_value, style_struct};

/// `line-height`.
#[derive(Copy, Clone, Debug, Default)]
pub enum LineHeight {
    /// From the font's metrics. The initial value.
    #[default]
    Normal,
    /// A multiple of the element's own font size.
    ///
    /// It inherits as the factor.
    Factor(f32),
    /// A length in CSS pixels.
    Px(f32),
}

impl Same for LineHeight {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Normal, Self::Normal) => true,
            (Self::Factor(a), Self::Factor(b)) | (Self::Px(a), Self::Px(b)) => a.same(b),
            _ => false,
        }
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        if let Self::Factor(n) | Self::Px(n) = self {
            n.feed(state);
        }
    }
}

/// `vertical-align`. Not inherited, but nested shifts add up.
#[derive(Copy, Clone, Debug, Default)]
pub enum VerticalAlign {
    /// Aligns with the parent baseline. The initial value.
    #[default]
    Baseline,
    /// Lowered to the parent's subscript position.
    Sub,
    /// Raised to the parent's superscript position.
    Super,
    /// The top on the top of the parent's text.
    TextTop,
    /// The bottom on the bottom of the parent's text.
    TextBottom,
    /// The middle on the parent's baseline plus half its x-height.
    Middle,
    /// The top on the line box's top.
    Top,
    /// The bottom on the line box's bottom.
    Bottom,
    /// Raised by this many CSS pixels.
    Px(f32),
    /// Raised by this fraction of the element's line height.
    Fraction(f32),
}

impl Same for VerticalAlign {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Px(a), Self::Px(b)) | (Self::Fraction(a), Self::Fraction(b)) => a.same(b),
            (Self::Px(_) | Self::Fraction(_), _) | (_, Self::Px(_) | Self::Fraction(_)) => false,
            _ => mem::discriminant(self) == mem::discriminant(other),
        }
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        if let Self::Px(n) | Self::Fraction(n) = self {
            n.feed(state);
        }
    }
}

/// `dominant-baseline`: the baseline used for parent alignment.
///
/// Ignored on inline boxes unless
/// [`Config::dominant_baseline`](crate::config::Config::dominant_baseline) applies it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum DominantBaseline {
    /// Alphabetic, in horizontal text. The initial value.
    #[default]
    Auto,
    /// The bottom of the primary font's ascent and descent: `text-bottom`.
    TextBottom,
    /// The alphabetic baseline.
    Alphabetic,
    /// The bottom of the ideographic em box.
    Ideographic,
    /// Halfway between the alphabetic baseline and the x-height: `middle`.
    Middle,
    /// The middle of the em box.
    Central,
    /// The mathematical baseline.
    Mathematical,
    /// The hanging baseline.
    Hanging,
    /// The top of the primary font's ascent and descent: `text-top`.
    TextTop,
}

/// `initial-letter-align`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum InitialLetterAlign {
    /// From the first line's cap height to the last line's alphabetic baseline.
    ///
    /// The last line is the last the letter spans. The initial value.
    #[default]
    Alphabetic,
    /// On the first line's hanging baseline.
    Hanging,
    /// The em box across the lines spanned, for CJK.
    Ideographic,
}

style_struct! {
    /// `initial-letter` and `initial-letter-align`.
    ///
    /// Applies only where `size > 1` and `sink >= 1`. Layout works out the
    /// letter's used size from font metrics.
    pub struct InitialLetter {
        /// Its height, in lines.
        pub size: f32,
        /// How many lines it sinks into: 1 is a raised cap.
        pub sink: u32,
        /// `initial-letter-align`.
        pub align: InitialLetterAlign,
    }
}

impl InitialLetter {
    /// `normal`: no initial letter. The initial value.
    pub const NONE: Self = Self {
        size: 0.0,
        sink: 0,
        align: InitialLetterAlign::Alphabetic,
    };

    /// Returns `true` if an initial letter applies: `size > 1` and `sink >= 1`.
    pub fn is_set(self) -> bool {
        self.size > 1.0 && self.sink >= 1
    }
}

/// `text-box-trim`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextBoxTrim {
    /// Nothing trimmed. The initial value.
    #[default]
    None,
    /// The start side.
    TrimStart,
    /// The end side.
    TrimEnd,
    /// Both sides.
    TrimBoth,
}

impl TextBoxTrim {
    /// Returns which ends of its block it trims, as `(start, end)`.
    pub(crate) fn sides(self) -> (bool, bool) {
        match self {
            Self::None => (false, false),
            Self::TrimStart => (true, false),
            Self::TrimEnd => (false, true),
            Self::TrimBoth => (true, true),
        }
    }
}

/// Where a trimmed over edge lands.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextEdgeOver {
    /// The font's ascent, which removes only the half-leading.
    #[default]
    Text,
    /// The cap height.
    Cap,
    /// The x-height.
    Ex,
    /// The top of the ideographic em box.
    Ideographic,
    /// The top of the ideographic character face.
    IdeographicInk,
}

/// Where a trimmed under edge lands.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextEdgeUnder {
    /// The font's descent, which removes only the half-leading.
    #[default]
    Text,
    /// The alphabetic baseline.
    Alphabetic,
    /// The bottom of the ideographic em box.
    Ideographic,
    /// The bottom of the ideographic character face.
    IdeographicInk,
}

/// `text-box-edge`: where each trimmed edge lands.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct TextBoxEdge {
    /// The over edge.
    pub over: TextEdgeOver,
    /// The under edge.
    pub under: TextEdgeUnder,
}

impl TextBoxEdge {
    /// `auto`, which is `text` for both edges. The initial value.
    pub const AUTO: Self = Self {
        over: TextEdgeOver::Text,
        under: TextEdgeUnder::Text,
    };
}

/// `line-fit-edge`: which edge of an inline box's text a line fits.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum LineFitEdge {
    /// The half-leading edges. The initial value.
    #[default]
    Leading,
    /// The edges given by these metrics.
    Edge(TextBoxEdge),
}

same_by_value!(
    DominantBaseline,
    InitialLetterAlign,
    TextBoxTrim,
    TextBoxEdge,
    LineFitEdge,
);

eq_and_hash_by_bits!(LineHeight, VerticalAlign);
