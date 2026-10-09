//! Word motion tests. They pin:
//! - word stops on Windows and elsewhere, as Chrome's platforms have them;
//! - a run of punctuation and symbols as one word;
//! - Thai and Japanese words, beside Latin and without dictionaries;
//! - break opportunities leaving a word whole;
//! - word boundaries around each stop equal to the whole paragraph's.

use super::*;

/// Word motion on Windows goes forward to the next word's start and back to the previous word's start.
///
/// A comma and a forced break are words of their own (`selection.png`).
#[test]
fn word_motion_on_windows_skips_spaces() {
    let windows = WordMotion::SkipSpaces;
    let forward = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(windows);
    let backward = MotionDirection::Backward
        .moving(Granularity::Word)
        .with_word_motion(windows);
    let layout = laid("hi there, big world", 400.0);
    assert_eq!(walk(&layout, 0, forward), [0, 3, 8, 10, 14, 19]);
    assert_eq!(walk(&layout, 19, backward), [19, 14, 10, 8, 3, 0]);
    for (from, to) in [(1, 3), (4, 8), (9, 10), (11, 14), (15, 19)] {
        let at = moved(&layout, Selection::from(Position::from(from)), forward);
        assert_eq!(at.focus().offset, to, "forward from {from}");
    }
    for (from, to) in [(2, 0), (7, 3), (9, 8), (13, 10), (17, 14)] {
        let at = moved(&layout, Selection::from(Position::from(from)), backward);
        assert_eq!(at.focus().offset, to, "backward from {from}");
    }
    // A full stop between letters parts two words, and between digits none.
    let layout = laid("x.y 3.14 end", 400.0);
    assert_eq!(walk(&layout, 0, forward), [0, 1, 2, 4, 9, 12]);
    // A forced break is a stop, both ways.
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "ab");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "cd ef");
    });
    assert_eq!(walk(&layout, 0, forward), [0, 2, 3, 6, 8]);
    assert_eq!(walk(&layout, 8, backward), [8, 6, 3, 2, 0]);
    // Collapsed white space counts once, and a wrap is not seen.
    let layout = laid("abc   def", 400.0);
    assert_eq!(walk(&layout, 0, forward), [0, 4, 7]);
    let layout = laid("one two three four five", 100.0);
    assert_eq!(walk(&layout, 0, forward), [0, 4, 8, 14, 19, 23]);
}

/// A run of punctuation and symbols is one word, as Chrome 155 takes it on
/// Windows: Ctrl+Right crosses `,,,,` or `.,;:` in one step, but stops at
/// each of `x.y,z`, where letters part them.
#[test]
fn a_run_of_punctuation_is_one_word() {
    let forward = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::SkipSpaces);
    let backward = MotionDirection::Backward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::SkipSpaces);
    for (text, stops) in [
        ("a,,,,b cd", &[0, 1, 5, 7, 9][..]),
        ("a.,;:b", &[0, 1, 5, 6]),
        ("a ,, b", &[0, 2, 5, 6]),
        ("x.y,z", &[0, 1, 2, 3, 4, 5]),
        ("a + b", &[0, 2, 4, 5]),
        ("a ++ b c", &[0, 2, 5, 7, 8]),
        ("12,,34", &[0, 2, 4, 6]),
        ("a$$b", &[0, 1, 3, 4]),
        ("a()b", &[0, 1, 3, 4]),
        ("a\u{2022}\u{2022},,b", &[0, 1, 9, 10]),
        ("a\u{1F600}\u{1F600}b", &[0, 1, 9, 10]),
    ] {
        let layout = laid(text, 400.0);
        assert_eq!(walk(&layout, 0, forward), stops, "{text:?}");
        let mut back: Vec<usize> = stops.to_vec();
        back.reverse();
        assert_eq!(walk(&layout, text.len(), backward), back, "{text:?}");
    }
}

/// On macOS and Linux a forward word motion stops at the word's end, as Blink's `NextWordPositionForPlatform`.
///
/// Backward it stops at the word's start, as on Windows.
#[test]
fn word_motion_elsewhere_stops_at_a_words_end() {
    let layout = laid("hi there, big world", 400.0);
    let forward = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::StopAtWordEnd);
    assert_eq!(walk(&layout, 0, forward), [0, 2, 8, 9, 13, 19]);
}

/// A motion carries the platform's word stops, so one layout moves both ways from two calls.
///
/// By default a word motion stops as Chrome does on the target platform.
#[test]
fn a_motion_carries_the_platforms_choices() {
    let motion = MotionDirection::Forward.moving(Granularity::Word);
    assert_eq!(motion.word_motion, WordMotion::PLATFORM);
    let platform = if cfg!(windows) {
        WordMotion::SkipSpaces
    } else {
        WordMotion::StopAtWordEnd
    };
    assert_eq!(WordMotion::PLATFORM, platform);
    assert_eq!(WordMotion::default(), platform);
    let extending = MotionDirection::Right
        .extending(Granularity::Character)
        .with_word_motion(WordMotion::StopAtWordEnd);
    assert!(extending.extend);
    assert_eq!(extending.word_motion, WordMotion::StopAtWordEnd);
    let layout = laid("hi there, big world", 400.0);
    let from = Selection::from(Position::from(0));
    let skip = moved(
        &layout,
        from,
        motion.with_word_motion(WordMotion::SkipSpaces),
    );
    let stop = moved(
        &layout,
        from,
        motion.with_word_motion(WordMotion::StopAtWordEnd),
    );
    assert_eq!((skip.focus().offset, stop.focus().offset), (3, 2));
}

/// Returns Chrome's positions in UTF-16 units as byte offsets into `text`.
fn from_utf16(text: &str, units: &[usize]) -> Vec<usize> {
    units
        .iter()
        .map(|&unit| {
            let mut counted = 0;
            text.char_indices()
                .find(|&(_, ch)| {
                    let here = counted;
                    counted += ch.len_utf16();
                    here >= unit
                })
                .map_or(text.len(), |(at, _)| at)
        })
        .collect()
}

/// Word motion over a Thai and a Japanese sentence stops where Chrome 153's does on Windows.
///
/// With the `dictionaries` feature, ICU4X finds the words ICU4C finds for
/// Chrome: `ฉัน|ชอบ|กิน|ข้าว|…` and
/// `私|は|コンピューター|で|日本語|の|テキスト|を|書き|ます|。`.
/// Without it, the LSTM finds the same Thai words. UAX #29 then makes each
/// ideograph and hiragana a word, and a katakana run one word.
#[test]
fn word_motion_finds_thai_and_japanese_words() {
    let forward = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::SkipSpaces);
    let backward = MotionDirection::Backward.moving(Granularity::Word);
    let chrome_thai = [0, 3, 6, 9, 13, 16, 19, 22, 26, 29, 32, 35, 39];
    #[cfg(feature = "dictionaries")]
    let japanese = [0, 1, 2, 9, 10, 13, 14, 18, 19, 21, 23, 24];
    #[cfg(not(feature = "dictionaries"))]
    let japanese = [0, 1, 2, 9, 10, 11, 12, 13, 14, 18, 19, 20, 21, 22, 23, 24];
    for (text, stops) in [(THAI, &chrome_thai[..]), (JAPANESE, &japanese[..])] {
        let layout = laid(text, 400.0);
        let stops = from_utf16(text, stops);
        assert_eq!(walk(&layout, 0, forward), stops, "{text}");
        let back: Vec<usize> = stops.iter().rev().copied().collect();
        assert_eq!(walk(&layout, text.len(), backward), back, "{text}");
    }
}

/// Where a Thai run meets Latin letters or digits, ICU4X breaks, so `ภาษา|abc|ไทย 123|ไทย` is five words.
///
/// ICU4X always breaks before a dictionary run and after one of two or more
/// characters. ICU4C counts Thai as a letter that joins its neighbours, so
/// Chrome 153 stops only at `ภาษาabcไทย` and `123ไทย`. The difference is
/// ICU4X's segmenting and is recorded, not worked around.
#[test]
fn thai_beside_latin_is_parted_where_icu4x_parts_it() {
    let text = "\u{E20}\u{E32}\u{E29}\u{E32}abc\u{E44}\u{E17}\u{E22} 123\u{E44}\u{E17}\u{E22}";
    let layout = laid(text, 400.0);
    let forward = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::SkipSpaces);
    assert_eq!(
        walk(&layout, 0, forward),
        from_utf16(text, &[0, 4, 7, 11, 14, 17])
    );
    let chrome = from_utf16(text, &[0, 11, 17]);
    assert_ne!(walk(&layout, 0, forward), chrome);
}

/// Without dictionaries, Chinese and Japanese words are UAX #29's own: an
/// ideograph and a hiragana are words of their own (Word_Break Other,
/// WB999), and a run of katakana one (WB13), which joins an underscore and
/// what follows it (ExtendNumLet, WB13a, WB13b); `々` is a letter (ALetter),
/// which joins the Latin after it (WB5); and an underscore joins digits
/// after it, but not an ideograph before it. So
/// `時|々abc|と|東|京|_2024|年|の|カタ_カナ|。`.
#[cfg(not(feature = "dictionaries"))]
#[test]
fn without_dictionaries_chinese_and_japanese_words_are_uax_29s() {
    let text = "\u{6642}\u{3005}abc\u{3068}\u{6771}\u{4EAC}_2024\u{5E74}\u{306E}\u{30AB}\u{30BF}_\u{30AB}\u{30CA}\u{3002}";
    let layout = laid(text, 400.0);
    let forward = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::SkipSpaces);
    let stops = from_utf16(text, &[0, 1, 5, 6, 7, 8, 13, 14, 15, 20, 21]);
    assert_eq!(walk(&layout, 0, forward), stops);
    let back: Vec<usize> = stops.iter().rev().copied().collect();
    let backward = MotionDirection::Backward.moving(Granularity::Word);
    assert_eq!(walk(&layout, text.len(), backward), back);
}

/// A `<wbr>` inside a word leaves it one word.
#[test]
fn a_break_opportunity_leaves_a_word_whole() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "super");
        b.break_opportunity();
        b.text(NodeKey(1), "cali fragile");
    });
    let forward = walk(
        &layout,
        0,
        MotionDirection::Forward
            .moving(Granularity::Word)
            .with_word_motion(WordMotion::SkipSpaces),
    );
    assert_eq!(forward, [0, 13, 20]);
    let backward = walk(
        &layout,
        20,
        MotionDirection::Backward.moving(Granularity::Word),
    );
    assert_eq!(backward, [20, 13, 0]);
    // The words either side of one are read without it: a full stop
    // between letters parts them, and between digits joins them.
    let joined = |before: &str, after: &str| {
        laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
            b.text(NodeKey(1), before);
            b.break_opportunity();
            b.text(NodeKey(1), after);
        })
    };
    let forward = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::SkipSpaces);
    assert_eq!(walk(&joined("x.", "y z"), 0, forward), [0, 1, 2, 7, 8]);
    assert_eq!(walk(&joined("3.", "14 z"), 0, forward), [0, 8, 9]);
    assert_eq!(walk(&joined("can", "'t go"), 0, forward), [0, 9, 11]);
    // A word ending at one ends on the side a forward motion reaches first,
    // as a character motion stops there.
    let end = MotionDirection::Forward
        .moving(Granularity::Word)
        .with_word_motion(WordMotion::StopAtWordEnd);
    assert_eq!(walk(&joined("abc", " def"), 0, end), [0, 3, 10]);
}

/// A run too long for the stack copy is segmented in the pieces its `<wbr>`s part.
///
/// In long Thai with a `<wbr>`, a word starts at each stop near it where each
/// side segmented alone has one, and at the `<wbr>`.
#[test]
fn a_long_run_finds_words_on_each_side_of_its_wbr() {
    use crate::unicode::word_segmenter;
    use words::Words;
    // "Thai" 60 times on each side: 540 bytes a side, 1,080 the run.
    let piece = "\u{E44}\u{E17}\u{E22}".repeat(60);
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), &piece);
        b.break_opportunity();
        b.text(NodeKey(2), &piece);
    });
    let words: Vec<usize> = word_segmenter().segment_str(&piece).collect();
    // The far side, after the U+200B.
    let far = piece.len() + '\u{200B}'.len_utf8();
    let near = (piece.len() - 18..=piece.len()).map(|at| (at, words.contains(&at)));
    let beyond = (far..=far + 18).map(|at| (at, words.contains(&(at - far))));
    let text = layout.text();
    for (at, starts) in near
        .chain(beyond)
        .filter(|&(at, _)| text.is_char_boundary(at))
    {
        if is_stop(&layout, at) {
            assert_eq!(
                Words::new(&layout).starts_word(cluster(&layout, at)),
                starts,
                "at {at}"
            );
        }
    }
    assert!(Words::new(&layout).starts_word(cluster(&layout, piece.len())));
}

/// Word boundaries found around each stop equal those of the whole paragraph segmented at once.
///
/// The whole paragraph is segmented without builder break opportunities.
/// The text is generated from a wide mix: letters, digits, joining
/// punctuation, spaces, lone and split marks, format characters, joiners,
/// pictographs, regional indicators, CJK, Hebrew, Southeast Asian scripts,
/// `<wbr>`s, atomic inlines and forced breaks. Each stop is asked of fresh
/// words, and of one motion's words in order each way.
#[test]
fn words_around_a_stop_are_the_whole_paragraphs() {
    use crate::data::{Id, IdRange};
    use crate::stages::analysis::{ClusterClass, ClusterId};
    use crate::unicode::{self, segmenter_char, word_boundaries, word_segmenter};
    use alloc::collections::BTreeMap;
    use words::Words;

    // Letters, digits and the punctuation the rules join across; spaces; a
    // mark on a letter and one alone; a soft hyphen, a mark and a U+200B the
    // caller wrote; a joiner, a pictograph and regional indicators; katakana
    // and the prolonged sound mark, an ideograph, Hebrew and Thai; Thai
    // words and a mark, Lao, Khmer and Myanmar; ideographs, hiragana and the
    // ideographic iteration mark, which UAX #29 makes a letter.
    #[rustfmt::skip]
    const PIECES: &[&str] = &[
        "a", "b", "Z", "1", "2", "'", ".", ",", ":", ";", "_", "-", "!", "\"", "\u{2019}", "\u{DF}",
        " ", " ", "  ", "\t", "\u{A0}",
        "e\u{301}", "\u{301}",
        "\u{AD}", "\u{200E}", "\u{200B}",
        "\u{200D}", "\u{1F600}", "\u{1F1FA}", "\u{1F1F8}",
        "\u{30AB}", "\u{30FC}", "\u{5B57}", "\u{5D0}", "\u{E01}",
        "\u{E20}\u{E32}\u{E29}\u{E32}", "\u{E44}\u{E17}\u{E22}", "\u{E31}", "\u{E81}\u{EB2}",
        "\u{1780}\u{17B6}", "\u{1019}\u{103C}\u{1014}\u{103A}", "\u{65E5}\u{672C}", "\u{306E}",
        "\u{306F}", "\u{3005}",
    ];
    // Whether a word starts and ends at each caret stop, from each whole
    // paragraph without builder break opportunities, each character as the
    // segmenter is given it.
    let whole = |layout: &Layout| {
        let analysis = layout.analysis();
        let clusters = &analysis.clusters;
        let text = layout.text();
        let segmenter = word_segmenter();
        let mut found: BTreeMap<usize, (bool, bool)> = BTreeMap::new();
        for (id, _) in analysis.paragraphs.iter() {
            let range = analysis.paragraphs.clusters(id);
            let mut words = String::new();
            let mut places = Vec::new();
            // Where each cluster of text starts in `words`, and its first character.
            let mut firsts: Vec<(usize, char)> = Vec::new();
            let indicator = |cluster: usize| {
                let at = clusters.start(ClusterId::new(cluster)).get();
                text[at..]
                    .chars()
                    .next()
                    .is_some_and(|ch| unicode::core_props(ch).is_regional_indicator())
            };
            for cluster in range.clone().ids() {
                let at = clusters.range(cluster);
                places.push((at.start.get(), words.len()));
                let generated = clusters
                    .attrs(cluster)
                    .is_some_and(|attrs| attrs.class() == ClusterClass::BreakOpportunity);
                // Left out, but between regional indicators, which it keeps
                // apart.
                let apart = cluster.get() > 0
                    && indicator(cluster.get() - 1)
                    && indicator(cluster.get() + 1);
                if !generated && let Some(first) = text[at.start.get()..at.end.get()].chars().next()
                {
                    firsts.push((words.len(), first));
                }
                if !generated || apart {
                    for ch in text[at.start.get()..at.end.get()].chars() {
                        words.push(segmenter_char(ch).unwrap_or(ch));
                    }
                }
            }
            places.push((clusters.start(range.end).get(), words.len()));
            let mut segments = Vec::new();
            let mut from = 0;
            // A run of punctuation and symbols is one word, as Chrome takes it.
            let punctuation = |ch: char| {
                !(ch.is_alphanumeric()
                    || ch.is_whitespace()
                    || ch.is_control()
                    || ch == '\u{FFFC}'
                    || unicode::core_props(ch).is_regional_indicator()
                    || unicode::rare_props(ch).is_default_ignorable())
            };
            let joined = |to: usize| {
                let after = firsts.iter().find(|&&(at, _)| at == to);
                let before = firsts.iter().rev().find(|&&(at, _)| at < to);
                matches!((before, after), (Some(&(_, b)), Some(&(_, a))) if punctuation(b) && punctuation(a))
            };
            for to in word_boundaries(segmenter, &words, 0) {
                if to <= from || (to < words.len() && joined(to)) {
                    continue;
                }
                let white = words[from..to].chars().all(|ch| {
                    matches!(
                        ch,
                        ' ' | '\t' | '\u{A0}' | '\u{1680}' | '\u{2000}'
                            ..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}'
                    )
                });
                segments.push((from, to, !white));
                from = to;
            }
            for (at, place) in places {
                let starts = segments.iter().any(|&(s, _, word)| s == place && word);
                let ends = segments.iter().any(|&(_, e, word)| e == place && word);
                let entry = found.entry(at).or_default();
                *entry = (entry.0 | starts, entry.1 | ends);
            }
        }
        found
    };
    let mut seed = 0x2545_F491_4F6C_DD1D_u64;
    let mut next = |below: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % below as u64) as usize
    };
    let preserved = styled(|style| style.text.white_space_collapse = WhiteSpaceCollapse::Preserve);
    let bigger = styled(|style| style.font.size = 30.0);
    for case in 0..500 {
        let count = 1 + next(20);
        let picks: Vec<usize> = (0..count).map(|_| next(PIECES.len() + 4)).collect();
        let style = if case % 2 == 0 { ahem() } else { preserved };
        let layout = laid_with(&ComputedBlockStyle::new(&style), 200.0, |b| {
            for (key, &pick) in picks.iter().enumerate() {
                let key = NodeKey(10 + key as u64);
                match pick.checked_sub(PIECES.len()) {
                    None => b.text(key, PIECES[pick]),
                    Some(0) => b.break_opportunity(),
                    Some(1) => {
                        let size = BoxSize {
                            inline: 20.0,
                            block: 20.0,
                            baseline: None,
                        };
                        b.atomic(key, &ahem(), None, size);
                    }
                    Some(2) => b.line_break(key),
                    _ => {
                        // A mark in a span of its own, which splits its
                        // grapheme.
                        b.open_box(key, &bigger, None);
                        b.text(NodeKey(9), "\u{301}");
                        b.close_box();
                    }
                }
            }
        });
        let stops: Vec<(usize, (bool, bool))> = whole(&layout)
            .into_iter()
            .filter(|&(at, _)| is_stop(&layout, at))
            .collect();
        let text = layout.text();
        for &(at, expected) in &stops {
            let stop = cluster(&layout, at);
            let fresh = (
                Words::new(&layout).starts_word(stop),
                Words::new(&layout).ends_word(stop),
            );
            assert_eq!(fresh, expected, "case {case}: {text:?} at {at}");
        }
        for order in [false, true] {
            let mut words = Words::new(&layout);
            let mut each = |&(at, expected): &(usize, (bool, bool))| {
                let stop = cluster(&layout, at);
                let asked = (words.starts_word(stop), words.ends_word(stop));
                assert_eq!(asked, expected, "case {case}, in order: {text:?} at {at}");
            };
            if order {
                stops.iter().rev().for_each(&mut each);
            } else {
                stops.iter().for_each(&mut each);
            }
        }
    }
}
