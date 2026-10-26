//! The differential against ICU4X's segmenter, whose line data and rules
//! are Unicode 15.1's.
//!
//! Under 15.1's data and rules and ICU4X's quirks ([`icu_config`]) the
//! crate must give ICU's answer exactly. Under 18.0 each difference is
//! counted by its cause: the one 15.1 rule, quirk or data change that, put
//! back alone, gives ICU's answer.

use std::collections::{BTreeMap, BTreeSet};
use std::string::{String, ToString};
use std::vec::Vec;
use std::{eprintln, format, thread};

use super::conformance::test_cases;
use super::*;

/// Returns whether `text` has an SA character, which ICU4X's segmenter
/// hands to a dictionary or model that this comparison leaves out.
fn has_complex(text: &str) -> bool {
    text.chars()
        .any(|ch| crate::core_props(ch).is_complex_context())
}

/// What makes the crate's 18.0 answer differ from ICU's: a data change, a
/// rule of Unicode 15.1, or a quirk of ICU4X's segmenter.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Cause {
    /// A character's line class, East_Asian_Width or pictograph bit differs
    /// between 15.1 and 18.0.
    Data,
    /// Unicode 15.1's LB12a: a break between BA and GL.
    Lb12a,
    /// Unicode 15.1's LB13, `× IS`, where 18.0's LB15c breaks `SP ÷ IS NU`.
    Lb13,
    /// Unicode 15.1's LB19, `× QU` and `QU ×`, where 18.0's LB19 and LB19a
    /// break beside East Asian characters.
    Lb19,
    /// No LB20a in 15.1: a word-initial hyphen breaks before a letter.
    Lb20a,
    /// Unicode 15.1's LB21a, `HL (HY | BA) ×`, where 18.0 holds only a
    /// non-Hebrew letter after the hyphen.
    Lb21a,
    /// Unicode 15.1's LB25 of class pairs.
    Lb25,
    /// ICU4X tailors by character after LB12, whatever the context, where
    /// ICU4C reads the tailored characters as classes of their own; and its
    /// `loose` breaks before a hyphen after an ideograph in every language.
    IcuTailoring,
    /// ICU4X holds the pairs its Japanese and Chinese `loose` tailoring
    /// breaks where that tailoring is off.
    LooseHolds,
    /// ICU4X applies the tailorings before LB4 to LB12.
    TailoringFirst,
    /// ICU4X applies no tailoring partway through a longer rule.
    TailoringOutsideRules,
    /// ICU4X keeps an LB10 mark apart from AL for `keep-all`.
    MarksApart,
    /// ICU4X's `keep-all` holds HY too.
    KeepAllHyphens,
    /// ICU4X breaks after two U+25CC before anything but a letter.
    DottedPairs,
    /// ICU4X reads the character after a break, or after a pair a
    /// tailoring decides, with no context.
    ContextForgotten,
    /// Unicode 17.0's GB9c, which ICU4X's graphemes follow under `anywhere`.
    Graphemes17,
}

impl Cause {
    const ALL: [Cause; 16] = [
        Cause::Data,
        Cause::Lb12a,
        Cause::Lb13,
        Cause::Lb19,
        Cause::Lb20a,
        Cause::Lb21a,
        Cause::Lb25,
        Cause::IcuTailoring,
        Cause::LooseHolds,
        Cause::TailoringFirst,
        Cause::TailoringOutsideRules,
        Cause::MarksApart,
        Cause::KeepAllHyphens,
        Cause::DottedPairs,
        Cause::ContextForgotten,
        Cause::Graphemes17,
    ];

    /// Returns `config` with the cause put back.
    fn apply(self, config: Config, versions: &Versions) -> Config {
        let (old, graphemes) = (versions.old, versions.graphemes);
        let legacy = match self {
            Cause::Data => {
                return Config {
                    classes: Some(old),
                    ..config
                };
            }
            Cause::Graphemes17 => {
                return Config {
                    graphemes_17: true,
                    graphemes: Some(graphemes),
                    ..config
                };
            }
            Cause::Lb12a => Legacy::LB12A,
            Cause::Lb13 => Legacy::LB13,
            Cause::Lb19 => Legacy::LB19,
            Cause::Lb20a => Legacy::LB20A,
            Cause::Lb21a => Legacy::LB21A,
            Cause::Lb25 => Legacy::LB25,
            // ICU4X's quirks of its tailorings need them.
            Cause::IcuTailoring => Legacy::ICU_TAILORING,
            Cause::LooseHolds => Legacy::ICU_TAILORING | Legacy::LOOSE_HOLDS,
            Cause::TailoringFirst => Legacy::ICU_TAILORING | Legacy::TAILORING_FIRST,
            Cause::TailoringOutsideRules => Legacy::ICU_TAILORING | Legacy::TAILORING_OUTSIDE_RULES,
            Cause::MarksApart => Legacy::MARKS_APART,
            Cause::KeepAllHyphens => Legacy::KEEP_ALL_HYPHENS,
            Cause::DottedPairs => Legacy::DOTTED_PAIRS,
            Cause::ContextForgotten => Legacy::CONTEXT_FORGOTTEN,
        };
        Config {
            legacy: config.legacy | legacy,
            ..config
        }
    }
}

/// The causes of one option set's differences, as counts by cause; a
/// difference that needs two or three causes together is counted once,
/// under their names joined with `+`.
type Tally = BTreeMap<String, usize>;

/// The two versions' line bytes, and ICU's grapheme properties, kept for
/// the test run.
struct Versions {
    old: &'static [u8],
    new: &'static [u8],
    graphemes: &'static [crate::GraphemeProps],
}

impl Versions {
    fn new() -> Option<Self> {
        Some(Self {
            old: VersionClasses::unicode_15_1()?.leak(),
            new: VersionClasses::unicode_18()?.leak(),
            graphemes: crate::tests::GraphemeVersion::new(crate::tests::ICU_VERSION)?.leak(),
        })
    }

    /// Returns the configuration that reproduces ICU4X's segmenter.
    fn icu(&self) -> Config {
        icu_config(self.old, self.graphemes)
    }

    /// Checks that the crate under ICU's configuration gives ICU's answer
    /// for `text`, and counts any difference under 18.0 by its cause in
    /// `tally`. Returns whether the text agrees under ICU's configuration.
    fn compare(&self, key: &Key, icu: &Icu, text: &str, tally: &mut Tally) -> bool {
        let expected = icu.breaks(text);
        if breaks(text, key.options, self.icu()) != expected {
            return false;
        }
        let current = Config::default();
        if breaks(text, key.options, current) == expected {
            return true;
        }
        let gives_icu = |config: Config| breaks(text, key.options, config) == expected;
        let changed = text
            .chars()
            .any(|ch| self.old[ch as usize] != self.new[ch as usize]);
        let candidates: Vec<Cause> = Cause::ALL
            .into_iter()
            .filter(|&cause| cause != Cause::Data || changed)
            .collect();
        let name = causes_restoring(&candidates, 3, |causes| {
            let config = causes
                .iter()
                .fold(current, |config, cause| cause.apply(config, self));
            gives_icu(config)
        })
        .unwrap_or_else(|| {
            eprintln!(
                "{}: {:04X?}: no cause: ICU {expected:?}, crate {:?}",
                key.name,
                text.chars().map(|ch| ch as u32).collect::<Vec<_>>(),
                breaks(text, key.options, current)
            );
            String::from("unexplained")
        });
        *tally.entry(name).or_default() += 1;
        true
    }
}

/// A source of texts: it calls its argument with each text.
type Texts<'a> = dyn Fn(&mut dyn FnMut(&str)) + Sync + 'a;

/// Runs `compare` over `texts` under every option set on threads of their
/// own, and returns each set's tally by name, after checking that every
/// text agrees under ICU's configuration.
fn tally_keys(versions: &Versions, texts: &Texts<'_>) -> BTreeMap<&'static str, Tally> {
    let keys = keys();
    let results: Vec<(&'static str, Tally, usize)> = thread::scope(|scope| {
        let handles: Vec<_> = keys
            .iter()
            .map(|key| {
                scope.spawn(move || {
                    let icu = Icu::new(key.options);
                    let mut tally = Tally::new();
                    let mut mismatches = 0;
                    texts(&mut |text| {
                        if !versions.compare(key, &icu, text, &mut tally) {
                            mismatches += 1;
                            if mismatches <= 10 {
                                eprintln!(
                                    "{}: {:04X?}: ICU {:?}, crate under ICU's rules {:?}",
                                    key.name,
                                    text.chars().map(|ch| ch as u32).collect::<Vec<_>>(),
                                    icu.breaks(text),
                                    breaks(text, key.options, versions.icu())
                                );
                            }
                        }
                    });
                    (key.name, tally, mismatches)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a key's thread"))
            .collect()
    });
    let mismatches: usize = results.iter().map(|(_, _, count)| count).sum();
    assert_eq!(mismatches, 0, "texts where ICU's configuration is not ICU");
    results
        .into_iter()
        .map(|(name, tally, _)| (name, tally))
        .collect()
}

/// Prints `tallies` in the form the tests pin them.
fn print_tallies(tallies: &BTreeMap<&'static str, Tally>) {
    for (name, tally) in tallies {
        let causes: Vec<String> = tally
            .iter()
            .map(|(cause, count)| format!("(\"{cause}\", {count})"))
            .collect();
        eprintln!("(\"{name}\", &[{}]),", causes.join(", "));
    }
}

/// Checks `tallies` against `pinned`, each option set's causes and counts.
fn assert_tallies(tallies: &BTreeMap<&'static str, Tally>, pinned: &[(&str, &[(&str, usize)])]) {
    let found: Vec<(String, Vec<(String, usize)>)> = tallies
        .iter()
        .map(|(name, tally)| {
            (
                name.to_string(),
                tally
                    .iter()
                    .map(|(cause, &count)| (cause.clone(), count))
                    .collect(),
            )
        })
        .collect();
    let mut pinned: Vec<(String, Vec<(String, usize)>)> = pinned
        .iter()
        .map(|(name, causes)| {
            (
                name.to_string(),
                causes
                    .iter()
                    .map(|(cause, count)| (cause.to_string(), *count))
                    .collect(),
            )
        })
        .collect();
    pinned.sort();
    if found != pinned {
        print_tallies(tallies);
    }
    assert_eq!(found, pinned);
}

/// The characters that stand for the line classes: for each line byte of
/// 18.0, a character with it in both versions where one has, or else any;
/// then each 15.1 line byte no character so far has; then the characters
/// each tailoring names. SA is left out.
fn representatives(versions: &Versions) -> Vec<char> {
    let all = || (0..=0x10_FFFF).filter_map(char::from_u32);
    let complex = |byte: u8| matches!(byte & 0x3F, SA | SA_MARK);
    let mut chars: Vec<char> = Vec::new();
    let mut new_bytes = BTreeSet::new();
    let mut old_bytes = BTreeSet::new();
    let stable = all().filter(|&ch| versions.old[ch as usize] == versions.new[ch as usize]);
    for ch in stable.chain(all()) {
        let (old, new) = (versions.old[ch as usize], versions.new[ch as usize]);
        if complex(old) || complex(new) || new_bytes.contains(&new) {
            continue;
        }
        new_bytes.insert(new);
        old_bytes.insert(old);
        chars.push(ch);
    }
    for ch in all() {
        let (old, new) = (versions.old[ch as usize], versions.new[ch as usize]);
        if complex(old) || complex(new) || old_bytes.contains(&old) {
            continue;
        }
        old_bytes.insert(old);
        chars.push(ch);
    }
    for ch in [
        '\u{2010}', '\u{2013}', '\u{301C}', '\u{3005}', '\u{30FB}', '\u{FF01}', '\u{201C}',
        '\u{201D}',
    ] {
        if !chars.contains(&ch) {
            chars.push(ch);
        }
    }
    chars
}

/// What a pair of representatives is tried with between them: nothing,
/// spaces, marks and a joiner.
const INFIXES: [&str; 6] = ["", " ", "  ", "\u{308}", "\u{200D}", " \u{308}"];

#[test]
fn icu_agrees_on_the_15_1_test_file_under_15_1() {
    let (Some(file), Some(versions)) = (
        pinned_file(ICU_LINE_VERSION, "LineBreakTest_15.1.txt"),
        Versions::new(),
    ) else {
        return;
    };
    let cases = test_cases(&file);
    let texts: Vec<&str> = cases
        .iter()
        .map(|(text, _)| text.as_str())
        .filter(|text| !has_complex(text))
        .collect();
    let tallies = tally_keys(&versions, &|visit| {
        for text in &texts {
            visit(text);
        }
    });
    assert_tallies(&tallies, TEST_FILE_DIFFERENCES);
}

/// The differences from ICU on 15.1's test file under 18.0, by option set
/// and cause.
const TEST_FILE_DIFFERENCES: &[(&str, &[(&str, usize)])] = &[
    ("anywhere", &[("Graphemes17", 3)]),
    (
        "loose break-all",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
        ],
    ),
    (
        "loose break-all ja",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("TailoringOutsideRules", 2),
        ],
    ),
    (
        "loose keep-all",
        &[
            ("Data", 48),
            ("Data+KeepAllHyphens", 2),
            ("KeepAllHyphens", 19),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 4),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("MarksApart", 54),
        ],
    ),
    (
        "loose keep-all ja",
        &[
            ("Data", 48),
            ("Data+KeepAllHyphens", 2),
            ("KeepAllHyphens", 19),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 4),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("MarksApart", 54),
            ("TailoringOutsideRules", 2),
        ],
    ),
    (
        "loose normal",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
        ],
    ),
    (
        "loose normal ja",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("TailoringOutsideRules", 2),
        ],
    ),
    (
        "normal break-all",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
        ],
    ),
    (
        "normal break-all ja",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("TailoringOutsideRules", 2),
        ],
    ),
    (
        "normal keep-all",
        &[
            ("Data", 48),
            ("Data+KeepAllHyphens", 2),
            ("KeepAllHyphens", 19),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 4),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("MarksApart", 54),
        ],
    ),
    (
        "normal keep-all ja",
        &[
            ("Data", 48),
            ("Data+KeepAllHyphens", 2),
            ("KeepAllHyphens", 19),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 4),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("MarksApart", 54),
            ("TailoringOutsideRules", 2),
        ],
    ),
    (
        "normal normal",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
        ],
    ),
    (
        "normal normal ja",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("TailoringOutsideRules", 2),
        ],
    ),
    (
        "strict break-all",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
        ],
    ),
    (
        "strict keep-all",
        &[
            ("Data", 46),
            ("Data+KeepAllHyphens", 2),
            ("KeepAllHyphens", 17),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 4),
            ("Lb21a", 1),
            ("Lb25", 20),
            ("MarksApart", 45),
        ],
    ),
    (
        "strict normal",
        &[
            ("Data", 10),
            ("Lb12a", 2),
            ("Lb13", 1),
            ("Lb20a", 8),
            ("Lb21a", 1),
            ("Lb25", 20),
        ],
    ),
];

#[test]
fn every_pair_and_triple_of_classes_agrees_with_icu_or_differs_by_cause() {
    let Some(versions) = Versions::new() else {
        return;
    };
    let chars = representatives(&versions);
    eprintln!(
        "{} representatives: {:04X?}",
        chars.len(),
        chars.iter().map(|&ch| ch as u32).collect::<Vec<_>>()
    );
    let tallies = tally_keys(&versions, &|visit| {
        let mut text = String::new();
        for &first in &chars {
            for &second in &chars {
                for infix in INFIXES {
                    text.clear();
                    text.push(first);
                    text.push_str(infix);
                    text.push(second);
                    visit(&text);
                }
                for &third in &chars {
                    text.clear();
                    text.extend([first, second, third]);
                    visit(&text);
                }
            }
        }
    });
    assert_tallies(&tallies, CLASS_DIFFERENCES);
}

/// The differences from ICU on the pairs and triples of classes under 18.0,
/// by option set and cause.
const CLASS_DIFFERENCES: &[(&str, &[(&str, usize)])] = &[
    ("anywhere", &[("Graphemes17", 34)]),
    (
        "loose break-all",
        &[
            ("Data", 9915),
            ("Data+IcuTailoring", 352),
            ("Data+Lb12a", 4),
            ("Data+Lb12a+IcuTailoring", 4),
            ("Data+Lb20a", 23),
            ("Data+Lb25", 40),
            ("Data+LooseHolds", 228),
            ("Data+LooseHolds+TailoringFirst", 20),
            ("DottedPairs", 9),
            ("IcuTailoring", 1436),
            ("Lb12a", 978),
            ("Lb12a+IcuTailoring", 2),
            ("Lb12a+TailoringFirst", 6),
            ("Lb19", 2026),
            ("Lb20a", 654),
            ("Lb21a", 126),
            ("Lb25", 5724),
            ("Lb25+IcuTailoring", 10),
            ("Lb25+LooseHolds", 266),
            ("Lb25+LooseHolds+TailoringFirst", 20),
            ("LooseHolds", 17083),
            ("LooseHolds+TailoringFirst", 4856),
            ("TailoringFirst", 486),
            ("TailoringFirst+DottedPairs", 34),
            ("TailoringOutsideRules", 13),
        ],
    ),
    (
        "loose break-all ja",
        &[
            ("Data", 9884),
            ("Data+IcuTailoring", 954),
            ("Data+Lb12a", 4),
            ("Data+Lb12a+IcuTailoring", 4),
            ("Data+Lb20a", 22),
            ("Data+Lb20a+IcuTailoring", 2),
            ("Data+Lb25", 41),
            ("Data+TailoringFirst", 44),
            ("DottedPairs", 5),
            ("IcuTailoring", 18950),
            ("Lb12a", 930),
            ("Lb12a+IcuTailoring", 14),
            ("Lb12a+TailoringFirst", 42),
            ("Lb19", 948),
            ("Lb19+IcuTailoring", 14),
            ("Lb20a", 634),
            ("Lb20a+IcuTailoring", 20),
            ("Lb21a", 141),
            ("Lb25", 5190),
            ("Lb25+IcuTailoring", 469),
            ("Lb25+TailoringFirst", 74),
            ("TailoringFirst", 6122),
            ("TailoringFirst+DottedPairs", 30),
            ("TailoringOutsideRules", 84),
        ],
    ),
    (
        "loose keep-all",
        &[
            ("Data", 17102),
            ("Data+IcuTailoring", 14),
            ("Data+KeepAllHyphens", 16),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 23),
            ("Data+Lb25", 47),
            ("Data+LooseHolds", 228),
            ("Data+LooseHolds+TailoringFirst", 20),
            ("Data+MarksApart", 32),
            ("DottedPairs", 10),
            ("IcuTailoring", 2058),
            ("IcuTailoring+KeepAllHyphens", 6),
            ("IcuTailoring+MarksApart", 12),
            ("KeepAllHyphens", 1582),
            ("Lb12a", 978),
            ("Lb12a+IcuTailoring", 6),
            ("Lb12a+TailoringFirst", 6),
            ("Lb13", 1),
            ("Lb19", 2026),
            ("Lb20a", 276),
            ("Lb21a", 126),
            ("Lb25", 5860),
            ("Lb25+IcuTailoring", 10),
            ("Lb25+LooseHolds", 266),
            ("Lb25+LooseHolds+TailoringFirst", 20),
            ("LooseHolds", 17079),
            ("LooseHolds+TailoringFirst", 4856),
            ("MarksApart", 2132),
            ("TailoringFirst", 486),
            ("TailoringFirst+DottedPairs", 34),
            ("TailoringOutsideRules", 13),
        ],
    ),
    (
        "loose keep-all ja",
        &[
            ("Data", 17252),
            ("Data+IcuTailoring", 1022),
            ("Data+KeepAllHyphens", 16),
            ("Data+Lb12a", 4),
            ("Data+Lb12a+IcuTailoring", 4),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 24),
            ("Data+Lb20a+IcuTailoring", 2),
            ("Data+Lb25", 48),
            ("Data+MarksApart", 32),
            ("Data+TailoringFirst", 44),
            ("DottedPairs", 6),
            ("IcuTailoring", 20070),
            ("IcuTailoring+KeepAllHyphens", 24),
            ("KeepAllHyphens", 1564),
            ("Lb12a", 930),
            ("Lb12a+IcuTailoring", 14),
            ("Lb12a+TailoringFirst", 42),
            ("Lb13", 1),
            ("Lb19", 948),
            ("Lb19+IcuTailoring", 14),
            ("Lb20a", 270),
            ("Lb20a+IcuTailoring", 12),
            ("Lb21a", 141),
            ("Lb25", 5298),
            ("Lb25+IcuTailoring", 499),
            ("Lb25+TailoringFirst", 74),
            ("MarksApart", 2128),
            ("TailoringFirst", 6106),
            ("TailoringFirst+DottedPairs", 30),
            ("TailoringFirst+MarksApart", 16),
            ("TailoringOutsideRules", 84),
        ],
    ),
    (
        "loose normal",
        &[
            ("Data", 11885),
            ("Data+IcuTailoring", 2),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 28),
            ("Data+Lb25", 47),
            ("Data+LooseHolds", 228),
            ("Data+LooseHolds+TailoringFirst", 20),
            ("DottedPairs", 10),
            ("IcuTailoring", 2088),
            ("Lb12a", 978),
            ("Lb12a+IcuTailoring", 6),
            ("Lb12a+TailoringFirst", 6),
            ("Lb13", 1),
            ("Lb19", 2026),
            ("Lb20a", 644),
            ("Lb21a", 126),
            ("Lb25", 5860),
            ("Lb25+IcuTailoring", 10),
            ("Lb25+LooseHolds", 266),
            ("Lb25+LooseHolds+TailoringFirst", 20),
            ("LooseHolds", 17079),
            ("LooseHolds+TailoringFirst", 4856),
            ("TailoringFirst", 486),
            ("TailoringFirst+DottedPairs", 34),
            ("TailoringOutsideRules", 13),
        ],
    ),
    (
        "loose normal ja",
        &[
            ("Data", 12062),
            ("Data+IcuTailoring", 1020),
            ("Data+Lb12a", 4),
            ("Data+Lb12a+IcuTailoring", 4),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 29),
            ("Data+Lb20a+IcuTailoring", 2),
            ("Data+Lb25", 48),
            ("Data+TailoringFirst", 44),
            ("DottedPairs", 6),
            ("IcuTailoring", 20078),
            ("Lb12a", 930),
            ("Lb12a+IcuTailoring", 14),
            ("Lb12a+TailoringFirst", 42),
            ("Lb13", 1),
            ("Lb19", 948),
            ("Lb19+IcuTailoring", 14),
            ("Lb20a", 622),
            ("Lb20a+IcuTailoring", 28),
            ("Lb21a", 141),
            ("Lb25", 5298),
            ("Lb25+IcuTailoring", 499),
            ("Lb25+TailoringFirst", 74),
            ("TailoringFirst", 6122),
            ("TailoringFirst+DottedPairs", 30),
            ("TailoringOutsideRules", 84),
        ],
    ),
    (
        "normal break-all",
        &[
            ("Data", 9859),
            ("Data+Lb20a", 21),
            ("Data+Lb25", 48),
            ("DottedPairs", 9),
            ("Lb12a", 990),
            ("Lb19", 2028),
            ("Lb20a", 648),
            ("Lb21a", 123),
            ("Lb25", 5720),
            ("TailoringFirst+DottedPairs", 35),
        ],
    ),
    (
        "normal break-all ja",
        &[
            ("Data", 10139),
            ("Data+IcuTailoring", 4),
            ("Data+Lb20a", 22),
            ("Data+Lb25", 69),
            ("DottedPairs", 9),
            ("IcuTailoring", 1043),
            ("Lb12a", 984),
            ("Lb12a+TailoringFirst", 6),
            ("Lb19", 1014),
            ("Lb20a", 648),
            ("Lb21a", 129),
            ("Lb25", 7960),
            ("Lb25+IcuTailoring", 21),
            ("TailoringFirst", 492),
            ("TailoringFirst+DottedPairs", 34),
            ("TailoringOutsideRules", 16),
        ],
    ),
    (
        "normal keep-all",
        &[
            ("Data", 17368),
            ("Data+KeepAllHyphens", 16),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 23),
            ("Data+Lb25", 57),
            ("Data+MarksApart", 32),
            ("DottedPairs", 10),
            ("KeepAllHyphens", 1588),
            ("Lb12a", 990),
            ("Lb13", 1),
            ("Lb19", 2028),
            ("Lb20a", 276),
            ("Lb21a", 123),
            ("Lb25", 5862),
            ("MarksApart", 2144),
            ("TailoringFirst+DottedPairs", 35),
        ],
    ),
    (
        "normal keep-all ja",
        &[
            ("Data", 17641),
            ("Data+IcuTailoring", 4),
            ("Data+KeepAllHyphens", 16),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 24),
            ("Data+Lb25", 78),
            ("Data+MarksApart", 32),
            ("DottedPairs", 10),
            ("IcuTailoring", 1043),
            ("KeepAllHyphens", 1588),
            ("Lb12a", 984),
            ("Lb12a+TailoringFirst", 6),
            ("Lb13", 1),
            ("Lb19", 1014),
            ("Lb20a", 276),
            ("Lb21a", 129),
            ("Lb25", 8088),
            ("Lb25+IcuTailoring", 21),
            ("MarksApart", 2144),
            ("TailoringFirst", 492),
            ("TailoringFirst+DottedPairs", 34),
            ("TailoringOutsideRules", 16),
        ],
    ),
    (
        "normal normal",
        &[
            ("Data", 12139),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 28),
            ("Data+Lb25", 57),
            ("DottedPairs", 10),
            ("Lb12a", 990),
            ("Lb13", 1),
            ("Lb19", 2028),
            ("Lb20a", 644),
            ("Lb21a", 123),
            ("Lb25", 5862),
            ("TailoringFirst+DottedPairs", 35),
        ],
    ),
    (
        "normal normal ja",
        &[
            ("Data", 12413),
            ("Data+IcuTailoring", 4),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 29),
            ("Data+Lb25", 78),
            ("DottedPairs", 10),
            ("IcuTailoring", 1043),
            ("Lb12a", 984),
            ("Lb12a+TailoringFirst", 6),
            ("Lb13", 1),
            ("Lb19", 1014),
            ("Lb20a", 644),
            ("Lb21a", 129),
            ("Lb25", 8088),
            ("Lb25+IcuTailoring", 21),
            ("TailoringFirst", 492),
            ("TailoringFirst+DottedPairs", 34),
            ("TailoringOutsideRules", 16),
        ],
    ),
    (
        "strict break-all",
        &[
            ("Data", 9859),
            ("Data+Lb20a", 21),
            ("Data+Lb25", 48),
            ("DottedPairs", 9),
            ("Lb12a", 990),
            ("Lb19", 2028),
            ("Lb20a", 648),
            ("Lb21a", 123),
            ("Lb25", 5720),
            ("TailoringFirst+DottedPairs", 35),
        ],
    ),
    (
        "strict keep-all",
        &[
            ("Data", 17085),
            ("Data+KeepAllHyphens", 14),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 22),
            ("Data+Lb25", 57),
            ("Data+MarksApart", 28),
            ("DottedPairs", 10),
            ("KeepAllHyphens", 1425),
            ("Lb12a", 990),
            ("Lb13", 1),
            ("Lb19", 1976),
            ("Lb20a", 276),
            ("Lb21a", 120),
            ("Lb25", 5862),
            ("MarksApart", 1876),
            ("TailoringFirst+DottedPairs", 36),
        ],
    ),
    (
        "strict normal",
        &[
            ("Data", 12142),
            ("Data+Lb13", 1),
            ("Data+Lb20a", 28),
            ("Data+Lb25", 57),
            ("DottedPairs", 10),
            ("Lb12a", 990),
            ("Lb13", 1),
            ("Lb19", 1976),
            ("Lb20a", 644),
            ("Lb21a", 120),
            ("Lb25", 5862),
            ("TailoringFirst+DottedPairs", 36),
        ],
    ),
];

/// Returns the breaks of `text` where neither character beside the break is
/// SA, which this comparison leaves to the dictionaries.
fn outside_complex(text: &str, breaks: &[usize]) -> BTreeSet<usize> {
    let complex =
        |ch: Option<char>| ch.is_some_and(|ch| crate::core_props(ch).is_complex_context());
    breaks
        .iter()
        .copied()
        .filter(|&at| {
            let before = text.get(..at).and_then(|text| text.chars().next_back());
            let after = text.get(at..).and_then(|text| text.chars().next());
            !complex(before) && !complex(after)
        })
        .collect()
}

#[test]
fn the_segmentation_corpora_agree_with_icu() {
    let Some(versions) = Versions::new() else {
        return;
    };
    let corpora = testing::corpora::segmentation_corpora();
    let keys = keys();
    thread::scope(|scope| {
        for corpus in &corpora {
            for key in &keys {
                let versions = &versions;
                scope.spawn(move || {
                    // Each corpus repeats a seed of a few hundred bytes, so a
                    // debug build reads only its first 64 KiB.
                    let mut length = corpus.text.len();
                    if cfg!(debug_assertions) {
                        length = length.min(1 << 16);
                        while !corpus.text.is_char_boundary(length) {
                            length -= 1;
                        }
                    }
                    let text = &corpus.text[..length];
                    let icu = Icu::new_dictionary(key.options);
                    let expected = outside_complex(text, &icu.breaks(text));
                    let under =
                        |config: Config| outside_complex(text, &breaks(text, key.options, config));
                    assert!(
                        under(versions.icu()) == expected,
                        "{} {}: ICU's configuration is not ICU",
                        corpus.name,
                        key.name
                    );
                    // Unicode 18.0 changes no break in the corpora, nor do
                    // ICU4C's tailorings. `keep-all` without HY breaks after
                    // the hyphens of the Latin corpus; ICU4X's tailorings put
                    // back give ICU's breaks.
                    let found = under(Config::default());
                    let differ = found.symmetric_difference(&expected).count();
                    if differ != 0 {
                        eprintln!("{} {}: {differ} differ", corpus.name, key.name);
                    }
                    let icu_tailoring = Config {
                        legacy: Legacy::ICU_TAILORING
                            | Legacy::LOOSE_HOLDS
                            | Legacy::TAILORING_FIRST
                            | Legacy::TAILORING_OUTSIDE_RULES
                            | Legacy::KEEP_ALL_HYPHENS,
                        ..Config::default()
                    };
                    assert!(
                        under(icu_tailoring) == expected,
                        "{} {}",
                        corpus.name,
                        key.name
                    );
                });
            }
        }
    });
}
