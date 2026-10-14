//! The builder's memo of lowered styles, and the style keys it uses.
//!
//! Most styles repeat box after box; the memo maps each to the facts it
//! lowered into, so it is lowered once per build.
//!
//! - **Keys.** A [`StyleKey`] is a [`ComputedStyle`](crate::ComputedStyle)
//!   with its lists interned and named by id
//!   ([`Lists::key`](super::Lists::key)). [`FontKey`] and [`TextKey`] are
//!   the two groups that borrow lists. Keys compare and hash by their bits
//!   (`style::same`), so the memo compares list ids, never list contents.
//!   Only the builder holds keys; the content keeps the facts.
//! - **Lifetime.** The memo lives in the context's content scratch. It is
//!   cleared at each build, since it maps to that build's ids, and once it
//!   holds [`MOST`] styles. Clearing keeps its capacity, so a warm build
//!   allocates nothing.
//! - **Node kind.** Box facts also depend on the node's kind. An entry
//!   holds those for the last kind lowered; a node of another kind lowers
//!   them again. The block's styles are lowered once per build and are not
//!   held.

use super::NodeKind;
use super::facts::{BoxFactsId, TextFactsId};
use super::lists::{FeatureSettingsId, HyphenStringId, LanguageId, Lookup, VariationSettingsId};
use crate::data::{Table, define_id, hash_one, heap_bytes};
use crate::stages::fonts::NamedListId;
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
    /// It holds the group's values as given. It is part of a style's memo
    /// key, and a [`FontRequest`](super::FontRequest) holds it.
    pub(crate) struct FontKey {
        /// `font-family`.
        pub(crate) families: NamedListId,
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
        NamedListId::INITIAL,
        FeatureSettingsId::EMPTY,
        VariationSettingsId::EMPTY,
    );

    /// Creates the key of `group`, whose lists the build's tables interned
    /// as these ids.
    pub(super) const fn new(
        group: &FontGroup<'_>,
        families: NamedListId,
        features: FeatureSettingsId,
        variations: VariationSettingsId,
    ) -> Self {
        // Every field is named, so a field added to the group fails to
        // compile here.
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
    /// A numeric `line-height` multiplies it, synthesized small capitals
    /// are drawn at 0.7 of it, and font selection derives the used size
    /// from it.
    pub(crate) fn computed_size(&self) -> f32 {
        sanitized_font_size(self.size)
    }
}

style_struct! {
    /// A [`TextGroup`] with its language and hyphenation string named by id.
    ///
    /// It holds the group's values as given. It is part of a style's memo
    /// key.
    pub(super) struct TextKey {
        /// The content language; `und` where the style sets none.
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
        pub(super) letter_spacing: LengthPercentage,
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

    /// Creates the key of `group`, whose language and hyphenation string
    /// the build's tables interned as these ids.
    pub(super) const fn new(
        group: &TextGroup<'_>,
        language: LanguageId,
        hyphenate_character: Option<HyphenStringId>,
    ) -> Self {
        // Every field is named, so a field added to the group fails to
        // compile here.
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

    /// Whether spaces, tabs and other space separators ending a line hang.
    ///
    /// They hang where white space collapses, and where it is kept and wraps
    /// (`pre-wrap`). Under `break-spaces` they wrap as content. Where white
    /// space is kept and doesn't wrap (`pre`) they overflow as content, as
    /// in Blink's `ComputeTrailingSpaceWidth`. Chrome doesn't parse
    /// `preserve-spaces`, so it counts as `preserve`.
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
    /// True for kept white space that wraps (`pre-wrap`), before a forced
    /// break or at the block's end. Such white space counts in max-content
    /// but not min-content, per CSS Text 3. Collapsible white space there
    /// hangs unconditionally, as at any line's end.
    pub(super) fn white_space_hangs_conditionally(&self) -> bool {
        self.white_space_collapse.keeps_spaces() && self.white_space_hangs()
    }
}

style_struct! {
    /// A style as the writer lowers it, and the memo's key.
    ///
    /// It holds the style's groups as given, with the font and text groups'
    /// lists named by their interned ids. It compares and hashes by its
    /// bits.
    pub(super) struct StyleKey {
        pub(super) font: FontKey,
        pub(super) text: TextKey,
        pub(super) line: LineGroup,
        pub(super) bidi: BidiGroup,
        pub(super) orientation: OrientationGroup,
        pub(super) edges: EdgesGroup,
        pub(super) ruby: RubyGroup,
        pub(super) paints: bool,
        pub(super) outlines: bool,
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
        outlines: false,
        decorates: false,
    };
}

/// How many styles the memo holds before it clears.
///
/// It is more than any bench document uses (1 to 6), and few enough that
/// the memo stays small where styles do not repeat.
const MOST: usize = 128;

define_id! {
    /// Indexes the memo's entries.
    struct MemoId(u16);
}

/// One style the memo holds, and the facts it lowered into.
struct MemoEntry {
    key: StyleKey,
    text: TextFactsId,
    /// The kind of node its box facts were lowered for.
    kind: NodeKind,
    /// The box facts, or `None` where their table was full.
    box_: Option<BoxFactsId>,
}

/// The builder's memo of lowered styles (see the module documentation).
pub(super) struct StyleMemo {
    entries: Table<MemoId, MemoEntry>,
    /// The hash lookup over `entries`, separate from the lists' lookup.
    lookup: Lookup,
}

impl StyleMemo {
    /// Creates an empty memo, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            entries: Table::new(),
            lookup: Lookup::new(),
        }
    }

    /// Forgets every style, keeping the capacity. Each build calls it,
    /// since each build has its own fact ids.
    pub(super) fn clear(&mut self) {
        self.entries.clear();
        self.lookup.clear();
    }

    /// Returns the text and box facts `key` lowers into for a node of
    /// `kind`. Each is `None` where its table was full.
    ///
    /// A new style calls `lower_text` and `lower_box` with `cx` and is held
    /// for next time, unless its text facts did not fit. A style held for
    /// another kind calls only `lower_box`, and holds the new box facts in
    /// place of the old.
    pub(super) fn facts<C>(
        &mut self,
        cx: &mut C,
        key: &StyleKey,
        kind: NodeKind,
        lower_text: impl FnOnce(&mut C) -> Option<TextFactsId>,
        lower_box: impl FnOnce(&mut C) -> Option<BoxFactsId>,
    ) -> (Option<TextFactsId>, Option<BoxFactsId>) {
        let entries = &self.entries;
        let found = self.lookup.find(
            entries.len(),
            || hash_one(key),
            |id: MemoId| entries.get(id).is_some_and(|entry| entry.key == *key),
        );
        let hash = match found {
            Ok(id) => {
                let Some(entry) = self.entries.get_mut(id) else {
                    return (lower_text(cx), lower_box(cx));
                };
                if entry.kind != kind {
                    entry.kind = kind;
                    entry.box_ = lower_box(cx);
                }
                return (Some(entry.text), entry.box_);
            }
            Err(hash) => hash,
        };
        let text = lower_text(cx);
        let box_ = lower_box(cx);
        let Some(text) = text else {
            return (None, box_);
        };
        if self.entries.len() >= MOST {
            self.clear();
        }
        let entry = MemoEntry {
            key: *key,
            text,
            kind,
            box_,
        };
        if let Some(id) = self.entries.push(entry) {
            let Self { entries, lookup } = self;
            lookup.add(id, hash, |held| {
                entries.get(held).map_or(0, |entry| hash_one(&entry.key))
            });
        }
        (Some(text), box_)
    }
}

heap_bytes! {
    StyleMemo { entries, lookup }
}
