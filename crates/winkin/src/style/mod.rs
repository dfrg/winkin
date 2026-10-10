//! Computed CSS styles for inline content and block containers.
//!
//! [`ComputedStyle`] groups layout properties by subject: [`FontGroup`],
//! [`TextGroup`], [`LineGroup`], [`BidiGroup`], [`OrientationGroup`],
//! [`EdgesGroup`] and [`RubyGroup`]. [`ComputedBlockStyle`] adds first-line
//! styles and block-container properties.
//!
//! The host runs the cascade and retains paint-only properties, including
//! colors, decoration lines, shadows and emphasis shapes, indexed by node key.
//! The builder copies and interns required values; it retains no borrowed
//! styles. Reusing styles and lists does not allocate.
//!
//! # Values
//!
//! Supply computed values. Resolve inheritance, `match-parent`, containing-block
//! percentages and font-relative lengths before building. Retain values that
//! CSS preserves or layout must resolve: unitless line height, percentage word
//! spacing, percentage vertical alignment and `font-size-adjust` metrics.
//!
//! Initial values follow CSS, except where Chrome computes a different initial
//! value, as documented for `text-autospace` and `ruby-position`.

// - `font`, `text`, `emphasis` (part of the text group), `line`, `bidi`,
//   `orientation`, `edges` and `ruby`: the values of one style group each,
//   with their initial values. `block` holds the values a block has one of.
// - `same`: equality and hashing by bits, which the builder's memo keys on.
// - `first_line`: the first-line variant and the tables kept for it.
mod bidi;
mod block;
mod edges;
mod emphasis;
mod first_line;
mod font;
mod line;
mod orientation;
mod ruby;
mod same;
mod text;

pub use bidi::*;
pub use block::*;
pub use edges::*;
pub use emphasis::*;
pub use font::*;
pub use line::*;
pub use orientation::*;
pub use parlance::{
    BaseDirection, FontFamilyName, FontFeature, FontStyle, FontVariation, FontWeight, FontWidth,
    GenericFamily, Language, OverflowWrap, Tag, TextWrapMode, WordBreak,
};
pub use ruby::*;
pub use text::*;

pub(crate) use first_line::{FirstLine, FirstLineState, FirstLineVariant};
pub(crate) use same::{Same, same_by_value, style_struct};

use crate::stages::content::BlockFacts;
use crate::unit::LayoutUnit;

style_struct! {
    /// The font group: what selects a font, and what shapes in it.
    ///
    /// It borrows the family list and the settings lists for the call.
    pub struct FontGroup<'a> {
        /// The CSS family list in preference order.
        ///
        /// The language-specific Standard font follows the list, matching Chrome's
        /// `-webkit-standard` fallback. An empty list uses only that font.
        /// For example, on Windows, Simplified Chinese defaults to Microsoft YaHei,
        /// while `serif` resolves to SimSun.
        pub families: &'a [FontFamilyName<'a>],
        /// The computed `font-size`, in pixels, before `font-size-adjust`.
        ///
        /// `font-size-adjust` is applied during layout after font selection.
        pub size: f32,
        /// `font-weight`.
        pub weight: FontWeight,
        /// `font-width` (`font-stretch`).
        pub width: FontWidth,
        /// `font-style`.
        pub style: FontStyle,
        /// `font-synthesis`.
        pub synthesis: FontSynthesis,
        /// `font-variant-caps`.
        pub variant_caps: FontVariantCaps,
        /// `font-variant-position`.
        pub variant_position: FontVariantPosition,
        /// `font-variant-ligatures`, `-numeric`, `-east-asian`, `-alternates`.
        pub variants: FontVariants,
        /// `font-variant-emoji`.
        pub variant_emoji: FontVariantEmoji,
        /// `font-optical-sizing`.
        pub optical_sizing: FontOpticalSizing,
        /// `font-kerning`.
        pub kerning: FontKerning,
        /// `font-language-override`.
        pub language_override: FontLanguageOverride,
        /// `font-size-adjust`.
        pub size_adjust: FontSizeAdjust,
        /// `font-feature-settings`.
        pub features: &'a [FontFeature],
        /// `font-variation-settings`.
        pub variations: &'a [FontVariation],
    }
}

impl FontGroup<'_> {
    /// Every font property at its initial value: `medium`, which is 16 px.
    pub const INITIAL: Self = Self {
        families: &[],
        size: 16.0,
        weight: FontWeight::NORMAL,
        width: FontWidth::NORMAL,
        style: FontStyle::Normal,
        synthesis: FontSynthesis::ALL,
        variant_caps: FontVariantCaps::Normal,
        variant_position: FontVariantPosition::Normal,
        variants: FontVariants::NORMAL,
        variant_emoji: FontVariantEmoji::Normal,
        optical_sizing: FontOpticalSizing::Auto,
        kerning: FontKerning::Auto,
        language_override: FontLanguageOverride::Normal,
        size_adjust: FontSizeAdjust::None,
        features: &[],
        variations: &[],
    };
}

/// The largest font size used, in pixels: Chrome's `kMaximumAllowedFontSize`.
/// A larger size is used at this one.
const MAX_FONT_SIZE: f32 = 10_000.0;

/// Returns `px` as a usable font size, clamped as Chrome clamps a computed size.
///
/// A size that is not finite or is below zero becomes zero. A size above
/// [`MAX_FONT_SIZE`] becomes that. Every size a font is used at, computed or
/// adjusted, passes through here, so no caller or font value escapes it.
pub(crate) fn sanitized_font_size(px: f32) -> f32 {
    if px.is_finite() && px > 0.0 {
        px.min(MAX_FONT_SIZE)
    } else {
        0.0
    }
}

style_struct! {
    /// Whitespace, wrapping, breaking, transforms, spacing and emphasis properties.
    ///
    /// Borrows `hyphenate-character` for the builder call.
    pub struct TextGroup<'a> {
        /// The content language, from the nearest `lang`.
        ///
        /// `None` where there is none.
        pub language: Option<Language>,
        /// `white-space-collapse`.
        pub white_space_collapse: WhiteSpaceCollapse,
        /// `text-wrap-mode`.
        pub wrap_mode: TextWrapMode,
        /// `white-space-trim`. Not inherited.
        pub white_space_trim: WhiteSpaceTrim,
        /// `text-transform`.
        pub transform: TextTransform,
        /// `-webkit-text-security`.
        pub security: TextSecurity,
        /// `word-break`.
        pub word_break: WordBreak,
        /// `line-break`.
        pub line_break: LineBreak,
        /// `overflow-wrap` (`word-wrap`).
        pub overflow_wrap: OverflowWrap,
        /// `hyphens`.
        pub hyphens: Hyphens,
        /// `hyphenate-character`: `None` for `auto`, else the string.
        ///
        /// The string may be empty.
        pub hyphenate_character: Option<&'a str>,
        /// `tab-size`.
        pub tab_size: TabSize,
        /// `letter-spacing`, resolved to pixels.
        ///
        /// Percentages use the computed font size and inherit as percentages.
        /// Resolve them separately for each element, matching CSS Text 4 and
        /// Chrome.
        pub letter_spacing: f32,
        /// `word-spacing`, as a length plus a percentage.
        ///
        /// Percentages use the computed font size, not the space width, and
        /// inherit unchanged. Layout resolves them, matching CSS Text 4 and
        /// Chrome.
        pub word_spacing: LengthPercentage,
        /// `text-autospace`.
        pub autospace: TextAutospace,
        /// `text-spacing-trim`.
        pub spacing_trim: TextSpacingTrim,
        /// `hanging-punctuation`.
        pub hanging_punctuation: HangingPunctuation,
        /// `text-justify`.
        pub justify: TextJustify,
        /// `text-emphasis`, as far as layout reads it.
        pub emphasis: TextEmphasis,
    }
}

impl TextGroup<'_> {
    /// Every text property at its initial value.
    pub const INITIAL: Self = Self {
        language: None,
        white_space_collapse: WhiteSpaceCollapse::Collapse,
        wrap_mode: TextWrapMode::Wrap,
        white_space_trim: WhiteSpaceTrim::NONE,
        transform: TextTransform::NONE,
        security: TextSecurity::None,
        word_break: WordBreak::Normal,
        line_break: LineBreak::Normal,
        overflow_wrap: OverflowWrap::Normal,
        hyphens: Hyphens::Manual,
        hyphenate_character: None,
        tab_size: TabSize::INITIAL,
        letter_spacing: 0.0,
        word_spacing: LengthPercentage::ZERO,
        autospace: TextAutospace::NO_AUTOSPACE,
        spacing_trim: TextSpacingTrim::Normal,
        hanging_punctuation: HangingPunctuation::NONE,
        justify: TextJustify::Auto,
        emphasis: TextEmphasis::NONE,
    };
}

style_struct! {
    /// The line group: how tall text makes a line, and where it sits in it.
    pub struct LineGroup {
        /// `line-height`.
        pub height: LineHeight,
        /// `vertical-align`. Not inherited.
        pub vertical_align: VerticalAlign,
        /// `dominant-baseline`.
        pub dominant_baseline: DominantBaseline,
        /// `initial-letter` and `initial-letter-align`.
        pub initial_letter: InitialLetter,
        /// `line-padding`, in pixels. Supported by winkin but not Chrome.
        pub padding: f32,
        /// `text-box-trim` on an inline box.
        ///
        /// Set block trimming through [`ComputedBlockStyle`] instead.
        pub text_box_trim: TextBoxTrim,
        /// `text-box-edge` on an inline box.
        pub text_box_edge: TextBoxEdge,
        /// `line-fit-edge`.
        pub fit_edge: LineFitEdge,
    }
}

impl LineGroup {
    /// Every line property at its initial value.
    pub const INITIAL: Self = Self {
        height: LineHeight::Normal,
        vertical_align: VerticalAlign::Baseline,
        dominant_baseline: DominantBaseline::Auto,
        initial_letter: InitialLetter::NONE,
        padding: 0.0,
        text_box_trim: TextBoxTrim::None,
        text_box_edge: TextBoxEdge::AUTO,
        fit_edge: LineFitEdge::Leading,
    };
}

style_struct! {
    /// The bidi group.
    pub struct BidiGroup {
        /// `direction`.
        pub direction: Direction,
        /// `unicode-bidi`. Not inherited.
        pub unicode_bidi: UnicodeBidi,
    }
}

impl BidiGroup {
    /// Both at their initial values.
    pub const INITIAL: Self = Self {
        direction: Direction::Ltr,
        unicode_bidi: UnicodeBidi::Normal,
    };
}

style_struct! {
    /// The orientation group, read in vertical lines.
    pub struct OrientationGroup {
        /// `text-orientation`.
        pub text_orientation: TextOrientation,
        /// `text-combine-upright`.
        pub text_combine_upright: TextCombineUpright,
    }
}

impl OrientationGroup {
    /// Both at their initial values.
    pub const INITIAL: Self = Self {
        text_orientation: TextOrientation::Mixed,
        text_combine_upright: TextCombineUpright::None,
    };
}

style_struct! {
    /// Margin, border and padding on physical sides. Not inherited.
    ///
    /// Percentages in margins and padding are of the containing block's
    /// inline size, which a layout takes from [`BuildOptions::percentage_basis`]
    /// and [`Layout::measure`]. Layout maps edges to the writing mode and
    /// truncates lengths toward zero on the 1/64-pixel grid, matching Chrome:
    /// 10.012 px becomes 10 px and 10.99 px becomes 10.984375 px.
    ///
    /// [`BuildOptions::percentage_basis`]: crate::BuildOptions::percentage_basis
    /// [`Layout::measure`]: crate::Layout::measure
    pub struct EdgesGroup {
        /// `margin`, in pixels and as a fraction of the basis.
        pub margin: Sides<LengthPercentage>,
        /// Used `border-width` in pixels, or zero for border style `none`.
        ///
        /// Snap widths before building, matching Chrome: positive widths below
        /// one pixel become one; larger widths round down to whole pixels.
        /// Thus 0.5 px becomes 1 px and 2.5 px becomes 2 px.
        pub border: Sides<f32>,
        /// `padding`, in pixels and as a fraction of the basis.
        pub padding: Sides<LengthPercentage>,
        /// `box-decoration-break`.
        pub decoration_break: BoxDecorationBreak,
    }
}

impl EdgesGroup {
    /// No margin, border or padding.
    pub const INITIAL: Self = Self {
        margin: Sides::from_px(0.0),
        border: Sides::ZERO,
        padding: Sides::from_px(0.0),
        decoration_break: BoxDecorationBreak::Slice,
    };

    /// Returns `true` if any side has a nonzero margin, border or padding,
    /// at any basis.
    pub fn any(&self) -> bool {
        self.margin.any() || self.border.any() || self.padding.any()
    }

    /// Whether a margin or padding has a percentage.
    pub(crate) fn has_percentage(&self) -> bool {
        self.margin.has_percentage() || self.padding.has_percentage()
    }

    /// Returns the edges in pixels, each percentage taken of `basis` pixels.
    pub(crate) fn used(&self, basis: f32) -> UsedEdges {
        UsedEdges {
            margin: self.margin.resolve(basis),
            border: self.border,
            padding: self.padding.resolve(basis),
        }
    }
}

/// A box's margin, border and padding in pixels, its percentages resolved.
#[derive(Copy, Clone, Debug)]
pub(crate) struct UsedEdges {
    /// `margin`, in pixels.
    pub(crate) margin: Sides<f32>,
    /// `border-width`, in pixels.
    pub(crate) border: Sides<f32>,
    /// `padding`, in pixels.
    pub(crate) padding: Sides<f32>,
}

impl UsedEdges {
    /// Returns the room a box takes along the line at each end.
    ///
    /// The pair is `(line-left, line-right)`. Each end sums the margin,
    /// border and padding on the physical side `writing_mode` puts there.
    /// Each length is truncated onto layout's grid, as Chrome makes a
    /// `LayoutUnit` from a float.
    ///
    /// Every stage that charges a box's edges reads this: measurement for the
    /// prefix sums, the breaker for a line's ends, and line layout for the
    /// pieces. So fitting and placing cannot disagree. The caller picks the
    /// end an opening or closing edge takes from the edge's bidi level. The
    /// sides across the line take no room on it.
    pub(crate) fn inline(&self, writing_mode: WritingMode) -> (LayoutUnit, LayoutUnit) {
        let side = |sides: Sides<f32>| sides.along_line_on_grid(writing_mode);
        let (margin, border, padding) = (side(self.margin), side(self.border), side(self.padding));
        (
            margin.0 + border.0 + padding.0,
            margin.1 + border.1 + padding.1,
        )
    }
}

style_struct! {
    /// The ruby group.
    pub struct RubyGroup {
        /// `ruby-position`.
        pub position: RubyPosition,
        /// `ruby-align`.
        pub align: RubyAlign,
        /// `ruby-overhang`.
        pub overhang: RubyOverhang,
    }
}

impl RubyGroup {
    /// All three at their initial values.
    pub const INITIAL: Self = Self {
        position: RubyPosition::Over,
        align: RubyAlign::SpaceAround,
        overhang: RubyOverhang::Auto,
    };
}

macro_rules! initial_is_default {
    ($($ty:ty),*) => {$(
        impl Default for $ty {
            /// Every property at its initial value.
            fn default() -> Self {
                Self::INITIAL
            }
        }
    )*};
}

initial_is_default!(
    FontGroup<'_>,
    TextGroup<'_>,
    LineGroup,
    BidiGroup,
    OrientationGroup,
    EdgesGroup,
    RubyGroup
);

/// Computed layout properties for a box or inline element.
///
/// Used for blocks, inline boxes, atomic inlines, floats, ruby containers
/// and annotations. Properties are grouped by subject. Variable-length values
/// are borrowed during the builder call, then copied or interned; the layout
/// retains no borrowed styles.
///
/// ```
/// use winkin::style::{ComputedStyle, FontFamilyName, FontGroup, FontWeight, GenericFamily};
///
/// let families = [FontFamilyName::named("Inter"), GenericFamily::SansSerif.into()];
/// let heading = ComputedStyle {
///     font: FontGroup {
///         families: &families,
///         size: 24.0,
///         weight: FontWeight::BOLD,
///         ..FontGroup::INITIAL
///     },
///     ..ComputedStyle::initial()
/// };
/// assert_eq!(heading.text, ComputedStyle::initial().text);
/// ```
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ComputedStyle<'a> {
    /// The font properties, with the family list and the settings lists.
    pub font: FontGroup<'a>,
    /// The text properties, with the language and the hyphenation string.
    pub text: TextGroup<'a>,
    /// The line properties.
    pub line: LineGroup,
    /// `direction` and `unicode-bidi`.
    pub bidi: BidiGroup,
    /// `text-orientation` and `text-combine-upright`.
    pub orientation: OrientationGroup,
    /// Margin, border and padding. Not inherited.
    pub edges: EdgesGroup,
    /// The ruby properties.
    pub ruby: RubyGroup,
    /// Whether the box paints a background, border or outline. Not inherited.
    ///
    /// Decoration lines do not count. Boxes that only decorate text may be
    /// culled, but [`Line::paints`](crate::Line::paints) still returns their
    /// decorations.
    ///
    /// Affects fragment retention only, not geometry. Culled-box fragments
    /// are derived from descendants on demand.
    pub paints: bool,
    /// Whether the box specifies an underline, overline or line-through.
    ///
    /// Not inherited. Does not retain a box fragment. [`Line::paints`](crate::Line::paints)
    /// queries decoration styles for culled boxes only if this flag is set;
    /// retained boxes are queried regardless.
    pub decorates: bool,
}

impl ComputedStyle<'static> {
    /// Returns the initial style.
    ///
    /// Uses CSS initial values, except for Chrome defaults documented for
    /// `text-autospace` and `ruby-position`.
    pub const fn initial() -> Self {
        Self {
            font: FontGroup::INITIAL,
            text: TextGroup::INITIAL,
            line: LineGroup::INITIAL,
            bidi: BidiGroup::INITIAL,
            orientation: OrientationGroup::INITIAL,
            edges: EdgesGroup::INITIAL,
            ruby: RubyGroup::INITIAL,
            paints: false,
            decorates: false,
        }
    }
}

impl Default for ComputedStyle<'static> {
    fn default() -> Self {
        Self::initial()
    }
}

impl<'a> ComputedStyle<'a> {
    /// Returns `first_line` with only the properties `::first-line` may change.
    ///
    /// Those come from `first_line`, and the rest from `self`, the element's
    /// own style. `::first-line` may change the font, the spacing, the
    /// transform, `line-height`, `vertical-align` and emphasis. Anything else
    /// is pinned to the element's. Honoring it would make analysis depend on
    /// where the first line ends, which only line breaking knows. A cascade
    /// that already applies this list passes the same values, and this
    /// changes nothing.
    pub(crate) fn pinned_first_line<'b>(&self, first_line: &ComputedStyle<'b>) -> ComputedStyle<'b>
    where
        'a: 'b,
    {
        ComputedStyle {
            font: first_line.font,
            text: TextGroup {
                transform: first_line.text.transform,
                letter_spacing: first_line.text.letter_spacing,
                word_spacing: first_line.text.word_spacing,
                emphasis: first_line.text.emphasis,
                ..self.text
            },
            line: LineGroup {
                height: first_line.line.height,
                vertical_align: first_line.line.vertical_align,
                ..self.line
            },
            ..*self
        }
    }
}

/// Computed styles and container properties for one block.
///
/// [`Layout::builder`](crate::Layout::builder) uses `style` for text inheritance
/// and inline properties. The host owns the block box, so margin, border,
/// padding, vertical alignment, inline text-box trimming and paint flags in
/// `style` are ignored for that box. Block `text-box-trim` is specified
/// separately here, along with other container properties.
///
/// ```
/// use winkin::style::{TextAlign, WritingMode};
/// use winkin::{ComputedBlockStyle, ComputedStyle};
///
/// let style = ComputedStyle::initial();
/// let block = ComputedBlockStyle {
///     writing_mode: WritingMode::VerticalRl,
///     text_align: TextAlign::Justify,
///     ..ComputedBlockStyle::new(&style)
/// };
/// assert_eq!(block.first_line, None);
/// ```
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ComputedBlockStyle<'a> {
    /// The block's own computed style.
    ///
    /// Its inline properties, and what its text inherits.
    pub style: &'a ComputedStyle<'a>,
    /// The block `::first-line` style, if present.
    ///
    /// Supply first-line styles for descendant nodes as needed. Nodes without
    /// one use their ordinary styles.
    pub first_line: Option<&'a ComputedStyle<'a>>,
    /// The base direction of the block's paragraphs.
    ///
    /// `Auto` uses the first strong character of each paragraph.
    pub direction: BaseDirection,
    /// `writing-mode`.
    pub writing_mode: WritingMode,
    /// `text-align`.
    pub text_align: TextAlign,
    /// `text-align-last`.
    pub text_align_last: TextAlignLast,
    /// `text-indent`.
    pub text_indent: TextIndent,
    /// `text-wrap-style`.
    pub text_wrap_style: TextWrapStyle,
    /// `text-overflow`.
    pub text_overflow: TextOverflow,
    /// `line-clamp`.
    pub line_clamp: LineClamp,
    /// `text-group-align`.
    pub text_group_align: TextGroupAlign,
    /// `text-box-trim` on the block.
    pub text_box_trim: TextBoxTrim,
    /// `text-box-edge` on the block.
    pub text_box_edge: TextBoxEdge,
}

impl<'a> ComputedBlockStyle<'a> {
    /// Creates a block style with `style` and initial container properties.
    ///
    /// Uses LTR writing and no `::first-line` style.
    pub const fn new(style: &'a ComputedStyle<'a>) -> Self {
        let initial = BlockFacts::INITIAL;
        Self {
            style,
            first_line: None,
            direction: initial.direction,
            writing_mode: initial.writing_mode,
            text_align: initial.text_align,
            text_align_last: initial.text_align_last,
            text_indent: initial.text_indent,
            text_wrap_style: initial.text_wrap_style,
            text_overflow: initial.text_overflow,
            line_clamp: initial.line_clamp,
            text_group_align: initial.text_group_align,
            text_box_trim: initial.text_box_trim,
            text_box_edge: initial.text_box_edge,
        }
    }
}

/// The initial style, which the crate's tests make a default block in.
#[cfg(test)]
static INITIAL_STYLE: ComputedStyle<'static> = ComputedStyle::initial();

#[cfg(test)]
impl Default for ComputedBlockStyle<'static> {
    /// Returns the initial block in the initial style.
    ///
    /// Tests set block-container properties on it, then swap in their own
    /// style with `ComputedBlockStyle { style, ..block }`.
    fn default() -> Self {
        Self::new(&INITIAL_STYLE)
    }
}
