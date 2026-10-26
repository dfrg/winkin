//! The longest dictionary word at a position, found by a forward scan.
//!
//! [`next_word`] reads a run's characters from a word's start and returns
//! where the next word starts, as ICU4X's dictionary segmenter does: the
//! longest word of the dictionary that the text starts with. Two things
//! differ:
//! - a word ends only at a grapheme cluster boundary, so a word never splits
//!   a cluster, even one the dictionary ends inside;
//! - where no word ends at a boundary, the word is one grapheme cluster,
//!   and the run's end is a miss like any other.
//!
//! The scan keeps one copy of the grapheme state and nothing else, so it
//! never allocates. A word's end is a cluster boundary, where the grapheme
//! state starts afresh, so the next word needs no state from this one.

use super::Dictionary;
use crate::{GraphemeProps, GraphemeState};

/// Returns the offset of the character that starts the next word, after the
/// longest word of `dictionary` that `chars` starts with, or `None` where
/// that word, or the cluster that stands for one, reaches the end of
/// `chars`.
///
/// `chars` holds the run's characters from the word's start on, with their
/// offsets, and ends where the run does. `props` gives each character's
/// grapheme properties, and `graphemes` is the grapheme state at a text's
/// start. A word ends only where a grapheme cluster starts; where none does,
/// the word is the first cluster.
pub(crate) fn next_word(
    dictionary: &Dictionary,
    chars: impl Iterator<Item = (usize, char)>,
    props: impl Fn(char) -> GraphemeProps,
    graphemes: GraphemeState,
) -> Option<usize> {
    longest_word(dictionary, chars, props, graphemes).0
}

/// Returns what [`next_word`] returns, and whether the word is one of the
/// dictionary's rather than a cluster that stands for a miss.
pub(crate) fn longest_word(
    dictionary: &Dictionary,
    chars: impl Iterator<Item = (usize, char)>,
    props: impl Fn(char) -> GraphemeProps,
    mut graphemes: GraphemeState,
) -> (Option<usize>, bool) {
    let automaton = dictionary.automaton();
    let mut state = 0;
    // Whether the scan is still on a path of the automaton.
    let mut matching = true;
    // Whether the characters read so far spell a word, which ends at the
    // next character if a cluster starts there.
    let mut word = false;
    // The start of the next word after the longest word found so far.
    let mut longest = None;
    // The start of the second grapheme cluster.
    let mut second_cluster = None;
    let mut first = true;
    for (offset, ch) in chars {
        let starts_cluster = graphemes.advance(props(ch));
        if !first && starts_cluster {
            if word {
                longest = Some(offset);
            }
            second_cluster.get_or_insert(offset);
        }
        first = false;
        word = false;
        if matching {
            match dictionary
                .symbol(ch)
                .and_then(|symbol| automaton.step(state, symbol))
            {
                Some(next) => {
                    state = next;
                    word = automaton.is_terminal(next);
                }
                None => matching = false,
            }
        }
        if !matching {
            // A miss: the longest word, or else the first cluster once it
            // is known.
            if longest.is_some() {
                return (longest, true);
            }
            if second_cluster.is_some() {
                return (second_cluster, false);
            }
        }
    }
    // The run ends: a word reaching its end ends there.
    if matching && word {
        return (None, true);
    }
    match longest {
        Some(longest) => (Some(longest), true),
        None => (second_cluster, false),
    }
}
