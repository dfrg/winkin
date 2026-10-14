//! Finds the extent of `::first-letter` text.
//!
//! The rules are CSS Pseudo-Elements 4, section 2.2.1, as in Blink's
//! `FirstLetterPseudoElement::FirstLetterLength`:
//!
//! - **Leading white space** is skipped where it collapses and included
//!   where it is kept, tabs and no-break spaces too (Blink's
//!   `IsSpaceForFirstLetter`).
//! - **Punctuation before the letter** (General_Category P) is included,
//!   and after it any Zs space except U+3000.
//! - **The letter** is one grapheme cluster of any kind (letter, digit,
//!   symbol), found by UAX #29 as the analysis finds clusters.
//! - **Punctuation after it** is included except Ps and Pd. A space between
//!   them is included only when punctuation follows it, and never U+0020,
//!   U+00A0 or U+3000.
//!
//! Each grapheme is classed by its first character, as in Blink. The text
//! is read as collapsing leaves it:
//! - a collapsible space, tab or segment break is a word space;
//! - a kept segment break or a character that always forces a break ends
//!   the first line and the search;
//! - white space `discard` removes is skipped.
//!
//! Chrome instead reads the source text and simulates collapsing, so a
//! segment break after an opening quote ends its search where a space
//! would not. CSS reads the text as drawn, as this does.
//!
//! **Across texts.** Punctuation ending a text leaves the letter to the
//! next text, as Blink's `LeadingPunctuationState` does. The box then holds
//! only the first text's part, as in Chrome; CSS allows this when the text
//! spans elements. A text of only white space leaves the search unchanged.
//!
//! **Chrome 153** follows CSS 2.1 instead: Ps, Pe, Pi, Pf and Po on both
//! sides and no space after leading punctuation, so `“ Once` has no first
//! letter and a leading dash counts as the letter. Blink's source follows
//! CSS Pseudo-Elements 4, as this does.

use icu_segmenter::GraphemeClusterSegmenter;

use crate::style::WhiteSpaceCollapse;
use crate::unicode::{self, FirstLetterClass};
use crate::work;

/// The part of the first letter in one text, in that text's bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum FirstLetterScan {
    /// Only white space; the search continues in the next text.
    Nothing,
    /// A forced break or a disallowed space came before any letter; there
    /// is no first letter.
    Ends,
    /// First-letter text in `start..end`.
    ///
    /// `found` is false when the text ended in leading punctuation, so the
    /// search continues in the next text.
    Text {
        start: usize,
        end: usize,
        found: bool,
    },
}

/// A grapheme as the search classes it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Seen {
    /// White space `discard` removes.
    Nothing,
    /// A collapsible space, tab or segment break, read as U+0020.
    Collapsible,
    /// A kept segment break or a character that always forces a break; the
    /// first line ends here.
    Break,
    /// A drawn character: its class, whether it is White_Space, and whether
    /// it is a word separator (U+0020 or U+00A0).
    Drawn {
        class: FirstLetterClass,
        white: bool,
        separator: bool,
    },
}

impl Seen {
    /// Classes `grapheme` in text with white space mode `mode`.
    fn new(grapheme: &str, mode: WhiteSpaceCollapse) -> Self {
        use WhiteSpaceCollapse as Ws;
        let Some(first) = grapheme.chars().next() else {
            return Self::Nothing;
        };
        // A kept space is U+0020, a word separator.
        let kept_space = Self::Drawn {
            class: FirstLetterClass::Space,
            white: true,
            separator: true,
        };
        match first {
            // A CRLF reads as its LF. A lone CR is a space, as collapsing
            // treats it.
            '\r' if grapheme.contains('\n') => Self::new("\n", mode),
            ' ' | '\t' | '\r' => match mode {
                Ws::Discard => Self::Nothing,
                Ws::Collapse | Ws::PreserveBreaks => Self::Collapsible,
                Ws::Preserve | Ws::BreakSpaces | Ws::PreserveSpaces if first == '\t' => {
                    Self::Drawn {
                        class: FirstLetterClass::Other,
                        white: true,
                        separator: false,
                    }
                }
                Ws::Preserve | Ws::BreakSpaces | Ws::PreserveSpaces => kept_space,
            },
            '\n' => match mode {
                Ws::Discard => Self::Nothing,
                Ws::Collapse => Self::Collapsible,
                Ws::PreserveSpaces => kept_space,
                Ws::Preserve | Ws::BreakSpaces | Ws::PreserveBreaks => Self::Break,
            },
            // VT, FF, NEL, U+2028 and U+2029 break the line in every mode.
            '\u{B}' | '\u{C}' | '\u{85}' | '\u{2028}' | '\u{2029}' => Self::Break,
            _ => Self::Drawn {
                class: unicode::rare_props(first).first_letter(),
                white: unicode::core_props(first).is_white_space(),
                separator: first == '\u{A0}',
            },
        }
    }
}

impl FirstLetterScan {
    /// Finds the first-letter text in `text`, read a grapheme at a time.
    ///
    /// `after_punctuation` means an earlier text ended in leading
    /// punctuation and the letter is still to be found. A text ending in
    /// leading punctuation gives `Text { found: false, .. }`. A space after
    /// the letter with no punctuation following it in this text is left
    /// out, since Blink ignores trailing punctuation in later text nodes.
    pub(super) fn new(text: &str, mode: WhiteSpaceCollapse, after_punctuation: bool) -> Self {
        /// The scan's position relative to the letter.
        #[derive(Copy, Clone)]
        enum Phase {
            Leading,
            Punctuation,
            /// After the letter; holds the end of the text taken so far.
            Trailing(usize),
        }
        let mut phase = if after_punctuation {
            Phase::Punctuation
        } else {
            Phase::Leading
        };
        let mut start = 0;
        let mut boundaries = GraphemeClusterSegmenter::new().segment_str(text);
        // The segmenter yields 0 first.
        let mut at = boundaries.next().unwrap_or(0);
        for next in boundaries {
            work::step();
            let seen = Seen::new(text.get(at..next).unwrap_or_default(), mode);
            at = next;
            phase = match (phase, seen) {
                (_, Seen::Nothing) => phase,
                (Phase::Leading | Phase::Punctuation, Seen::Break) => {
                    return Self::Ends;
                }
                (Phase::Leading, Seen::Collapsible) => {
                    start = next;
                    Phase::Leading
                }
                (Phase::Leading, Seen::Drawn { white: true, .. }) => Phase::Leading,
                (Phase::Leading, Seen::Drawn { class, .. }) => match class {
                    FirstLetterClass::Opening | FirstLetterClass::Punctuation => Phase::Punctuation,
                    _ => Phase::Trailing(next),
                },
                (
                    Phase::Punctuation,
                    Seen::Collapsible
                    | Seen::Drawn {
                        class:
                            FirstLetterClass::Space
                            | FirstLetterClass::Opening
                            | FirstLetterClass::Punctuation,
                        ..
                    },
                ) => Phase::Punctuation,
                (Phase::Punctuation, Seen::Drawn { white: true, .. }) => {
                    return Self::Ends;
                }
                (Phase::Punctuation, Seen::Drawn { .. }) => Phase::Trailing(next),
                (
                    Phase::Trailing(_),
                    Seen::Drawn {
                        class: FirstLetterClass::Punctuation,
                        ..
                    },
                ) => Phase::Trailing(next),
                // The space joins only if punctuation follows, so the end
                // stays before it.
                (
                    Phase::Trailing(_),
                    Seen::Drawn {
                        class: FirstLetterClass::Space,
                        separator: false,
                        ..
                    },
                ) => phase,
                (Phase::Trailing(end), _) => {
                    return Self::Text {
                        start,
                        end,
                        found: true,
                    };
                }
            };
        }
        match phase {
            Phase::Leading => Self::Nothing,
            Phase::Punctuation => Self::Text {
                start,
                end: text.len(),
                found: false,
            },
            Phase::Trailing(end) => Self::Text {
                start,
                end,
                found: true,
            },
        }
    }
}
