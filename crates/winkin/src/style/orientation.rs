//! Text orientation and combination values.

use crate::style::same::same_by_value;

/// `text-orientation`: the orientation of glyphs in vertical lines.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextOrientation {
    /// By each character's Vertical_Orientation. The initial value.
    #[default]
    Mixed,
    /// Every character upright.
    Upright,
    /// Every character on its side.
    Sideways,
}

/// `text-combine-upright`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum TextCombineUpright {
    /// Set as the rest of the line is. The initial value.
    #[default]
    None,
    /// The element's text set across the line in one upright em.
    All,
    /// Each run of up to this many ASCII digits, combined.
    Digits(u8),
}

same_by_value!(TextOrientation, TextCombineUpright);
