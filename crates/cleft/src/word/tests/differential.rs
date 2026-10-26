//! The differential against ICU4X 2.3's word segmenter, with and without
//! its dictionaries.
//!
//! ICU4X's word data is Unicode 17.0's Word_Break, Extended_Pictographic,
//! Line_Break SA and Script, and its rules are UAX #29's, which 18.0 leaves
//! as they were. It hands each run of SA, Han and Hiragana characters (its
//! complex property) to a dictionary, but not Katakana, which the crate
//! hands to the Chinese and Japanese dictionary as ICU4C does. The configuration that reproduces it
//! reads 17.0's word bytes and graphemes and turns on the [`Legacy`]
//! switches that model its complex runs; under it the two agree exactly on
//! every text here. Under the crate's own data and rules, each difference is
//! counted by the fewest causes that, put back, give ICU's answer.

use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;
use std::{eprintln, format};

use icu_segmenter::WordSegmenter;
use icu_segmenter::options::WordBreakInvariantOptions;

use super::conformance::with_ends;
use super::*;
use crate::Utf8Scalars;
use crate::tests::{
    GraphemeVersion, ICU_VERSION, WordVersion, causes_restoring, dictionaries, list_words, xorshift,
};

/// The data and switches a scan reads in place of the crate's.
#[derive(Copy, Clone, Debug, Default)]
struct Config {
    classes: Option<&'static [u8]>,
    graphemes: Option<&'static [GraphemeProps]>,
    legacy: u32,
}

/// Returns ICU4X's Unicode 17.0 data, read from the pinned files once, or
/// `None` where they are absent.
fn icu_data() -> Option<Config> {
    static DATA: std::sync::OnceLock<Option<Config>> = std::sync::OnceLock::new();
    *DATA.get_or_init(|| {
        Some(Config {
            classes: Some(WordVersion::new(ICU_VERSION)?.leak()),
            graphemes: Some(GraphemeVersion::new(ICU_VERSION)?.leak()),
            legacy: 0,
        })
    })
}

/// Returns the boundaries the crate finds in `text` under `config` and
/// `dictionaries`, both ends included, as ICU yields them.
fn boundaries(text: &str, config: Config, dictionaries: &Dictionaries) -> Vec<usize> {
    let mut breaks =
        WordBreaks::new(Utf8Scalars::new(text), WordOptions::new()).with_dictionaries(dictionaries);
    breaks.test.classes = config.classes;
    breaks.test.graphemes = config.graphemes;
    breaks.test.graphemes_17 = config.graphemes.is_some();
    breaks.test.legacy = config.legacy;
    with_ends(text, breaks)
}

/// A cause of a difference from ICU4X.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Cause {
    /// Unicode 17.0's data: its word bytes and graphemes.
    Data,
    /// A boundary before every complex run ([`Legacy::RUN_STARTS`]).
    RunStarts,
    /// A boundary after every complex run ([`Legacy::RUN_ENDS`]).
    RunEnds,
    /// A complex character is a class no rule joins
    /// ([`Legacy::COMPLEX_CLASS`]).
    ComplexClass,
    /// A Katakana and a joiner hold no ExtendNumLet
    /// ([`Legacy::KATAKANA_JOINER`]).
    KatakanaJoiner,
    /// The rules keep a Katakana run whole ([`Legacy::KATAKANA_RULES`]).
    KatakanaRules,
    /// ICU4X's script ranges part runs ([`Legacy::PIECES`]).
    Pieces,
    /// A word ends inside a cluster ([`Legacy::LEAVES`]).
    Leaves,
    /// A miss takes everything read ([`Legacy::MISSES`]).
    Misses,
    /// The rest of a part is one word ([`Legacy::REST`]).
    Rest,
    /// A run of Han and Hiragana with no dictionary is one word
    /// ([`Legacy::CJ_WHOLE`]).
    CjWhole,
}

impl Cause {
    const ALL: [Cause; 11] = [
        Cause::Data,
        Cause::RunStarts,
        Cause::RunEnds,
        Cause::ComplexClass,
        Cause::KatakanaJoiner,
        Cause::KatakanaRules,
        Cause::Pieces,
        Cause::Leaves,
        Cause::Misses,
        Cause::Rest,
        Cause::CjWhole,
    ];

    /// Returns `config` with the cause put back, from `icu`'s data.
    fn apply(self, config: Config, icu: Config) -> Config {
        let switch = match self {
            Cause::Data => {
                return Config {
                    classes: icu.classes,
                    graphemes: icu.graphemes,
                    ..config
                };
            }
            Cause::RunStarts => Legacy::RUN_STARTS,
            Cause::RunEnds => Legacy::RUN_ENDS,
            Cause::ComplexClass => Legacy::COMPLEX_CLASS,
            Cause::KatakanaJoiner => Legacy::KATAKANA_JOINER,
            Cause::KatakanaRules => Legacy::KATAKANA_RULES,
            Cause::Pieces => Legacy::PIECES,
            Cause::Leaves => Legacy::LEAVES,
            Cause::Misses => Legacy::MISSES,
            Cause::Rest => Legacy::REST,
            Cause::CjWhole => Legacy::CJ_WHOLE,
        };
        Config {
            legacy: config.legacy | Legacy::RUN_MODEL | switch,
            ..config
        }
    }
}

/// The causes of one text set's differences, as counts by cause.
type Tally = BTreeMap<String, usize>;

/// Returns the differences from ICU4X's word segmenter on `texts`, with all
/// dictionaries or none, by cause, after checking that ICU's configuration
/// gives ICU's boundaries on every text.
fn tally(texts: &[String], with_dictionaries: bool) -> Option<Tally> {
    let icu_data = icu_data()?;
    let (segmenter, dictionaries, legacy) = if with_dictionaries {
        (
            WordSegmenter::new_dictionary(WordBreakInvariantOptions::default()),
            dictionaries(),
            Legacy::ICU,
        )
    } else {
        (
            WordSegmenter::new_for_non_complex_scripts(WordBreakInvariantOptions::default()),
            Dictionaries::new(),
            Legacy::ICU | Legacy::CJ_WHOLE,
        )
    };
    let full = Config { legacy, ..icu_data };
    let mut tally = Tally::new();
    for text in texts {
        let expected: Vec<usize> = segmenter.segment_str(text).collect();
        assert_eq!(
            boundaries(text, full, &dictionaries),
            expected,
            "{text:?}: ICU's configuration is not ICU"
        );
        if boundaries(text, Config::default(), &dictionaries) == expected {
            continue;
        }
        let name = causes_restoring(&Cause::ALL, 4, |causes| {
            let config = causes.iter().fold(Config::default(), |config, cause| {
                cause.apply(config, icu_data)
            });
            boundaries(text, config, &dictionaries) == expected
        })
        .unwrap_or_else(|| {
            eprintln!("{text:?}: no cause");
            String::from("unexplained")
        });
        *tally.entry(name).or_default() += 1;
    }
    Some(tally)
}

/// Checks `tally` against `pinned`, printing it where they differ.
fn assert_pinned(name: &str, tally: &Tally, pinned: &[(&str, usize)]) {
    let expected: Tally = pinned
        .iter()
        .map(|&(cause, count)| (String::from(cause), count))
        .collect();
    if *tally != expected {
        let found: Vec<String> = tally
            .iter()
            .map(|(cause, count)| format!("(\"{cause}\", {count})"))
            .collect();
        eprintln!("{name}: &[{}]", found.join(", "));
    }
    assert_eq!(*tally, expected, "{name}");
}

/// Returns a character for each word byte of Unicode 17.0 and 18.0, the
/// first code point that has it, and the characters the rules and the runs
/// name: the full stop, Thai and Lao letters and marks, Lao's U+0EA3, Han
/// that is a letter or a mark, kana, and an ideograph past ICU4X's ranges.
fn representatives() -> Option<Vec<char>> {
    let old = WordVersion::new(ICU_VERSION)?;
    let new = WordVersion::new(crate::tests::TABLES_VERSION)?;
    let mut seen = BTreeMap::new();
    for ch in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
        seen.entry((old.get(ch), new.get(ch))).or_insert(ch);
    }
    let mut chars: Vec<char> = seen.into_values().collect();
    chars.extend([
        '.',
        'a',
        '1',
        ',',
        '\'',
        '"',
        ':',
        ' ',
        'ก',
        '\u{E31}',
        'ລ',
        '\u{EA3}',
        'က',
        'ក',
        '\u{3005}',
        '\u{16FF0}',
        '日',
        'か',
        'カ',
        'ー',
        '\u{2B81E}',
        '\u{1A20}',
        '👍',
        '\u{200D}',
        '\u{1F1EF}',
    ]);
    chars.sort_unstable();
    chars.dedup();
    Some(chars)
}

/// Returns every pair and triple of `chars`, and every pair with an Extend,
/// a Format, a ZWJ, and an Extend and a ZWJ between.
fn pairs_and_triples(chars: &[char]) -> Vec<String> {
    let mut texts = Vec::new();
    for &first in chars {
        for &second in chars {
            texts.push(format!("{first}{second}"));
            for between in ["\u{301}", "\u{AD}", "\u{200D}", "\u{301}\u{200D}"] {
                texts.push(format!("{first}{between}{second}"));
            }
            for &third in chars {
                texts.push(format!("{first}{second}{third}"));
            }
        }
    }
    texts
}

#[test]
fn pairs_and_triples_of_classes_agree_with_icu_or_differ_by_cause() {
    let Some(chars) = representatives() else {
        return;
    };
    let texts = pairs_and_triples(&chars);
    eprintln!("{} characters, {} texts", chars.len(), texts.len());
    let Some(with) = tally(&texts, true) else {
        return;
    };
    assert_pinned("with dictionaries", &with, CLASS_DIFFERENCES);
    let Some(without) = tally(&texts, false) else {
        return;
    };
    assert_pinned("without", &without, CLASS_DIFFERENCES_WITHOUT);
}

/// The differences from ICU4X's segmenter with its dictionaries on the
/// pairs and triples, by cause.
const CLASS_DIFFERENCES: &[(&str, usize)] = &[
    ("ComplexClass", 2171),
    ("ComplexClass+Misses", 32),
    ("Data", 6091),
    ("Data+ComplexClass", 20),
    ("Data+KatakanaRules", 15),
    ("Data+Misses", 8),
    ("Data+Pieces", 16),
    ("Data+RunEnds", 120),
    ("Data+RunStarts", 182),
    ("KatakanaJoiner", 3),
    ("KatakanaRules", 439),
    ("KatakanaRules+Pieces", 12),
    ("Misses", 671),
    ("Pieces", 570),
    ("Pieces+Misses", 1),
    ("Pieces+Rest", 15),
    ("RunEnds", 5122),
    ("RunEnds+ComplexClass", 69),
    ("RunEnds+Misses", 24),
    ("RunEnds+Pieces", 23),
    ("RunStarts", 4568),
    ("RunStarts+Misses", 52),
    ("RunStarts+RunEnds", 324),
];

/// The same without dictionaries.
const CLASS_DIFFERENCES_WITHOUT: &[(&str, usize)] = &[
    ("CjWhole", 1221),
    ("ComplexClass", 567),
    ("ComplexClass+CjWhole", 276),
    ("Data", 6820),
    ("Data+CjWhole", 128),
    ("Data+ComplexClass", 5),
    ("Data+Pieces", 4),
    ("Data+Pieces+CjWhole", 12),
    ("Data+RunEnds", 77),
    ("Data+RunEnds+CjWhole", 131),
    ("Data+RunStarts", 110),
    ("Data+RunStarts+CjWhole", 230),
    ("KatakanaJoiner", 12),
    ("KatakanaRules+CjWhole", 102),
    ("Pieces", 354),
    ("Pieces+CjWhole", 497),
    ("RunEnds", 3561),
    ("RunEnds+CjWhole", 2510),
    ("RunEnds+ComplexClass", 20),
    ("RunEnds+ComplexClass+CjWhole", 67),
    ("RunEnds+KatakanaRules+CjWhole", 57),
    ("RunEnds+Pieces", 16),
    ("RunEnds+Pieces+CjWhole", 24),
    ("RunStarts", 3168),
    ("RunStarts+CjWhole", 2713),
    ("RunStarts+KatakanaRules+CjWhole", 99),
    ("RunStarts+Pieces+CjWhole", 6),
    ("RunStarts+RunEnds", 204),
    ("RunStarts+RunEnds+CjWhole", 225),
];

/// Returns the distinct lines of the first 16 KiB of each corpus.
fn corpus_lines() -> Vec<String> {
    let mut lines = Vec::new();
    for corpus in testing::corpora::segmentation_corpora() {
        let text: String = corpus.text.chars().take(16 << 10).collect();
        for line in text.split_inclusive('\n') {
            if !lines.iter().any(|seen: &String| seen == line) {
                lines.push(String::from(line));
            }
        }
    }
    lines
}

#[test]
fn the_corpora_agree_with_icu_or_differ_by_cause() {
    let lines = corpus_lines();
    let Some(with) = tally(&lines, true) else {
        return;
    };
    assert_pinned("with dictionaries", &with, CORPUS_DIFFERENCES);
    let Some(without) = tally(&lines, false) else {
        return;
    };
    assert_pinned("without", &without, CORPUS_DIFFERENCES_WITHOUT);
}

/// The differences on the corpora's lines with dictionaries, by cause.
const CORPUS_DIFFERENCES: &[(&str, usize)] = &[("Misses", 2), ("Pieces+Misses", 1)];

/// The same without dictionaries.
const CORPUS_DIFFERENCES_WITHOUT: &[(&str, usize)] =
    &[("CjWhole", 4), ("KatakanaRules+CjWhole", 1), ("Pieces", 1)];

/// The word lists of every dictionary, by name.
const LISTS: [&str; 5] = ["thaidict", "laodict", "khmerdict", "burmesedict", "cjdict"];

#[test]
fn random_runs_of_dictionary_words_agree_with_icu_or_differ_by_cause() {
    let mut random = xorshift(0x2545_F491_4F6C_DD1D);
    let mut texts = Vec::new();
    for name in LISTS {
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
    let Some(with) = tally(&texts, true) else {
        return;
    };
    assert_pinned("random runs", &with, RANDOM_DIFFERENCES);
}

/// The differences on 2,000 random runs of two to six dictionary words, 400
/// a list, by cause.
const RANDOM_DIFFERENCES: &[(&str, usize)] = &[
    ("KatakanaRules", 38),
    ("Leaves+Misses", 1),
    ("Misses", 4),
    ("Pieces", 15),
    ("Pieces+Leaves+Misses+Rest", 1),
    ("Pieces+Misses", 11),
    ("Pieces+Misses+Rest", 17),
    ("Pieces+Rest", 17),
    ("RunStarts+RunEnds", 1),
];

/// What stands beside a run at a seam: letters, digits, marks and joiners,
/// spaces, punctuation, kana, and runs of other scripts.
const SEAMS: [&str; 20] = [
    "a",
    "Zebra",
    "漢字",
    "かな",
    "カナ",
    "12",
    "\u{0E51}",
    "\u{0301}",
    "\u{3099}",
    "\u{200D}",
    "\u{200B}",
    " ",
    "\u{A0}",
    ".",
    ")",
    "\"",
    "'",
    "\u{0E31}",
    "\u{1A20}\u{1A21}",
    "\u{3005}",
];

#[test]
fn script_seams_agree_with_icu_or_differ_by_cause() {
    let mut random = xorshift(0x9E37_79B9_7F4A_7C15);
    let mut lists = Vec::new();
    for name in LISTS {
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
        for other in &lists {
            let first = &words[(random() as usize) % words.len()];
            let second = &other[(random() as usize) % other.len()];
            texts.push(format!("{first}{second}{first}"));
        }
    }
    let Some(with) = tally(&texts, true) else {
        return;
    };
    assert_pinned("seams", &with, SEAM_DIFFERENCES);
}

/// The differences at seams, by cause.
const SEAM_DIFFERENCES: &[(&str, usize)] = &[
    ("ComplexClass", 30),
    ("KatakanaRules", 6),
    ("Leaves", 8),
    ("Misses", 10),
    ("Pieces", 5),
    ("Pieces+Misses", 5),
    ("Pieces+Misses+Rest", 5),
    ("Pieces+Rest", 5),
    ("RunEnds", 115),
    ("RunEnds+Pieces", 2),
    ("RunEnds+Pieces+Rest", 2),
    ("RunStarts+RunEnds+Misses", 3),
];

/// With no switch on but the model's own, the model of ICU4X's complex runs
/// gives the crate's boundaries: so each switch puts back exactly one of
/// ICU4X's ways.
#[test]
fn the_bare_run_model_gives_the_crates_boundaries() {
    let model = Config {
        legacy: Legacy::RUN_MODEL,
        ..Config::default()
    };
    let mut texts = corpus_lines();
    let mut random = xorshift(0x5851_F42D_4C95_7F2D);
    for name in LISTS {
        let Some(words) = list_words(name) else {
            return;
        };
        for _ in 0..200 {
            let count = 2 + random() % 5;
            let mut text = String::new();
            for _ in 0..count {
                text.push_str(&words[(random() as usize) % words.len()]);
                text.push_str(SEAMS[(random() as usize) % SEAMS.len()]);
            }
            texts.push(text);
        }
    }
    for set in [dictionaries(), Dictionaries::new()] {
        for text in &texts {
            assert_eq!(
                boundaries(text, model, &set),
                boundaries(text, Config::default(), &set),
                "{text:?}"
            );
        }
    }
}
