//! Computed CSS styles for inline content and block containers.
//!
//! [`ComputedStyle`] groups layout properties by subject: [`FontGroup`],
//! [`TextGroup`], [`LineGroup`], [`BidiGroup`], [`OrientationGroup`],
//! [`EdgesGroup`] and [`RubyGroup`]. [`ComputedBlockStyle`] adds first-line
//! styles and block-container properties.
//!
//! The host runs the cascade and keeps paint-only properties, such as
//! colors, decoration lines, shadows and emphasis shapes, by node key. The
//! builder copies and interns the values it needs and keeps no borrowed
//! styles. Reusing styles and lists does not allocate.
//!
//! # Values
//!
//! Supply computed values. Resolve inheritance, `match-parent`,
//! containing-block percentages and font-relative lengths before building.
//! Keep the values that CSS computes as given or that layout resolves:
//! unitless line heights, percentage letter and word spacing, percentage
//! vertical alignment and `font-size-adjust` metrics.
//!
//! Initial values are those CSS specifies, except for `text-autospace` and
//! `ruby-position`, whose initial values are those Chrome computes.

// - `font`, `text`, `emphasis` (part of the text group), `line`, `bidi`,
//   `orientation`, `edges` and `ruby`: one style group's values each, with
//   their initial values. `block` holds the block container's values.
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
    /// The font properties: those that select a font and those that shape
    /// with it.
    ///
    /// Borrows the family list and the settings lists for the builder call.
    pub struct FontGroup<'a> {
        /// `font-family`, in preference order.
        ///
        /// The platform's standard font for the language follows the list,
        /// and an empty list uses only that font. On Windows, for example,
        /// Simplified Chinese text defaults to Microsoft YaHei, while `serif`
        /// gives SimSun.
        pub families: &'a [FontFamilyName<'a>],
        /// The computed `font-size`, in CSS pixels, before `font-size-adjust`.
        ///
        /// Layout applies `font-size-adjust` after font selection. A size that
        /// is negative or not finite is zero, and sizes above 10,000 px are
        /// 10,000 px.
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
    /// Every font property at its initial value, with a size of `medium`,
    /// 16 px.
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
/// A size that is negative or not finite becomes zero, and one above
/// [`MAX_FONT_SIZE`] becomes that. Every size a font is used at, computed or
/// adjusted, passes through here.
pub(crate) fn sanitized_font_size(px: f32) -> f32 {
    if px.is_finite() && px > 0.0 {
        px.min(MAX_FONT_SIZE)
    } else {
        0.0
    }
}

style_struct! {
    /// The text properties: white space, wrapping, breaking, transforms,
    /// spacing, justification and emphasis.
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
        /// `letter-spacing`, as a length plus a percentage.
        ///
        /// A percentage is of the computed font size and inherits as a
        /// percentage. Layout resolves it, as CSS Text 4 specifies. Under
        /// `text-fit`, the percentage scales with the text and the length
        /// does not.
        pub letter_spacing: LengthPercentage,
        /// `word-spacing`, as a length plus a percentage.
        ///
        /// A percentage is of the computed font size, not the space width,
        /// and inherits as a percentage. Layout resolves it, as CSS Text 4
        /// specifies. Under `text-fit`, the percentage scales with the text
        /// and the length does not.
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
        letter_spacing: LengthPercentage::ZERO,
        word_spacing: LengthPercentage::ZERO,
        autospace: TextAutospace::NO_AUTOSPACE,
        spacing_trim: TextSpacingTrim::Normal,
        hanging_punctuation: HangingPunctuation::NONE,
        justify: TextJustify::Auto,
        emphasis: TextEmphasis::NONE,
    };
}

style_struct! {
    /// The line properties: how tall text makes a line, and where it sits.
    pub struct LineGroup {
        /// `line-height`.
        pub height: LineHeight,
        /// `vertical-align`. Not inherited.
        pub vertical_align: VerticalAlign,
        /// `dominant-baseline`.
        pub dominant_baseline: DominantBaseline,
        /// `initial-letter` and `initial-letter-align`.
        pub initial_letter: InitialLetter,
        /// `line-padding`, in CSS pixels. Chrome does not support it.
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
    /// The bidi properties.
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
    /// The orientation properties, read in vertical lines.
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
    /// inline size, set by [`BuildOptions::percentage_basis`] and
    /// [`Layout::measure`]. Layout maps the sides to the writing mode and
    /// truncates each length toward zero onto a 1/64 px grid, as Chrome
    /// does: 10.012 px becomes 10 px and 10.99 px becomes 10.984375 px.
    ///
    /// [`BuildOptions::percentage_basis`]: crate::BuildOptions::percentage_basis
    /// [`Layout::measure`]: crate::Layout::measure
    pub struct EdgesGroup {
        /// `margin`, in CSS pixels plus a fraction of the basis.
        pub margin: Sides<LengthPercentage>,
        /// The used `border-width` in CSS pixels, or zero for `border-style: none`.
        ///
        /// Snap widths before building, as Chrome does: a positive width
        /// below 1 px becomes 1 px, and a larger one rounds down to whole
        /// pixels. So 0.5 px becomes 1 px and 2.5 px becomes 2 px.
        pub border: Sides<f32>,
        /// `padding`, in CSS pixels plus a fraction of the basis.
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

    /// Returns whether a margin or padding has a percentage.
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
    /// Returns the room a box takes along the line at each end, as
    /// `(line-left, line-right)`.
    ///
    /// Each end sums the margin, border and padding on the physical side
    /// `writing_mode` puts there, each truncated onto the layout grid as
    /// Chrome makes a `LayoutUnit` from a float.
    ///
    /// Measurement, the breaker and line layout all read this, so fitting
    /// and placing agree. The caller picks an edge's end by its bidi level.
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
    /// The ruby properties.
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
/// and annotations. Properties are grouped by subject. Lists and strings are
/// borrowed for the builder call, then copied or interned; the layout keeps
/// no borrowed styles.
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
    /// Whether the box paints a background, a border or a box shadow. Not
    /// inherited.
    ///
    /// Outlines and decoration lines do not count; see
    /// [`outlines`](Self::outlines) and [`decorates`](Self::decorates). A box
    /// that paints nothing and draws no outline may be culled: it keeps no
    /// fragment, and its fragments are derived from its descendants when
    /// asked for. [`Line::paints`](crate::Line::paints) still returns its
    /// decorations.
    ///
    /// It also decides how a line ends in the box's own text where the line
    /// ends at white space shaped with the text around it. With `paints`,
    /// the text before a dropped collapsible space, or before spaces that
    /// hang, is shaped again without them and ends at its own advance.
    /// Without it, the text keeps any kerning against those spaces.
    /// Alignments other than `start` and `left`, and
    /// [`decorates`](Self::decorates) on the box or a box around it, have
    /// the same effect.
    pub paints: bool,
    /// Whether the box draws an outline. Not inherited.
    ///
    /// An outlined box keeps its fragment, as a box that
    /// [`paints`](Self::paints) does. Unlike `paints`, it leaves a line's
    /// end as the paragraph shaped it.
    pub outlines: bool,
    /// Whether the box sets an underline, overline or line-through. Not
    /// inherited.
    ///
    /// Does not keep a box fragment. [`Line::paints`](crate::Line::paints)
    /// asks for decoration styles only for boxes that set this flag. A line
    /// ending in text inside the box ends as [`paints`](Self::paints)
    /// describes.
    pub decorates: bool,
}

impl ComputedStyle<'static> {
    /// Returns the initial style.
    ///
    /// Uses the CSS initial values, except for `text-autospace` and
    /// `ruby-position`, which take the values Chrome computes.
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
            outlines: false,
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
    /// The font, letter and word spacing, `text-transform`, `line-height`,
    /// `vertical-align`, emphasis and `ruby-position` come from
    /// `first_line`; the rest come from `self`. Taking any other property
    /// would make analysis depend on where the first line ends, which only
    /// line breaking knows.
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
            ruby: RubyGroup {
                position: first_line.ruby.position,
                ..self.ruby
            },
            ..*self
        }
    }
}

/// Computed styles and container properties for one block.
///
/// [`Layout::builder`](crate::Layout::builder) uses `style` for the
/// block's text and inline properties. The host lays out the block box, so
/// the margin, border, padding, vertical alignment, inline text-box trimming
/// and paint flags in `style` are ignored for it. The block's
/// `text-box-trim` and its other container properties are fields here.
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
    /// The block's computed style: its inline properties, and what its text
    /// inherits.
    pub style: &'a ComputedStyle<'a>,
    /// The block's `::first-line` style, or `None`.
    ///
    /// Descendant nodes take first-line styles of their own through the
    /// builder. A node without one uses its ordinary style.
    pub first_line: Option<&'a ComputedStyle<'a>>,
    /// The base direction of the block's paragraphs.
    ///
    /// `Auto` takes each paragraph's direction from its first strong
    /// character.
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
    /// `text-fit`.
    pub text_fit: TextFit,
}

impl<'a> ComputedBlockStyle<'a> {
    /// Creates a block style with `style` and initial container properties.
    ///
    /// Sets left-to-right horizontal text and no `::first-line` style.
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
            text_fit: TextFit::NONE,
        }
    }
}

/// The initial style, for the tests' default block.
#[cfg(test)]
static INITIAL_STYLE: ComputedStyle<'static> = ComputedStyle::initial();

#[cfg(test)]
impl Default for ComputedBlockStyle<'static> {
    /// Returns the initial block in the initial style.
    ///
    /// Tests set container properties on it and swap in their own style
    /// with `ComputedBlockStyle { style, ..block }`.
    fn default() -> Self {
        Self::new(&INITIAL_STYLE)
    }
}
