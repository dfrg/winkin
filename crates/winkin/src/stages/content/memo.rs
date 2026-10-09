//! The builder's memo: the text facts each style in a build was lowered
//! into.
//!
//! Most styles are given again, box after box, and the memo keeps them from
//! being lowered again.
//!
//! **Keyed by the style's bits.** A [`StyleKey`] is the caller's
//! [`ComputedStyle`](crate::ComputedStyle) with its lists interned and named
//! by id ([`Lists::key`](super::Lists::key)). A [`FontKey`] and a
//! [`TextKey`] stand for the two groups that borrow lists. The key is
//! compared and hashed by its bits (`style::same`), so the memo compares
//! list ids, never list contents. Only the builder holds keys: while a
//! style is lowered, in a `::first-letter` waiting for its letter, and
//! here. The content never holds one; its stages read the facts.
//!
//! **In the builder's scratch, bounded.** The memo lives in the content
//! scratch that the layout keeps for its builder, not in the content. It is
//! cleared at each build, since it maps to that build's ids. It is also
//! cleared once it holds [`MOST`] styles, since past that a document's
//! styles are not the few that repeat. Clearing keeps its capacity, so a
//! warm build allocates nothing.
//!
//! Only the text facts are memoized. A box's facts also depend on its
//! node's kind, so they are lowered at each call, comparing one row. The
//! block's own styles are lowered once per build and are not held.

use super::facts::TextFactsId;
use super::lists::{
    FamilyListId, FeatureSettingsId, HyphenStringId, LanguageId, Lookup, VariationSettingsId,
};
use crate::data::{Table, define_id, hash_one, heap_bytes};
use crate::style::style_struct;
use crate::style::{
    BidiGroup, EdgesGroup, FontGroup, FontKerning, FontLanguageOverride, FontOpticalSizing,
    FontSizeAdjust, FontStyle, FontSynthesis, FontVariantCaps, FontVariantEmoji,
    FontVariantPosition, FontVariants, FontWeight, FontWidth, HangingPunctuation, Hyphens,
    LengthPercentage, LineBreak, LineGroup, OrientationGroup, OverflowWrap, RubyGroup, TabSize,
    TextAutospace, TextEmphasis, TextGroup, TextJustify, TextSecurity, TextSpacingTrim,
    TextTransform, TextWrapMode, WhiteSpaceCollapse, WhiteSpaceTrim, WordBreak,
    sanitized_font_size,
};

style_struct! {
    /// A [`FontGroup`] with its lists named by id.
    ///
    /// It holds the group's values as given. The memo keys a style by it,
    /// and a font request holds it.
    pub(crate) struct FontKey {
        /// `font-family`.
        pub(crate) families: FamilyListId,
        pub(crate) size: f32,
        pub(crate) weight: FontWeight,
        pub(crate) width: FontWidth,
        pub(crate) style: FontStyle,
        pub(crate) synthesis: FontSynthesis,
        pub(crate) variant_caps: FontVariantCaps,
        pub(crate) variant_position: FontVariantPosition,
        pub(crate) variants: FontVariants,
        pub(crate) variant_emoji: FontVariantEmoji,
        pub(crate) optical_sizing: FontOpticalSizing,
        pub(crate) kerning: FontKerning,
        pub(crate) language_override: FontLanguageOverride,
        pub(crate) size_adjust: FontSizeAdjust,
        /// `font-feature-settings`.
        pub(crate) features: FeatureSettingsId,
        /// `font-variation-settings`.
        pub(crate) variations: VariationSettingsId,
    }
}

impl FontKey {
    /// Every font property at its initial value, naming the initial lists.
    pub(crate) const INITIAL: Self = Self::new(
        &FontGroup::INITIAL,
        FamilyListId::INITIAL,
        FeatureSettingsId::EMPTY,
        VariationSettingsId::EMPTY,
    );

    /// Keys `group`, whose lists the content's tables gave these ids.
    pub(super) const fn new(
        group: &FontGroup<'_>,
        families: FamilyListId,
        features: FeatureSettingsId,
        variations: VariationSettingsId,
    ) -> Self {
        // Every field is named, so a field added to the group fails here.
        let FontGroup {
            families: _,
            size,
            weight,
            width,
            style,
            synthesis,
            variant_caps,
            variant_position,
            variants,
            variant_emoji,
            optical_sizing,
            kerning,
            language_override,
            size_adjust,
            features: _,
            variations: _,
        } = *group;
        Self {
            families,
            size,
            weight,
            width,
            style,
            synthesis,
            variant_caps,
            variant_position,
            variants,
            variant_emoji,
            optical_sizing,
            kerning,
            language_override,
            size_adjust,
            features,
            variations,
        }
    }

    /// Returns the computed `font-size` in pixels, sanitized by
    /// [`sanitized_font_size`].
    ///
    /// A numeric `line-height` multiplies it. Synthesized small capitals are
    /// drawn at 0.7 of it. Font selection works the used size out from it.
    pub(crate) fn computed_size(&self) -> f32 {
        sanitized_font_size(self.size)
    }
}

style_struct! {
    /// A [`TextGroup`] with its language and hyphenation string named by id.
    ///
    /// It holds the group's values as given. The memo keys a style by it.
    pub(super) struct TextKey {
        /// The content language; `und` where the caller gave none.
        pub(super) language: LanguageId,
        pub(super) white_space_collapse: WhiteSpaceCollapse,
        pub(super) wrap_mode: TextWrapMode,
        pub(super) white_space_trim: WhiteSpaceTrim,
        pub(super) transform: TextTransform,
        pub(super) security: TextSecurity,
        pub(super) word_break: WordBreak,
        pub(super) line_break: LineBreak,
        pub(super) overflow_wrap: OverflowWrap,
        pub(super) hyphens: Hyphens,
        /// `hyphenate-character`, or `None` for `auto`.
        pub(super) hyphenate_character: Option<HyphenStringId>,
        pub(super) tab_size: TabSize,
        pub(super) letter_spacing: f32,
        pub(super) word_spacing: LengthPercentage,
        pub(super) autospace: TextAutospace,
        pub(super) spacing_trim: TextSpacingTrim,
        pub(super) hanging_punctuation: HangingPunctuation,
        pub(super) justify: TextJustify,
        pub(super) emphasis: TextEmphasis,
    }
}

impl TextKey {
    /// Every text property at its initial value, naming the initial language.
    pub(super) const INITIAL: Self = Self::new(&TextGroup::INITIAL, LanguageId::UNDETERMINED, None);

    /// Keys `group`, whose language and hyphenation string the content's
    /// tables gave these ids.
    pub(super) const fn new(
        group: &TextGroup<'_>,
        language: LanguageId,
        hyphenate_character: Option<HyphenStringId>,
    ) -> Self {
        // Every field is named, so a field added to the group fails here.
        let TextGroup {
            language: _,
            white_space_collapse,
            wrap_mode,
            white_space_trim,
            transform,
            security,
            word_break,
            line_break,
            overflow_wrap,
            hyphens,
            hyphenate_character: _,
            tab_size,
            letter_spacing,
            word_spacing,
            autospace,
            spacing_trim,
            hanging_punctuation,
            justify,
            emphasis,
        } = *group;
        Self {
            language,
            white_space_collapse,
            wrap_mode,
            white_space_trim,
            transform,
            security,
            word_break,
            line_break,
            overflow_wrap,
            hyphens,
            hyphenate_character,
            tab_size,
            letter_spacing,
            word_spacing,
            autospace,
            spacing_trim,
            hanging_punctuation,
            justify,
            emphasis,
        }
    }

    /// Whether spaces, tabs and other space separators ending a line hang past it.
    ///
    /// They hang where white space collapses, and where it is kept and wraps
    /// (`pre-wrap`). They don't hang under `break-spaces`, where they are
    /// content that wraps. They don't hang where white space is kept and
    /// doesn't wrap (`pre`), where they are content that overflows. Blink's
    /// `ComputeTrailingSpaceWidth` hangs nothing there either. Chrome doesn't
    /// parse `preserve-spaces`, so it counts as `preserve`.
    pub(super) fn white_space_hangs(&self) -> bool {
        match self.white_space_collapse {
            WhiteSpaceCollapse::BreakSpaces => false,
            WhiteSpaceCollapse::Preserve | WhiteSpaceCollapse::PreserveSpaces => {
                self.wrap_mode == TextWrapMode::Wrap
            }
            WhiteSpaceCollapse::Collapse
            | WhiteSpaceCollapse::PreserveBreaks
            | WhiteSpaceCollapse::Discard => true,
        }
    }

    /// Whether white space ending a paragraph hangs only where it overflows.
    ///
    /// This holds for kept white space that wraps (`pre-wrap`), before a
    /// forced break or at the block's end. Such white space counts in
    /// max-content and not in min-content, as CSS Text 3 says. Where white
    /// space collapses, the space separators left there hang whole, as at any
    /// line's end.
    pub(super) fn white_space_hangs_conditionally(&self) -> bool {
        self.white_space_collapse.keeps_spaces() && self.white_space_hangs()
    }
}

style_struct! {
    /// A caller's style as the writer lowers it.
    ///
    /// It holds the style's groups as given, the font and text groups with
    /// their lists named by the ids the content's tables gave them. It is
    /// compared and hashed by its bits, as the memo keys it.
    pub(super) struct StyleKey {
        pub(super) font: FontKey,
        pub(super) text: TextKey,
        pub(super) line: LineGroup,
        pub(super) bidi: BidiGroup,
        pub(super) orientation: OrientationGroup,
        pub(super) edges: EdgesGroup,
        pub(super) ruby: RubyGroup,
        pub(super) paints: bool,
        pub(super) decorates: bool,
    }
}

impl StyleKey {
    /// Every property at its initial value, naming the initial lists.
    pub(super) const INITIAL: Self = Self {
        font: FontKey::INITIAL,
        text: TextKey::INITIAL,
        line: LineGroup::INITIAL,
        bidi: BidiGroup::INITIAL,
        orientation: OrientationGroup::INITIAL,
        edges: EdgesGroup::INITIAL,
        ruby: RubyGroup::INITIAL,
        paints: false,
        decorates: false,
    };
}

/// How many styles the memo holds before it starts again.
///
/// It is more than any bench document gives (1 to 6), and few enough that
/// the memo stays small in a document whose styles do not repeat.
const MOST: usize = 128;

define_id! {
    /// The id of a style the memo holds.
    struct MemoId(u16);
}

/// One style the memo holds, and the text facts it was lowered into.
struct MemoEntry {
    key: StyleKey,
    text: TextFactsId,
}

/// The builder's memo of lowered styles (see the module documentation).
pub(super) struct StyleMemo {
    entries: Table<MemoId, MemoEntry>,
    /// How a held style is found. It is the memo's own lookup, apart from
    /// the content's.
    lookup: Lookup,
}

impl Default for StyleMemo {
    /// Creates an empty memo, allocating nothing.
    fn default() -> Self {
        Self {
            entries: Table::new(),
            lookup: Lookup::new(),
        }
    }
}

impl StyleMemo {
    /// Forgets every style, keeping the capacity. Each build calls it,
    /// since each build has its own text facts.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.lookup.clear();
    }

    /// Returns the text facts `key` was lowered into in this build.
    ///
    /// Otherwise it calls `lower`, and holds a `Some` result for the next
    /// time.
    pub(super) fn text(
        &mut self,
        key: &StyleKey,
        lower: impl FnOnce() -> Option<TextFactsId>,
    ) -> Option<TextFactsId> {
        let entries = &self.entries;
        let found = self.lookup.find(
            entries.len(),
            || hash_one(key),
            |id: MemoId| entries.get(id).is_some_and(|entry| entry.key == *key),
        );
        let hash = match found {
            Ok(id) => return entries.get(id).map(|entry| entry.text),
            Err(hash) => hash,
        };
        let text = lower()?;
        if self.entries.len() >= MOST {
            self.clear();
        }
        let entry = MemoEntry { key: *key, text };
        if let Some(id) = self.entries.push(entry) {
            let Self { entries, lookup } = self;
            lookup.add(id, hash, |held| {
                entries.get(held).map_or(0, |entry| hash_one(&entry.key))
            });
        }
        Some(text)
    }
}

heap_bytes! {
    StyleMemo { entries, lookup }
}
