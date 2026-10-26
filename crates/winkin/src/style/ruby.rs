//! Ruby layout values.

use crate::style::same::same_by_value;

/// `ruby-position`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum RubyPosition {
    /// Above the base.
    ///
    /// The initial value. CSS gives [`Alternate`](Self::Alternate).
    #[default]
    Over,
    /// Below the base.
    Under,
    /// Alternating sides, starting above.
    Alternate,
    /// Alternating sides, starting below.
    AlternateUnder,
}

/// `ruby-align`: how the narrower of base and annotation fills the wider.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum RubyAlign {
    /// Equal gaps between characters, half a gap at each end.
    ///
    /// The initial value.
    #[default]
    SpaceAround,
    /// Equal gaps between characters, none at the ends.
    SpaceBetween,
    /// Centered.
    Center,
    /// At the start.
    Start,
}

/// `ruby-overhang`: whether a wide annotation may extend over adjacent text.
///
/// A wide annotation is one wider than its base.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum RubyOverhang {
    /// Over adjacent text, within limits. The initial value.
    #[default]
    Auto,
    /// Never.
    None,
    /// Over the spaces and tabs beside it only, and never over a blank that
    /// another column already reaches over.
    Spaces,
}

same_by_value!(RubyPosition, RubyAlign, RubyOverhang,);
