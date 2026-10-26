//! Dictionary tests:
//! - the compiled payloads validate and hold exactly ICU's word lists;
//! - ICU4X's tries hold the same words, and their sizes against the
//!   payloads';
//! - damaged bytes give an error, or a dictionary that reads without
//!   panicking;
//! - a set refuses a dictionary of another script;
//! - matching takes the longest word, ends words only between grapheme
//!   clusters, and allocates nothing.

use std::collections::BTreeSet;
use std::string::String;
use std::vec::Vec;
use std::{eprintln, format};

use icu_collections::char16trie::{Char16Trie, TrieResult};
use icu_provider::prelude::*;
use icu_segmenter::provider::{Baked, SegmenterDictionaryAutoV1, SegmenterDictionaryExtendedV1};

use super::*;
use crate::tests::pinned_file;
use crate::{GraphemeState, grapheme_props};

/// The folder of the pinned word lists.
const LISTS: &str = "icu-78.1rc";

/// Each payload: its bytes, its script, its word list and ICU4X's name for
/// it.
const PAYLOADS: [(&[u8], DictionaryScript, &str); 5] = [
    (
        include_bytes!("data/thai.dafsa"),
        DictionaryScript::Thai,
        "thaidict",
    ),
    (
        include_bytes!("data/lao.dafsa"),
        DictionaryScript::Lao,
        "laodict",
    ),
    (
        include_bytes!("data/khmer.dafsa"),
        DictionaryScript::Khmer,
        "khmerdict",
    ),
    (
        include_bytes!("data/myanmar.dafsa"),
        DictionaryScript::Myanmar,
        "burmesedict",
    ),
    (
        include_bytes!("data/cj.dafsa"),
        DictionaryScript::ChineseJapanese,
        "cjdict",
    ),
];

/// Returns the words of an ICU word list, as the generator reads them.
fn read_words(text: &str) -> BTreeSet<String> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| String::from(line.split('\t').next().unwrap_or_default().trim()))
        .collect()
}

/// Returns the number of strings `dictionary` accepts.
fn accepted(dictionary: &Dictionary) -> u64 {
    let records = dictionary.records_slice();
    let width = if dictionary.wide { 2 } else { 1 };
    let mut starts = Vec::new();
    let mut at = 0;
    while at < records.len() {
        starts.push(at);
        at = record_layout(records, at, width)
            .expect("a valid record")
            .end;
    }
    let mut below = std::collections::HashMap::new();
    for &start in starts.iter().rev() {
        let layout = record_layout(records, start, width).expect("a valid record");
        let mut count = u64::from(records[start] & TERMINAL != 0);
        for edge in 0..layout.count {
            count += below[&read_target(records, &layout, edge)];
        }
        below.insert(start, count);
    }
    below[&0]
}

/// Returns ICU4X's trie for the dictionary it names `name`.
fn icu_trie(name: &str) -> Char16Trie<'static> {
    let attributes = DataMarkerAttributes::try_from_str(name).expect("a marker attribute");
    let request = DataRequest {
        id: DataIdentifierBorrowed::for_marker_attributes(attributes),
        ..Default::default()
    };
    let trie = if name == "cjdict" {
        let response: DataResponse<SegmenterDictionaryAutoV1> =
            Baked.load(request).expect("ICU4X's dictionary");
        response.payload.get().trie_data.clone().into_owned()
    } else {
        let response: DataResponse<SegmenterDictionaryExtendedV1> =
            Baked.load(request).expect("ICU4X's dictionary");
        response.payload.get().trie_data.clone().into_owned()
    };
    Char16Trie::new(trie)
}

/// Returns whether ICU4X's trie holds `word`.
fn icu_contains(trie: &Char16Trie<'_>, word: &str) -> bool {
    let mut cursor = trie.iter();
    let mut result = TrieResult::NoMatch;
    for ch in word.chars() {
        result = cursor.next(ch);
        if result == TrieResult::NoMatch {
            return false;
        }
    }
    matches!(
        result,
        TrieResult::FinalValue(_) | TrieResult::Intermediate(_)
    )
}

#[test]
fn the_payloads_hold_exactly_icus_word_lists() {
    for (bytes, script, name) in PAYLOADS {
        let dictionary = Dictionary::new(bytes).expect("a valid payload");
        assert_eq!(dictionary.script(), script);
        assert_eq!(dictionary.source(), format!("ICU {LISTS} {name}.txt"));
        let Some(list) = pinned_file(LISTS, &format!("{name}.txt")) else {
            continue;
        };
        let words = read_words(&list);
        for word in &words {
            assert!(dictionary.contains(word), "{name}: {word}");
        }
        assert_eq!(accepted(&dictionary), words.len() as u64, "{name}");
    }
}

/// ICU4X 2.3's tries hold the same words: every word of the list, and none
/// of the strings one character from a word (a prefix, a suffix, the word
/// with its first character again) that the list lacks.
#[test]
fn icu4x_holds_the_same_words() {
    for (bytes, _, name) in PAYLOADS {
        let Some(list) = pinned_file(LISTS, &format!("{name}.txt")) else {
            continue;
        };
        let dictionary = Dictionary::new(bytes).expect("a valid payload");
        let trie = icu_trie(name);
        let words = read_words(&list);
        let mut probes = 0;
        for word in &words {
            assert!(icu_contains(&trie, word), "{name}: ICU lacks {word}");
            let chars: Vec<char> = word.chars().collect();
            let near = [
                chars[..chars.len() - 1].iter().collect::<String>(),
                chars[1..].iter().collect::<String>(),
                chars.iter().chain(&chars[..1]).collect::<String>(),
            ];
            for probe in near {
                if probe.is_empty() || words.contains(&probe) {
                    continue;
                }
                probes += 1;
                assert!(!dictionary.contains(&probe), "{name}: {probe}");
                assert!(!icu_contains(&trie, &probe), "{name}: ICU holds {probe}");
            }
        }
        eprintln!("{name}: {} words, {probes} near misses", words.len());
    }
}

/// The payloads are 28.9% smaller than ICU4X's tries: 2,688,894
/// bytes, and ICU4X's tries 3,780,186; the target is 18%.
#[test]
fn the_payloads_are_smaller_than_icus_tries() {
    let mut ours = 0;
    let mut icu = 0;
    let mut sizes = Vec::new();
    for (bytes, _, name) in PAYLOADS {
        let trie_bytes = {
            let attributes = DataMarkerAttributes::try_from_str(name).expect("an attribute");
            let request = DataRequest {
                id: DataIdentifierBorrowed::for_marker_attributes(attributes),
                ..Default::default()
            };
            if name == "cjdict" {
                let response: DataResponse<SegmenterDictionaryAutoV1> =
                    Baked.load(request).expect("ICU4X's dictionary");
                response.payload.get().trie_data.as_bytes().len()
            } else {
                let response: DataResponse<SegmenterDictionaryExtendedV1> =
                    Baked.load(request).expect("ICU4X's dictionary");
                response.payload.get().trie_data.as_bytes().len()
            }
        };
        sizes.push((name, bytes.len(), trie_bytes));
        ours += bytes.len();
        icu += trie_bytes;
    }
    eprintln!("{sizes:?}");
    assert_eq!(
        sizes,
        [
            ("thaidict", 134_295, 224_978),
            ("laodict", 172_579, 292_460),
            ("khmerdict", 481_782, 798_374),
            ("burmesedict", 257_313, 457_264),
            ("cjdict", 1_642_925, 2_007_110),
        ]
    );
    assert!(ours * 100 <= icu * 82, "{ours} against {icu}");
}

/// Mutated payloads are refused, or give a dictionary that reads every
/// string without panicking.
#[test]
fn any_bytes_give_a_dictionary_or_an_error() {
    let (bytes, _, _) = PAYLOADS[0];
    // xorshift64, so the run is the same every time.
    let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let probes = ["กินข้าว", "ภาษาไทย", "abc", "\u{E01}\u{E31}\u{E48}", ""];
    let mut accepted = 0;
    for round in 0..400 {
        let mut damaged = Vec::from(bytes);
        let changes = 1 + random() % 4;
        for _ in 0..changes {
            let at = (random() as usize) % damaged.len();
            // Damage the header and alphabet more often than the records.
            let at = if round % 3 == 0 { at % 200 } else { at };
            damaged[at] = random() as u8;
        }
        if round % 5 == 0 {
            let length = (random() as usize) % damaged.len();
            damaged.truncate(length);
        }
        if let Ok(dictionary) = Dictionary::new(damaged) {
            accepted += 1;
            for probe in probes {
                let _ = dictionary.contains(probe);
                let _ = next_word(
                    &dictionary,
                    probe.char_indices(),
                    grapheme_props,
                    GraphemeState::new(),
                );
            }
        }
    }
    eprintln!("{accepted} damaged payloads read as dictionaries");
    for bytes in [
        &[][..],
        b"WKDF",
        &bytes[..HEADER],
        &bytes[..bytes.len() - 1],
    ] {
        assert!(Dictionary::new(bytes).is_err());
    }
}

#[test]
fn a_set_refuses_a_dictionary_of_another_script() {
    let lao = Dictionary::new(PAYLOADS[1].0).expect("a valid payload");
    assert_eq!(
        Dictionaries::new().with_thai(lao.clone()).err(),
        Some(DictionaryError::WrongScript {
            expected: DictionaryScript::Thai,
            found: DictionaryScript::Lao,
        })
    );
    let set = Dictionaries::new().with_lao(lao).expect("a Lao slot");
    assert!(set.get(DictionaryScript::Lao).is_some());
    assert!(set.get(DictionaryScript::Thai).is_none());
    assert!(Dictionaries::new().is_empty());
}

/// Returns where the words of `text` start after the first, matching from
/// each word's start.
fn words(dictionary: &Dictionary, text: &str) -> Vec<usize> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(next) = next_word(
        dictionary,
        text[at..]
            .char_indices()
            .map(|(offset, ch)| (at + offset, ch)),
        grapheme_props,
        GraphemeState::new(),
    ) {
        found.push(next);
        at = next;
    }
    found
}

#[test]
fn matching_takes_the_longest_word_between_clusters() {
    let thai = Dictionary::new(PAYLOADS[0].0).expect("a valid payload");
    // กิน ข้าว: eat rice.
    assert_eq!(words(&thai, "กินข้าว"), [9]);
    // ภาษา ไทย: the longest word, not ภา.
    assert_eq!(words(&thai, "ภาษาไทย"), [12]);
    // A word the dictionary lacks is one cluster at a time.
    assert_eq!(words(&thai, "\u{E01}\u{E34}\u{E48}\u{E01}"), [9]);
    // ICU4X's dictionary ends กก before the tone mark; the cluster กั่
    // keeps it, and the word is ก.
    let text = "กกั่";
    assert!(thai.contains("กก"));
    assert_eq!(words(&thai, text), [3]);
}

#[test]
fn matching_allocates_nothing() {
    let thai = Dictionary::new(PAYLOADS[0].0).expect("a valid payload");
    let text: String = "ภาษาไทยเป็นภาษาราชการของประเทศไทย".repeat(20);
    let allocations = testing::allocator::count_allocations(|| {
        let mut at = 0;
        let mut count = 0;
        while let Some(next) = next_word(
            &thai,
            text[at..]
                .char_indices()
                .map(|(offset, ch)| (at + offset, ch)),
            grapheme_props,
            GraphemeState::new(),
        ) {
            at = next;
            count += 1;
        }
        core::hint::black_box(count);
    });
    assert_eq!(allocations, 0);
}
