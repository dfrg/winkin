//! Letter- and word-spacing: the room added after a cluster.
//!
//! The room depends only on the cluster, its style and the build's
//! [`WordSpacingRule`], so the prefix includes it. The breaker spaces a
//! reshaped line edge with the same [`LetterWordSpacing::after`].
//!
//! The rules follow Blink's `ShapeResultSpacing::ComputeSpacing`:
//! - `letter-spacing` goes after every cluster that draws, once per ligature.
//!   Zero-width characters, U+FFFC and tabs take none.
//! - A cluster of a cursive script takes no letter-spacing, as CSS Text 3,
//!   section 7.2.1 allows.
//! - `word-spacing` goes after U+0020 and U+00A0, but not after a U+0020
//!   that starts the text unless the block keeps its spaces.
//!   [`WordSeparators`](crate::config::WordSpacing::WordSeparators) uses CSS
//!   Text 3's separator list instead.
//! - A space's room goes after the marks set on it, which are a cluster of
//!   their own, as one shaped cluster holds both. Where a line starts
//!   at the marks, they take their own room.
//! - A percentage resolves against the computed font size (CSS Text 4).
//!
//! Both are 16.16 `TextUnit`s, as Blink's `TextRunLayoutUnit` is, and sums
//! saturate.

use crate::config::WordSpacing;
use crate::data::Id;
use crate::stages::analysis::{Analysis, ClusterClass, ClusterId, Clusters};
use crate::stages::content::{Content, Facts, FixedSpacing, NodeId, ShapingFactsId, VariantText};
use crate::style::FirstLineVariant;
use crate::unicode::{self, ScriptId};
use crate::unit::TextUnit;

/// The scripts CSS Text 3 calls cursive, which take no letter-spacing.
const CURSIVE: [[u8; 4]; 7] = [
    *b"Arab", *b"Rohg", *b"Mand", *b"Mong", *b"Nkoo", *b"Phag", *b"Syrc",
];

/// The rule for which clusters of one build's text take `word-spacing`.
///
/// Measurement computes it once a build from
/// [`Config::word_spacing`](crate::config::Config::word_spacing)
/// and the block's `white-space-collapse`, and stores it in
/// [`Measured`](super::Measured), so a reshaped edge follows the prefix's
/// rule even after the config changes.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct WordSpacingRule {
    /// Whether CSS Text's list of separators applies, rather than U+0020 and
    /// U+00A0 alone.
    css_separators: bool,
    /// Whether a U+0020 that starts the text takes word-spacing.
    first_space: bool,
}

impl WordSpacingRule {
    /// The U+0020 and U+00A0 rule in a block that collapses spaces; the empty
    /// default.
    pub(super) const CHROME: Self = Self {
        css_separators: false,
        first_space: false,
    };

    /// Returns the rule `choice` makes for `content`.
    ///
    /// Under `SpaceAndNoBreakSpace`, a U+0020 that starts the text takes
    /// spacing only where the block keeps its spaces, as Blink's
    /// `allow_word_spacing_anywhere_` follows `ShouldPreserveWhiteSpaces`.
    /// `preserve-spaces` keeps spaces.
    pub(super) fn new(choice: WordSpacing, content: &Content) -> Self {
        match choice {
            WordSpacing::WordSeparators => Self {
                css_separators: true,
                first_space: true,
            },
            WordSpacing::SpaceAndNoBreakSpace => {
                let block = content
                    .nodes
                    .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
                Self {
                    css_separators: false,
                    first_space: content.facts.text(block).collapse.keeps_spaces(),
                }
            }
        }
    }

    /// Returns whether a cluster of `class` with text `text` takes
    /// word-spacing; `first` marks a cluster that starts the text.
    #[inline]
    fn takes(self, class: ClusterClass, text: &str, first: bool) -> bool {
        match class {
            ClusterClass::Space => !first || self.first_space,
            ClusterClass::NoBreakSpace => text.starts_with('\u{A0}'),
            ClusterClass::Text => self.css_separators && is_script_separator(text),
            _ => false,
        }
    }
}

/// One style's letter- and word-spacing, in 16.16 `TextUnit`s, with the
/// rule for which clusters take the word-spacing.
///
/// Both are shaping properties, so one value covers a shaping run, a
/// reshaped line edge or a piece of ruby annotation.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct LetterWordSpacing {
    letter: TextUnit,
    word: TextUnit,
    words: WordSpacingRule,
}

impl LetterWordSpacing {
    /// No letter- or word-spacing.
    pub(super) const NONE: Self = Self {
        letter: TextUnit::from_raw(0),
        word: TextUnit::from_raw(0),
        words: WordSpacingRule::CHROME,
    };

    /// Returns the spacing of text that shapes as `shaping`.
    ///
    /// `words` is the rule the prefix was built with
    /// ([`Measured::word_spacing_rule`]), so every reader spaces alike.
    ///
    /// [`Measured::word_spacing_rule`]: super::Measured::word_spacing_rule
    pub(crate) fn from_shaping(
        facts: &Facts,
        shaping: ShapingFactsId,
        words: WordSpacingRule,
    ) -> Self {
        let shaping = facts.shaping(shaping);
        Self {
            letter: shaping.letter,
            word: shaping.word,
            words,
        }
    }

    /// Returns only the spacing of `shaping` set as fixed lengths, which
    /// `text-fit` leaves unscaled; the rest is zero.
    pub(crate) fn from_fixed(
        facts: &Facts,
        shaping: ShapingFactsId,
        words: WordSpacingRule,
    ) -> Self {
        let row = facts.shaping(shaping);
        let none = TextUnit::from_raw(0);
        let kept = |flag, spacing| {
            if row.fixed.contains(flag) {
                spacing
            } else {
                none
            }
        };
        Self {
            letter: kept(FixedSpacing::LETTER, row.letter),
            word: kept(FixedSpacing::WORD, row.word),
            words,
        }
    }

    /// Returns the room after `cluster`, its text read in `source`.
    ///
    /// As [`after`](Self::after); cluster 0 starts the text, and a cluster
    /// without a class counts as `Text`.
    #[inline]
    pub(crate) fn after_cluster(
        self,
        analysis: &Analysis,
        source: VariantText<'_>,
        cluster: ClusterId,
        continuation: bool,
    ) -> TextUnit {
        if self.is_none() {
            return TextUnit::from_raw(0);
        }
        let clusters = &analysis.clusters;
        let class = clusters.class(cluster).unwrap_or(ClusterClass::Text);
        let text = clusters.text(source, cluster);
        self.after_text(clusters, source, class, text, cluster, continuation)
    }

    /// Returns the room after `cluster` where it starts a line.
    ///
    /// As [`after_cluster`](Self::after_cluster), but the marks after a space
    /// take their own room and not the space's: they reshape alone there,
    /// and are spaced as the marks they are.
    pub(crate) fn after_line_start(
        self,
        analysis: &Analysis,
        source: VariantText<'_>,
        cluster: ClusterId,
        continuation: bool,
    ) -> TextUnit {
        if self.is_none() {
            return TextUnit::from_raw(0);
        }
        let clusters = &analysis.clusters;
        let class = clusters.class(cluster).unwrap_or(ClusterClass::Text);
        let text = clusters.text(source, cluster);
        self.after(class, text, continuation, cluster == ClusterId::new(0))
    }

    /// Returns the room after `cluster`, of `class` and with text `text` in
    /// `source`.
    ///
    /// As [`after`](Self::after), but a space's room goes after the marks
    /// set on it, which start a cluster of their own
    /// ([`Clusters::is_space_marks`]). One shaped cluster holds the space and
    /// its marks, and `ShapeResultSpacing` spaces the whole once,
    /// after the marks, as the space would be spaced.
    #[inline]
    pub(super) fn after_text(
        self,
        clusters: &Clusters,
        source: VariantText<'_>,
        class: ClusterClass,
        text: &str,
        cluster: ClusterId,
        continuation: bool,
    ) -> TextUnit {
        let first = cluster == ClusterId::new(0);
        match class {
            ClusterClass::Space => {
                let next = ClusterId::new(cluster.get() + 1);
                // Only marks of class `Text` take the room on.
                let marks = clusters.class(next) == Some(ClusterClass::Text)
                    && clusters
                        .first_char(source, next)
                        .is_some_and(|ch| clusters.is_space_marks(next, ch));
                if marks {
                    return TextUnit::from_raw(0);
                }
            }
            ClusterClass::Text
                if text
                    .chars()
                    .next()
                    .is_some_and(|ch| clusters.is_space_marks(cluster, ch)) =>
            {
                let space = ClusterId::new(cluster.get().saturating_sub(1));
                return self.after(ClusterClass::Space, " ", false, space == ClusterId::new(0));
            }
            _ => {}
        }
        self.after(class, text, continuation, first)
    }

    /// Returns whether it adds nothing after any cluster.
    pub(crate) fn is_none(self) -> bool {
        self.letter.raw() == 0 && self.word.raw() == 0
    }

    /// Returns whether a cluster of `class` may take any spacing, before its
    /// text is read. Where false, [`after`](Self::after) returns zero.
    #[inline]
    pub(super) fn may_space(self, class: ClusterClass) -> bool {
        (self.letter.raw() != 0 && takes_letter_spacing(class))
            || (self.word.raw() != 0
                && matches!(
                    class,
                    ClusterClass::Space | ClusterClass::NoBreakSpace | ClusterClass::Text
                ))
    }

    /// Returns the room after a cluster of `class` whose text is `text`.
    ///
    /// A ligature `continuation` takes nothing. Letter-spacing skips clusters
    /// that draw nothing or are cursive. `first` marks the text's first
    /// cluster.
    pub(super) fn after(
        self,
        class: ClusterClass,
        text: &str,
        continuation: bool,
        first: bool,
    ) -> TextUnit {
        if continuation {
            return TextUnit::from_raw(0);
        }
        let mut sum = 0i32;
        if self.letter.raw() != 0 && takes_letter_spacing(class) && !is_cursive(text) {
            sum = sum.saturating_add(self.letter.raw());
        }
        if self.word.raw() != 0 && self.words.takes(class, text, first) {
            sum = sum.saturating_add(self.word.raw());
        }
        TextUnit::from_raw(sum)
    }
}

/// Returns whether a cluster of `class` draws, and so takes letter-spacing.
/// A drawn separator (VT, FF, NEL) counts (CSS Text 3, section 4).
fn takes_letter_spacing(class: ClusterClass) -> bool {
    matches!(
        class,
        ClusterClass::Text
            | ClusterClass::Emoji
            | ClusterClass::Symbol
            | ClusterClass::Space
            | ClusterClass::NoBreakSpace
            | ClusterClass::OtherSpace
            | ClusterClass::DrawnSeparator
    )
}

/// Returns whether a cluster whose text is `text` is part of a word in a
/// cursive script.
///
/// The first character not of script `Zinh` decides. A join-causing
/// character (tatweel, U+200D) counts as cursive.
fn is_cursive(text: &str) -> bool {
    // ASCII is Latin or Common, never cursive: the common case in one byte.
    if text.as_bytes().first().is_none_or(u8::is_ascii) {
        return false;
    }
    for ch in text.chars() {
        let props = unicode::core_props(ch);
        let script = props.script();
        if CURSIVE.contains(&script.tag()) || unicode::rare_props(ch).is_join_causing() {
            return true;
        }
        if script != ScriptId::INHERITED {
            return false;
        }
    }
    false
}

/// Returns whether `text` starts with a script's own word separator (CSS
/// Text 3, "word-separator characters"): Ethiopic, Aegean, Ugaritic or
/// Phoenician. None is ASCII.
fn is_script_separator(text: &str) -> bool {
    if text.as_bytes().first().is_none_or(u8::is_ascii) {
        return false;
    }
    text.chars().next().is_some_and(|ch| {
        matches!(
            ch,
            '\u{1361}' | '\u{10100}' | '\u{10101}' | '\u{1039F}' | '\u{1091F}'
        )
    })
}
