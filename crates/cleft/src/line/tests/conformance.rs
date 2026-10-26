//! UAX #14's test files: Unicode 18.0's under the crate's rules and data,
//! and ICU4X's copy of Unicode 15.1's under 15.1's.

use std::string::String;
use std::vec::Vec;

use super::*;
use crate::Utf16Scalars;
use crate::tests::break_test_cases;

/// Returns the cases of a line break test file, with the start of each
/// text among its breaks, as ICU yields them; the files mark no break there
/// (LB2).
pub(super) fn test_cases(file: &str) -> Vec<(String, Vec<usize>)> {
    let mut cases = break_test_cases(file);
    for (_, breaks) in &mut cases {
        if breaks.first() != Some(&0) {
            breaks.insert(0, 0);
        }
    }
    cases
}

#[test]
fn unicode_18_conformance_passes() {
    let Some(file) = pinned_file(TABLES_VERSION, "LineBreakTest.txt") else {
        return;
    };
    let cases = test_cases(&file);
    assert_eq!(cases.len(), 19346);
    let options = LineOptions::new();
    for (text, expected) in &cases {
        let found = with_ends(text, LineBreaks::new(Utf8Scalars::new(text), options));
        assert_eq!(&found, expected, "{text:?}");
        // The same breaks in UTF-16 code units.
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut found = vec![0];
        found.extend(LineBreaks::new(Utf16Scalars::new(&units), options));
        if !units.is_empty() {
            found.push(units.len());
        }
        let expected_units: Vec<usize> = expected
            .iter()
            .map(|&at| text[..at].encode_utf16().count())
            .collect();
        assert_eq!(found, expected_units, "{text:?} in UTF-16");
        // A scan from any break finds the breaks after it.
        for &start in expected.iter().filter(|&&at| at > 0 && at < text.len()) {
            let preceding = text[..start].chars().rev();
            let from: Vec<usize> = LineBreaks::with_preceding(
                Utf8Scalars::with_start(text, start),
                options,
                preceding,
            )
            .collect();
            let after: Vec<usize> = expected
                .iter()
                .copied()
                .filter(|&at| at > start && at < text.len())
                .collect();
            assert_eq!(from, after, "{text:?} from {start}");
        }
    }
}

#[test]
fn unicode_15_1_conformance_passes_under_15_1_data_and_rules() {
    let (Some(file), Some(classes)) = (
        pinned_file(ICU_LINE_VERSION, "LineBreakTest_15.1.txt"),
        VersionClasses::unicode_15_1(),
    ) else {
        return;
    };
    let config = Config {
        classes: Some(classes.leak()),
        legacy: Legacy::UNICODE_15_1,
        graphemes_17: true,
        graphemes: None,
    };
    let cases = test_cases(&file);
    assert_eq!(cases.len(), 10274);
    for (text, expected) in &cases {
        assert_eq!(
            &breaks(text, LineOptions::new(), config),
            expected,
            "{text:?}"
        );
    }
}

#[test]
fn every_break_is_a_safe_restart_under_every_option_set() {
    let Some(file) = pinned_file(TABLES_VERSION, "LineBreakTest.txt") else {
        return;
    };
    let cases = test_cases(&file);
    let corpora = testing::corpora::segmentation_corpora();
    // A few hundred characters of each corpus, past its seed's repeats.
    let samples = corpora
        .iter()
        .map(|corpus| corpus.text.chars().take(600).collect::<String>());
    let texts: Vec<String> = cases
        .into_iter()
        .map(|(text, _)| text)
        .chain(samples)
        .collect();
    for key in keys() {
        for text in &texts {
            let whole: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), key.options).collect();
            for &start in &whole {
                let from: Vec<usize> = LineBreaks::with_preceding(
                    Utf8Scalars::with_start(text, start),
                    key.options,
                    text[..start].chars().rev(),
                )
                .collect();
                let after: Vec<usize> = whole.iter().copied().filter(|&at| at > start).collect();
                assert_eq!(from, after, "{}: {text:?} from {start}", key.name);
            }
        }
    }
}

#[test]
fn options_change_at_a_break() {
    // Japanese under `strict`, then `loose` from the break before `々`: the
    // second part breaks before the iteration mark as `loose` does.
    let text = "日本 人々の";
    let strict = LineOptions::new();
    let loose = LineOptions {
        strictness: LineStrictness::Loose,
        ..LineOptions::new()
    };
    let whole: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), strict).collect();
    assert_eq!(whole, [3, 7, 13]);
    let seam = 7;
    let mut found: Vec<usize> = whole.iter().copied().filter(|&at| at <= seam).collect();
    found.extend(LineBreaks::with_preceding(
        Utf8Scalars::with_start(text, seam),
        loose,
        text[..seam].chars().rev(),
    ));
    assert_eq!(found, [3, 7, 10, 13]);
}

/// LB19a breaks beside a quotation mark between ideographs, as Chrome 155
/// does at `width: 1px` (its ICU is Unicode 17.0, whose LB19a is 18.0's):
/// before an initial mark and after a final one, never on the other side,
/// and not beside a letter.
#[test]
fn quotation_marks_between_ideographs_break_as_chrome_does() {
    let options = LineOptions {
        strictness: LineStrictness::Normal,
        ..LineOptions::new()
    };
    for expected in [
        "日|“日",
        "日”|日",
        "a“日",
        "日”a",
        "日\"日",
        "日|‘日",
        "日’|日",
        "日|«日",
        "日»|日",
        "「“日",
        "日 |“日",
    ] {
        assert_eq!(marked(&expected.replace('|', ""), options), expected);
    }
}

/// Returns `text` with a `|` at each break opportunity under `options`.
fn marked(text: &str, options: LineOptions) -> String {
    let breaks: Vec<usize> = LineBreaks::new(Utf8Scalars::new(text), options).collect();
    let mut marked = String::new();
    for (at, ch) in text.char_indices() {
        if breaks.contains(&at) {
            marked.push('|');
        }
        marked.push(ch);
    }
    marked
}

/// The strictness tailorings break as Chrome 155 does at `width: 0`, in
/// `lang="ja"` and `lang="en"` text. Its ICU4C reads each character a
/// strictness lets a line break before as a class no rule names, so the
/// rules that hold before any character still hold before it: after an
/// opening bracket, a quotation mark, an acute accent (BB), a word joiner, a
/// no-break space and `HL HY`. An East Asian postfix is still held after a
/// letter, and an East Asian prefix is not held before a letter or a digit.
/// The hyphens break after an ideograph only in Japanese and Chinese, or with
/// `break-all`, which reads letters and digits as ideographs, and start a
/// word there. `keep-all` breaks after a hyphen-minus.
#[test]
fn the_strictness_tailorings_break_as_chrome_does() {
    let options = |strictness, ja_zh, word| LineOptions {
        strictness,
        word,
        ja_zh,
        cj_quotes: ja_zh,
        loose_hyphens: false,
    };
    let normal_ja = options(LineStrictness::Normal, true, LineWordOption::Normal);
    let loose_ja = options(LineStrictness::Loose, true, LineWordOption::Normal);
    let loose = options(LineStrictness::Loose, false, LineWordOption::Normal);
    let normal = options(LineStrictness::Normal, false, LineWordOption::Normal);
    let keep_all = options(LineStrictness::Normal, false, LineWordOption::KeepAll);
    let loose_break_all = options(LineStrictness::Loose, false, LineWordOption::BreakAll);
    let cases = [
        (normal_ja, "漢|(〜|漢"),
        (normal_ja, "漢)|〜|漢"),
        (normal_ja, "漢\"〜|漢"),
        (normal_ja, "漢|«〜|漢"),
        (normal_ja, "漢|´〜|漢"),
        (normal_ja, "漢\u{2060}〜|漢"),
        (normal_ja, "漢\u{A0}〜|漢"),
        (normal_ja, "漢|א-〜|漢"),
        (normal_ja, "漢|〜|漢|漢"),
        (loose_ja, "漢|漢|々|漢"),
        (loose_ja, "漢|(々|漢"),
        (loose_ja, "漢|€|a|漢"),
        (loose_ja, "漢|a|€|漢"),
        (loose_ja, "漢|€)|漢"),
        (loose_ja, "漢|€|1|漢"),
        (loose_ja, "漢|1)€|漢"),
        (loose_ja, "漢|漢|‐|漢"),
        (loose_ja, "漢|‐a|漢"),
        (loose_ja, "漢…|‥|漢"),
        (loose_ja, "漢|漢|！|漢"),
        (loose_ja, "漢|(！|漢"),
        (loose, "漢|漢|々|漢"),
        (loose, "漢|(々|漢"),
        (loose, "漢|漢‐|漢"),
        (loose, "漢|漢〜|漢"),
        (loose, "漢…|‥|漢"),
        (normal, "漢…‥|漢"),
        (loose_break_all, "a|a|‐a"),
        (loose_break_all, "1|‐a"),
        (loose_break_all, "漢|漢|–|漢"),
        (loose_break_all, "漢|漢〜|漢"),
        (keep_all, "漢-|a漢"),
        (keep_all, "漢a-|漢"),
        (keep_all, "漢‐|a漢"),
    ];
    for (options, expected) in cases {
        assert_eq!(
            marked(&expected.replace('|', ""), options),
            expected,
            "{options:?}"
        );
    }
}

/// `loose_hyphens` breaks before U+2010 HYPHEN and U+2013 EN DASH after an
/// ideograph in any language, as CSS Text 3 allows and as the Japanese and
/// Chinese tailoring does; without it only that tailoring breaks there.
/// After a letter it changes nothing.
#[test]
fn loose_hyphens_break_after_ideographs_in_any_language() {
    let loose = LineOptions {
        strictness: LineStrictness::Loose,
        ..LineOptions::new()
    };
    let css = LineOptions {
        loose_hyphens: true,
        ..loose
    };
    let normal = LineOptions {
        strictness: LineStrictness::Normal,
        loose_hyphens: true,
        ..LineOptions::new()
    };
    let cases = [
        (loose, "文|文‐|文"),
        (loose, "文|文–|文"),
        (css, "文|文|‐|文"),
        (css, "文|文|–|文"),
        (css, "aa‐"),
        (normal, "文|文‐|文"),
    ];
    for (options, expected) in cases {
        assert_eq!(
            marked(&expected.replace('|', ""), options),
            expected,
            "{options:?}"
        );
    }
}

/// Unicode 18.0 makes U+2012 FIGURE DASH and U+2013 EN DASH BA rather than
/// HH, and forbids a break between BA and GL (LB12a). Chrome 155, on 17.0,
/// breaks otherwise, at `width: 0`; each case gives its answer, then the
/// crate's:
/// - before a no-break space after either dash: `漢–|\u{A0}漢`;
/// - after the dash in Hebrew, which 17.0's LB21a holds as `HL HH`:
///   `漢|א–漢`;
/// - under Japanese `loose`, before U+2012 after an ideograph, which ICU4C
///   breaks for all of HH, and after either dash before a letter, which
///   LB20a holds after a break: `漢|漢|‒|漢`, `漢|–a|漢`.
#[test]
fn unicode_18_dashes_break_apart_from_chrome() {
    let normal = LineOptions {
        strictness: LineStrictness::Normal,
        ..LineOptions::new()
    };
    let loose_ja = LineOptions {
        strictness: LineStrictness::Loose,
        word: LineWordOption::Normal,
        ja_zh: true,
        cj_quotes: true,
        loose_hyphens: false,
    };
    let cases = [
        (normal, "漢–\u{A0}|漢", "漢–\u{A0}漢"),
        (normal, "漢‒|\u{A0}漢", "漢‒\u{A0}漢"),
        (normal, "漢|א–漢", "漢|א–|漢"),
        (normal, "漢|א‒漢", "漢|א‒|漢"),
        (loose_ja, "漢|漢|‒|漢", "漢|漢‒|漢"),
        (loose_ja, "漢|–a|漢", "漢|–|a|漢"),
        (loose_ja, "漢|‒a|漢", "漢‒|a|漢"),
    ];
    for (options, chrome, ours) in cases {
        assert_eq!(chrome.replace('|', ""), ours.replace('|', ""));
        assert_eq!(marked(&ours.replace('|', ""), options), ours, "{options:?}");
    }
}
