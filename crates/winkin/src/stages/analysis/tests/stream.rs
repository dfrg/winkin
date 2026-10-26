//! Line stream tests, over UAX #14's own test strings. They pin:
//! - a stream resumed at any opportunity or boundary matching the whole;
//! - the pair table breaking as the engine does wherever it answers;
//! - the stream's opportunities equalling the engine's under every key.

use cleft::{LineBreaks, Utf8Scalars};

use super::*;
use crate::tests::unicode_test_data;

/// Returns each test string of UAX #14 15.1's `LineBreakTest.txt` (ICU4X's
/// copy) as text. Skips strings in scripts that need dictionaries, which
/// the dictionary tests of `cleft` cover. `None` where the file is
/// absent.
fn line_break_strings() -> Option<Vec<String>> {
    let file = unicode_test_data("LineBreakTest-15.1.txt")?;
    let strings = file
        .lines()
        .filter_map(|line| {
            let data = line.split('#').next()?.trim();
            let text: String = data
                .split_whitespace()
                .filter(|token| !matches!(*token, "÷" | "×"))
                .map(|hex| {
                    u32::from_str_radix(hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .expect("a character")
                })
                .collect();
            let complex = text
                .chars()
                .any(|ch| unicode::core_props(ch).is_complex_context());
            (!text.is_empty() && !complex).then_some(text)
        })
        .collect();
    Some(strings)
}

/// The opportunities a stream from `from` finds at each character boundary
/// after it.
fn stream_breaks(text: &str, from: usize, cx: &AnalysisContext, key: LineKey) -> Vec<usize> {
    let mut stream = LineStream::new(text, from, &cx.dictionaries, key);
    text.char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .filter(|&at| at > from && stream.at(at))
        .collect()
}

fn keys() -> Vec<LineKey> {
    let mut keys = Vec::new();
    for strictness in [
        LineBreak::Loose,
        LineBreak::Normal,
        LineBreak::Strict,
        LineBreak::Anywhere,
    ] {
        for word in [WordBreak::Normal, WordBreak::BreakAll, WordBreak::KeepAll] {
            for language in ["en", "ja"] {
                let key = LineKey::new(
                    strictness,
                    word,
                    language == "ja",
                    LooseHyphens::JapaneseChinese,
                );
                if !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }
    keys
}

/// Resuming at any opportunity reproduces the whole paragraph's stream, for
/// every key, over UAX #14's own test strings. Resuming at a key change rests
/// on this.
#[test]
fn a_stream_started_at_an_opportunity_matches_the_whole() {
    let cx = AnalysisContext::new();
    let Some(strings) = line_break_strings() else {
        return;
    };
    assert!(strings.len() > 9000);
    let keys = keys();
    // Strict ignores the language, and anywhere the word option too.
    assert_eq!(keys.len(), 16);
    for key in keys {
        for text in &strings {
            let whole = stream_breaks(text, 0, &cx, key);
            for &from in whole.iter().filter(|&&at| at < text.len()) {
                let rest: Vec<_> = whole.iter().copied().filter(|&at| at > from).collect();
                assert_eq!(
                    stream_breaks(text, from, &cx, key),
                    rest,
                    "{key:?} {text:?} from {from}"
                );
            }
        }
    }
}

/// And the stream's own resume, at every character boundary, under one key,
/// changes nothing.
#[test]
fn resuming_under_the_same_key_changes_nothing() {
    let cx = AnalysisContext::new();
    let key = LineKey::UAX14;
    let Some(strings) = line_break_strings() else {
        return;
    };
    for text in strings {
        let whole = stream_breaks(&text, 0, &cx, key);
        let boundaries: Vec<_> = text
            .char_indices()
            .map(|(at, _)| at)
            .chain([text.len()])
            .collect();
        for &seam in boundaries.iter().filter(|&&at| at > 0) {
            let mut stream = LineStream::new(&text, 0, &cx.dictionaries, key);
            let mut found = Vec::new();
            for &at in boundaries.iter().filter(|&&at| at > 0) {
                if at == seam {
                    stream.resume(key, seam);
                }
                if stream.at(at) {
                    found.push(at);
                }
            }
            assert_eq!(found, whole, "{text:?} resumed at {seam}");
        }
    }
}

/// The engine's opportunities in `text` from its start, with the text's
/// end, which a stream reports too.
fn engine_breaks(text: &str, cx: &AnalysisContext, key: LineKey) -> Vec<usize> {
    let mut found: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), key.options())
        .with_dictionaries(&cx.dictionaries)
        .collect();
    if !text.is_empty() {
        found.push(text.len());
    }
    found
}

/// The pair table's opportunities in `text` from its start, where it covers
/// every character.
fn table_breaks(text: &str) -> Option<Vec<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while from < text.len() {
        let at = super::pairs::next_break(text, from)?;
        assert!(at > from, "{text:?} from {from}");
        found.push(at);
        from = at;
    }
    Some(found)
}

/// The keys the pair table answers under, one for each set of engine options:
/// every strictness but `anywhere`, in every language that tailors them. The
/// word options share the engine's.
fn pair_keys() -> Vec<LineKey> {
    let mut distinct: Vec<LineKey> = Vec::new();
    for key in keys().into_iter().filter(|key| key.breaks_by_pairs()) {
        if !distinct.iter().any(|kept| kept.options() == key.options()) {
            distinct.push(key);
        }
    }
    assert_eq!(distinct.len(), 5);
    distinct
}

/// The pair table breaks every pair of the characters it covers as the
/// engine does, under every key it answers for: so each character is in its
/// right class.
#[test]
fn the_pair_table_breaks_every_pair_it_covers_as_the_engine_does() {
    let cx = AnalysisContext::new();
    let covered: Vec<char> = (0..=0xFF_u8)
        .map(char::from)
        .filter(|&ch| super::pairs::covers(ch))
        .collect();
    // Tab, printable ASCII, and Latin-1 but its controls, `«`, `»`, `¤`,
    // `°` and `±`.
    assert_eq!(covered.len(), 187);
    let mut text = String::new();
    for key in pair_keys() {
        for &first in &covered {
            for &second in &covered {
                text.clear();
                text.push(first);
                text.push(second);
                if let Some(table) = table_breaks(&text) {
                    assert_eq!(table, engine_breaks(&text, &cx, key), "{key:?} {text:?}");
                }
            }
        }
    }
}

/// And every string of its classes up to four long, which holds each row of
/// the table to the engine after every class, and the contexts it keeps.
#[test]
fn the_pair_table_breaks_every_short_string_of_its_classes_as_the_engine_does() {
    let cx = AnalysisContext::new();
    // One character of each class: AL, NU, SP, BA, HY, IS, SY, EX, QU, OP,
    // CP, CL, GL, PR, PO, BB and HH.
    let classes = [
        'a', '1', ' ', '|', '-', ',', '/', '!', '"', '(', ')', '}', '\u{A0}', '$', '%', '\u{B4}',
        '\u{AD}',
    ];
    let keys = pair_keys();
    let mut text = String::new();
    for len in 1..=4_u32 {
        for mut code in 0..classes.len().pow(len) {
            text.clear();
            for _ in 0..len {
                text.push(classes[code % classes.len()]);
                code /= classes.len();
            }
            // Where the table declines, the stream asks the engine.
            let Some(table) = table_breaks(&text) else {
                continue;
            };
            for &key in &keys {
                assert_eq!(table, engine_breaks(&text, &cx, key), "{key:?} {text:?}");
            }
        }
    }
}

/// Text of the characters the stream hands over between the table and the
/// engine at: the table's own and those it leaves to the engine, `«` and `»`
/// and marks and controls, ideographs, Thai that needs the dictionaries, and
/// a paragraph's end.
fn mixed_strings() -> Vec<String> {
    let pool = [
        'a', 'Z', 'é', '1', ' ', ' ', '(', ')', '-', ',', '.', '"', '/', '!', '}', '$', '%', '\t',
        '\u{A0}', '\u{AD}', '\u{B4}', '«', '»', '°', '¤', '\u{301}', '\u{200B}', '\u{200D}',
        '\u{FFFC}', '\u{2014}', '\u{2010}', '\u{5D0}', '日', '本', '、', '\u{3005}', 'ก', 'า', 'ร',
        '😀', '\n', '\u{1}',
    ];
    // A fixed sequence, so that a failure is found again.
    let mut state: u32 = 0x2545_F491;
    let mut next = move |below: usize| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        usize::try_from(state).unwrap_or(0) % below
    };
    (0..1500)
        .map(|_| {
            let len = 1 + next(16);
            (0..len).map(|_| pool[next(pool.len())]).collect()
        })
        .collect()
}

/// The stream takes the table's opportunities where it answers and the
/// engine's where it hands over. Together they equal the engine's alone,
/// under every key. The test runs over UAX #14's own test strings and over
/// text that alternates between the two, starting at each of its characters
/// too.
#[test]
fn the_line_streams_opportunities_are_the_engines() {
    let cx = AnalysisContext::new();
    let keys = keys();
    // Runs of Thai as long as the dictionaries break, which the stream hands
    // to the engine whole.
    let thai = [
        "ภาษาไทยง่ายนิดเดียว",
        "ok ภาษาไทยง่ายนิดเดียว (see) สวัสดีครับ, well-known",
        "(ภาษาไทยง่ายนิดเดียว)สวัสดีครับ«a»",
    ];
    let Some(strings) = line_break_strings() else {
        return;
    };
    let whole = strings.into_iter().chain(thai.map(String::from));
    for text in whole {
        for &key in &keys {
            assert_eq!(
                stream_breaks(&text, 0, &cx, key),
                engine_breaks(&text, &cx, key),
                "{key:?} {text:?}"
            );
        }
    }
    for text in mixed_strings() {
        for &key in &keys {
            for (from, _) in text.char_indices() {
                let engine: Vec<_> = engine_breaks(&text[from..], &cx, key)
                    .into_iter()
                    .map(|at| from + at)
                    .collect();
                assert_eq!(
                    stream_breaks(&text, from, &cx, key),
                    engine,
                    "{key:?} {text:?} from {from}"
                );
            }
        }
    }
}
