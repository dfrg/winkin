//! Line breaking tests:
//! - the line bytes against the pinned 18.0 files, for every character;
//! - Unicode 18.0's LineBreakTest.txt, over UTF-8 and UTF-16, and from every
//!   break (`conformance`);
//! - the differential against ICU4X's segmenter, whose rules and line data
//!   are Unicode 15.1's: under 15.1's data and rules and ICU4X's quirks the
//!   two agree exactly on 15.1's test file, every pair and triple of
//!   classes, and the corpora, under each of the 16 option sets winkin
//!   uses; under 18.0 each difference is counted by its cause
//!   (`differential`);
//! - the pair table against the rules;
//! - random text over UTF-8 and UTF-16, scans that allocate nothing, streams
//!   whose offsets do not increase, and the state's size.

mod conformance;
mod dictionary;
mod differential;

use std::boxed::Box;
use std::string::String;
use std::vec;
use std::vec::Vec;

use icu_locale_core::langid;
use icu_segmenter::LineSegmenter;
use icu_segmenter::options::{LineBreakOptions, LineBreakStrictness, LineBreakWordOption};

use super::rules::*;
use super::*;
use crate::Utf8Scalars;
use crate::tests::causes_restoring;
use crate::tests::{ICU_VERSION, TABLES_VERSION, Ucd, pinned_file};

/// The Unicode version of ICU4X's line rules and line data.
const ICU_LINE_VERSION: &str = "15.1.0";

/// Returns the line byte of a character from its Line_Break, General_Category,
/// East_Asian_Width and Extended_Pictographic, as the generator derives it.
fn line_byte(cp: usize, line_break: &str, gc: &str, width: &str, pictographic: bool) -> u8 {
    let east_asian = matches!(width, "F" | "W" | "H");
    let class = match line_break {
        "XX" | "SG" => XX,
        "AL" if cp == 0x25CC => DOTTED,
        "AL" => AL,
        "AI" => AI,
        "SA" if matches!(gc, "Mn" | "Mc") => SA_MARK,
        "SA" => SA,
        "BK" => BK,
        "CR" => CR,
        "LF" => LF,
        "NL" => NL,
        "SP" => SP,
        "ZW" => ZW,
        "WJ" => WJ,
        "GL" => GL,
        "CM" => CM,
        "ZWJ" => ZWJ,
        "CL" => CL,
        "CP" => CP,
        "EX" if matches!(cp, 0xFF01 | 0xFF1F) => EX_WIDE,
        "EX" => EX,
        "IS" => IS,
        "SY" => SY,
        "OP" if east_asian => OP_EA,
        "OP" => OP,
        "QU" if gc == "Pi" => QU_PI,
        "QU" if gc == "Pf" => QU_PF,
        "QU" => QU,
        "NS" if matches!(cp, 0x301C | 0x30A0) => NS_DASH,
        "NS" if matches!(cp, 0x3005 | 0x303B | 0x309D | 0x309E | 0x30FD | 0x30FE) => NS_ITERATION,
        "NS" if matches!(
            cp,
            0x30FB | 0xFF1A | 0xFF1B | 0xFF65 | 0x203C | 0x2047..=0x2049
        ) =>
        {
            NS_CENTRED
        }
        "NS" => NS,
        "CJ" => CJ,
        "B2" => B2,
        "BA" => BA,
        "BB" => BB,
        "HY" => HY,
        "HH" => HH,
        "CB" => CB,
        "IN" => IN,
        "NU" => NU,
        "PO" if matches!(width, "A" | "F" | "W") => PO_EAW,
        "PO" => PO,
        "PR" if matches!(width, "A" | "F" | "W") => PR_EAW,
        "PR" => PR,
        "HL" => HL,
        "ID" if pictographic && gc == "Cn" => ID_CN,
        "ID" => ID,
        "EB" => EB,
        "EM" => EM,
        "JL" => JL,
        "JV" => JV,
        "JT" => JT,
        "H2" => H2,
        "H3" => H3,
        "RI" => RI,
        "AK" => AK,
        "AP" => AP,
        "AS" => AS,
        "VF" => VF,
        "VI" => VI,
        other => panic!("Line_Break {other}"),
    };
    let unassigned_pictograph = pictographic && gc == "Cn";
    class
        | if east_asian { EAST_ASIAN } else { 0 }
        | if unassigned_pictograph {
            UNASSIGNED_PICTOGRAPHIC
        } else {
            0
        }
}

/// Each code point's line byte under one version's data, read from the
/// pinned files.
struct VersionClasses(Vec<u8>);

impl VersionClasses {
    /// Returns the 18.0 line bytes, from the pinned 18.0 files.
    fn unicode_18() -> Option<Self> {
        let ucd = Ucd::new(TABLES_VERSION)?;
        let line_break = ucd.map("LineBreak.txt", "lb", "XX", None);
        let gc = ucd.map("DerivedGeneralCategory.txt", "gc", "Cn", None);
        let width = ucd.map("EastAsianWidth.txt", "ea", "N", None);
        let pictographic = ucd.set("emoji-data.txt", "Extended_Pictographic");
        let bytes = (0..line_break.len())
            .map(|cp| line_byte(cp, &line_break[cp], &gc[cp], &width[cp], pictographic[cp]))
            .collect();
        Some(Self(bytes))
    }

    /// Returns the line bytes ICU4X's segmenter reads: Line_Break from the
    /// pinned 15.1 LineBreak.txt, unassigned where its comments say Cn, and
    /// General_Category and East_Asian_Width from the 18.0 files.
    ///
    /// ICU4X's data reads Extended_Pictographic at 17.0, and adds three
    /// ranges by hand (U+1F774 to U+1F77F, U+1F8AE to U+1F8FF and U+1F947 to
    /// U+1FAFF) where 15.1 had unassigned ID that 17.0 assigned. Only that
    /// unassigned ID is held before EM (LB30b).
    fn unicode_15_1() -> Option<Self> {
        let text = pinned_file(ICU_LINE_VERSION, "LineBreak.txt")?;
        let ucd = Ucd::new(TABLES_VERSION)?;
        let gc = ucd.map("DerivedGeneralCategory.txt", "gc", "Cn", None);
        let width = ucd.map("EastAsianWidth.txt", "ea", "N", None);
        let mut pictographic =
            Ucd::new(ICU_VERSION)?.set("emoji-data.txt", "Extended_Pictographic");
        for range in [0x1F774..=0x1F77F, 0x1F8AE..=0x1F8FF, 0x1F947..=0x1FAFF] {
            for cp in range {
                pictographic[cp] = true;
            }
        }
        let count = gc.len();
        let mut line_break = vec![String::from("XX"); count];
        let mut assigned = vec![false; count];
        let range = |field: &str| {
            let parse = |hex: &str| usize::from_str_radix(hex.trim(), 16).expect("a code point");
            match field.split_once("..") {
                Some((start, end)) => parse(start)..=parse(end),
                None => parse(field)..=parse(field),
            }
        };
        for line in text.lines() {
            if let Some(missing) = line.strip_prefix("# @missing:") {
                let fields: Vec<&str> = missing.split(';').map(str::trim).collect();
                for cp in range(fields[0]) {
                    line_break[cp] = String::from(fields[1]);
                }
                continue;
            }
            let (data, comment) = line.split_once('#').unwrap_or((line, ""));
            let fields: Vec<&str> = data.split(';').map(str::trim).collect();
            if fields.len() != 2 {
                continue;
            }
            let category = comment.split_whitespace().next().unwrap_or_default();
            for cp in range(fields[0]) {
                line_break[cp] = String::from(fields[1]);
                assigned[cp] = category != "Cn";
            }
        }
        let bytes = (0..count)
            .map(|cp| {
                let category = if assigned[cp] { gc[cp].as_str() } else { "Cn" };
                let byte = line_byte(cp, &line_break[cp], category, &width[cp], pictographic[cp]);
                if byte & CLASS_MASK == ID_CN {
                    byte
                } else {
                    byte & !UNASSIGNED_PICTOGRAPHIC
                }
            })
            .collect();
        Some(Self(bytes))
    }

    /// Returns the line byte of `ch`.
    fn get(&self, ch: char) -> u8 {
        self.0[ch as usize]
    }

    /// Returns the bytes for an iterator to read in place of the crate's,
    /// kept for the rest of the test run.
    fn leak(self) -> &'static [u8] {
        Box::leak(self.0.into_boxed_slice())
    }
}

/// One of the 16 option sets winkin breaks under.
#[derive(Copy, Clone, Debug)]
struct Key {
    options: LineOptions,
    name: &'static str,
}

/// Returns winkin's 16 option sets: `loose` and `normal` with each word
/// option, with and without the Japanese and Chinese tailoring; `strict`
/// with each word option; and `anywhere`. The Japanese and Chinese ones read
/// curly double quotes as brackets, as winkin's do.
fn keys() -> Vec<Key> {
    let mut keys = Vec::new();
    let words = [
        (LineWordOption::Normal, "normal"),
        (LineWordOption::BreakAll, "break-all"),
        (LineWordOption::KeepAll, "keep-all"),
    ];
    for (strictness, name) in [
        (LineStrictness::Loose, "loose"),
        (LineStrictness::Normal, "normal"),
    ] {
        for (word, word_name) in words {
            for ja_zh in [false, true] {
                keys.push(Key {
                    options: LineOptions {
                        strictness,
                        word,
                        ja_zh,
                        cj_quotes: ja_zh,
                        loose_hyphens: false,
                    },
                    name: key_name(name, word_name, ja_zh),
                });
            }
        }
    }
    for (word, word_name) in words {
        keys.push(Key {
            options: LineOptions {
                word,
                ..LineOptions::new()
            },
            name: key_name("strict", word_name, false),
        });
    }
    keys.push(Key {
        options: LineOptions {
            strictness: LineStrictness::Anywhere,
            ..LineOptions::new()
        },
        name: "anywhere",
    });
    keys
}

/// Returns a key's name, kept for the test run.
fn key_name(strictness: &str, word: &str, ja_zh: bool) -> &'static str {
    let language = if ja_zh { " ja" } else { "" };
    String::leak(std::format!("{strictness} {word}{language}"))
}

/// ICU4X's segmenter under one option set, without dictionaries.
struct Icu {
    segmenter: icu_segmenter::LineSegmenterBorrowed<'static>,
    /// Whether the text it reads has its curly double quotes swapped for
    /// brackets, as winkin swaps them.
    cj_quotes: bool,
}

impl Icu {
    fn new(options: LineOptions) -> Self {
        Self {
            segmenter: LineSegmenter::new_for_non_complex_scripts(icu_options(options)),
            cj_quotes: options.cj_quotes,
        }
    }

    /// Returns ICU4X's segmenter under `options` with its dictionaries, as
    /// winkin makes it.
    fn new_dictionary(options: LineOptions) -> Self {
        let icu = Self::new(options);
        Self {
            segmenter: LineSegmenter::new_dictionary(icu_options(options)),
            ..icu
        }
    }

    /// Returns ICU's breaks in `text`, both ends included.
    fn breaks(&self, text: &str) -> Vec<usize> {
        if self.cj_quotes && text.contains(['\u{201C}', '\u{201D}']) {
            let swapped: String = text
                .chars()
                .map(|ch| match ch {
                    '\u{201C}' => '\u{2045}',
                    '\u{201D}' => '\u{2046}',
                    ch => ch,
                })
                .collect();
            return self.segmenter.segment_str(&swapped).collect();
        }
        self.segmenter.segment_str(text).collect()
    }
}

/// Returns ICU4X's options for `options`.
fn icu_options(options: LineOptions) -> LineBreakOptions<'static> {
    static JAPANESE: icu_locale_core::LanguageIdentifier = langid!("ja");
    let mut icu = LineBreakOptions::default();
    icu.strictness = Some(match options.strictness {
        LineStrictness::Loose => LineBreakStrictness::Loose,
        LineStrictness::Normal => LineBreakStrictness::Normal,
        LineStrictness::Strict => LineBreakStrictness::Strict,
        LineStrictness::Anywhere => LineBreakStrictness::Anywhere,
    });
    icu.word_option = Some(match options.word {
        LineWordOption::Normal => LineBreakWordOption::Normal,
        LineWordOption::BreakAll => LineBreakWordOption::BreakAll,
        LineWordOption::KeepAll => LineBreakWordOption::KeepAll,
    });
    icu.content_locale = options.ja_zh.then_some(&JAPANESE);
    icu
}

/// Returns the crate's breaks in `text` under `options` and `config`, both
/// ends included, as ICU yields them.
fn breaks(text: &str, options: LineOptions, config: Config) -> Vec<usize> {
    with_ends(
        text,
        LineBreaks::with_config(Utf8Scalars::new(text), options, config),
    )
}

/// Returns `found` with 0 before it and the text's end after it, as ICU
/// yields breaks.
fn with_ends(text: &str, found: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut all = vec![0];
    all.extend(found);
    if !text.is_empty() {
        all.push(text.len());
    }
    all
}

/// Returns the configuration that reproduces ICU4X's segmenter
/// ([`icu_config`]), read from the pinned files once, or `None` where they
/// are absent.
fn icu_configuration() -> Option<Config> {
    static CONFIG: std::sync::OnceLock<Option<Config>> = std::sync::OnceLock::new();
    *CONFIG.get_or_init(|| {
        let classes = VersionClasses::unicode_15_1()?.leak();
        let graphemes = crate::tests::GraphemeVersion::new(ICU_VERSION)?.leak();
        Some(icu_config(classes, graphemes))
    })
}

/// The configuration that reproduces ICU4X's segmenter: Unicode 15.1's line
/// data and rules, ICU4X's quirks, and Unicode 17.0's graphemes.
fn icu_config(classes: &'static [u8], graphemes: &'static [crate::GraphemeProps]) -> Config {
    Config {
        classes: Some(classes),
        legacy: Legacy::UNICODE_15_1
            | Legacy::LOOSE_HOLDS
            | Legacy::TAILORING_FIRST
            | Legacy::TAILORING_OUTSIDE_RULES
            | Legacy::MARKS_APART
            | Legacy::DOTTED_PAIRS
            | Legacy::CONTEXT_FORGOTTEN
            | Legacy::SA_MARKS
            | Legacy::ICU_TAILORING
            | Legacy::KEEP_ALL_HYPHENS
            | Legacy::DICTIONARY_RUNS,
        graphemes_17: true,
        graphemes: Some(graphemes),
    }
}

#[test]
fn the_line_bytes_are_the_pinned_files() {
    let Some(classes) = VersionClasses::unicode_18() else {
        return;
    };
    for ch in (0..=0x10_FFFF).filter_map(char::from_u32) {
        assert_eq!(
            crate::props::line_props(ch).0,
            classes.get(ch),
            "U+{:04X}",
            ch as u32
        );
    }
}

#[test]
fn a_stream_ends_where_its_offsets_stop_increasing() {
    let options = LineOptions::new();
    let backwards = [(0, 'a'), (1, ' '), (2, 'b'), (2, ' '), (3, 'c')];
    let found: Vec<usize> = LineBreaks::new(backwards.into_iter(), options).collect();
    assert_eq!(found, [2]);
    let backwards = [(5, 'a'), (6, ' '), (2, 'b'), (9, 'c')];
    let found: Vec<usize> = LineBreaks::new(backwards.into_iter(), options).collect();
    assert_eq!(found, Vec::<usize>::new());
    // An offset that wraps ends the stream rather than looping.
    let wrapping = [(usize::MAX - 2, ' '), (usize::MAX - 1, 'a'), (0, 'c')];
    let mut breaks = LineBreaks::new(wrapping.into_iter(), options);
    assert_eq!(breaks.next(), Some(usize::MAX - 1));
    assert_eq!(breaks.next(), None);
    assert_eq!(breaks.next(), None);
    // Lookahead stops where offsets stop increasing, reading the end there:
    // a final quotation mark before the end is held (LB15b).
    let quote = [(0, 'a'), (1, ' '), (2, '\u{201D}'), (2, 'b')];
    let found: Vec<usize> = LineBreaks::new(quote.into_iter(), options).collect();
    assert_eq!(found, Vec::<usize>::new());
}

#[test]
fn the_state_keeps_its_size() {
    assert_eq!(size_of::<ScanState>(), 2);
    assert_eq!(size_of::<LineProps>(), 1);
    // The tests add what they read in place of the crate's data.
    let test = size_of::<TestState>();
    assert_eq!(size_of::<LineBreaks<'_, Utf8Scalars<'_>>>() - test, 104);
}

#[test]
fn the_pair_table_answers_as_the_rules_do() {
    let mut answered = 0;
    for left in 0..TABLE_STATES as u8 {
        for right in 0..CLASS_COUNT as u8 {
            let entry = PAIRS[usize::from(left)][usize::from(right)];
            if entry & PAIR_READS_MORE != 0 {
                continue;
            }
            answered += 1;
            for keep_all in [false, true] {
                let rules = Rules {
                    tailoring: Tailoring::None,
                    ja_zh: false,
                    keep_all,
                    loose_hyphens: false,
                    legacy: 0,
                };
                let bit = if keep_all {
                    PAIR_BREAKS_KEEP_ALL
                } else {
                    PAIR_BREAKS
                };
                for combination in 0..16u8 {
                    let flags = ((combination & 1) * LEFT_EAST_ASIAN)
                        | (((combination >> 1) & 1) * BEFORE_EAST_ASIAN)
                        | (((combination >> 2) & 1) * RIGHT_EAST_ASIAN)
                        | (((combination >> 3) & 1) * LEFT_UNASSIGNED_PICTOGRAPH);
                    for next in [UNREAD, EOT, NU, VF, SP, AL] {
                        let pair = Pair {
                            left,
                            right,
                            flags: flags | NEXT_EAST_ASIAN,
                            flags_unread: false,
                            ch: 'a',
                            next,
                            second: NU,
                        };
                        let answer = decide(&pair, &rules);
                        assert_eq!(
                            answer == BREAK,
                            entry & bit != 0,
                            "{left} {right} {flags} {next}"
                        );
                    }
                }
            }
        }
    }
    // Most pairs need nothing but their two classes.
    assert_eq!(answered, 4654);
}

#[test]
fn scanning_lines_allocates_nothing_warm() {
    let keys = keys();
    for corpus in testing::corpora::segmentation_corpora() {
        let text = corpus.text.as_str();
        for key in &keys {
            let allocations = testing::allocator::count_allocations(|| {
                core::hint::black_box(LineBreaks::new(Utf8Scalars::new(text), key.options).count());
            });
            assert_eq!(allocations, 0, "{} {}", corpus.name, key.name);
        }
    }
}

/// Characters the rules read apart, which random text meets too rarely.
const TRICKY: &[char] = &[
    ' ',
    '\n',
    '\r',
    '\u{0B}',
    '\u{85}',
    '\u{200B}',
    '\u{200D}',
    '\u{0308}',
    '\u{2060}',
    '\u{A0}',
    '«',
    '»',
    '“',
    '”',
    '"',
    '(',
    ')',
    '「',
    '」',
    '中',
    'あ',
    'ぁ',
    'ー',
    '〜',
    '々',
    '・',
    '！',
    '…',
    '-',
    '\u{2010}',
    '\u{2013}',
    '\u{2014}',
    '\u{AD}',
    '\u{25CC}',
    '\u{1B05}',
    '\u{1B44}',
    '\u{1BF2}',
    '0',
    '.',
    ',',
    '/',
    '$',
    '%',
    '￥',
    '％',
    '\u{1F1E6}',
    '\u{1F3FB}',
    '\u{261D}',
    '\u{1F8FF}',
    'א',
    '\u{0E01}',
    '\u{0E34}',
    '\u{FFFC}',
    '\u{1100}',
    '\u{1160}',
    '\u{11A8}',
    '가',
    'a',
    'Z',
];

#[test]
fn any_text_gives_valid_breaks() {
    // xorshift64, so the run is the same every time.
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut random = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let keys = keys();
    for _ in 0..2000 {
        let length = random() % 24;
        let mut units: Vec<u16> = Vec::new();
        let mut text = String::new();
        for _ in 0..length {
            let pick = random();
            if pick % 7 == 0 {
                // A lone surrogate, which only UTF-16 can hold.
                units.push(0xD800 + (pick >> 8) as u16 % 0x800);
                continue;
            }
            let ch = if pick % 3 == 0 {
                char::from_u32((pick >> 8) as u32 % 0x11_0000).unwrap_or('\u{FFFD}')
            } else {
                TRICKY[(pick >> 8) as usize % TRICKY.len()]
            };
            text.push(ch);
            let mut buffer = [0; 2];
            units.extend_from_slice(ch.encode_utf16(&mut buffer));
        }
        for key in &keys {
            let found: Vec<usize> = LineBreaks::new(Utf8Scalars::new(&text), key.options).collect();
            assert!(found.windows(2).all(|pair| pair[0] < pair[1]), "{text:?}");
            assert!(
                found
                    .iter()
                    .all(|&at| at > 0 && at < text.len() && text.is_char_boundary(at)),
                "{text:?}"
            );
            let found: Vec<usize> =
                LineBreaks::new(crate::Utf16Scalars::new(&units), key.options).collect();
            assert!(found.windows(2).all(|pair| pair[0] < pair[1]), "{units:X?}");
            assert!(
                found.iter().all(|&at| at > 0 && at < units.len()),
                "{units:X?}"
            );
        }
    }
}
