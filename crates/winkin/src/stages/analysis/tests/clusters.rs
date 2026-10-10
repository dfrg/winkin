//! Cluster tests. They pin:
//! - grapheme clusters, against UAX #29's own test file and ICU's segmenter;
//! - Latin-1 characters and objects standing alone, and item boundaries
//!   dividing a grapheme;
//! - each cluster's class.

use super::*;
use crate::tests::unicode_test_data;

/// Grapheme boundaries in `text`, `boundaries`, with the boundary clusters add
/// inside a grapheme of a space and the marks after it.
///
/// UAX #14 sets no mark on a space and breaks between them, so the marks are
/// a cluster of their own. Such a grapheme is the only one that starts with
/// a space and goes on past it.
fn with_spaces_apart(text: &str, boundaries: &[usize]) -> Vec<usize> {
    let mut out = Vec::new();
    for pair in boundaries.windows(2) {
        out.push(pair[0]);
        if text.as_bytes().get(pair[0]) == Some(&b' ') && pair[1] > pair[0] + 1 {
            out.push(pair[0] + 1);
        }
    }
    out.extend(boundaries.last());
    out
}

/// The grapheme clusters are UAX #29 17.0's: `GraphemeBreakTest.txt`, from
/// ICU4X's copy of the Unicode file, every line, as one text preserved, but
/// for a space and its marks ([`with_spaces_apart`]).
///
/// The file has no U+FFFC; a boundary after one is the next test's.
#[test]
fn clusters_follow_the_grapheme_break_test() {
    let Some(file) = unicode_test_data("GraphemeBreakTest.txt") else {
        return;
    };
    let root = white_space(WhiteSpaceCollapse::Preserve);
    let mut cx = no_fonts();
    let mut layout = Layout::new();
    let mut cases = 0;
    for line in file.lines() {
        let data = line.split('#').next().unwrap_or_default().trim();
        if data.is_empty() {
            continue;
        }
        let mut text = String::new();
        let mut expected = Vec::new();
        for token in data.split_whitespace() {
            match token {
                "÷" => expected.push(text.len()),
                "×" => {}
                hex => text.push(
                    u32::from_str_radix(hex, 16)
                        .ok()
                        .and_then(char::from_u32)
                        .expect("a character"),
                ),
            }
        }
        // A CR never reaches the analysis. The builder writes a lone one as
        // a space and a CRLF as its LF.
        if text.contains('\r') {
            continue;
        }
        let mut builder = layout.builder(
            NodeKey(0),
            &ComputedBlockStyle::new(&root),
            BuildOptions::default(),
        );
        builder.text(key(1), &text);
        builder.finish(&mut cx);
        assert_eq!(layout.content().text, text, "preserved as written");
        let mut found = vec![0];
        let clusters = &analysis(&layout).clusters;
        found.extend(
            clusters
                .ids()
                .filter_map(|id| clusters.end(id))
                .map(|end| end.end().get()),
        );
        assert_eq!(found, with_spaces_apart(&text, &expected), "{line}");
        cases += 1;
    }
    // The file's 766, less the 75 with a CR.
    assert_eq!(cases, 691);
}

/// Every pair of Latin-1 characters has a grapheme boundary between them, but a
/// CRLF. This lets the writer skip the segmenter on Latin-1 text. The test checks
/// every pair against ICU.
#[test]
fn latin_1_characters_are_graphemes_of_their_own_but_a_crlf() {
    let cx = AnalysisContext::new();
    let mut text = String::new();
    for first in (0..=0xFF_u8).map(char::from) {
        for second in (0..=0xFF_u8).map(char::from) {
            text.clear();
            text.push(first);
            text.push(second);
            let found: Vec<_> = cx.graphemes().segment_str(&text).collect();
            let expected = if first == '\r' && second == '\n' {
                vec![0, 2]
            } else {
                vec![0, first.len_utf8(), text.len()]
            };
            assert_eq!(found, expected, "{text:?}");
        }
    }
}

/// Two characters whose Grapheme_Cluster_Break stands alone in the crate's
/// table (Other, a control, CR or LF) have a boundary between them too. This
/// lets the writer skip the segmenter beyond Latin-1.
///
/// The test takes every such code point up to plane 3 and in plane 14, and one
/// in 64 of the planes between and of private use. Each sits beside one
/// character of each kind the segmenter tells apart among them: a letter, a
/// pictograph, a conjunct's consonant, a control and an ideograph. The
/// segmenter reads the whole text at once.
#[test]
fn characters_that_stand_alone_are_graphemes_of_their_own() {
    let cx = AnalysisContext::new();
    let beside = ['a', '😀', 'क', '\u{1}', '日'];
    let mut text = String::new();
    let mut expected = vec![0];
    let mut count = 0;
    for cp in 0..=0x10_FFFF_u32 {
        let sampled = (0x4_0000..0xE_0000).contains(&cp) || cp >= 0xF_0000;
        if sampled && cp % 64 != 0 {
            continue;
        }
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        if !unicode::rare_props(ch)
            .grapheme_cluster_break()
            .stands_alone()
        {
            continue;
        }
        for piece in [beside[count % beside.len()], ch] {
            text.push(piece);
            expected.push(text.len());
        }
        count += 1;
    }
    assert!(count > 300_000, "{count}");
    let found: Vec<_> = cx.graphemes().segment_str(&text).collect();
    if let Some(at) = found.iter().zip(&expected).position(|(a, b)| a != b) {
        let near = text.get(expected[at.saturating_sub(2)]..expected[at + 1]);
        panic!("no boundary as expected near {near:?}");
    }
    assert_eq!(found, expected);
}

/// Clusters match ICU's graphemes in text that alternates between Latin-1 and
/// what joins characters into graphemes: marks, joiners, emoji sequences,
/// flags, Hangul syllables, conjuncts, prepended and spacing marks. The writer
/// restarts the segmenter wherever it skipped it.
#[test]
fn clusters_across_latin_1_and_the_rest_are_icus_graphemes() {
    let pool: Vec<char> = concat!(
        "aZé1 .\t\u{A0}\u{AD}©®",
        "\u{301}\u{308}\u{200D}\u{FE0F}👍\u{1F3FB}❤\u{1F1EB}\u{1F1F7}",
        "\u{1100}\u{1161}\u{11A8}क\u{94D}ष\u{903}\u{600}日\n",
    )
    .chars()
    .collect();
    // A fixed sequence, so that a failure is found again.
    let mut state: u32 = 0x9E37_79B9;
    let mut next = move |below: usize| {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        usize::try_from(state).unwrap_or(0) % below
    };
    let cx = AnalysisContext::new();
    for _ in 0..2000 {
        let len = 1 + next(20);
        let text: String = (0..len).map(|_| pool[next(pool.len())]).collect();
        let layout = pre(&text);
        assert_eq!(layout.content().text, text, "preserved as written");
        let graphemes: Vec<_> = cx.graphemes().segment_str(&text).collect();
        let graphemes = with_spaces_apart(&text, &graphemes);
        let expected: Vec<&str> = graphemes
            .windows(2)
            .map(|pair| &text[pair[0]..pair[1]])
            .collect();
        assert_eq!(clusters(&layout), expected, "{text:?}");
    }
}

/// A cluster always ends after U+FFFC, an atomic inline's or the caller's. A
/// mark after one is a cluster of its own, not a continuation.
#[test]
fn a_mark_after_an_object_does_not_join_it() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.atomic(key(2), &ComputedStyle::initial(), None, BoxSize::default());
        b.text(key(3), "\u{308}b");
    });
    assert_eq!(clusters(&layout), ["a", "\u{FFFC}", "\u{308}", "b"]);
    assert_eq!(continuations(&layout), [] as [usize; 0]);
    assert_eq!(
        classes(&layout),
        [
            ClusterClass::Text,
            ClusterClass::Object,
            ClusterClass::Text,
            ClusterClass::Text
        ]
    );

    let layout = plain("a\u{FFFC}\u{308}b");
    assert_eq!(clusters(&layout), ["a", "\u{FFFC}", "\u{308}", "b"]);
    assert_eq!(
        classes(&layout)[1],
        ClusterClass::Text,
        "the caller's U+FFFC is text"
    );

    // Nor is an atomic inline the rest of what precedes it, though UAX #29
    // joins a prepended character to whatever follows.
    let layout = build(|b| {
        b.text(key(1), "\u{600}");
        b.atomic(key(2), &ComputedStyle::initial(), None, BoxSize::default());
    });
    assert_eq!(clusters(&layout), ["\u{600}", "\u{FFFC}"]);
    assert_eq!(continuations(&layout), [] as [usize; 0]);
}

/// Every item boundary divides a grapheme, and the later part is a
/// continuation, so each cluster has one item and one style. A bare seam
/// between text nodes divides a grapheme too, so carets and fonts see the
/// same clusters.
#[test]
fn an_item_boundary_divides_a_grapheme_and_marks_the_rest() {
    let painted = styled(|style| style.paints = true);
    let initial = ComputedStyle::initial();
    for style in [&painted, &initial] {
        let layout = pieces(&[("e", &initial), ("\u{301}", style), ("x", &initial)]);
        assert_eq!(clusters(&layout), ["e", "\u{301}", "x"]);
        assert_eq!(continuations(&layout), [1]);
    }
    // A bare seam between two text nodes.
    let layout = build(|b| {
        b.text(key(1), "e");
        b.text(key(2), "\u{301}");
    });
    assert_eq!(clusters(&layout), ["e", "\u{301}"]);
    assert_eq!(continuations(&layout), [1]);
    // More than once.
    let layout = build(|b| {
        b.text(key(1), "e");
        b.text(key(2), "\u{301}");
        b.text(key(3), "\u{302}");
    });
    assert_eq!(continuations(&layout), [1, 3]);
    // And no line break inside it, even where every grapheme boundary is an
    // opportunity: after `a` and after the divided `e\u{301}`, not between.
    let anywhere = line_break(LineBreak::Anywhere);
    let layout = build_with(&ComputedBlockStyle::new(&anywhere), |b| {
        b.text(key(1), "ae");
        b.text(key(2), "\u{301}b");
    });
    assert_eq!(breaks(&layout), [1, 4]);
}

#[test]
fn a_clusters_class_comes_from_its_characters() {
    use ClusterClass as C;
    let layout = pre(" \t\u{A0}\u{2003}\u{3000}\u{202F}\u{2007}\u{AD}\u{200B}\u{2060}a");
    assert_eq!(
        classes(&layout),
        [
            C::Space,
            C::Tab,
            C::NoBreakSpace,
            C::OtherSpace,
            C::OtherSpace,
            C::NoBreakSpace,
            C::NoBreakSpace,
            C::SoftHyphen,
            C::ZeroWidthSpace,
            C::Control,
            C::Text,
        ]
    );
    // Emoji and symbols, by presentation.
    let cases = [
        ("🙂", C::Emoji),
        ("❤", C::Symbol),
        ("❤\u{FE0E}", C::Symbol),
        ("❤\u{FE0F}", C::Emoji),
        ("⌚", C::Emoji),
        ("⌚\u{FE0E}", C::Symbol),
        ("1\u{FE0F}\u{20E3}", C::Emoji),
        ("1", C::Text),
        ("🇫🇷", C::Emoji),
        ("🇦", C::Emoji),
        ("👍🏽", C::Emoji),
        ("❤\u{200D}🔥", C::Emoji),
        (
            "🏴\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}",
            C::Emoji,
        ),
        ("©", C::Symbol),
        ("©\u{FE0F}", C::Emoji),
    ];
    for (text, class) in cases {
        let layout = plain(text);
        assert_eq!(classes(&layout), [class], "{text:?}");
    }
    // A separator is a separator from the text and from a `<br>`.
    let layout = build(|b| {
        b.text(key(1), "a");
        b.line_break(key(2));
        b.text(key(3), "b");
    });
    assert_eq!(classes(&layout), [C::Text, C::Separator, C::Text]);
}

/// A U+200B the builder generates, for a `<wbr>` or the wrap opportunity
/// collapsing keeps, has a class of its own and is not shaped, as Blink's
/// control items are not. One the caller writes is shaped with its text. Each
/// is a break opportunity.
#[test]
fn a_generated_zero_width_space_is_a_class_of_its_own() {
    use ClusterClass as C;
    let layout = build(|b| {
        b.text(key(1), "a\u{200B}b");
        b.break_opportunity();
        b.text(key(1), "c");
        b.open_box(key(2), &nowrap(), None);
        b.text(key(3), " d ");
        b.close_box();
        b.text(key(4), " e");
    });
    assert_eq!(
        clusters(&layout),
        [
            "a", "\u{200B}", "b", "\u{200B}", "c", " ", "d", " ", "\u{200B}", "e"
        ]
    );
    assert_eq!(
        classes(&layout),
        [
            C::Text,
            C::ZeroWidthSpace,
            C::Text,
            C::BreakOpportunity,
            C::Text,
            C::Space,
            C::Text,
            C::Space,
            C::BreakOpportunity,
            C::Text,
        ]
    );
    let shaped: Vec<bool> = classes(&layout)
        .into_iter()
        .map(ClusterClass::is_shaped)
        .collect();
    assert_eq!(
        shaped,
        [true, true, true, false, true, true, true, true, false, true]
    );
    assert_eq!(breaks(&layout), [4, 8, 15], "after each");
}
