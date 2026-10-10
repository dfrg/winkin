//! The facts the writer lowers each style into. The content keeps no style:
//! every later stage reads these facts instead.
//!
//! There are four kinds: what text in a style is, what shapes alike, what
//! font selection selects fonts for, and what a box is. Each is interned
//! once, by value, and a node names it by id.
//!
//! **Lowered once, by the writer.** The writer lowers each opening call's
//! style as it takes it. It puts lengths on the grid as their readers do,
//! resolves sides for the block's writing mode, and answers the style's
//! tests (`edge_breaks_shaping`, `white_space_hangs`, what bidi resolves
//! upright). A later stage reads the answer, not the style. Facts that need
//! a font or the content belong to the first stage that has them: the used
//! size, the primary font, `line-height: normal`, tab stops in spaces.
//!
//! **Nested keys, coarsest to finest.**
//! - A [`FontRequest`] is what font selection selects fonts for.
//! - A [`ShapingFacts`] names one and adds the other shaping properties.
//!   Equal [`ShapingFactsId`]s are text that shapes alike, as Blink's equal
//!   `Font` is.
//! - A [`TextFacts`] names one and adds every other inherited fact.
//! - [`BoxFacts`] hold a box's non-inherited facts. This follows CSS's split
//!   between inherited properties and the rest.
//!
//! **A style given again is not lowered again.** Its text facts are a
//! function of it, and the builder's memo (`memo`) holds what each style was
//! lowered into. A box's facts also depend on its node's kind, so they are
//! lowered at each call, comparing one row. In debug builds and the tests
//! the oracle (`facts_check`) compares every record the writer hands a node with
//! its own projection of the style.
//!
//! The ids are `u16`, each defined once by its `define_id!` below. A
//! document with more distinct facts gives the rest a stand-in and reports
//! it in [`BuildReport::replaced_styles`](crate::BuildReport).

use alloc::boxed::Box;
use core::hash::{Hash, Hasher};
use core::mem;

use super::lists::{HyphenStringId, LanguageId, Languages, Lookup};
use super::memo::{FontKey, StyleKey};
use super::{ContentFlags, NodeId, NodeKind};
use crate::data::{Id, Table, define_flags, define_id, heap_bytes};
use crate::style::{
    BaseDirection, BoxDecorationBreak, ComputedBlockStyle, ComputedStyle, Direction,
    DominantBaseline, EdgesGroup, EmphasisSide, EmphasisSkip, FontVariantPosition,
    HangingPunctuation, Hyphens, InitialLetter, LengthPercentage, LineBreak, LineClamp, LineHeight,
    OrientationGroup, OverflowWrap, RubyGroup, TabSize, TextAlign, TextAlignLast, TextBoxEdge,
    TextBoxTrim, TextCombineUpright, TextGroupAlign, TextIndent, TextJustify, TextOrientation,
    TextOverflow, TextSpacingTrim, TextWrapMode, TextWrapStyle, UnicodeBidi, VerticalAlign,
    WhiteSpaceCollapse, WordBreak, WritingMode,
};
use crate::style::{Same, same_by_value, style_struct};
use crate::unit::{LayoutUnit, TextUnit};

define_id! {
    /// The id of a distinct [`TextFacts`] row in a layout.
    ///
    /// A node names one for each variant (`Node.text`, `first_line_text`).
    /// The id has sixteen bits. Past them a node takes its parent's.
    pub(crate) struct TextFactsId(u16);
}

define_id! {
    /// The id of a distinct [`ShapingFacts`] row in a layout.
    ///
    /// Equal ids are text that shapes alike. The id has sixteen bits. Past
    /// them a node takes its parent's text facts.
    pub(crate) struct ShapingFactsId(u16);
}

define_id! {
    /// The id of a distinct [`FontRequest`] in a layout.
    ///
    /// A request is font selection's unit of work. The id has sixteen bits.
    /// Past them a node takes its parent's text facts.
    pub(crate) struct FontRequestId(u16);
}

define_id! {
    /// The id of a distinct [`BoxFacts`] row in a layout.
    ///
    /// A node names one for each variant (`Node.box_`, `first_line_box`).
    /// The id has sixteen bits. Past them a node takes
    /// [`BoxFactsId::INITIAL`].
    pub(crate) struct BoxFactsId(u16);
}

define_id! {
    /// The id of a box's margin, border and padding as its style gives them,
    /// in the table of those with a percentage.
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
    /// full. The row is stored only once another row is stored after it.
    pub(super) const INITIAL: Self = Self(0);
}

same_by_value!(
    TextFactsId,
    ShapingFactsId,
    FontRequestId,
    EdgesId,
    TextFlags,
    BoxFlags,
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

/// How a style's text stands in the block's lines.
///
/// `writing-mode` and `text-orientation` decide it together. The writer
/// lowers it, and the analysis stands each cluster by it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TextSetting {
    /// In a horizontal line.
    Horizontal,
    /// Each cluster by its first character's Vertical_Orientation:
    /// `text-orientation: mixed` in `vertical-rl` and `vertical-lr`.
    Mixed,
    /// Upright but for a vertical script's characters: `upright`.
    Upright,
    /// On its side: `sideways`, and every text in `sideways-rl` and
    /// `sideways-lr`, whose lines are set as horizontal ones turned.
    Sideways,
}

impl TextSetting {
    /// Returns how text set `orientation` stands in a block of
    /// `writing_mode`.
    ///
    /// `text-orientation` applies only in the vertical typographic modes
    /// (CSS Writing Modes 4, section 5.1). `sideways-rl` and `sideways-lr`
    /// set every character on its side whatever it says.
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
/// `normal` waits for the measurements, which have the primary font. A
/// factor is taken of the computed size on layout's grid. A length stays as
/// given: the measurements round it onto the grid, and font selection takes
/// it as it is to size an initial letter.
#[derive(Copy, Clone, Debug)]
pub(crate) enum TextLineHeight {
    /// The primary font's line spacing.
    Normal,
    /// A factor of the computed size, each truncated onto the grid, as
    /// Blink keeps a number as a percentage of the size.
    Factor(LayoutUnit),
    /// A length in pixels, as given. The measurements round it onto the
    /// grid, saturating. Font selection takes one that is not finite as
    /// nothing.
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
    /// gives it, with `normal` as the height for `normal`.
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
    /// What text in a style is, as bits (see [`TextFacts`]).
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
        /// The text wraps, and `overflow-wrap` is `anywhere` or
        /// `break-word`: an emergency break is allowed.
        pub(crate) const EMERGENCY = 1 << 4;
        /// `overflow-wrap: anywhere`, which lowers min-content to the widest
        /// cluster. `break-word` does not.
        pub(crate) const ANYWHERE = 1 << 5;
        /// The content language is Japanese or Chinese, which the line
        /// segmenter tailors for under `line-break: normal` and `loose`.
        pub(crate) const CJK_LINE_BREAKS = 1 << 6;
        /// Set upright in a vertical typographic line, or combined whole:
        /// bidi resolves each cluster as one left-to-right unit.
        pub(crate) const UPRIGHT_BIDI = 1 << 7;
        /// `direction: rtl`.
        pub(crate) const RTL = 1 << 8;
        /// `text-emphasis-style` sets marks.
        pub(crate) const EMPHASIS = 1 << 9;
        /// `text-autospace` spaces a letter's seam. It needs
        /// `ideograph-alpha`, and text not set upright in a vertical line,
        /// where Chrome ends a seam (`kVerticalUpright`).
        pub(crate) const AUTOSPACE_ALPHA = 1 << 10;
        /// `text-autospace` spaces a digit's seam. It needs
        /// `ideograph-numeric`, and text not upright in a vertical line.
        pub(crate) const AUTOSPACE_NUMERIC = 1 << 11;
    }
}

style_struct! {
    /// What text in a style is: every inherited fact a later stage reads.
    ///
    /// There is one row per distinct set of facts, named by each node in
    /// each variant. A text node's are its container's. A row is about 44
    /// bytes.
    ///
    /// The row holds inherited properties only. Styles that differ only in
    /// a box's properties share a row. So does a first-line style that
    /// differs only in its transform, which the writer applies. The
    /// analysis facts are pinned: a node's first-line row differs from its
    /// own only in the shaping, the emphasis, the line height and the
    /// spacing.
    pub(crate) struct TextFacts {
        /// What it shapes as, and through it the font request.
        pub(crate) shaping: ShapingFactsId,
        pub(crate) flags: TextFlags,
        // What the analysis stage reads.
        /// `line-break`.
        pub(crate) line_break: LineBreak,
        /// `word-break`.
        pub(crate) word_break: WordBreak,
        /// `white-space-collapse`.
        pub(crate) collapse: WhiteSpaceCollapse,
        /// How it stands in the block's lines.
        pub(crate) setting: TextSetting,
        /// What `text-combine-upright` combines in a vertical typographic
        /// line. It is `none` elsewhere (CSS Writing Modes 4, section 9.1).
        /// Digits are clamped to two to four, as CSS takes the integer.
        pub(crate) combine: TextCombineUpright,
        // What font selection reads.
        /// `hyphenate-character`, or `None` for `auto`.
        pub(crate) hyphen: Option<HyphenStringId>,
        // What the measurements, the breaker, line layout and the readers
        // read.
        /// `line-padding` on layout's grid, truncated as a box's padding is.
        ///
        /// The measurements charge it to a line at the end where this text
        /// holds the line's outermost content. Line layout places it inside
        /// the box there. It is zero for a length that is not finite and
        /// positive. Chrome does not parse `line-padding` at all.
        pub(crate) padding: LayoutUnit,
        /// `hanging-punctuation`.
        pub(crate) hanging: HangingPunctuation,
        /// The side of the line its emphasis marks take in the block's
        /// writing mode (`EmphasisPosition::side`).
        pub(crate) emphasis_side: EmphasisSide,
        /// `text-emphasis-skip`.
        pub(crate) emphasis_skip: EmphasisSkip,
        /// `text-justify`.
        pub(crate) justify: TextJustify,
        /// `dominant-baseline`, which is inherited.
        pub(crate) dominant: DominantBaseline,
        /// `line-height`, for the measurements and the initial letter.
        pub(crate) line_height: TextLineHeight,
        /// `tab-size`, for the measurements, which count spaces in the
        /// primary font.
        pub(crate) tab_size: TabSize,
    }
}

impl TextFacts {
    /// Lowers the text facts of `style` in a block of `writing_mode`.
    ///
    /// `shaping` names its shaping facts. `cjk` says whether its language is
    /// Japanese or Chinese.
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
        let mut set = |flag: TextFlags, on: bool| {
            if on {
                flags.insert(flag);
            }
        };
        set(TextFlags::WRAPS, wraps);
        set(TextFlags::WHITE_SPACE_HANGS, text.white_space_hangs());
        set(
            TextFlags::HANGS_CONDITIONALLY,
            text.white_space_hangs_conditionally(),
        );
        set(TextFlags::HYPHENS_NONE, text.hyphens == Hyphens::None);
        set(
            TextFlags::EMERGENCY,
            wraps && text.overflow_wrap != OverflowWrap::Normal,
        );
        set(
            TextFlags::ANYWHERE,
            text.overflow_wrap == OverflowWrap::Anywhere,
        );
        set(TextFlags::CJK_LINE_BREAKS, cjk);
        set(
            TextFlags::UPRIGHT_BIDI,
            vertical
                && (orientation.text_orientation == TextOrientation::Upright
                    || orientation.text_combine_upright == TextCombineUpright::All),
        );
        set(TextFlags::RTL, style.bidi.direction == Direction::Rtl);
        set(TextFlags::EMPHASIS, text.emphasis.marks);
        // `text-orientation` has no effect in the sideways modes, so upright
        // text ends a seam only in a vertical typographic mode. Chrome's
        // `TextAutoSpace` reads the font's orientation, which is horizontal
        // there.
        let upright_in_vertical =
            vertical && orientation.text_orientation == TextOrientation::Upright;
        set(
            TextFlags::AUTOSPACE_ALPHA,
            text.autospace.ideograph_alpha && !upright_in_vertical,
        );
        set(
            TextFlags::AUTOSPACE_NUMERIC,
            text.autospace.ideograph_numeric && !upright_in_vertical,
        );
        // Truncates `line-padding` onto the grid, as a box's padding is.
        // A length that is not finite and positive gives zero; no computed
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

    /// Whether every flag of `flag` is set.
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

style_struct! {
    /// What shapes alike: the font request and the other shaping
    /// properties.
    ///
    /// The font request holds the font group, the family list, the feature
    /// and variation settings and the language. This row adds
    /// `letter-spacing`, `word-spacing`, `text-spacing-trim`,
    /// `text-orientation` and `text-combine-upright`. A row is 16 bytes.
    ///
    /// Equal ids are text that shapes alike, as Blink ends a shaping run
    /// where the `Font` differs. The shaping walk, the font walk's position
    /// runs and divided graphemes, and a shaped run's link
    /// (`ShapedRun.shaping`) compare them.
    pub(crate) struct ShapingFacts {
        /// What font selection selects fonts for.
        pub(crate) font: FontRequestId,
        /// `letter-spacing` on glyph geometry's grid, rounded to the
        /// nearest 1/65536 px as Chrome's
        /// `TextRunLayoutUnit::FromFloatRound` rounds it.
        ///
        /// The measurements add it after a cluster. With the request's
        /// `spaced`, it also turns optional ligatures off, so both read one
        /// rounding. It saturates, and NaN gives zero.
        pub(crate) letter: TextUnit,
        /// `word-spacing`, its percentage taken of the computed size and the
        /// sum rounded as `letter-spacing` is.
        ///
        /// The measurements add it after a word separator, and the breaker
        /// adds it after one on a line edge it reshapes. It saturates, and
        /// NaN gives zero.
        pub(crate) word: TextUnit,
        /// `text-spacing-trim`.
        pub(crate) trim: TextSpacingTrim,
        /// `text-orientation` and `text-combine-upright`.
        pub(super) orientation: OrientationGroup,
    }
}

impl ShapingFacts {
    /// Lowers the shaping facts of `style`, given its font request `font`
    /// and its letter spacing on the grid `letter`.
    fn new(style: &StyleKey, font: FontRequestId, letter: TextUnit) -> Self {
        let word = style.text.word_spacing.resolve(style.font.computed_size());
        Self {
            font,
            letter,
            word: TextUnit::from_px(word),
            trim: style.text.spacing_trim,
            orientation: style.orientation,
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
    /// What font selection selects fonts for: its unit of work.
    ///
    /// Font selection resolves each request once into a `FontResolution`.
    /// The row holds the font properties, the lists, the language, and the
    /// three facts of the text that choose features. It is about 72 bytes.
    pub(crate) struct FontRequest {
        /// The font properties, the computed size and the lists among them.
        pub(crate) font: FontKey,
        /// The content language; `und` where none was given.
        pub(crate) language: LanguageId,
        /// The used `initial-letter`, set only on the block's initial
        /// letter.
        pub(crate) initial_letter: InitialLetter,
        /// The text is set upright or mixed in a vertical typographic line.
        /// It shapes with the vertical forms of the features chosen for its
        /// spacing, as Blink's `IsVerticalAnyUpright` chooses them.
        pub(crate) upright: bool,
        /// Its letter spacing is not zero on glyph geometry's grid, which
        /// turns optional ligatures off.
        pub(crate) spaced: bool,
        /// Its `text-spacing-trim` trims punctuation, which turns `chws` on.
        pub(crate) trims_punctuation: bool,
    }
}

impl FontRequest {
    /// Lowers the font request of `style` in a block of `writing_mode`,
    /// given its letter spacing on the grid `letter`.
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
    /// What a box is, as bits (see [`BoxFacts`]).
    pub(crate) struct BoxFlags(u16) {
        /// It draws something of its own the caller keeps (`paints`).
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
        /// `text-box-trim` on it is not `none`.
        pub(crate) const TRIMS_TEXT_BOX = 1 << 7;
        /// It is the block's initial letter, whose ink takes room at its
        /// edges however its own edges are set.
        pub(crate) const INITIAL_LETTER = 1 << 8;
    }
}

style_struct! {
    /// What a box is: its non-inherited facts, lowered for the block's
    /// writing mode.
    ///
    /// Inline boxes, ruby containers and annotations, `::first-letter`
    /// boxes and atomic inlines have one. A node that is no box has
    /// [`BoxFacts::INITIAL`]. A row is about 40 bytes.
    ///
    /// Of what `::first-line` may change, only `vertical-align` is a box's
    /// (`pinned_first_line`). That is why a node has a first-line row.
    pub(crate) struct BoxFacts {
        pub(super) flags: BoxFlags,
        /// The room it takes along the line at each end, as `(line-left,
        /// line-right)`.
        ///
        /// Each is the margin, border and padding on the physical side the
        /// writing mode puts there, truncated onto the grid
        /// (`EdgesGroup::inline`). Its direction, [`BoxFlags::RTL`], says
        /// which one an opening or a closing edge takes.
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
        /// Its margin, border and padding as the style gives them, where a
        /// margin or padding has a percentage: `room`, `margin_line`,
        /// `margin_across` and `across` are resolved from them again for
        /// each basis.
        pub(super) edges: EdgesId,
        /// `vertical-align` where it applies, and `baseline` elsewhere.
        ///
        /// It applies to inline boxes, `::first-letter` boxes and atomic
        /// inlines. A length stays as given for the measurements, which
        /// have the fonts that a percentage and the keywords need.
        pub(crate) align: VerticalAlign,
        /// `unicode-bidi`.
        pub(crate) bidi: UnicodeBidi,
        /// `ruby-position`, `ruby-align` and `ruby-overhang`.
        pub(crate) ruby: RubyGroup,
    }
}

impl BoxFacts {
    /// No edges, no shift and nothing painted.
    ///
    /// A culled box has these facts, and so does every node that is no box.
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

    /// Lowers the box facts of a node of `kind` set in `style`, in a block
    /// of `writing_mode` whose percentages are of `basis` pixels. `edges`
    /// names the style's edges where they have a percentage.
    ///
    /// - A container (an inline box, a ruby container or annotation, a
    ///   `::first-letter` box) has every fact, `vertical-align` only where
    ///   it applies.
    /// - An atomic inline has only its `vertical-align`. Its margins are in
    ///   its own row.
    /// - Anything else, the block and floats among them, has none.
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
        let mut set = |flag: BoxFlags, on: bool| {
            if on {
                flags.insert(flag);
            }
        };
        set(BoxFlags::PAINTS, style.paints);
        set(BoxFlags::DECORATES, style.decorates);
        set(BoxFlags::HAS_EDGES, edges.any());
        set(
            BoxFlags::CLONES,
            edges.decoration_break == BoxDecorationBreak::Clone,
        );
        set(BoxFlags::RTL, rtl);
        set(
            BoxFlags::TRIMS_TEXT_BOX,
            style.line.text_box_trim != TextBoxTrim::None,
        );
        set(BoxFlags::INITIAL_LETTER, style.line.initial_letter.is_set());
        // An edge stops shaping as Blink's `ShouldBreakShapingBeforeBox`
        // and `AfterBox` say: margin, border or padding on that inline side,
        // or `vertical-align` other than `baseline`. Any `unicode-bidi`
        // stops it too, as Blink ends a run at every bidi control. A box
        // that only paints is shaped across. The test reads the unrounded
        // lengths, as Blink tests the computed ones.
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
        set(
            BoxFlags::BREAKS_SHAPING_AT_START,
            breaks_everywhere || any_along(true),
        );
        set(
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

    /// Resolves `edges`, its style's, onto the grid, their percentages taken
    /// of `basis` pixels, in a block of `writing_mode`.
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

    /// Whether every flag of `flag` is set.
    #[inline]
    pub(crate) fn has(&self, flag: BoxFlags) -> bool {
        self.flags.contains(flag)
    }

    /// Whether its opening edge (`opens`) or its closing edge stops
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
/// It holds a [`ComputedBlockStyle`] without its
/// styles, which the builder lowers as the block node's own. It adds what
/// the block's own style says of the whole block, lowered by the builder.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct BlockFacts {
    /// The base direction bidi sets the block's paragraphs at.
    ///
    /// It is the block's direction, or `auto` where its `unicode-bidi` is
    /// `plaintext`. With `auto` each paragraph takes its first strong
    /// character's (CSS Writing Modes 3, section 2.4.2), as Blink's
    /// `BidiParagraph` does.
    pub(crate) direction: BaseDirection,
    pub(crate) writing_mode: WritingMode,
    pub(crate) text_align: TextAlign,
    pub(crate) text_align_last: TextAlignLast,
    pub(crate) text_indent: TextIndent,
    pub(crate) text_wrap_style: TextWrapStyle,
    pub(crate) text_overflow: TextOverflow,
    pub(crate) line_clamp: LineClamp,
    pub(crate) text_group_align: TextGroupAlign,
    /// `text-box-trim` on the block.
    pub(crate) text_box_trim: TextBoxTrim,
    /// `text-box-edge` on the block.
    pub(crate) text_box_edge: TextBoxEdge,
    /// The direction the block's `unicode-bidi` overrides every paragraph
    /// in, or `None` where it overrides none.
    ///
    /// Under `bidi-override` and `isolate-override` each paragraph starts
    /// with an override in the block's own `direction`.
    pub(crate) overrides: Option<Direction>,
    /// Its lines are set on the central baseline.
    ///
    /// This holds when its text stands upright or mixed in a vertical
    /// typographic line. Blink sets every box of an inline formatting
    /// context on the baseline its block's style chooses.
    pub(crate) central_baseline: bool,
    /// Its content language is in the Chinese macrolanguage, where
    /// `text-autospace` takes some narrow characters for ideographs.
    pub(crate) chinese: bool,
    /// Its initial letter's box, where it has one.
    ///
    /// The writer writes it as it writes the box. That box is the only
    /// one that keeps `initial-letter`.
    pub(crate) initial_letter: Option<NodeId>,
}

impl BlockFacts {
    /// Every property at its initial value, reading left to right, with no
    /// root style lowered.
    ///
    /// A layout never built holds it, and
    /// [`ComputedBlockStyle::new`](crate::ComputedBlockStyle::new) sets it.
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
    };

    /// Lowers the block `block` describes, reading what its own style
    /// `root` says of the whole block.
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
        }
    }

    /// Whether line layout may cut a line and end it with an ellipsis,
    /// under `text-overflow: ellipsis` or a `line-clamp`.
    pub(crate) fn may_cut_lines(&self) -> bool {
        self.text_overflow == TextOverflow::Ellipsis || self.line_clamp.clamps()
    }
}

/// Whether `language` is one of the Chinese macrolanguage's, as Blink's
/// `LayoutLocale::IsMacrolanguageChinese` lists them from the IANA registry.
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

/// The row a lookup in a fact table returns when it misses.
///
/// A lookup of an id the content handed out never misses.
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
///
/// The writer lowers every style through it (see the module documentation).
pub(crate) struct Facts {
    texts: Table<TextFactsId, TextFacts>,
    shapings: Table<ShapingFactsId, ShapingFacts>,
    requests: Table<FontRequestId, FontRequest>,
    boxes: Table<BoxFactsId, BoxFacts>,
    /// The edges of boxes with percentage margins or padding, boxed: a
    /// layout with none makes no table.
    edges: Option<Box<Table<EdgesId, EdgesGroup>>>,
}

/// The result of lowering a style: the id, or none if a table was full,
/// and the flags its new rows set.
pub(super) struct Lowered<I> {
    /// The id, or `None` where a table was full.
    pub(super) id: Option<I>,
    /// The content flags its new rows hold.
    pub(super) flags: ContentFlags,
}

impl Facts {
    /// Creates empty tables, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            texts: Table::new(),
            shapings: Table::new(),
            requests: Table::new(),
            boxes: Table::new(),
            edges: None,
        }
    }

    /// Empties every table, keeping its capacity.
    ///
    /// [`BoxFacts::INITIAL`] takes the box table's first id without being
    /// stored, until a row is stored after it (see
    /// [`lower_box`](Self::lower_box)). Content with no box then allocates
    /// nothing for boxes, as the content stores no `und` and no settings.
    pub(super) fn clear(&mut self) {
        self.texts.clear();
        self.shapings.clear();
        self.requests.clear();
        self.boxes.clear();
        if let Some(edges) = &mut self.edges {
            edges.clear();
        }
    }

    /// Lowers `style` into its text facts, in a block of `writing_mode`.
    ///
    /// It interns the font request, the shaping facts and the text facts in
    /// turn, each found through the content's `lookup`. `languages` holds
    /// the style's language. The id is `None` where a table is full.
    pub(super) fn lower_text(
        &mut self,
        lookup: &mut Lookup,
        languages: &Languages,
        style: &StyleKey,
        writing_mode: WritingMode,
    ) -> Lowered<TextFactsId> {
        let mut flags = ContentFlags::NONE;
        let cjk = matches!(languages.get(style.text.language).language(), "ja" | "zh");
        let letter = TextUnit::from_px(style.text.letter_spacing);
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

    /// Lowers the box facts of a node of `kind` set in `style`, in a block
    /// of `writing_mode` whose percentages are of `basis` pixels.
    ///
    /// It finds them through `lookup` and interns them if they are new.
    /// The id is `None` where the table is full.
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
        // Where their table is full, the box keeps the lengths it has now.
        let edges = if kind.is_container() && style.edges.has_percentage() {
            let table = self.edges.get_or_insert_with(Box::default);
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
        // Stores the initial row before the first other row. It is then
        // found among the few rows compared, and `Lookup` indexes it with
        // them once the table holds more.
        if self.boxes.is_empty() {
            self.boxes
                .push_bounded(BoxFacts::INITIAL, "an empty table has room");
        }
        let id = intern(lookup, &mut self.boxes, facts, &mut flags);
        Lowered { id, flags }
    }

    /// Resolves every box's edges again, their percentages taken of `basis`
    /// pixels, in a block of `writing_mode`.
    ///
    /// Rows keep their ids: a row with percentage edges names them, so two
    /// rows that resolve alike at one basis stay apart at another.
    pub(super) fn resolve_boxes(&mut self, writing_mode: WritingMode, basis: f32) {
        let Some(table) = self.edges.as_deref() else {
            return;
        };
        for facts in self.boxes.as_mut_slice() {
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
    /// Every id in the content came from this table, so the lookup cannot
    /// miss. A miss is a bug, and returns the initial style's facts.
    #[inline]
    pub(crate) fn text(&self, id: TextFactsId) -> &TextFacts {
        self.texts.get(id).unwrap_or_else(|| {
            debug_assert!(false, "{id:?} is not in the table");
            &INITIAL_TEXT
        })
    }

    /// The shaping facts `id` names.
    #[inline]
    pub(crate) fn shaping(&self, id: ShapingFactsId) -> &ShapingFacts {
        self.shapings.get(id).unwrap_or_else(|| {
            debug_assert!(false, "{id:?} is not in the table");
            &INITIAL_SHAPING
        })
    }

    /// The font request `id` names.
    #[inline]
    pub(crate) fn request(&self, id: FontRequestId) -> &FontRequest {
        self.requests.get(id).unwrap_or_else(|| {
            debug_assert!(false, "{id:?} is not in the table");
            &INITIAL_REQUEST
        })
    }

    /// The box facts `id` names.
    #[inline]
    pub(crate) fn box_facts(&self, id: BoxFactsId) -> &BoxFacts {
        self.boxes.get(id).unwrap_or_else(|| {
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

    /// Yields every font request with its id, in the order they were
    /// interned.
    ///
    /// The block's comes first, then its first line's, then each node's
    /// in order. Each request is font selection's unit of work.
    pub(crate) fn requests(&self) -> impl ExactSizeIterator<Item = (FontRequestId, &FontRequest)> {
        self.requests.iter()
    }

    /// How many font requests there are.
    pub(crate) fn request_count(&self) -> usize {
        self.requests.len()
    }

    /// Every text facts row's id, in the order they were interned.
    pub(crate) fn text_ids(&self) -> impl ExactSizeIterator<Item = TextFactsId> {
        self.texts.ids()
    }

    /// How many distinct text facts there are, which the tests count.
    #[cfg(test)]
    pub(crate) fn text_count(&self) -> usize {
        self.texts.len()
    }

    /// How many distinct box facts there are, the initial row among them
    /// once it is stored, which the tests count.
    #[cfg(test)]
    pub(crate) fn box_count(&self) -> usize {
        self.boxes.len()
    }
}

/// Returns `value`'s id in `table`, found through `lookup`, or `None` where
/// the table is full.
///
/// A new value is interned, and its content flags are added to `flags`.
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

/// A fact row that sets content flags when it is first interned.
///
/// The flags summarize what the content holds, so later stages can skip
/// work the content never needs.
trait HoldsFlags {
    fn content_flags(&self) -> ContentFlags;
}

impl HoldsFlags for EdgesGroup {
    /// Edges are kept only where a margin or padding has a percentage.
    fn content_flags(&self) -> ContentFlags {
        ContentFlags::PERCENTAGES
    }
}

heap_bytes! {
    Facts { texts, shapings, requests, boxes, edges }
}
