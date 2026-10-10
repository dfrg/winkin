//! Letter- and word-spacing: the room added after a cluster.
//!
//! The room depends only on the cluster, its style and the build's
//! [`WordSpacingRule`], so it goes into the prefix before any width is known.
//! [`LetterWordSpacing::after`] decides it. The measure scan adds it to the
//! prefix, and the breaker adds it to a line edge it reshapes with the same
//! rule, so a reshaped piece is spaced exactly as the paragraph was.
//!
//! The rules follow Chrome's `ShapeResultSpacing::ComputeSpacing`:
//! - `letter-spacing` goes after every cluster that draws, once per ligature.
//!   Zero-width characters, U+FFFC and tabs take none.
//! - No letter-spacing goes after a cluster of a cursive script, as CSS Text
//!   3, section 7.2.1 allows and Chrome 153 does (probe `joining`).
//! - `word-spacing` goes after U+0020 and U+00A0, but not after a U+0020
//!   that starts the block's text unless the block keeps its spaces. With
//!   [`WordSeparators`](crate::config::WordSpacing::WordSeparators) it goes after every separator
//!   in CSS Text 3's list, the first included.
//! - A percentage of either is of the style's own computed font size, as in
//!   CSS Text 4 and Chrome 153, not of the space's advance.
//!
//! Both round to 16.16, as Chrome's `TextRunLayoutUnit` does, and saturate.

use crate::config::WordSpacing;
use crate::data::Id;
use crate::stages::analysis::{Analysis, ClusterClass, ClusterId};
use crate::stages::content::{Content, Facts, NodeId, ShapingFactsId, VariantText};
use crate::style::FirstLineVariant;
use crate::unicode::{self, ScriptId};
use crate::unit::TextUnit;

/// The scripts CSS Text 3 calls cursive, whose letters take no spacing
/// between them.
const CURSIVE: [[u8; 4]; 7] = [
    *b"Arab", *b"Rohg", *b"Mand", *b"Mong", *b"Nkoo", *b"Phag", *b"Syrc",
];

/// Which clusters of one build's text take `word-spacing`: the separators
/// the context's [`Config::word_spacing`](crate::config::Config::word_spacing)
/// names, and whether a U+0020 that starts the text takes it.
///
/// It is worked out once a build, from the config and the block's own
/// `white-space-collapse`, and kept in the measurements. The breaker then
/// spaces a reshaped edge by the rule the prefix was built with, even if the
/// context's config has changed since.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct WordSpacingRule {
    /// CSS Text's list of separators, where Chrome's are U+0020 and U+00A0.
    css_separators: bool,
    /// A U+0020 starting the text takes word-spacing.
    first_space: bool,
}

impl WordSpacingRule {
    /// Chrome's rule in a block that collapses its spaces, and the default
    /// of empty measurements.
    pub(super) const CHROME: Self = Self {
        css_separators: false,
        first_space: false,
    };

    /// Returns the rule `choice` makes for `content`.
    ///
    /// `Css` spaces every separator. `Chrome` spaces U+0020 and U+00A0, and a
    /// space starting the text only where the block keeps its spaces, as
    /// Chrome's `allow_word_spacing_anywhere_` is the block's
    /// `ShouldPreserveWhiteSpaces`. `preserve-spaces`, which Chrome does not
    /// parse, keeps spaces as `preserve` does, and counts.
    pub(super) fn new(choice: WordSpacing, content: &Content) -> Self {
        match choice {
            WordSpacing::WordSeparators => Self {
                css_separators: true,
                first_space: true,
            },
            WordSpacing::SpaceAndNoBreakSpace => {
                // The block's own text's facts.
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

    /// Whether a cluster of `class` whose text is `text` takes
    /// word-spacing, `first` where it starts the text.
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

/// What one style's text adds after its clusters: its letter- and
/// word-spacing on glyph geometry's grid, and which clusters take the
/// word-spacing.
///
/// Letter- and word-spacing are shaping properties, so one value covers a
/// shaping run, a reshaped piece of a line, or a piece of a ruby annotation.
/// Each caller resolves it once ([`from_shaping`](Self::from_shaping)) and
/// asks it of each cluster ([`after_cluster`](Self::after_cluster)).
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct LetterWordSpacing {
    letter: TextUnit,
    word: TextUnit,
    words: WordSpacingRule,
}

impl LetterWordSpacing {
    /// Nothing either way: plain text's.
    pub(super) const NONE: Self = Self {
        letter: TextUnit::from_raw(0),
        word: TextUnit::from_raw(0),
        words: WordSpacingRule::CHROME,
    };

    /// Returns the spacing of text that shapes as `shaping`.
    ///
    /// The letter- and word-spacing come as the builder resolved them.
    /// Word-spacing goes where `words`, the rule the prefix was built with
    /// ([`Measured::word_spacing_rule`]), says. The measure scan asks this of
    /// each segment, and the breaker of each line edge it reshapes. Line
    /// layout and the readers of ruby annotation text ask it too, so all
    /// space text by one rule.
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

    /// Returns the room after `cluster`, its text read in `source`.
    ///
    /// It reads the cluster's class and text and calls
    /// [`after`](Self::after). The text's first cluster is the one at offset 0.
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
        self.after(class, text, continuation, cluster == ClusterId::new(0))
    }

    /// Whether it adds nothing after any cluster.
    pub(super) fn is_none(self) -> bool {
        self.letter.raw() == 0 && self.word.raw() == 0
    }

    /// Returns whether a cluster of `class` may take some of it, before its
    /// text is read.
    ///
    /// Where this is false, [`after`](Self::after) returns zero whatever the
    /// text.
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
    /// Letter-spacing applies unless the cluster is a ligature's
    /// `continuation`, draws nothing or is cursive. Word-spacing applies where
    /// the rule makes it a word separator. `first` marks the text's start.
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

/// Whether a cluster of `class` draws, and so takes letter-spacing after it:
/// a control character that forces a break among them, which CSS Text 3,
/// section 4 has drawn as any symbol is.
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
/// The first character with a script of its own decides. A mark is `Zinh`
/// and takes the script of what it sits on. A join-causing character (the
/// tatweel, U+200D) belongs to the word it is in.
fn is_cursive(text: &str) -> bool {
    // ASCII is Latin or Common, and nothing here: the common case in one
    // byte.
    if text.as_bytes().first().is_none_or(u8::is_ascii) {
        return false;
    }
    for ch in text.chars() {
        let props = unicode::core_props(ch);
        let script = unicode::script(props);
        if CURSIVE.contains(&script.tag()) || unicode::rare_props(ch).is_join_causing() {
            return true;
        }
        if script != ScriptId::INHERITED {
            return false;
        }
    }
    false
}

/// Returns whether a cluster whose text is `text` is a word separator of a
/// script that has its own (CSS Text 3, "word-separator characters").
///
/// These are the Ethiopic word space, the Aegean word separators, the
/// Ugaritic word divider and the Phoenician word separator. None is ASCII.
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
