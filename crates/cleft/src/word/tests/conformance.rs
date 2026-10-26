//! UAX #29's word test files, Unicode 18.0's under the crate's data and
//! 17.0's under 17.0's; the word byte swept against the UCD; restarts; the
//! POSIX full stop; streams that misbehave; scans that allocate nothing.

use std::string::String;
use std::vec;
use std::vec::Vec;

use super::*;
use crate::tests::{TABLES_VERSION, WordVersion, break_test_cases, pinned_file};
use crate::{Utf8Scalars, Utf16Scalars};

/// Returns the boundaries `breaks` finds in `text`, with both ends, as the
/// test files mark them.
pub(super) fn with_ends(text: &str, breaks: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut found = vec![0];
    found.extend(breaks);
    if !text.is_empty() {
        found.push(text.len());
    }
    found
}

/// Returns the boundaries of `text` under `options`, with both ends.
fn boundaries(text: &str, options: WordOptions) -> Vec<usize> {
    with_ends(text, WordBreaks::new(Utf8Scalars::new(text), options))
}

#[test]
fn unicode_18_conformance_passes() {
    let Some(file) = pinned_file(TABLES_VERSION, "WordBreakTest.txt") else {
        return;
    };
    let cases = break_test_cases(&file);
    assert_eq!(cases.len(), 1944);
    let options = WordOptions::new();
    for (text, expected) in &cases {
        assert_eq!(&boundaries(text, options), expected, "{text:?}");
        // The same boundaries in UTF-16 code units.
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut found = vec![0];
        found.extend(WordBreaks::new(Utf16Scalars::new(&units), options));
        if !units.is_empty() {
            found.push(units.len());
        }
        let expected_units: Vec<usize> = expected
            .iter()
            .map(|&at| text[..at].encode_utf16().count())
            .collect();
        assert_eq!(found, expected_units, "{text:?} in UTF-16");
        // A scan from any boundary finds the boundaries after it.
        for &start in expected.iter().filter(|&&at| at > 0 && at < text.len()) {
            let from: Vec<usize> =
                WordBreaks::new(Utf8Scalars::with_start(text, start), options).collect();
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
fn unicode_17_conformance_passes_under_17_data() {
    let (Some(file), Some(classes)) = (
        pinned_file("17.0.0", "WordBreakTest.txt"),
        WordVersion::new("17.0.0"),
    ) else {
        return;
    };
    let classes = classes.leak();
    let cases = break_test_cases(&file);
    assert_eq!(cases.len(), 1944);
    for (text, expected) in &cases {
        let mut breaks = WordBreaks::new(Utf8Scalars::new(text), WordOptions::new());
        breaks.test.classes = Some(classes);
        assert_eq!(&with_ends(text, breaks), expected, "{text:?}");
    }
}

/// The word byte of every code point is the one the pinned 18.0 files give.
#[test]
fn the_word_byte_is_the_ucds() {
    let Some(version) = WordVersion::new(TABLES_VERSION) else {
        return;
    };
    for ch in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
        assert_eq!(
            crate::props::word_props(ch).0,
            version.get(ch),
            "U+{:04X}",
            u32::from(ch)
        );
    }
}

/// A scan from any boundary of the corpora finds the boundaries after it,
/// with the dictionaries.
#[test]
fn every_boundary_is_a_safe_restart() {
    let dictionaries = crate::tests::dictionaries();
    for corpus in testing::corpora::segmentation_corpora() {
        let text: String = corpus.text.chars().take(600).collect();
        for options in [
            WordOptions::new(),
            WordOptions {
                posix_full_stop: true,
            },
        ] {
            let scan = |start: usize| {
                WordBreaks::new(Utf8Scalars::with_start(&text, start), options)
                    .with_dictionaries(&dictionaries)
            };
            let whole: Vec<usize> = scan(0).collect();
            for &start in &whole {
                let from: Vec<usize> = scan(start).collect();
                let after: Vec<usize> = whole.iter().copied().filter(|&at| at > start).collect();
                assert_eq!(from, after, "{}: from {start}", corpus.name);
            }
        }
    }
}

/// Under the POSIX tailoring a full stop joins digits only, as ICU's
/// `en_US_POSIX` rules, which Blink breaks words by, have it: Chrome 153
/// selects `x`, `.` and `y` in `x.y`, and `3.14` whole.
#[test]
fn a_posix_full_stop_joins_only_digits() {
    let posix = WordOptions {
        posix_full_stop: true,
    };
    let segments = |text: &str, options| {
        let found = boundaries(text, options);
        found
            .windows(2)
            .map(|pair| String::from(&text[pair[0]..pair[1]]))
            .collect::<Vec<_>>()
    };
    assert_eq!(segments("x.y 3.14", posix), ["x", ".", "y", " ", "3.14"]);
    assert_eq!(
        segments("x.y 3.14", WordOptions::new()),
        ["x.y", " ", "3.14"]
    );
    assert_eq!(
        segments("a.1 2.b", posix),
        ["a", ".", "1", " ", "2", ".", "b"]
    );
    // Other middle letters still join letters.
    assert_eq!(segments("can't e:g", posix), ["can't", " ", "e:g"]);
    // A mark after the full stop attaches to it.
    assert_eq!(segments("x.\u{301}y", posix), ["x", ".\u{301}", "y"]);
}

/// A stream whose offsets stop increasing ends there, with no panic.
#[test]
fn offsets_that_do_not_increase_end_the_stream() {
    let scalars = [(0, 'a'), (1, ' '), (2, 'b'), (2, ' '), (3, 'c')];
    let found: Vec<usize> = WordBreaks::new(scalars.iter().copied(), WordOptions::new()).collect();
    assert_eq!(found, [1, 2]);
    // The lookahead stops there too: `a.` with the stream ending at the
    // repeated offset is a letter and a full stop at the end.
    let scalars = [(0, 'a'), (1, '.'), (1, 'b')];
    let found: Vec<usize> = WordBreaks::new(scalars.iter().copied(), WordOptions::new()).collect();
    assert_eq!(found, [1]);
}

/// Scanning words allocates nothing, with or without dictionaries.
#[test]
fn word_scans_allocate_nothing() {
    let dictionaries = crate::tests::dictionaries();
    for corpus in testing::corpora::segmentation_corpora() {
        let text: String = corpus.text.chars().take(4000).collect();
        let allocations = testing::allocator::count_allocations(|| {
            let count = WordBreaks::new(Utf8Scalars::new(&text), WordOptions::new())
                .with_dictionaries(&dictionaries)
                .count();
            let plain = WordBreaks::new(Utf8Scalars::new(&text), WordOptions::new()).count();
            assert!(count > 0, "{}: {plain}", corpus.name);
        });
        assert_eq!(allocations, 0, "{}", corpus.name);
    }
}

/// Returns the words of `text` with the dictionaries, joined by `|`.
fn words(text: &str) -> String {
    let dictionaries = crate::tests::dictionaries();
    let found = with_ends(
        text,
        WordBreaks::new(Utf8Scalars::new(text), WordOptions::new())
            .with_dictionaries(&dictionaries),
    );
    let pieces: Vec<&str> = found
        .windows(2)
        .map(|pair| &text[pair[0]..pair[1]])
        .collect();
    pieces.join("|")
}

/// Chinese and Japanese words are the dictionary's longest at each
/// position, and a miss is one character, which ICU4C's cost-based search
/// in Chrome 155 (`Intl.Segmenter`, word motion) gives on these too.
///
/// Where the two differ, Chrome's words are noted: it weighs word costs, so
/// it prefers `ま|した` and `い|ます` to the longest `まし|た` and `いま|す`.
#[test]
fn chinese_and_japanese_words_are_the_longest_in_the_dictionary() {
    assert_eq!(
        words("日本語の文章は、単語の間に空白を置かずに書かれます。"),
        "日本語|の|文章|は|、|単語|の|間|に|空白|を|置|か|ず|に|書|か|れ|ます|。"
    );
    assert_eq!(
        words("漢字とひらがな、カタカナ"),
        "漢字|と|ひ|ら|が|な|、|カタカナ"
    );
    assert_eq!(
        words("中文书写时词与词之间没有空格。"),
        "中文|书写|时|词|与|词|之间|没有|空格|。"
    );
    // Chrome: 行き|ま|した.
    assert_eq!(words("行きました"), "行き|まし|た");
    // With the Chinese and Japanese dictionary `々` reads as no letter, so
    // the Latin after it starts a word, as in Chrome.
    assert_eq!(words("時々abc"), "時々|abc");
}

/// Katakana goes to the Chinese and Japanese dictionary, as ICU4C's word
/// rules send it, and a run of at most eight that the dictionary's words do
/// not cover is one word. Chrome 155 parts each of these alike, in
/// `Intl.Segmenter` and in word motion, with or without `lang`.
///
/// Chrome's cost-based search differs on two: it parts
/// `ウィン|キン|コンピューター`, where the longest words give
/// `ウィン|キンコ|ン|ピ|ュ|ータ|ー`, and `ｺﾝﾋﾟｭｰﾀｰ|ｻｲｴﾝｽ`, whose halfwidth forms
/// it reaches through NFKC, where WB13 keeps the run whole.
#[test]
fn katakana_parts_into_words_as_chrome_parts_it() {
    for expected in [
        "コンピューター|サイエンス",
        "エディター|エディター",
        "ウィン|キン",
        "ソフトウェア|エンジニアリング",
        "データベース|管理|システム",
        "カタカナ|を|書き|ます",
        "ゲーム|ソフト",
        "グーグルマップ",
        "マイクロ|ソフト|オフィス",
        "ニュー|ヨーク|タイムズ",
        "スマート|フォン|アプリケーション|デベロッパー",
        "アメリカ|合衆国|大統領",
        "ジョン|・|スミス|さん",
        "ラーメン|屋|さん",
        "ヴァイオリン|と|ピアノ",
        "Wi|-|Fi|ルー|ター",
        "3D|プリンター",
        "abc|カタカナ|def",
        "カタカナ_abc",
        "ヽヾ|ゝ|ゞ",
        "す|ご|ー|い",
        "ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア|ア",
    ] {
        assert_eq!(words(&expected.replace('|', "")), expected);
    }
    assert_eq!(
        words("ウィンキンコンピューター"),
        "ウィン|キンコ|ン|ピ|ュ|ータ|ー"
    );
    assert_eq!(words("ｺﾝﾋﾟｭｰﾀｰｻｲｴﾝｽ"), "ｺﾝﾋﾟｭｰﾀｰｻｲｴﾝｽ");
}
