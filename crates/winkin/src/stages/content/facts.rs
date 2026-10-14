//! The facts the writer lowers each style into. The content keeps no style;
//! later stages read these facts instead.
//!
//! **Lowered once, by the writer.** The writer lowers each style as it
//! takes it: it puts lengths on the grid as their readers do, resolves
//! sides for the block's writing mode, and answers the style's tests
//! (`edge_breaks_shaping`, `white_space_hangs`, what bidi resolves upright).
//! Facts that need a font or the content belong to the first stage that
//! has them: the used size, the primary font, `line-height: normal`, tab
//! stops in spaces.
//!
//! **Four kinds, each interned by value and named by id:**
//! - A [`FontRequest`] is what font selection selects fonts for.
//! - A [`ShapingFacts`] names a request and adds the other shaping
//!   properties. Equal [`ShapingFactsId`]s shape alike, as an equal `Font`
//!   does in Blink.
//! - A [`TextFacts`] names a shaping row and adds every other inherited
//!   fact.
//! - A [`BoxFacts`] holds a box's non-inherited facts, following CSS's split
//!   between inherited properties and the rest.
//!
//! **A repeated style is not lowered again.** The builder's memo (`memo`)
//! holds the facts each style lowered into; box facts are held for the last
//! node kind only. In debug builds and tests the oracle (`facts_check`)
//! compares every row the writer gives a node with its own projection of
//! the style.
//!
//! The ids are `u16`. Past that many distinct rows a node takes a stand-in,
//! counted in [`BuildReport::replaced_styles`](crate::BuildReport).

use alloc::boxed::Box;
use core::hash::{Hash, Hasher};
use core::mem;

use super::lists::{HyphenStringId, LanguageId, Languages, Lookup};
use super::memo::{FontKey, StyleKey};
use super::{ContentFlags, NodeId, NodeKind};
use crate::data::{Id, Staging, Table, define_flags, define_id, heap_bytes};
use crate::style::{
    BaseDirection, BoxDecorationBreak, ComputedBlockStyle, ComputedStyle, Direction,
    DominantBaseline, EdgesGroup, EmphasisSide, EmphasisSkip, FirstLineVariant,
    FontVariantPosition, HangingPunctuation, Hyphens, InitialLetter, LengthPercentage, LineBreak,
    LineClamp, LineHeight, OrientationGroup, OverflowWrap, RubyGroup, TabSize, TextAlign,
    TextAlignLast, TextBoxEdge, TextBoxTrim, TextCombineUpright, TextEdgeOver, TextEdgeUnder,
    TextFit, TextFitKind, TextFitTarget, TextGroupAlign, TextIndent, TextJustify, TextOrientation,
    TextOverflow, TextSpacingTrim, TextWrapMode, TextWrapStyle, UnicodeBidi, VerticalAlign,
    WhiteSpaceCollapse, WordBreak, WritingMode,
};
use crate::style::{Same, same_by_value, style_struct};
use crate::unit::{LayoutUnit, TextUnit};

define_id! {
    /// Indexes a layout's table of distinct [`TextFacts`].
    ///
    /// A node names one per variant (`Node.text`, `first_line_text`). Where
    /// the table is full, a node takes its parent's.
    pub(crate) struct TextFactsId(u16);
}

define_id! {
    /// Indexes a layout's table of distinct [`ShapingFacts`].
    ///
    /// Equal ids shape alike. Where the table is full, a node takes its
    /// parent's text facts.
    pub(crate) struct ShapingFactsId(u16);
}

define_id! {
    /// Indexes a layout's table of distinct [`FontRequest`]s.
    ///
    /// A request is font selection's unit of work. Where the table is full,
    /// a node takes its parent's text facts.
    pub(crate) struct FontRequestId(u16);
}

define_id! {
    /// Indexes a layout's table of distinct [`BoxFacts`].
    ///
    /// A node names one per variant (`Node.box_`, `first_line_box`). Where
    /// the table is full, a node takes [`BoxFactsId::INITIAL`].
    pub(crate) struct BoxFactsId(u16);
}

define_id! {
    /// Indexes the table of box edges (margin, border and padding as the
    /// style gives them) that have a percentage.
    ///
    /// Row 0 stands for none: a box whose edges have no percentage keeps the
    /// lengths it was lowered with, whatever the basis.
    pub(crate) struct EdgesId(u16);
}

impl EdgesId {
    /// No percentage edges.
    pub(super) const NONE: Self = Self(0);
}

impl BoxFactsId {
    /// The id of [`BoxFacts::INITIAL`], the table's first row.
    ///
    /// A node that is no box has it, and a box takes it when the table is
    /// full. The row is stored only once another row follows it.
    pub(super) const INITIAL: Self = Self(0);
}

same_by_value!(
    TextFactsId,
    ShapingFactsId,
    FontRequestId,
    EdgesId,
    TextFlags,
    BoxFlags,
    FixedSpacing,
    TextSetting,
    EmphasisSide,
    EmphasisSkip,
    LayoutUnit,
    TextUnit,
);

impl<A: Same, B: Same> Same for (A, B) {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self.0.same(&other.0) && self.1.same(&other.1)
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        self.0.feed(state);
        self.1.feed(state);
    }
}

/// How a style's text is oriented in the block's lines.
///
/// `writing-mode` and `text-orientation` decide it together. Analysis
/// orients each cluster by it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TextSetting {
    /// In a horizontal line.
    Horizontal,
    /// Each cluster by its first character's Vertical_Orientation:
    /// `text-orientation: mixed` in `vertical-rl` and `vertical-lr`.
    Mixed,
    /// `upright`: upright, except characters of a vertical-only script,
    /// which go on their side.
    Upright,
    /// On its side: `sideways`, and all text in `sideways-rl` and
    /// `sideways-lr`, whose lines are horizontal lines rotated.
    Sideways,
}

impl TextSetting {
    /// Returns the setting of text with `orientation` in a block of
    /// `writing_mode`.
    ///
    /// `text-orientation` applies only in the vertical typographic modes
    /// (CSS Writing Modes 4, section 5.1). `sideways-rl` and `sideways-lr`
    /// set every character on its side regardless.
    fn new(orientation: TextOrientation, writing_mode: WritingMode) -> Self {
        match writing_mode {
            WritingMode::HorizontalTb => Self::Horizontal,
            WritingMode::SidewaysRl | WritingMode::SidewaysLr => Self::Sideways,
            WritingMode::VerticalRl | WritingMode::VerticalLr => match orientation {
                TextOrientation::Mixed => Self::Mixed,
                TextOrientation::Upright => Self::Upright,
                TextOrientation::Sideways => Self::Sideways,
            },
        }
    }
}

/// `line-height` as the writer lowers it.
///
/// `normal` waits for measuring, which has the primary font. A factor is
/// resolved against the computed size on the grid. A length stays as given:
/// measuring rounds it onto the grid, and font selection uses it unrounded
/// to size an initial letter.
#[derive(Copy, Clone, Debug)]
pub(crate) enum TextLineHeight {
    /// The primary font's line spacing.
    Normal,
    /// A factor times the computed size, each truncated onto the grid, as
    /// Blink keeps a number as a percentage of the size.
    Factor(LayoutUnit),
    /// A length in pixels, as given. Measuring rounds it onto the grid,
    /// saturating. Font selection treats a non-finite length as zero.
    Length(f32),
}

impl TextLineHeight {
    /// Lowers `height` for text whose computed size is `size`.
    fn new(height: LineHeight, size: f32) -> Self {
        match height {
            LineHeight::Normal => Self::Normal,
            LineHeight::Factor(factor) => {
                let size = LayoutUnit::from_px_truncated(size);
                Self::Factor(LayoutUnit::from_px_truncated(size.to_px() * factor))
            }
            LineHeight::Px(px) => Self::Length(px),
        }
    }

    /// Returns the line height in pixels, as Chrome's `ComputedLineHeight`
    /// gives it, using `normal` for `line-height: normal`.
    ///
    /// An initial letter is sized to span lines of this height.
    pub(crate) fn px(self, normal: LayoutUnit) -> f32 {
        match self {
            Self::Normal => normal.to_px(),
            Self::Factor(height) => height.to_px(),
            Self::Length(px) if px.is_finite() => px,
            Self::Length(_) => 0.0,
        }
    }
}

impl Same for TextLineHeight {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Normal, Self::Normal) => true,
            (Self::Factor(a), Self::Factor(b)) => a == b,
            (Self::Length(a), Self::Length(b)) => a.same(b),
            _ => false,
        }
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        match self {
            Self::Normal => {}
            Self::Factor(height) => height.hash(state),
            Self::Length(px) => px.feed(state),
        }
    }
}

define_flags! {
    /// Boolean text facts (see [`TextFacts`]).
    pub(crate) struct TextFlags(u16) {
        /// `text-wrap-mode: wrap`.
        pub(crate) const WRAPS = 1 << 0;
        /// Spaces, tabs and other space separators ending a line hang
        /// (`TextGroup::white_space_hangs`).
        pub(crate) const WHITE_SPACE_HANGS = 1 << 1;
        /// White space ending a paragraph hangs only where it overflows
        /// (`TextGroup::white_space_hangs_conditionally`).
        pub(crate) const HANGS_CONDITIONALLY = 1 << 2;
        /// `hyphens: none`.
        pub(crate) const HYPHENS_NONE = 1 << 3;
        /// The text wraps and `overflow-wrap` is `anywhere` or `break-word`,
        /// so an emergency break is allowed.
        pub(crate) const EMERGENCY = 1 << 4;
        /// `overflow-wrap: anywhere`, which lowers min-content to the widest
        /// cluster; `break-word` does not.
        pub(crate) const ANYWHERE = 1 << 5;
        /// The content language is Japanese or Chinese, which the line
        /// segmenter tailors for under `line-break: normal` and `loose`.
        pub(crate) const CJK_LINE_BREAKS = 1 << 6;
        /// Upright in a vertical typographic line, or combined whole: bidi
        /// resolves each cluster as one left-to-right unit.
        pub(crate) const UPRIGHT_BIDI = 1 << 7;
        /// `direction: rtl`.
        pub(crate) const RTL = 1 << 8;
        /// `text-emphasis-style` sets marks.
        pub(crate) const EMPHASIS = 1 << 9;
        /// `text-autospace` adds space between ideographs and letters
        /// (`ideograph-alpha`). Never set for text upright in a vertical
        /// line, which Chrome classes as `kVerticalUpright`.
        pub(crate) const AUTOSPACE_ALPHA = 1 << 10;
        /// `text-autospace` adds space between ideographs and digits
        /// (`ideograph-numeric`). Never set for text upright in a vertical
        /// line.
        pub(crate) const AUTOSPACE_NUMERIC = 1 << 11;
    }
}

style_struct! {
    /// Every inherited fact of a style that a later stage reads. At most 44
    /// bytes.
    ///
    /// Each node names one row per variant; a text node takes its
    /// container's. Styles that differ only in box properties share a row,
    /// as does a first-line style that differs only in its transform, which
    /// the writer applies. The analysis facts are pinned: a node's
    /// first-line row differs from its own only in shaping, emphasis, line
    /// height and spacing.
    pub(crate) struct TextFacts {
        /// Its shaping facts, which name the font request.
        pub(crate) shaping: ShapingFactsId,
        pub(crate) flags: TextFlags,
        // Read by analysis.
        pub(crate) line_break: LineBreak,
        pub(crate) word_break: WordBreak,
        /// `white-space-collapse`.
        pub(crate) collapse: WhiteSpaceCollapse,
        /// How it is oriented in the block's lines.
        pub(crate) setting: TextSetting,
        /// What `text-combine-upright` combines in a vertical typographic
        /// line; `none` elsewhere (CSS Writing Modes 4, section 9.1).
        /// Digits are clamped to 2 to 4, as CSS takes the integer.
        pub(crate) combine: TextCombineUpright,
        // Read by font selection.
        /// `hyphenate-character`, or `None` for `auto`.
        pub(crate) hyphen: Option<HyphenStringId>,
        // Read by measuring, the breaker, line layout and the readers.
        /// `line-padding` on the grid, truncated as a box's padding is.
        ///
        /// Measuring charges it to a line at an end where this text holds
        /// the line's outermost content, and line layout places it inside
        /// the box there. It is zero unless the length is finite and
        /// positive. Chrome does not parse `line-padding`.
        pub(crate) padding: LayoutUnit,
        pub(crate) hanging: HangingPunctuation,
        /// The side of the line its emphasis marks go on in the block's
        /// writing mode (`EmphasisPosition::side`).
        pub(crate) emphasis_side: EmphasisSide,
        pub(crate) emphasis_skip: EmphasisSkip,
        pub(crate) justify: TextJustify,
        /// `dominant-baseline`, which is inherited.
        pub(crate) dominant: DominantBaseline,
        /// `line-height`, for measuring and the initial letter.
        pub(crate) line_height: TextLineHeight,
        /// `tab-size`, for measuring, which counts spaces in the primary
        /// font.
        pub(crate) tab_size: TabSize,
    }
}

impl TextFacts {
    /// Lowers the text facts of `style` in a block of `writing_mode`.
    ///
    /// `shaping` names its shaping facts; `cjk` is set where its language
    /// is Japanese or Chinese.
    fn new(
        style: &StyleKey,
        shaping: ShapingFactsId,
        writing_mode: WritingMode,
        cjk: bool,
    ) -> Self {
        let text = &style.text;
        let vertical = writing_mode.is_vertical_typographic();
        let orientation = style.orientation;
        let wraps = text.wrap_mode == TextWrapMode::Wrap;
        let mut flags = TextFlags::NONE;
        flags.set(TextFlags::WRAPS, wraps);
        flags.set(TextFlags::WHITE_SPACE_HANGS, text.white_space_hangs());
        flags.set(
            TextFlags::HANGS_CONDITIONALLY,
            text.white_space_hangs_conditionally(),
        );
        flags.set(TextFlags::HYPHENS_NONE, text.hyphens == Hyphens::None);
        flags.set(
            TextFlags::EMERGENCY,
            wraps && text.overflow_wrap != OverflowWrap::Normal,
        );
        flags.set(
            TextFlags::ANYWHERE,
            text.overflow_wrap == OverflowWrap::Anywhere,
        );
        flags.set(TextFlags::CJK_LINE_BREAKS, cjk);
        flags.set(
            TextFlags::UPRIGHT_BIDI,
            vertical
                && (orientation.text_orientation == TextOrientation::Upright
                    || orientation.text_combine_upright == TextCombineUpright::All),
        );
        flags.set(TextFlags::RTL, style.bidi.direction == Direction::Rtl);
        flags.set(TextFlags::EMPHASIS, text.emphasis.marks);
        // `text-orientation` has no effect in the sideways modes, so upright
        // text ends an autospace boundary only in a vertical typographic
        // mode. Chrome's `TextAutoSpace` reads the font's orientation, which
        // is horizontal there.
        let upright_in_vertical =
            vertical && orientation.text_orientation == TextOrientation::Upright;
        flags.set(
            TextFlags::AUTOSPACE_ALPHA,
            text.autospace.ideograph_alpha && !upright_in_vertical,
        );
        flags.set(
            TextFlags::AUTOSPACE_NUMERIC,
            text.autospace.ideograph_numeric && !upright_in_vertical,
        );
        // Truncates `line-padding` onto the grid, as a box's padding is. A
        // length that is not finite and positive gives zero; no computed
        // value is one.
        let padding = style.line.padding;
        let padding = if padding.is_finite() && padding > 0.0 {
            LayoutUnit::from_px_truncated(padding)
        } else {
            LayoutUnit::ZERO
        };
        let combine = match (vertical, orientation.text_combine_upright) {
            (false, _) => TextCombineUpright::None,
            (true, TextCombineUpright::Digits(most)) => {
                TextCombineUpright::Digits(most.clamp(2, 4))
            }
            (true, combine) => combine,
        };
        Self {
            shaping,
            flags,
            line_break: text.line_break,
            word_break: text.word_break,
            collapse: text.white_space_collapse,
            setting: TextSetting::new(orientation.text_orientation, writing_mode),
            combine,
            hyphen: style.text.hyphenate_character,
            padding,
            hanging: text.hanging_punctuation,
            emphasis_side: text.emphasis.position.side(writing_mode),
            emphasis_skip: text.emphasis.skip,
            justify: text.justify,
            dominant: style.line.dominant_baseline,
            line_height: TextLineHeight::new(style.line.height, style.font.computed_size()),
            tab_size: text.tab_size,
        }
    }

    /// Whether every flag in `flag` is set.
    #[inline]
    pub(crate) fn has(&self, flag: TextFlags) -> bool {
        self.flags.contains(flag)
    }
}

impl HoldsFlags for TextFacts {
    fn content_flags(&self) -> ContentFlags {
        let mut flags = ContentFlags::NONE;
        if self.has(TextFlags::EMPHASIS) {
            flags.insert(ContentFlags::EMPHASIS);
        }
        if self.padding != LayoutUnit::ZERO {
            flags.insert(ContentFlags::LINE_PADDING);
        }
        if self.hanging != HangingPunctuation::NONE {
            flags.insert(ContentFlags::HANGING_PUNCTUATION);
        }
        if self.dominant != DominantBaseline::Auto {
            flags.insert(ContentFlags::DOMINANT_BASELINE);
        }
        if !self.has(TextFlags::WRAPS) {
            flags.insert(ContentFlags::NOWRAP);
        }
        if self.has(TextFlags::AUTOSPACE_ALPHA) || self.has(TextFlags::AUTOSPACE_NUMERIC) {
            flags.insert(ContentFlags::AUTOSPACE);
        }
        if self.justify != TextJustify::Auto {
            flags.insert(ContentFlags::TEXT_JUSTIFY);
        }
        flags
    }
}

define_flags! {
    /// Which spacings of a [`ShapingFacts`] row are fixed: a nonzero length
    /// with no percentage, as in Chrome's `HasFixedSpacing`.
    pub(crate) struct FixedSpacing(u8) {
        /// `letter-spacing`.
        pub(crate) const LETTER = 1 << 0;
        /// `word-spacing`.
        pub(crate) const WORD = 1 << 1;
    }
}

style_struct! {
    /// What shapes alike: the font request and the other shaping
    /// properties. At most 16 bytes.
    ///
    /// The font request holds the font group, the family list, the feature
    /// and variation settings and the language. This row adds
    /// `letter-spacing`, `word-spacing`, `text-spacing-trim`,
    /// `text-orientation` and `text-combine-upright`.
    ///
    /// Equal ids shape alike, as Blink ends a shaping run where the `Font`
    /// differs. The shaping walk, the font walk's position runs and split
    /// graphemes, and `ShapedRun.shaping` compare them.
    pub(crate) struct ShapingFacts {
        pub(crate) font: FontRequestId,
        /// `letter-spacing` in `TextUnit`, rounded to the nearest 1/65536 px
        /// as Chrome's `TextRunLayoutUnit::FromFloatRound` rounds it.
        ///
        /// Measuring adds it after each cluster. Through the request's
        /// `spaced`, it also turns optional ligatures off, so both use the
        /// same rounding. It saturates, and NaN gives zero.
        pub(crate) letter: TextUnit,
        /// `word-spacing`, its percentage resolved against the computed size
        /// and the sum rounded as `letter-spacing` is.
        ///
        /// Measuring adds it after each word separator, and the breaker adds
        /// it after one at a line edge it reshapes. It saturates, and NaN
        /// gives zero.
        pub(crate) word: TextUnit,
        pub(crate) trim: TextSpacingTrim,
        /// `text-orientation` and `text-combine-upright`.
        pub(super) orientation: OrientationGroup,
        /// Which of `letter` and `word` keep their size while `text-fit`
        /// scales the text.
        pub(crate) fixed: FixedSpacing,
    }
}

impl ShapingFacts {
    /// Lowers the shaping facts of `style`, given its font request `font`
    /// and its rounded letter spacing `letter`.
    fn new(style: &StyleKey, font: FontRequestId, letter: TextUnit) -> Self {
        let text = &style.text;
        let word = TextUnit::from_px(text.word_spacing.resolve(style.font.computed_size()));
        let none = TextUnit::from_raw(0);
        let mut fixed = FixedSpacing::NONE;
        fixed.set(
            FixedSpacing::LETTER,
            text.letter_spacing.fraction == 0.0 && letter != none,
        );
        fixed.set(
            FixedSpacing::WORD,
            text.word_spacing.fraction == 0.0 && word != none,
        );
        Self {
            font,
            letter,
            word,
            trim: text.spacing_trim,
            orientation: style.orientation,
            fixed,
        }
    }
}

impl HoldsFlags for ShapingFacts {
    fn content_flags(&self) -> ContentFlags {
        let mut flags = ContentFlags::NONE;
        let none = TextUnit::from_raw(0);
        if self.letter != none || self.word != none {
            flags.insert(ContentFlags::NONZERO_SPACING);
        }
        if self.trim.trims_wrapped_start() {
            flags.insert(ContentFlags::TRIMS_WRAPPED_START);
        }
        if self.trim.trims_punctuation() {
            flags.insert(ContentFlags::TRIMS_PUNCTUATION);
        }
        flags
    }
}

style_struct! {
    /// What font selection selects fonts for: its unit of work. At most 72
    /// bytes.
    ///
    /// Font selection resolves each request once into a `FontResolution`.
    /// The row holds the font properties, the lists, the language, and the
    /// three text facts that choose features.
    pub(crate) struct FontRequest {
        /// The font properties, including the computed size and the lists.
        pub(crate) font: FontKey,
        /// The content language; `und` where the style sets none.
        pub(crate) language: LanguageId,
        /// The used `initial-letter`, set only on the block's initial
        /// letter.
        pub(crate) initial_letter: InitialLetter,
        /// The text is upright or mixed in a vertical typographic line. It
        /// shapes with the vertical forms of the features chosen for its
        /// spacing, as Blink's `IsVerticalAnyUpright` chooses them.
        pub(crate) upright: bool,
        /// Its rounded letter spacing is nonzero, which turns optional
        /// ligatures off.
        pub(crate) spaced: bool,
        /// Its `text-spacing-trim` trims punctuation, which turns `chws` on.
        pub(crate) trims_punctuation: bool,
    }
}

impl FontRequest {
    /// Lowers the font request of `style` in a block of `writing_mode`,
    /// given its rounded letter spacing `letter`.
    fn new(style: &StyleKey, writing_mode: WritingMode, letter: TextUnit) -> Self {
        Self {
            font: style.font,
            language: style.text.language,
            initial_letter: style.line.initial_letter,
            upright: writing_mode.is_vertical_typographic()
                && style.orientation.text_orientation != TextOrientation::Sideways,
            spaced: letter != TextUnit::from_raw(0),
            trims_punctuation: style.text.spacing_trim.trims_punctuation(),
        }
    }
}

impl HoldsFlags for FontRequest {
    fn content_flags(&self) -> ContentFlags {
        let mut flags = ContentFlags::NONE;
        if self.initial_letter.is_set() {
            flags.insert(ContentFlags::INITIAL_LETTER);
        }
        if self.font.variant_position != FontVariantPosition::Normal {
            flags.insert(ContentFlags::VARIANT_POSITION);
        }
        flags
    }
}

define_flags! {
    /// Boolean box facts (see [`BoxFacts`]).
    pub(crate) struct BoxFlags(u16) {
        /// It paints a background, a border or a box shadow (`paints`).
        pub(crate) const PAINTS = 1 << 0;
        /// Its text is decorated (`decorates`).
        pub(crate) const DECORATES = 1 << 1;
        /// It has margin, border or padding on some side
        /// (`EdgesGroup::any`).
        pub(crate) const HAS_EDGES = 1 << 2;
        /// `box-decoration-break: clone`.
        pub(crate) const CLONES = 1 << 3;
        /// Its opening edge stops shaping (see [`BoxFacts::new`]).
        const BREAKS_SHAPING_AT_START = 1 << 4;
        /// Its closing edge stops shaping.
        const BREAKS_SHAPING_AT_END = 1 << 5;
        /// `direction: rtl`: its start is on its right.
        pub(crate) const RTL = 1 << 6;
        /// `text-box-trim` trims its over edge to `text-box-edge`'s.
        pub(crate) const TRIMS_OVER = 1 << 7;
        /// It is the block's initial letter, whose ink takes room at its
        /// edges whatever its own edges are.
        pub(crate) const INITIAL_LETTER = 1 << 8;
        /// `text-box-trim` trims its under edge to `text-box-edge`'s.
        pub(crate) const TRIMS_UNDER = 1 << 9;
        /// `text-box-edge`'s over edge is `cap`.
        const EDGE_CAP = 1 << 10;
        /// `text-box-edge`'s over edge is `ex`.
        const EDGE_EX = 1 << 11;
        /// `text-box-edge`'s over edge is `ideographic` or `ideographic-ink`.
        const EDGE_IDEOGRAPHIC_OVER = 1 << 12;
        /// `text-box-edge`'s under edge is `alphabetic`.
        const EDGE_ALPHABETIC = 1 << 13;
        /// `text-box-edge`'s under edge is `ideographic` or `ideographic-ink`.
        const EDGE_IDEOGRAPHIC_UNDER = 1 << 14;
        /// It draws an outline (`outlines`).
        pub(crate) const OUTLINES = 1 << 15;
    }
}

impl BoxFlags {
    /// Returns the flags encoding `text-box-trim` and `text-box-edge`.
    ///
    /// The edges are kept only where something is trimmed. `ideographic-ink`
    /// is kept as `ideographic`, which it measures as.
    fn from_text_box(trim: TextBoxTrim, edge: TextBoxEdge) -> Self {
        let (over, under) = trim.sides();
        let mut flags = Self::NONE;
        if !over && !under {
            return flags;
        }
        flags.set(Self::TRIMS_OVER, over);
        flags.set(Self::TRIMS_UNDER, under);
        flags.set(Self::EDGE_CAP, edge.over == TextEdgeOver::Cap);
        flags.set(Self::EDGE_EX, edge.over == TextEdgeOver::Ex);
        flags.set(
            Self::EDGE_IDEOGRAPHIC_OVER,
            matches!(
                edge.over,
                TextEdgeOver::Ideographic | TextEdgeOver::IdeographicInk
            ),
        );
        flags.set(
            Self::EDGE_ALPHABETIC,
            edge.under == TextEdgeUnder::Alphabetic,
        );
        flags.set(
            Self::EDGE_IDEOGRAPHIC_UNDER,
            matches!(
                edge.under,
                TextEdgeUnder::Ideographic | TextEdgeUnder::IdeographicInk
            ),
        );
        flags
    }
}

style_struct! {
    /// A box's non-inherited facts, lowered for the block's writing mode.
    /// At most 48 bytes.
    ///
    /// Inline boxes, ruby containers and annotations, `::first-letter`
    /// boxes and atomic inlines have one. A node that is no box has
    /// [`BoxFacts::INITIAL`].
    ///
    /// Of what `::first-line` may change, only `vertical-align` is a box
    /// property (`pinned_first_line`), which is why a node has a first-line
    /// row.
    pub(crate) struct BoxFacts {
        pub(super) flags: BoxFlags,
        /// The room it takes along the line at each end, `(line-left,
        /// line-right)`.
        ///
        /// Each is the margin, border and padding on the physical side the
        /// writing mode puts there, truncated onto the grid
        /// (`EdgesGroup::inline`). Its direction, [`BoxFlags::RTL`], says
        /// which one the opening and closing edges take.
        pub(crate) room: (LayoutUnit, LayoutUnit),
        /// Its margins along the line, `(line-left, line-right)`, on the
        /// grid. A culled box and its edges read them.
        pub(crate) margin_line: (LayoutUnit, LayoutUnit),
        /// Its margins across the line, `(over, under)`, on the grid. The
        /// initial letter's room in the lines beside it reaches this far
        /// past its border box.
        pub(crate) margin_across: (LayoutUnit, LayoutUnit),
        /// Its padding and border across the line, `(over, under)`, on the
        /// grid. A kept box's extent adds them to its font's.
        pub(crate) across: (LayoutUnit, LayoutUnit),
        /// Its edges as the style gives them, where a margin or padding has
        /// a percentage; `room`, `margin_line`, `margin_across` and `across`
        /// resolve from them again for each basis.
        pub(super) edges: EdgesId,
        /// `vertical-align` where it applies, and `baseline` elsewhere.
        ///
        /// It applies to inline boxes, `::first-letter` boxes and atomic
        /// inlines. A length stays as given for measuring, which has the
        /// fonts that a percentage and the keywords need.
        pub(crate) align: VerticalAlign,
        pub(crate) bidi: UnicodeBidi,
        /// `ruby-position`, `ruby-align` and `ruby-overhang`.
        pub(crate) ruby: RubyGroup,
    }
}

impl BoxFacts {
    /// No edges, no shift and nothing painted: the facts of a culled box
    /// and of every node that is no box.
    pub(super) const INITIAL: Self = Self {
        flags: BoxFlags::NONE,
        room: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        margin_line: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        margin_across: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        across: (LayoutUnit::ZERO, LayoutUnit::ZERO),
        edges: EdgesId::NONE,
        align: VerticalAlign::Baseline,
        bidi: UnicodeBidi::Normal,
        ruby: RubyGroup::INITIAL,
    };

    /// Lowers the box facts of a node of `kind` in `style`, in a block of
    /// `writing_mode`, resolving percentages against `basis` pixels.
    /// `edges_id` names the style's edges where they have a percentage.
    ///
    /// - A container (an inline box, a ruby container or annotation, a
    ///   `::first-letter` box) has every fact, `vertical-align` only where
    ///   it applies.
    /// - An atomic inline has only its `vertical-align`; its margins are in
    ///   its [`Atomic`](super::Atomic) row.
    /// - Anything else, including the block and floats, has none.
    fn new(
        style: &StyleKey,
        kind: NodeKind,
        writing_mode: WritingMode,
        basis: f32,
        edges_id: EdgesId,
    ) -> Self {
        let align = if kind.reads_vertical_align() {
            style.line.vertical_align
        } else {
            VerticalAlign::Baseline
        };
        if !kind.is_container() {
            return Self {
                align,
                ..Self::INITIAL
            };
        }
        let edges = &style.edges;
        let rtl = style.bidi.direction == Direction::Rtl;
        let mut flags = BoxFlags::NONE;
        flags.set(BoxFlags::PAINTS, style.paints);
        flags.set(BoxFlags::OUTLINES, style.outlines);
        flags.set(BoxFlags::DECORATES, style.decorates);
        flags.set(BoxFlags::HAS_EDGES, edges.any());
        flags.set(
            BoxFlags::CLONES,
            edges.decoration_break == BoxDecorationBreak::Clone,
        );
        flags.set(BoxFlags::RTL, rtl);
        flags.insert(BoxFlags::from_text_box(
            style.line.text_box_trim,
            style.line.text_box_edge,
        ));
        flags.set(BoxFlags::INITIAL_LETTER, style.line.initial_letter.is_set());
        // An edge stops shaping as in Blink's `ShouldBreakShapingBeforeBox`
        // and `AfterBox`: margin, border or padding on that inline side, or
        // `vertical-align` other than `baseline`. Any `unicode-bidi` stops
        // it too, as Blink ends a run at every bidi control. A box that only
        // paints is shaped across. The test reads the unrounded lengths, as
        // Blink tests the computed ones.
        let breaks_everywhere = !matches!(style.line.vertical_align, VerticalAlign::Baseline)
            || style.bidi.unicode_bidi != UnicodeBidi::Normal;
        let any_along = |opens: bool| {
            let side = |left: bool, right: bool| {
                let (start, end) = style.bidi.direction.line_order(left, right);
                if opens { start } else { end }
            };
            let (margin, padding) = (
                edges.margin.along_line(writing_mode),
                edges.padding.along_line(writing_mode),
            );
            let border = edges.border.along_line(writing_mode);
            side(!margin.0.is_zero(), !margin.1.is_zero())
                || side(border.0 != 0.0, border.1 != 0.0)
                || side(!padding.0.is_zero(), !padding.1.is_zero())
        };
        flags.set(
            BoxFlags::BREAKS_SHAPING_AT_START,
            breaks_everywhere || any_along(true),
        );
        flags.set(
            BoxFlags::BREAKS_SHAPING_AT_END,
            breaks_everywhere || any_along(false),
        );
        let mut facts = Self {
            flags,
            edges: edges_id,
            align,
            bidi: style.bidi.unicode_bidi,
            ruby: style.ruby,
            ..Self::INITIAL
        };
        facts.resolve(edges, writing_mode, basis);
        facts
    }

    /// Resolves `edges`, from its style, onto the grid in a block of
    /// `writing_mode`, with percentages against `basis` pixels.
    fn resolve(&mut self, edges: &EdgesGroup, writing_mode: WritingMode, basis: f32) {
        let edges = edges.used(basis);
        let (padding, border) = (
            edges.padding.across_line_on_grid(writing_mode),
            edges.border.across_line_on_grid(writing_mode),
        );
        self.room = edges.inline(writing_mode);
        self.margin_line = edges.margin.along_line_on_grid(writing_mode);
        self.margin_across = edges.margin.across_line_on_grid(writing_mode);
        self.across = (padding.0 + border.0, padding.1 + border.1);
    }

    /// Whether every flag in `flag` is set.
    #[inline]
    pub(crate) fn has(&self, flag: BoxFlags) -> bool {
        self.flags.contains(flag)
    }

    /// Returns whether `text-box-trim` trims its over and under edges, and
    /// the `text-box-edge` they are trimmed to.
    pub(crate) fn text_box(&self) -> (bool, bool, TextBoxEdge) {
        let has = |flag| self.flags.contains(flag);
        let over = if has(BoxFlags::EDGE_CAP) {
            TextEdgeOver::Cap
        } else if has(BoxFlags::EDGE_EX) {
            TextEdgeOver::Ex
        } else if has(BoxFlags::EDGE_IDEOGRAPHIC_OVER) {
            TextEdgeOver::Ideographic
        } else {
            TextEdgeOver::Text
        };
        let under = if has(BoxFlags::EDGE_ALPHABETIC) {
            TextEdgeUnder::Alphabetic
        } else if has(BoxFlags::EDGE_IDEOGRAPHIC_UNDER) {
            TextEdgeUnder::Ideographic
        } else {
            TextEdgeUnder::Text
        };
        (
            has(BoxFlags::TRIMS_OVER),
            has(BoxFlags::TRIMS_UNDER),
            TextBoxEdge { over, under },
        )
    }

    /// Whether its opening edge (if `opens`) or its closing edge stops
    /// shaping.
    #[inline]
    pub(crate) fn breaks_shaping(&self, opens: bool) -> bool {
        self.has(if opens {
            BoxFlags::BREAKS_SHAPING_AT_START
        } else {
            BoxFlags::BREAKS_SHAPING_AT_END
        })
    }

    /// Returns its direction.
    #[inline]
    pub(crate) fn direction(&self) -> Direction {
        if self.has(BoxFlags::RTL) {
            Direction::Rtl
        } else {
            Direction::Ltr
        }
    }
}

impl HoldsFlags for BoxFacts {
    fn content_flags(&self) -> ContentFlags {
        let mut flags = ContentFlags::NONE;
        if self.has(BoxFlags::HAS_EDGES) || self.has(BoxFlags::INITIAL_LETTER) {
            flags.insert(ContentFlags::BOXES_WITH_EDGES);
        }
        if self.has(BoxFlags::CLONES) {
            flags.insert(ContentFlags::CLONE_BOXES);
        }
        if self.has(BoxFlags::DECORATES) {
            flags.insert(ContentFlags::DECORATED_BOXES);
        }
        if !matches!(self.align, VerticalAlign::Baseline) {
            flags.insert(ContentFlags::VERTICAL_ALIGN);
        }
        flags
    }
}

/// The block-container properties and the facts of the block as a whole.
///
/// It holds a [`ComputedBlockStyle`] without its styles, which the builder
/// lowers as the block node's. It adds what the block's style says of the
/// whole block.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct BlockFacts {
    /// The base direction of the block's paragraphs.
    ///
    /// It is the block's direction, or `auto` where its `unicode-bidi` is
    /// `plaintext`. With `auto` each paragraph takes its first strong
    /// character's direction (CSS Writing Modes 3, section 2.4.2), as
    /// Blink's `BidiParagraph` does.
    pub(crate) direction: BaseDirection,
    pub(crate) writing_mode: WritingMode,
    pub(crate) text_align: TextAlign,
    pub(crate) text_align_last: TextAlignLast,
    pub(crate) text_indent: TextIndent,
    pub(crate) text_wrap_style: TextWrapStyle,
    pub(crate) text_overflow: TextOverflow,
    pub(crate) line_clamp: LineClamp,
    pub(crate) text_group_align: TextGroupAlign,
    pub(crate) text_box_trim: TextBoxTrim,
    pub(crate) text_box_edge: TextBoxEdge,
    /// The direction the block's `unicode-bidi` overrides every paragraph
    /// in, or `None` where it overrides none.
    ///
    /// Under `bidi-override` and `isolate-override` each paragraph starts
    /// with an override in the block's `direction`.
    pub(crate) overrides: Option<Direction>,
    /// Its lines are set on the central baseline.
    ///
    /// True when its text is upright or mixed in a vertical typographic
    /// line. Blink sets every box of an inline formatting context on the
    /// baseline its block's style chooses.
    pub(crate) central_baseline: bool,
    /// Its content language is in the Chinese macrolanguage, where
    /// `text-autospace` treats some narrow characters as ideographs.
    pub(crate) chinese: bool,
    /// Its initial letter's box, where it has one: the only box that keeps
    /// `initial-letter`. The writer sets it as it writes the box.
    pub(crate) initial_letter: Option<NodeId>,
    /// Its own style paints (`ComputedStyle::paints`) and decorates its
    /// text (`ComputedStyle::decorates`), as `(paints, decorates)`.
    pub(crate) paints: (bool, bool),
    /// The same of its `::first-line` style, or of its own where it has
    /// none.
    pub(crate) first_line_paints: (bool, bool),
}

impl BlockFacts {
    /// Every property at its initial value, left to right, with no root
    /// style lowered.
    ///
    /// A layout never built holds it, and
    /// [`ComputedBlockStyle::new`](crate::ComputedBlockStyle::new) takes its
    /// initial values from it.
    pub(crate) const INITIAL: Self = Self {
        direction: BaseDirection::Ltr,
        writing_mode: WritingMode::HorizontalTb,
        text_align: TextAlign::Start,
        text_align_last: TextAlignLast::Auto,
        text_indent: TextIndent {
            amount: LengthPercentage::ZERO,
            hanging: false,
            each_line: false,
        },
        text_wrap_style: TextWrapStyle::Auto,
        text_overflow: TextOverflow::Clip,
        line_clamp: LineClamp::None,
        text_group_align: TextGroupAlign::None,
        text_box_trim: TextBoxTrim::None,
        text_box_edge: TextBoxEdge::AUTO,
        overrides: None,
        central_baseline: false,
        chinese: false,
        initial_letter: None,
        paints: (false, false),
        first_line_paints: (false, false),
    };

    /// Lowers `block`, reading what its style `root` says of the whole
    /// block.
    pub(super) fn new(block: &ComputedBlockStyle<'_>, root: &ComputedStyle<'_>) -> Self {
        let bidi = root.bidi;
        let vertical = block.writing_mode.is_vertical_typographic();
        Self {
            direction: if bidi.unicode_bidi == UnicodeBidi::Plaintext {
                BaseDirection::Auto
            } else {
                block.direction
            },
            writing_mode: block.writing_mode,
            text_align: block.text_align,
            text_align_last: block.text_align_last,
            text_indent: block.text_indent,
            text_wrap_style: block.text_wrap_style,
            text_overflow: block.text_overflow,
            line_clamp: block.line_clamp,
            text_group_align: block.text_group_align,
            text_box_trim: block.text_box_trim,
            text_box_edge: block.text_box_edge,
            overrides: matches!(
                bidi.unicode_bidi,
                UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride
            )
            .then_some(bidi.direction),
            central_baseline: vertical
                && root.orientation.text_orientation != TextOrientation::Sideways,
            chinese: root
                .text
                .language
                .as_ref()
                .is_some_and(|language| is_chinese(language.language())),
            initial_letter: None,
            paints: (root.paints, root.decorates),
            first_line_paints: block
                .first_line
                .map_or((root.paints, root.decorates), |style| {
                    (style.paints, style.decorates)
                }),
        }
    }

    /// Returns whether its own style in `variant` paints, and whether it
    /// decorates its text, as `(paints, decorates)`.
    pub(crate) fn style_paints(&self, variant: FirstLineVariant) -> (bool, bool) {
        match variant {
            FirstLineVariant::Standard => self.paints,
            FirstLineVariant::FirstLine => self.first_line_paints,
        }
    }

    /// Whether line layout may cut a line and end it with an ellipsis,
    /// under `text-overflow: ellipsis` or a `line-clamp`.
    pub(crate) fn may_cut_lines(&self) -> bool {
        self.text_overflow == TextOverflow::Ellipsis || self.line_clamp.clamps()
    }
}

/// `text-fit` as line breaking reads it. 8 bytes.
///
/// The content keeps it in its extras, so the block's facts, which every
/// relayout reads, stay small.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct TextFitFacts {
    pub(crate) kind: TextFitKind,
    pub(crate) target: TextFitTarget,
    /// The bound the limit sets on the scale: at most `max(limit, 1)` for
    /// `grow`, at least `limit` clamped to `0..=1` for `shrink`.
    ///
    /// Without a limit, or with a NaN one, it is infinity for `grow` and
    /// zero for `shrink`, which bound nothing.
    pub(crate) bound: f32,
}

impl TextFitFacts {
    /// `text-fit: none`.
    pub(super) const NONE: Self = Self {
        kind: TextFitKind::None,
        target: TextFitTarget::Consistent,
        bound: f32::INFINITY,
    };

    /// Lowers `fit`.
    pub(super) fn new(fit: TextFit) -> Self {
        let limit = fit.limit.filter(|limit| !limit.is_nan());
        let bound = match fit.kind {
            TextFitKind::Shrink => limit.map_or(0.0, |limit| limit.clamp(0.0, 1.0)),
            TextFitKind::Grow | TextFitKind::None => limit.map_or(f32::INFINITY, |l| l.max(1.0)),
        };
        Self {
            kind: fit.kind,
            target: fit.target,
            bound,
        }
    }
}

/// Whether `language` is in the Chinese macrolanguage, as listed by Blink's
/// `LayoutLocale::IsMacrolanguageChinese` from the IANA registry.
pub(super) fn is_chinese(language: &str) -> bool {
    matches!(
        language,
        "zh" | "cdo"
            | "cjy"
            | "cmn"
            | "cnp"
            | "cpx"
            | "csp"
            | "czh"
            | "czo"
            | "gan"
            | "hak"
            | "hnm"
            | "hsn"
            | "luh"
            | "lzh"
            | "mnp"
            | "nan"
            | "sjc"
            | "wuu"
            | "yue"
    )
}

/// The row a text facts lookup returns when it misses, which never happens
/// for an id the content handed out.
static INITIAL_TEXT: TextFacts = TextFacts {
    shaping: ShapingFactsId(0),
    flags: TextFlags::WRAPS.union(TextFlags::WHITE_SPACE_HANGS),
    line_break: LineBreak::Normal,
    word_break: WordBreak::Normal,
    collapse: WhiteSpaceCollapse::Collapse,
    setting: TextSetting::Horizontal,
    combine: TextCombineUpright::None,
    hyphen: None,
    padding: LayoutUnit::ZERO,
    hanging: HangingPunctuation::NONE,
    emphasis_side: EmphasisSide::Over,
    emphasis_skip: EmphasisSkip::INITIAL,
    justify: TextJustify::Auto,
    dominant: DominantBaseline::Auto,
    line_height: TextLineHeight::Normal,
    tab_size: TabSize::INITIAL,
};

static INITIAL_SHAPING: ShapingFacts = ShapingFacts {
    font: FontRequestId(0),
    letter: TextUnit::from_raw(0),
    word: TextUnit::from_raw(0),
    trim: TextSpacingTrim::Normal,
    orientation: OrientationGroup::INITIAL,
    fixed: FixedSpacing::NONE,
};

static INITIAL_REQUEST: FontRequest = FontRequest {
    font: FontKey::INITIAL,
    language: LanguageId::UNDETERMINED,
    initial_letter: InitialLetter::NONE,
    upright: false,
    spaced: false,
    trims_punctuation: false,
};

/// The layout's fact tables, each distinct row stored once.
pub(crate) struct Facts {
    texts: Table<TextFactsId, TextFacts>,
    shapings: Table<ShapingFactsId, ShapingFacts>,
    requests: Table<FontRequestId, FontRequest>,
    /// The box facts and edges, boxed: content with no box makes no table.
    boxes: Option<Box<BoxTables>>,
}

/// The rooms the builder writes a new layout's fact tables in. The box
/// tables are not staged.
pub(super) struct FactsStaging {
    texts: Staging<Table<TextFactsId, TextFacts>>,
    shapings: Staging<Table<ShapingFactsId, ShapingFacts>>,
    requests: Staging<Table<FontRequestId, FontRequest>>,
}

impl FactsStaging {
    /// Creates empty rooms, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            texts: Staging::new(Table::new()),
            shapings: Staging::new(Table::new()),
            requests: Staging::new(Table::new()),
        }
    }
}

heap_bytes! {
    FactsStaging { texts, shapings, requests }
}

impl Facts {
    /// Lends the rooms of `staging` to the tables that have no capacity.
    pub(super) fn lend(&mut self, staging: &mut FactsStaging) {
        staging.texts.lend(&mut self.texts);
        staging.shapings.lend(&mut self.shapings);
        staging.requests.lend(&mut self.requests);
    }

    /// Copies the tables to exact-length allocations where needed and takes
    /// the rooms of `staging` back.
    pub(super) fn settle(&mut self, staging: &mut FactsStaging) {
        staging.texts.settle(&mut self.texts);
        staging.shapings.settle(&mut self.shapings);
        staging.requests.settle(&mut self.requests);
    }
}

/// The tables of [`Facts`] that only content with boxes fills.
#[derive(Default)]
struct BoxTables {
    /// The distinct box facts, [`BoxFacts::INITIAL`] first once another row
    /// is stored.
    boxes: Table<BoxFactsId, BoxFacts>,
    /// The edges of boxes with percentage margins or padding; row 0 stands
    /// for none.
    edges: Table<EdgesId, EdgesGroup>,
}

/// The result of lowering a style.
pub(super) struct Lowered<I> {
    /// The id, or `None` where a table was full.
    pub(super) id: Option<I>,
    /// The content flags its new rows set.
    pub(super) flags: ContentFlags,
}

impl Facts {
    /// Creates empty tables, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            texts: Table::new(),
            shapings: Table::new(),
            requests: Table::new(),
            boxes: None,
        }
    }

    /// Empties every table, keeping its capacity.
    ///
    /// [`BoxFacts::INITIAL`] takes the box table's first id without being
    /// stored until a row follows it (see [`lower_box`](Self::lower_box)),
    /// so content with no box allocates nothing for boxes.
    pub(super) fn clear(&mut self) {
        self.texts.clear();
        self.shapings.clear();
        self.requests.clear();
        if let Some(tables) = &mut self.boxes {
            tables.boxes.clear();
            tables.edges.clear();
        }
    }

    /// Lowers `style` into its text facts, in a block of `writing_mode`.
    ///
    /// Interns the font request, the shaping facts and the text facts in
    /// turn, through the build's `lookup`. `languages` holds the style's
    /// language. The id is `None` where a table is full.
    pub(super) fn lower_text(
        &mut self,
        lookup: &mut Lookup,
        languages: &Languages,
        style: &StyleKey,
        writing_mode: WritingMode,
    ) -> Lowered<TextFactsId> {
        let mut flags = ContentFlags::NONE;
        let cjk = matches!(languages.get(style.text.language).language(), "ja" | "zh");
        let size = style.font.computed_size();
        let letter = TextUnit::from_px(style.text.letter_spacing.resolve(size));
        let request = FontRequest::new(style, writing_mode, letter);
        let found = intern(lookup, &mut self.requests, request, &mut flags);
        let text = found.and_then(|font| {
            let shaping = ShapingFacts::new(style, font, letter);
            let found = intern(lookup, &mut self.shapings, shaping, &mut flags)?;
            let text = TextFacts::new(style, found, writing_mode, cjk);
            intern(lookup, &mut self.texts, text, &mut flags)
        });
        Lowered { id: text, flags }
    }

    /// Lowers the box facts of a node of `kind` in `style`, in a block of
    /// `writing_mode`, resolving percentages against `basis` pixels.
    ///
    /// Interns them through `lookup` if they are new. The id is `None`
    /// where the table is full.
    pub(super) fn lower_box(
        &mut self,
        lookup: &mut Lookup,
        style: &StyleKey,
        kind: NodeKind,
        writing_mode: WritingMode,
        basis: f32,
    ) -> Lowered<BoxFactsId> {
        let mut flags = ContentFlags::NONE;
        // A container's edges with a percentage, kept for the next basis.
        // Where their table is full, the box keeps its current lengths.
        let edges = if kind.is_container() && style.edges.has_percentage() {
            let table = &mut self.boxes.get_or_insert_with(Box::default).edges;
            if table.is_empty() {
                table.push_bounded(EdgesGroup::INITIAL, "an empty table has room");
            }
            intern(lookup, table, style.edges, &mut flags).unwrap_or(EdgesId::NONE)
        } else {
            EdgesId::NONE
        };
        let facts = BoxFacts::new(style, kind, writing_mode, basis, edges);
        if facts == BoxFacts::INITIAL {
            return Lowered {
                id: Some(BoxFactsId::INITIAL),
                flags,
            };
        }
        // Store the initial row before the first other row, so lookups find
        // it among the few rows compared and `Lookup` indexes it once the
        // table grows.
        let boxes = &mut self.boxes.get_or_insert_with(Box::default).boxes;
        if boxes.is_empty() {
            boxes.push_bounded(BoxFacts::INITIAL, "an empty table has room");
        }
        let id = intern(lookup, boxes, facts, &mut flags);
        Lowered { id, flags }
    }

    /// Resolves every box's percentage edges again against `basis` pixels,
    /// in a block of `writing_mode`.
    ///
    /// Rows keep their ids: a row with percentage edges names them, so two
    /// rows that resolve alike at one basis stay distinct at another.
    pub(super) fn resolve_boxes(&mut self, writing_mode: WritingMode, basis: f32) {
        let Some(BoxTables {
            boxes,
            edges: table,
        }) = self.boxes.as_deref_mut()
        else {
            return;
        };
        for facts in boxes.as_mut_slice() {
            if facts.edges == EdgesId::NONE {
                continue;
            }
            if let Some(edges) = table.get(facts.edges) {
                facts.resolve(edges, writing_mode, basis);
            }
        }
    }

    /// Returns the text facts `id` names.
    ///
    /// Every id in the content came from this table, so a miss is a bug; it
    /// returns the initial style's facts.
    #[inline]
    pub(crate) fn text(&self, id: TextFactsId) -> &TextFacts {
        self.texts.get(id).unwrap_or_else(|| {
            debug_assert!(false, "{id:?} is not in the table");
            &INITIAL_TEXT
        })
    }

    /// Returns the shaping facts `id` names.
    #[inline]
    pub(crate) fn shaping(&self, id: ShapingFactsId) -> &ShapingFacts {
        self.shapings.get(id).unwrap_or_else(|| {
            debug_assert!(false, "{id:?} is not in the table");
            &INITIAL_SHAPING
        })
    }

    /// Returns the font request `id` names.
    #[inline]
    pub(crate) fn request(&self, id: FontRequestId) -> &FontRequest {
        self.requests.get(id).unwrap_or_else(|| {
            debug_assert!(false, "{id:?} is not in the table");
            &INITIAL_REQUEST
        })
    }

    /// Returns the box facts `id` names.
    #[inline]
    pub(crate) fn box_facts(&self, id: BoxFactsId) -> &BoxFacts {
        let found = self
            .boxes
            .as_deref()
            .and_then(|tables| tables.boxes.get(id));
        found.unwrap_or_else(|| {
            debug_assert!(id == BoxFactsId::INITIAL, "{id:?} is not in the table");
            &BoxFacts::INITIAL
        })
    }

    /// Returns the font request of the text facts `id`, through their
    /// shaping facts.
    #[inline]
    pub(crate) fn text_request(&self, id: TextFactsId) -> FontRequestId {
        self.shaping(self.text(id).shaping).font
    }

    /// Returns every font request with its id, in interning order: the
    /// block's, then its first line's, then each node's in order.
    pub(crate) fn requests(&self) -> impl ExactSizeIterator<Item = (FontRequestId, &FontRequest)> {
        self.requests.iter()
    }

    /// Returns the number of font requests.
    pub(crate) fn request_count(&self) -> usize {
        self.requests.len()
    }

    /// Returns every text facts id, in interning order.
    pub(crate) fn text_ids(&self) -> impl ExactSizeIterator<Item = TextFactsId> {
        self.texts.ids()
    }

    /// Returns the number of distinct text facts rows.
    #[cfg(test)]
    pub(crate) fn text_count(&self) -> usize {
        self.texts.len()
    }

    /// Returns the number of distinct box facts rows, including the initial
    /// row once it is stored.
    #[cfg(test)]
    pub(crate) fn box_count(&self) -> usize {
        self.boxes.as_deref().map_or(0, |tables| tables.boxes.len())
    }
}

/// Returns `value`'s id in `table`, found through `lookup`, or `None` where
/// the table is full. A new value is interned and its content flags added
/// to `flags`.
fn intern<I, T>(
    lookup: &mut Lookup,
    table: &mut Table<I, T>,
    value: T,
    flags: &mut ContentFlags,
) -> Option<I>
where
    I: Id,
    T: PartialEq + Hash + Copy + HoldsFlags,
{
    let before = table.len();
    let id = lookup.intern(table, value)?;
    if table.len() > before {
        flags.insert(value.content_flags());
    }
    Some(id)
}

/// A fact row that sets content flags when it is first interned, so later
/// stages can skip work the content never needs.
trait HoldsFlags {
    fn content_flags(&self) -> ContentFlags;
}

impl HoldsFlags for EdgesGroup {
    /// Returns [`ContentFlags::PERCENTAGES`]: edges are kept only where a
    /// margin or padding has a percentage.
    fn content_flags(&self) -> ContentFlags {
        ContentFlags::PERCENTAGES
    }
}

heap_bytes! {
    Facts { texts, shapings, requests, boxes }
}

heap_bytes! {
    BoxTables { boxes, edges }
}
