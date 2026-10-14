//! Font synthesis, variants, kerning and size-adjustment values.

use core::hash::{Hash, Hasher};
use core::mem;
use parlance::Tag;

use crate::style::same::{Same, eq_and_hash_by_bits, same_by_value, style_struct};

style_struct! {
    /// `font-synthesis`: which missing faces may be synthesized, one field
    /// per longhand.
    pub struct FontSynthesis {
        /// `font-synthesis-weight`: emboldening.
        pub weight: bool,
        /// `font-synthesis-style`: skewing.
        pub style: bool,
        /// `font-synthesis-small-caps`: small capitals from scaled capitals.
        pub small_caps: bool,
        /// `font-synthesis-position`: scaled superscripts and subscripts.
        pub position: bool,
    }
}

impl FontSynthesis {
    /// `auto` for each longhand: everything may be synthesized. The initial
    /// value.
    pub const ALL: Self = Self {
        weight: true,
        style: true,
        small_caps: true,
        position: true,
    };
}

/// `font-variant-caps`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum FontVariantCaps {
    /// No capital forms. The initial value.
    #[default]
    Normal,
    /// Lowercase as small capitals (`smcp`).
    SmallCaps,
    /// Both cases as small capitals (`c2sc`, `smcp`).
    AllSmallCaps,
    /// Lowercase as petite capitals (`pcap`).
    PetiteCaps,
    /// Both cases as petite capitals (`c2pc`, `pcap`).
    AllPetiteCaps,
    /// Unicase forms (`unic`).
    Unicase,
    /// Titling capitals (`titl`).
    TitlingCaps,
}

/// `font-variant-position`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum FontVariantPosition {
    /// Ordinary glyphs. The initial value.
    #[default]
    Normal,
    /// Subscript forms (`subs`), synthesized where the font has none.
    Sub,
    /// Superscript forms (`sups`), synthesized where the font has none.
    Super,
}

/// A set of `font-variant-*` keywords.
///
/// Combines `font-variant-ligatures`, `font-variant-numeric`,
/// `font-variant-east-asian` and `font-variant-alternates`. The empty set is
/// `normal` for all four. Ligature `none` sets every `NO_` ligature bit.
///
/// Of the alternates, only `historical-forms` is supported. The others need
/// `@font-feature-values`, which layout does not receive.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct FontVariants(u32);

impl FontVariants {
    /// `no-common-ligatures`.
    pub const NO_COMMON_LIGATURES: Self = Self(1 << 0);
    /// `common-ligatures`.
    pub const COMMON_LIGATURES: Self = Self(1 << 1);
    /// `no-discretionary-ligatures`.
    pub const NO_DISCRETIONARY_LIGATURES: Self = Self(1 << 2);
    /// `discretionary-ligatures`.
    pub const DISCRETIONARY_LIGATURES: Self = Self(1 << 3);
    /// `no-historical-ligatures`.
    pub const NO_HISTORICAL_LIGATURES: Self = Self(1 << 4);
    /// `historical-ligatures`.
    pub const HISTORICAL_LIGATURES: Self = Self(1 << 5);
    /// `no-contextual`.
    pub const NO_CONTEXTUAL: Self = Self(1 << 6);
    /// `contextual`.
    pub const CONTEXTUAL: Self = Self(1 << 7);
    /// `lining-nums`.
    pub const LINING_NUMS: Self = Self(1 << 8);
    /// `oldstyle-nums`.
    pub const OLDSTYLE_NUMS: Self = Self(1 << 9);
    /// `proportional-nums`.
    pub const PROPORTIONAL_NUMS: Self = Self(1 << 10);
    /// `tabular-nums`.
    pub const TABULAR_NUMS: Self = Self(1 << 11);
    /// `diagonal-fractions`.
    pub const DIAGONAL_FRACTIONS: Self = Self(1 << 12);
    /// `stacked-fractions`.
    pub const STACKED_FRACTIONS: Self = Self(1 << 13);
    /// `ordinal`.
    pub const ORDINAL: Self = Self(1 << 14);
    /// `slashed-zero`.
    pub const SLASHED_ZERO: Self = Self(1 << 15);
    /// `jis78`.
    pub const JIS78: Self = Self(1 << 16);
    /// `jis83`.
    pub const JIS83: Self = Self(1 << 17);
    /// `jis90`.
    pub const JIS90: Self = Self(1 << 18);
    /// `jis04`.
    pub const JIS04: Self = Self(1 << 19);
    /// `simplified`.
    pub const SIMPLIFIED: Self = Self(1 << 20);
    /// `traditional`.
    pub const TRADITIONAL: Self = Self(1 << 21);
    /// `full-width`.
    pub const FULL_WIDTH: Self = Self(1 << 22);
    /// `proportional-width`.
    pub const PROPORTIONAL_WIDTH: Self = Self(1 << 23);
    /// `ruby`.
    pub const RUBY: Self = Self(1 << 24);
    /// `historical-forms`, of `font-variant-alternates`.
    pub const HISTORICAL_FORMS: Self = Self(1 << 25);

    /// No keyword: `normal` for every longhand. The initial value.
    pub const NORMAL: Self = Self(0);

    /// Every keyword's bit, which `from_bits` keeps.
    const ALL: u32 = (1 << 26) - 1;

    /// Creates a keyword set from `bits`, discarding unknown bits.
    ///
    /// The bit positions are those of the associated constants.
    ///
    /// ```
    /// use winkin::style::FontVariants;
    ///
    /// assert_eq!(FontVariants::from_bits(1 << 14), FontVariants::ORDINAL);
    /// assert_eq!(FontVariants::from_bits(1 << 31), FontVariants::NORMAL);
    /// ```
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & Self::ALL)
    }

    /// Returns `true` if every keyword in `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the keywords set in either.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// `font-variant-emoji`: the presentation of emoji characters without a
/// variation selector.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum FontVariantEmoji {
    /// Each character's own default presentation. The initial value.
    #[default]
    Normal,
    /// Text presentation.
    Text,
    /// Emoji presentation.
    Emoji,
    /// The default presentation from Unicode emoji data.
    Unicode,
}

/// `font-optical-sizing`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum FontOpticalSizing {
    /// The `opsz` axis follows the font size, unless
    /// `font-variation-settings` sets it. The initial value.
    #[default]
    Auto,
    /// The `opsz` axis stays at the font's default.
    None,
}

/// `font-kerning`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum FontKerning {
    /// The shaper's default, which kerns. The initial value.
    #[default]
    Auto,
    /// `kern` on.
    Normal,
    /// `kern` and `vkrn` off.
    None,
}

/// `font-language-override`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum FontLanguageOverride {
    /// The OpenType language system follows the text's language.
    ///
    /// The initial value.
    #[default]
    Normal,
    /// This OpenType language system tag.
    ///
    /// Its case is kept: `trk` is not the Turkish system `TRK`.
    System(Tag),
}

/// The font metric used by `font-size-adjust`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum AdjustMetric {
    /// The x-height; the default when no metric is specified.
    #[default]
    ExHeight,
    /// The cap height.
    CapHeight,
    /// The advance of `0`, the `ch` unit.
    ChWidth,
    /// The advance of U+6C34 (水), the `ic` unit.
    IcWidth,
    /// The vertical advance of U+6C34 (水).
    IcHeight,
}

/// `font-size-adjust`.
///
/// Scales the used font size so that the chosen metric of the first
/// available font is `value` times the computed size. Layout resolves it.
#[derive(Copy, Clone, Debug, Default)]
pub enum FontSizeAdjust {
    /// The size is used as computed. The initial value.
    #[default]
    None,
    /// `metric` is held at `value` times the computed size.
    Hold {
        /// The metric held.
        metric: AdjustMetric,
        /// Its fraction of the size.
        ///
        /// At `0` the text is set at size zero, as CSS Fonts 4 specifies. A
        /// value that is negative or not finite leaves the size as computed.
        value: f32,
    },
}

impl Same for FontSizeAdjust {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) => true,
            (
                Self::Hold { metric, value },
                Self::Hold {
                    metric: m,
                    value: v,
                },
            ) => metric == m && value.same(v),
            _ => false,
        }
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        if let Self::Hold { metric, value } = self {
            metric.hash(state);
            value.feed(state);
        }
    }
}

same_by_value!(
    FontVariantCaps,
    FontVariantPosition,
    FontVariants,
    FontVariantEmoji,
    FontOpticalSizing,
    FontKerning,
    FontLanguageOverride,
    AdjustMetric,
);

eq_and_hash_by_bits!(FontSizeAdjust);
