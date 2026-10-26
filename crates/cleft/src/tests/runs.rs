//! Models of ICU4X's ways that line and word segmentation share: how it
//! segments a run with a dictionary, and the search for the causes that
//! explain a difference from it.

use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

use super::pinned_file;
use crate::dictionary::{RunScript, is_katakana};
use crate::{Dictionaries, Dictionary, DictionaryScript, GraphemeProps, GraphemeState};

/// ICU4X's ways with a run a dictionary segments; none set gives the crate's
/// own.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct RunQuirks {
    /// A boundary falls at the end of each part of a run, even before a
    /// space, a closing bracket or a mark, where the part's words reach it.
    pub(crate) slice_ends: bool,
    /// A word with no longer one after it ends where the dictionary says,
    /// even inside a cluster.
    pub(crate) leaves: bool,
    /// Where no word is found, everything read is one word, the missing
    /// character with it.
    pub(crate) misses: bool,
    /// At a part's end, after a word and the start of another that ends no
    /// word, the rest is one word.
    pub(crate) rest: bool,
}

/// Returns the breaks ICU4X's segmenters find in the run `run`, its
/// characters with their offsets, after its first character, with the run
/// parted by `script`, under `quirks`, and whether they cover the run to
/// its end: whether no word of its last part is left unsegmented.
///
/// With none of them, it gives the crate's own breaks: a word ends only
/// between grapheme clusters, a miss takes one cluster, a line breaks where
/// the script changes if either script has a dictionary and no mark
/// follows, and the run's end is left to the rules. Each quirk puts back one
/// of ICU4X's ways.
pub(crate) fn icu_run_breaks(
    run: &[(usize, char)],
    dictionaries: Option<&Dictionaries>,
    script: impl Fn(char) -> RunScript,
    quirks: RunQuirks,
    props: impl Fn(char) -> GraphemeProps,
    graphemes: GraphemeState,
    is_mark: impl Fn(char) -> bool,
) -> (Vec<usize>, bool) {
    let mut breaks = Vec::new();
    let mut covers = true;
    let mut from = 0;
    while from < run.len() {
        let slice_script = script(run[from].1);
        let to = run[from..]
            .iter()
            .position(|&(_, ch)| script(ch) != slice_script)
            .map_or(run.len(), |length| from + length);
        let slice = &run[from..to];
        let (first, last) = (slice[0].0, slice[slice.len() - 1]);
        let end = last.0 + last.1.len_utf8();
        let dictionary = slice_script
            .dictionary()
            .and_then(|script| dictionaries?.get(script));
        let reaches_end = match dictionary {
            Some(dictionary) => {
                let chars: Vec<(usize, char)> =
                    slice.iter().map(|&(at, ch)| (at - first, ch)).collect();
                let words = icu_words(dictionary, &chars, &props, graphemes, quirks);
                let length = end - first;
                breaks.extend(
                    words
                        .iter()
                        .filter(|&&at| at < length)
                        .map(|&at| at + first),
                );
                words.last() == Some(&length)
            }
            None => true,
        };
        let inside = to < run.len();
        covers = reaches_end;
        let breaks_at_end = if quirks.slice_ends {
            reaches_end
        } else {
            let has_dictionary = |script: RunScript| {
                script
                    .dictionary()
                    .and_then(|script| dictionaries?.get(script))
                    .is_some()
            };
            inside
                && !is_mark(run[to].1)
                && (has_dictionary(slice_script) || has_dictionary(script(run[to].1)))
        };
        if breaks_at_end {
            breaks.push(end);
        }
        from = to;
    }
    (breaks, covers)
}

/// Returns the word ends ICU4X's dictionary iterator yields in `chars`, a
/// part of a run with offsets from its start, in order, under `quirks`.
///
/// With the Chinese and Japanese dictionary, a run of at most eight katakana
/// after none is one word where the words found in it miss, as the crate's
/// search takes it.
fn icu_words(
    dictionary: &Dictionary,
    chars: &[(usize, char)],
    props: impl Fn(char) -> GraphemeProps,
    graphemes: GraphemeState,
    quirks: RunQuirks,
) -> Vec<usize> {
    let RunQuirks {
        leaves,
        misses,
        rest,
        ..
    } = quirks;
    let automaton = dictionary.automaton();
    let Some(&(last_at, last_ch)) = chars.last() else {
        return Vec::new();
    };
    let length = last_at + last_ch.len_utf8();
    // The grapheme boundaries of the slice, both ends included, as ICU4X's
    // grapheme iterator yields them.
    let mut boundaries = vec![0];
    let mut state = graphemes;
    for (index, &(at, ch)) in chars.iter().enumerate() {
        if state.advance(props(ch)) && index > 0 {
            boundaries.push(at);
        }
    }
    boundaries.push(length);
    let end_of = |index: usize| chars[index].0 + chars[index].1.len_utf8();
    // The index of the first character at or after `at`.
    let index_of = |at: usize| chars.partition_point(|&(offset, _)| offset < at);
    // The end of the first cluster after `start`.
    let first_cluster = |start: usize| {
        boundaries
            .iter()
            .copied()
            .find(|&at| at > start)
            .unwrap_or(length)
    };
    // The word at `position`: its end, the index of the next word's first
    // character, and whether it stands for a miss.
    let word = |position: usize| -> Option<(usize, usize, bool)> {
        let start = chars[position].0;
        let mut node = 0;
        let mut intermediate = 0;
        let mut rewind = None;
        let mut prefix = false;
        let mut found = None;
        let mut missed = false;
        let mut index = position;
        while index < chars.len() {
            let (_, ch) = chars[index];
            let end = end_of(index);
            index += 1;
            let next = dictionary
                .symbol(ch)
                .and_then(|symbol| automaton.step(node, symbol));
            let Some(next) = next else {
                if intermediate > 0 {
                    if let Some(at) = rewind {
                        index = at;
                    }
                    found = Some(intermediate);
                } else if misses {
                    found = Some(end);
                    missed = true;
                } else {
                    let cluster = first_cluster(start);
                    index = index_of(cluster);
                    found = Some(cluster);
                    missed = true;
                }
                break;
            };
            node = next;
            if !automaton.is_terminal(node) {
                prefix = true;
                continue;
            }
            if leaves && automaton.is_leaf(node) {
                found = Some(end);
                break;
            }
            if boundaries.contains(&end) {
                intermediate = end;
                rewind = Some(index);
            }
        }
        let found = match found {
            Some(found) => found,
            None if intermediate > 0 => {
                if !rest && let Some(at) = rewind {
                    index = at;
                }
                intermediate
            }
            None if prefix => {
                missed = true;
                if misses {
                    length
                } else {
                    let cluster = first_cluster(start);
                    index = index_of(cluster);
                    cluster
                }
            }
            None => return None,
        };
        Some((found, index, missed))
    };
    let chinese_japanese = dictionary.script() == DictionaryScript::ChineseJapanese;
    let mut words = Vec::new();
    let mut position = 0;
    while position < chars.len() {
        let katakana = |index: usize| chars.get(index).is_some_and(|&(_, ch)| is_katakana(ch));
        if chinese_japanese && katakana(position) && (position == 0 || !katakana(position - 1)) {
            let run_end = (position..=chars.len())
                .find(|&index| !katakana(index))
                .unwrap_or(chars.len());
            let end = chars.get(run_end).map_or(length, |&(at, _)| at);
            let mut at = position;
            let mut missed = false;
            while run_end - position <= 8 && at < run_end {
                let Some((_, next, miss)) = word(at) else {
                    break;
                };
                if miss {
                    missed = true;
                    break;
                }
                at = next;
            }
            if missed {
                words.push(end);
                position = run_end;
                if end == length {
                    break;
                }
                continue;
            }
        }
        let Some((found, index, _)) = word(position) else {
            break;
        };
        words.push(found);
        position = index;
        if found == length {
            break;
        }
    }
    words
}

/// Returns the fewest causes, up to `most`, that together restore ICU's
/// answer, joined with `+`, or `None` if no such set does.
pub(crate) fn causes_restoring<C: Copy + core::fmt::Debug>(
    candidates: &[C],
    most: usize,
    mut restores: impl FnMut(&[C]) -> bool,
) -> Option<String> {
    let mut chosen = Vec::new();
    for size in 1..=most.min(candidates.len()) {
        if choose(candidates, size, &mut chosen, &mut restores) {
            let names: Vec<String> = chosen
                .iter()
                .map(|cause| std::format!("{cause:?}"))
                .collect();
            return Some(names.join("+"));
        }
    }
    None
}

/// Tries every set of `size` of `candidates`, in order, after the causes in
/// `chosen`, and returns whether one restores ICU's answer, leaving it in
/// `chosen`.
fn choose<C: Copy>(
    candidates: &[C],
    size: usize,
    chosen: &mut Vec<C>,
    restores: &mut impl FnMut(&[C]) -> bool,
) -> bool {
    if size == 0 {
        return restores(chosen);
    }
    for (at, &cause) in candidates.iter().enumerate() {
        chosen.push(cause);
        if choose(&candidates[at + 1..], size - 1, chosen, restores) {
            return true;
        }
        chosen.pop();
    }
    false
}

/// Returns the words of the pinned ICU word list `name`, in list order.
pub(crate) fn list_words(name: &str) -> Option<Vec<String>> {
    let list = pinned_file("icu-78.1rc", &format!("{name}.txt"))?;
    let list = list.strip_prefix('\u{FEFF}').unwrap_or(&list);
    Some(
        list.lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| String::from(line.split('\t').next().unwrap_or_default().trim()))
            .collect(),
    )
}

/// The SA word lists, by script name.
pub(crate) const SA_LISTS: [&str; 4] = ["thaidict", "laodict", "khmerdict", "burmesedict"];

/// Returns a random generator, the same every run.
pub(crate) fn xorshift(mut seed: u64) -> impl FnMut() -> u64 {
    move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    }
}
