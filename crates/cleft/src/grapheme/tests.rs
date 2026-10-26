//! Grapheme segmentation tests:
//! - Unicode 18.0's GraphemeBreakTest.txt, over UTF-8 and UTF-16, and from
//!   every boundary;
//! - Unicode 17.0's file under the 17.0 data and GB9c;
//! - the differential against ICU4X: under 17.0 data and rules, every
//!   sequence of up to four property classes and the segmentation corpora
//!   agree exactly; under 18.0, each difference is counted by its cause;
//! - streams whose offsets do not increase, and the state's size.

use std::string::String;
use std::vec;
use std::vec::Vec;

use icu_segmenter::GraphemeClusterSegmenter;

use super::*;
use crate::tests::{
    GraphemeVersion as VersionProps, ICU_VERSION, TABLES_VERSION, break_test_cases as cases,
    pinned_file,
};
use crate::{Utf8Scalars, Utf16Scalars};

/// Returns the text of `version`'s GraphemeBreakTest.txt, or `None` where
/// it is absent, as in a packaged crate.
fn test_file(version: &str) -> Option<String> {
    pinned_file(version, "GraphemeBreakTest.txt")
}

/// Returns the boundaries of `text` that `props` and a scan from `state`
/// find, both ends included, as ICU yields them.
fn boundaries(
    text: &str,
    state: GraphemeState,
    props: impl Fn(char) -> GraphemeProps,
) -> Vec<usize> {
    let mut state = state;
    let mut found: Vec<usize> = text
        .char_indices()
        .filter(|&(_, ch)| state.advance(props(ch)))
        .map(|(at, _)| at)
        .collect();
    if !text.is_empty() {
        found.push(text.len());
    }
    if found.is_empty() {
        found.push(0);
    }
    found
}

/// Returns the boundaries of `text` the crate's iterator finds over UTF-8.
fn utf8_boundaries(text: &str) -> Vec<usize> {
    let mut found: Vec<usize> = Graphemes::new(Utf8Scalars::new(text)).collect();
    found.push(text.len());
    found.dedup();
    found
}

#[test]
fn the_tables_are_the_pinned_files() {
    let Some(props) = VersionProps::new(TABLES_VERSION) else {
        return;
    };
    for ch in (0..=0x10_FFFF).filter_map(char::from_u32) {
        assert_eq!(grapheme_props(ch), props.get(ch), "U+{:04X}", ch as u32);
    }
}

#[test]
fn unicode_18_conformance_passes() {
    let Some(file) = test_file(TABLES_VERSION) else {
        return;
    };
    let cases = cases(&file);
    assert_eq!(cases.len(), 853);
    for (text, expected) in &cases {
        assert_eq!(&utf8_boundaries(text), expected, "{text:?}");
        assert_eq!(
            &boundaries(text, GraphemeState::new(), grapheme_props),
            expected,
            "{text:?}"
        );
        // The same boundaries in UTF-16 code units.
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut found: Vec<usize> = Graphemes::new(Utf16Scalars::new(&units)).collect();
        found.push(units.len());
        found.dedup();
        let expected_units: Vec<usize> = expected
            .iter()
            .map(|&at| text[..at].encode_utf16().count())
            .collect();
        assert_eq!(found, expected_units, "{text:?} in UTF-16");
        // A scan from any boundary finds the boundaries after it.
        for &start in expected {
            let from: Vec<usize> = Graphemes::new(Utf8Scalars::with_start(text, start)).collect();
            let after: Vec<usize> = expected
                .iter()
                .copied()
                .filter(|&at| at >= start && at < text.len())
                .collect();
            assert_eq!(from, after, "{text:?} from {start}");
        }
    }
}

#[test]
fn unicode_17_conformance_passes_under_17_data_and_rules() {
    let (Some(file), Some(props)) = (test_file(ICU_VERSION), VersionProps::new(ICU_VERSION)) else {
        return;
    };
    let cases = cases(&file);
    assert_eq!(cases.len(), 766);
    for (text, expected) in &cases {
        let found = boundaries(text, GraphemeState::with_unicode_17_conjuncts(), |ch| {
            props.get(ch)
        });
        assert_eq!(&found, expected, "{text:?}");
    }
}

/// Returns a character of each combination of Grapheme_Cluster_Break,
/// Indic_Conjunct_Break and Extended_Pictographic that `props` gives some
/// character: the first whose combination `other` shares, or else the first.
fn representatives(props: &VersionProps, other: &VersionProps) -> Vec<char> {
    let all = || (0..=0x10_FFFF).filter_map(char::from_u32);
    let mut chars: Vec<char> = Vec::new();
    let stable = all().filter(|&ch| props.get(ch) == other.get(ch));
    for ch in stable.chain(all()) {
        if !chars.iter().any(|&known| props.get(known) == props.get(ch)) {
            chars.push(ch);
        }
    }
    chars
}

/// Calls `visit` with every string of one to four characters of `chars`.
fn sequences(chars: &[char], mut visit: impl FnMut(&str)) {
    let mut text = String::new();
    let n = chars.len();
    for len in 1..=4u32 {
        for mut index in 0..n.pow(len) {
            text.clear();
            for _ in 0..len {
                text.push(chars[index % n]);
                index /= n;
            }
            visit(&text);
        }
    }
}

/// Why the crate's 18.0 boundaries of a text differ from ICU's.
#[derive(Default, Debug, PartialEq, Eq)]
struct Differences {
    /// The text has a character whose properties changed in 18.0.
    data: usize,
    /// Only 18.0's GB9c differs: a linker joins a consonant with no
    /// consonant before it.
    conjunct_rule: usize,
}

impl Differences {
    /// Compares the crate's boundaries of `text` with ICU's, and counts a
    /// difference by its cause.
    ///
    /// Under 17.0's data and GB9c the two must agree exactly, so every
    /// difference is 18.0's.
    fn compare(&mut self, text: &str, older: &VersionProps, icu: &[usize]) {
        let unicode_17 = boundaries(text, GraphemeState::with_unicode_17_conjuncts(), |ch| {
            older.get(ch)
        });
        assert_eq!(unicode_17, icu, "{text:?} under 17.0");
        let found = utf8_boundaries(text);
        if found == icu {
            return;
        }
        if text.chars().any(|ch| grapheme_props(ch) != older.get(ch)) {
            self.data += 1;
        } else {
            let unicode_17_rules = boundaries(
                text,
                GraphemeState::with_unicode_17_conjuncts(),
                grapheme_props,
            );
            assert_eq!(unicode_17_rules, icu, "{text:?}: not 18.0's GB9c");
            self.conjunct_rule += 1;
        }
    }
}

#[test]
fn every_short_sequence_of_classes_agrees_with_icu_or_differs_by_18() {
    let (Some(older), Some(newer)) = (
        VersionProps::new(ICU_VERSION),
        VersionProps::new(TABLES_VERSION),
    ) else {
        return;
    };
    let icu = GraphemeClusterSegmenter::new();
    // The 17.0 combinations, then the one 18.0 adds: a linker that is
    // Grapheme_Cluster_Break Other.
    let mut chars = representatives(&older, &newer);
    assert_eq!(chars.len(), 18);
    for ch in representatives(&newer, &older) {
        if !chars.iter().any(|&known| newer.get(known) == newer.get(ch)) {
            chars.push(ch);
        }
    }
    assert_eq!(chars.len(), 19);
    assert_eq!(chars[18], '\u{1CF5}');
    let mut differences = Differences::default();
    let mut count = 0;
    sequences(&chars, |text| {
        let expected: Vec<usize> = icu.segment_str(text).collect();
        differences.compare(text, &older, &expected);
        count += 1;
    });
    assert_eq!(count, 19 + 19 * 19 + 19 * 19 * 19 + 19 * 19 * 19 * 19);
    // The data differences are the sequences with U+1CF5 VEDIC SIGN
    // JIHVAMULIYA, Other and a linker in 18.0, Other alone in 17.0. The
    // rule differences have a linker and a consonant after it with no
    // consonant before.
    assert_eq!(
        differences,
        Differences {
            data: 1317,
            conjunct_rule: 1045
        }
    );
}

#[test]
fn the_segmentation_corpora_agree_with_icu() {
    let Some(older) = VersionProps::new(ICU_VERSION) else {
        return;
    };
    let icu = GraphemeClusterSegmenter::new();
    for corpus in testing::corpora::segmentation_corpora() {
        let text = corpus.text.as_str();
        let expected: Vec<usize> = icu.segment_str(text).collect();
        let unicode_17 = boundaries(text, GraphemeState::with_unicode_17_conjuncts(), |ch| {
            older.get(ch)
        });
        assert!(unicode_17 == expected, "{} under 17.0", corpus.name);
        assert!(utf8_boundaries(text) == expected, "{}", corpus.name);
    }
}

#[test]
fn a_stream_ends_where_its_offsets_stop_increasing() {
    let backwards = [(0, 'a'), (1, 'b'), (1, 'c'), (2, 'd')];
    let found: Vec<usize> = Graphemes::new(backwards.into_iter()).collect();
    assert_eq!(found, [0, 1]);
    let backwards = [(5, 'a'), (6, '\u{301}'), (2, 'b'), (9, 'c')];
    let found: Vec<usize> = Graphemes::new(backwards.into_iter()).collect();
    assert_eq!(found, [5]);
    // An offset that wraps ends the stream rather than looping.
    let wrapping = [(usize::MAX - 1, 'a'), (usize::MAX, 'b'), (0, 'c')];
    let mut graphemes = Graphemes::new(wrapping.into_iter());
    assert_eq!(graphemes.next(), Some(usize::MAX - 1));
    assert_eq!(graphemes.next(), Some(usize::MAX));
    assert_eq!(graphemes.next(), None);
    assert_eq!(graphemes.next(), None);
}

#[test]
fn a_stream_starting_off_a_boundary_is_empty_or_replaced() {
    assert_eq!(Utf8Scalars::with_start("añ", 2).next(), None);
    assert_eq!(Utf8Scalars::with_start("añ", 9).next(), None);
    let units = [0xD83D, 0xDE00];
    let chars: Vec<_> = Utf16Scalars::with_start(&units, 1).collect();
    assert_eq!(chars, [(1, char::REPLACEMENT_CHARACTER)]);
    assert_eq!(Utf16Scalars::with_start(&units, 9).next(), None);
    let lone = [0xDE00, 0xD83D];
    let chars: Vec<_> = Utf16Scalars::new(&lone).collect();
    assert_eq!(
        chars,
        vec![
            (0, char::REPLACEMENT_CHARACTER),
            (1, char::REPLACEMENT_CHARACTER)
        ]
    );
}

#[test]
fn the_state_keeps_its_size() {
    assert_eq!(size_of::<GraphemeState>(), 2);
    assert_eq!(size_of::<GraphemeProps>(), 1);
}
