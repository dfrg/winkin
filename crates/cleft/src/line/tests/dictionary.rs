//! Line breaking in SA runs with dictionaries:
//! - examples, with and without dictionaries;
//! - the model of ICU4X's SA runs with no switch on gives the crate's
//!   breaks, so each switch puts back one of ICU4X's ways alone;
//! - the differential against ICU4X's dictionary segmenter, under Unicode
//!   15.1's data and rules: on the SA corpora, on random runs of dictionary
//!   words in each script, and at seams with other scripts, marks, digits
//!   and punctuation, each difference counted by its cause;
//! - restarts at every break, and scans that allocate nothing.

use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;
use std::{eprintln, format};

use super::*;
use crate::tests::{SA_LISTS, dictionaries, list_words, xorshift};

/// Returns `text` with a `|` at each break opportunity under `options` and
/// `dictionaries`.
fn marked(text: &str, options: LineOptions, dictionaries: &Dictionaries) -> String {
    let breaks: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), options)
        .with_dictionaries(dictionaries)
        .collect();
    let mut marked = String::new();
    for (at, ch) in text.char_indices() {
        if breaks.contains(&at) {
            marked.push('|');
        }
        marked.push(ch);
    }
    marked
}

#[test]
fn sa_runs_break_at_the_words_of_their_dictionaries() {
    let all = dictionaries();
    let none = Dictionaries::new();
    let options = LineOptions::new();
    let cases = [
        // Thai: ภาษา ไทย เป็น ภาษา.
        ("ภาษาไทยเป็นภาษา", "ภาษา|ไทย|เป็น|ภาษา", "ภาษาไทยเป็นภาษา"),
        // A space and a closing parenthesis hold as UAX #14 says: no break
        // before either.
        ("(ภาษาไทย) ดี", "(ภาษา|ไทย) |ดี", "(ภาษาไทย) |ดี"),
        // Where the script changes inside a run, a line may break, with a
        // dictionary.
        ("ภาษาລາວ", "ภาษา|ລາວ", "ภาษาລາວ"),
        // Tai Tham has no dictionary: no break inside its run.
        (
            "\u{1A20}\u{1A21}\u{1A22}",
            "\u{1A20}\u{1A21}\u{1A22}",
            "\u{1A20}\u{1A21}\u{1A22}",
        ),
    ];
    for (text, with, without) in cases {
        assert_eq!(marked(text, options, &all), with);
        assert_eq!(marked(text, options, &none), without);
    }
    // `break-all` reads SA as ID.
    let break_all = LineOptions {
        word: LineWordOption::BreakAll,
        ..options
    };
    assert_eq!(marked("ภาษา", break_all, &all), "ภ|า|ษ|า");
}

/// One of ICU4X's ways with SA runs, which the differential puts back.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum RunCause {
    /// A line breaks at the end of each script's part of a run, even before
    /// a space.
    RunEnds,
    /// U+0EA3 is outside Lao.
    LaoLoLing,
    /// A word with no longer one after it ends inside a grapheme cluster.
    Leaves,
    /// A miss takes everything read, the missing character with it.
    Misses,
    /// After a word and the start of another at a part's end, the rest is
    /// one word.
    Rest,
}

impl RunCause {
    const ALL: [RunCause; 5] = [
        RunCause::RunEnds,
        RunCause::LaoLoLing,
        RunCause::Leaves,
        RunCause::Misses,
        RunCause::Rest,
    ];

    /// Returns the switch that puts the cause back.
    fn bits(self) -> u32 {
        Legacy::RUN_MODEL
            | match self {
                RunCause::RunEnds => Legacy::RUN_END_BREAKS,
                RunCause::LaoLoLing => Legacy::LAO_LO_LING,
                RunCause::Leaves => Legacy::DICTIONARY_LEAVES,
                RunCause::Misses => Legacy::DICTIONARY_MISSES,
                RunCause::Rest => Legacy::DICTIONARY_REST,
            }
    }
}

/// Returns the crate's breaks in `text` under `options`, `config` and
/// `dictionaries`, both ends included, as ICU yields them.
fn dictionary_breaks(
    text: &str,
    options: LineOptions,
    config: Config,
    dictionaries: &Dictionaries,
) -> Vec<usize> {
    with_ends(
        text,
        LineBreaks::with_config(Utf8Scalars::new(text), options, config)
            .with_dictionaries(dictionaries),
    )
}

/// The causes of one text set's differences under one option set, as counts
/// by cause.
type Tally = BTreeMap<String, usize>;

/// Returns, for each of `keys`, the differences from ICU4X's dictionary
/// segmenter on `texts` by cause, after checking that ICU's configuration
/// gives ICU's breaks on every text.
fn tally(
    texts: &[String],
    keys: &[Key],
    full: Config,
    dictionaries: &Dictionaries,
) -> BTreeMap<&'static str, Tally> {
    let base = Config {
        legacy: full.legacy & !Legacy::DICTIONARY_RUNS,
        ..full
    };
    let mut tallies = BTreeMap::new();
    for key in keys {
        let icu = Icu::new_dictionary(key.options);
        let mut tally = Tally::new();
        for text in texts {
            let expected = icu.breaks(text);
            assert_eq!(
                dictionary_breaks(text, key.options, full, dictionaries),
                expected,
                "{}: {text:?}: ICU's configuration is not ICU",
                key.name
            );
            if dictionary_breaks(text, key.options, base, dictionaries) == expected {
                continue;
            }
            let name = causes_restoring(&RunCause::ALL, RunCause::ALL.len(), |causes| {
                let legacy = causes
                    .iter()
                    .fold(base.legacy, |legacy, cause| legacy | cause.bits());
                let config = Config { legacy, ..base };
                dictionary_breaks(text, key.options, config, dictionaries) == expected
            })
            .unwrap_or_else(|| {
                eprintln!("{}: {text:?}: no cause", key.name);
                String::from("unexplained")
            });
            *tally.entry(name).or_default() += 1;
        }
        tallies.insert(key.name, tally);
    }
    tallies
}

/// Checks `tallies` against `pinned`, printing them where they differ.
fn assert_pinned(tallies: &BTreeMap<&'static str, Tally>, pinned: &[(&str, &[(&str, usize)])]) {
    let found: Vec<(String, Vec<(String, usize)>)> = tallies
        .iter()
        .map(|(name, tally)| {
            let causes = tally
                .iter()
                .map(|(cause, &count)| (cause.clone(), count))
                .collect();
            (String::from(*name), causes)
        })
        .collect();
    let expected: Vec<(String, Vec<(String, usize)>)> = pinned
        .iter()
        .map(|(name, causes)| {
            let causes = causes
                .iter()
                .map(|&(cause, count)| (String::from(cause), count))
                .collect();
            (String::from(*name), causes)
        })
        .collect();
    if found != expected {
        for (name, causes) in &found {
            let causes: Vec<String> = causes
                .iter()
                .map(|(cause, count)| format!("(\"{cause}\", {count})"))
                .collect();
            eprintln!("(\"{name}\", &[{}]),", causes.join(", "));
        }
    }
    assert_eq!(found, expected);
}

/// The option sets the dictionary differential runs under: the default, the
/// Japanese and Chinese ones, `keep-all`, `break-all` and `anywhere`.
fn dictionary_keys() -> Vec<Key> {
    let names = [
        "strict normal",
        "normal normal ja",
        "loose keep-all",
        "strict break-all",
        "anywhere",
    ];
    keys()
        .into_iter()
        .filter(|key| names.contains(&key.name))
        .collect()
}

/// The first 64 KiB of each SA corpus: Thai with spaces and in one run, Lao,
/// Khmer, Myanmar, and mixed scripts.
fn sa_corpora() -> Vec<String> {
    testing::corpora::segmentation_corpora()
        .into_iter()
        .filter(|corpus| {
            matches!(
                corpus.name,
                "Thai, spaced" | "Thai, one run" | "Lao" | "Khmer" | "Myanmar" | "mixed"
            )
        })
        .map(|corpus| {
            let mut length = corpus.text.len().min(1 << 16);
            while !corpus.text.is_char_boundary(length) {
                length -= 1;
            }
            String::from(&corpus.text[..length])
        })
        .collect()
}

#[test]
fn the_sa_corpora_agree_with_icu_or_differ_by_cause() {
    let Some(full) = icu_configuration() else {
        return;
    };
    let all = dictionaries();
    let tallies = tally(&sa_corpora(), &keys(), full, &all);
    assert_pinned(&tallies, CORPUS_DIFFERENCES);
}

/// The differences from ICU4X on the SA corpora, by option set and cause:
/// one text a corpus.
const CORPUS_DIFFERENCES: &[(&str, &[(&str, usize)])] = &[
    ("anywhere", &[]),
    ("loose break-all", &[]),
    ("loose break-all ja", &[]),
    (
        "loose keep-all",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    (
        "loose keep-all ja",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    (
        "loose normal",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    (
        "loose normal ja",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    ("normal break-all", &[]),
    ("normal break-all ja", &[]),
    (
        "normal keep-all",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    (
        "normal keep-all ja",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    (
        "normal normal",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    (
        "normal normal ja",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    ("strict break-all", &[]),
    (
        "strict keep-all",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
    (
        "strict normal",
        &[
            ("Misses", 1),
            ("RunEnds", 4),
            ("RunEnds+LaoLoLing+Misses+Rest", 1),
        ],
    ),
];

#[test]
fn random_runs_of_dictionary_words_agree_with_icu_or_differ_by_cause() {
    let Some(full) = icu_configuration() else {
        return;
    };
    let all = dictionaries();
    let mut random = xorshift(0x2545_F491_4F6C_DD1D);
    let mut texts = Vec::new();
    for name in SA_LISTS {
        let Some(words) = list_words(name) else {
            return;
        };
        for _ in 0..400 {
            let count = 2 + random() % 5;
            let text: String = (0..count)
                .map(|_| words[(random() as usize) % words.len()].as_str())
                .collect();
            texts.push(text);
        }
    }
    let tallies = tally(&texts, &dictionary_keys(), full, &all);
    assert_pinned(&tallies, RANDOM_DIFFERENCES);
}

/// The differences from ICU4X on 1,600 random runs of two to six dictionary
/// words, 400 a script, by option set and cause.
const RANDOM_DIFFERENCES: &[(&str, &[(&str, usize)])] = &[
    ("anywhere", &[]),
    (
        "loose keep-all",
        &[
            ("LaoLoLing", 10),
            ("LaoLoLing+Misses", 6),
            ("Leaves+Misses", 1),
            ("Misses", 4),
            ("RunEnds", 2),
            ("RunEnds+LaoLoLing", 5),
            ("RunEnds+LaoLoLing+Leaves+Misses+Rest", 1),
            ("RunEnds+LaoLoLing+Misses", 5),
            ("RunEnds+LaoLoLing+Misses+Rest", 17),
            ("RunEnds+LaoLoLing+Rest", 17),
        ],
    ),
    (
        "normal normal ja",
        &[
            ("LaoLoLing", 10),
            ("LaoLoLing+Misses", 6),
            ("Leaves+Misses", 1),
            ("Misses", 4),
            ("RunEnds", 2),
            ("RunEnds+LaoLoLing", 5),
            ("RunEnds+LaoLoLing+Leaves+Misses+Rest", 1),
            ("RunEnds+LaoLoLing+Misses", 5),
            ("RunEnds+LaoLoLing+Misses+Rest", 17),
            ("RunEnds+LaoLoLing+Rest", 17),
        ],
    ),
    ("strict break-all", &[]),
    (
        "strict normal",
        &[
            ("LaoLoLing", 10),
            ("LaoLoLing+Misses", 6),
            ("Leaves+Misses", 1),
            ("Misses", 4),
            ("RunEnds", 2),
            ("RunEnds+LaoLoLing", 5),
            ("RunEnds+LaoLoLing+Leaves+Misses+Rest", 1),
            ("RunEnds+LaoLoLing+Misses", 5),
            ("RunEnds+LaoLoLing+Misses+Rest", 17),
            ("RunEnds+LaoLoLing+Rest", 17),
        ],
    ),
];

/// What stands beside an SA run at a seam: letters, ideographs, digits,
/// marks and joiners, spaces, punctuation, and the run of another SA
/// script.
const SEAMS: [&str; 18] = [
    "a",
    "Zebra",
    "漢字",
    "かな",
    "12",
    "\u{0E51}",
    "\u{0301}",
    "\u{200D}",
    "\u{200B}",
    " ",
    "\u{A0}",
    ".",
    ")",
    "(",
    "\"",
    "-",
    "\u{0E31}",
    "\u{1A20}\u{1A21}",
];

#[test]
fn script_seams_agree_with_icu_or_differ_by_cause() {
    let Some(full) = icu_configuration() else {
        return;
    };
    let all = dictionaries();
    let mut random = xorshift(0x9E37_79B9_7F4A_7C15);
    let mut lists = Vec::new();
    for name in SA_LISTS {
        let Some(words) = list_words(name) else {
            return;
        };
        lists.push(words);
    }
    let mut texts = Vec::new();
    for words in &lists {
        for seam in SEAMS {
            for _ in 0..4 {
                let mut pick = || words[(random() as usize) % words.len()].clone();
                let (first, second) = (pick(), pick());
                texts.push(format!("{first}{seam}{second}"));
                texts.push(format!("{seam}{first}{second}"));
                texts.push(format!("{first}{second}{seam}"));
            }
        }
        // A run of each other SA script after this one's.
        for other in &lists {
            let first = &words[(random() as usize) % words.len()];
            let second = &other[(random() as usize) % other.len()];
            texts.push(format!("{first}{second}{first}"));
        }
    }
    let tallies = tally(&texts, &dictionary_keys(), full, &all);
    assert_pinned(&tallies, SEAM_DIFFERENCES);
}

/// The differences from ICU4X at seams, by option set and cause.
const SEAM_DIFFERENCES: &[(&str, &[(&str, usize)])] = &[
    ("anywhere", &[]),
    (
        "loose keep-all",
        &[
            ("LaoLoLing", 1),
            ("Leaves", 7),
            ("Misses", 2),
            ("RunEnds", 463),
            ("RunEnds+LaoLoLing", 2),
            ("RunEnds+LaoLoLing+Misses+Rest", 5),
            ("RunEnds+LaoLoLing+Rest", 4),
            ("RunEnds+Misses", 4),
        ],
    ),
    (
        "normal normal ja",
        &[
            ("LaoLoLing", 1),
            ("Leaves", 7),
            ("Misses", 2),
            ("RunEnds", 463),
            ("RunEnds+LaoLoLing", 2),
            ("RunEnds+LaoLoLing+Misses+Rest", 5),
            ("RunEnds+LaoLoLing+Rest", 4),
            ("RunEnds+Misses", 4),
        ],
    ),
    ("strict break-all", &[]),
    (
        "strict normal",
        &[
            ("LaoLoLing", 1),
            ("Leaves", 7),
            ("Misses", 2),
            ("RunEnds", 463),
            ("RunEnds+LaoLoLing", 2),
            ("RunEnds+LaoLoLing+Misses+Rest", 5),
            ("RunEnds+LaoLoLing+Rest", 4),
            ("RunEnds+Misses", 4),
        ],
    ),
];

/// With no switch on, the model of ICU4X's SA runs gives the crate's breaks:
/// so each switch puts back exactly one of ICU4X's ways.
#[test]
fn the_bare_run_model_gives_the_crates_breaks() {
    let all = dictionaries();
    let model = Config {
        legacy: Legacy::RUN_MODEL,
        ..Config::default()
    };
    let mut texts = sa_corpora();
    let mut random = xorshift(0x1234_5678_9ABC_DEF1);
    for name in SA_LISTS {
        let Some(words) = list_words(name) else {
            return;
        };
        for _ in 0..200 {
            let count = 2 + random() % 5;
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&words[(random() as usize) % words.len()]);
                if random().is_multiple_of(4) {
                    text.push_str(SEAMS[(random() as usize) % SEAMS.len()]);
                }
            }
            texts.push(text);
        }
    }
    for key in keys() {
        for text in &texts {
            assert_eq!(
                dictionary_breaks(text, key.options, model, &all),
                dictionary_breaks(text, key.options, Config::default(), &all),
                "{}: {text:?}",
                key.name
            );
        }
    }
}

#[test]
fn every_break_in_sa_text_is_a_safe_restart() {
    let all = dictionaries();
    for text in sa_corpora() {
        let text: String = text.chars().take(800).collect();
        for key in keys() {
            let whole: Vec<usize> = LineBreaks::new(Utf8Scalars::new(&text), key.options)
                .with_dictionaries(&all)
                .collect();
            for &start in &whole {
                let mut found: Vec<usize> =
                    whole.iter().copied().filter(|&at| at <= start).collect();
                found.extend(
                    LineBreaks::with_preceding(
                        Utf8Scalars::with_start(&text, start),
                        key.options,
                        text[..start].chars().rev(),
                    )
                    .with_dictionaries(&all),
                );
                assert_eq!(found, whole, "{} from {start}", key.name);
            }
        }
    }
}

#[test]
fn scanning_sa_text_allocates_nothing_warm() {
    let all = dictionaries();
    for corpus in testing::corpora::segmentation_corpora() {
        let text = corpus.text.as_str();
        for key in keys() {
            let allocations = testing::allocator::count_allocations(|| {
                core::hint::black_box(
                    LineBreaks::new(Utf8Scalars::new(text), key.options)
                        .with_dictionaries(&all)
                        .count(),
                );
            });
            assert_eq!(allocations, 0, "{} {}", corpus.name, key.name);
        }
    }
}
