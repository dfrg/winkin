//! Hyphenation and phrases the host finds:
//! - building again with a hyphenator or a phrase segmenter allocates
//!   nothing;
//! - breaking such a layout again allocates nothing.

use super::test_fonts::{self, han, han_fallback};
use super::{count_allocations, relayout, text};
use winkin::segment::{Hyphenator, PhraseSegmenter};
use winkin::style::{ComputedStyle, Hyphens, Language, TextGroup, WordBreak};
use winkin::{BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey};

/// A hyphenator that allows a break after every third character of a word,
/// and allocates nothing.
struct EveryThird;

impl Hyphenator for EveryThird {
    fn hyphenate(&self, word: &str, _: Language, found: &mut dyn FnMut(usize)) {
        for (index, (offset, _)) in word.char_indices().enumerate() {
            if index % 3 == 0 {
                found(offset);
            }
        }
    }
}

/// A phrase segmenter that starts a phrase after each `は`, `の`, `で` and
/// `、`, and allocates nothing.
struct Particles;

impl PhraseSegmenter for Particles {
    fn segment(&self, text: &str, _: Language, found: &mut dyn FnMut(usize)) {
        for (offset, ch) in text.char_indices() {
            if matches!(ch, 'は' | 'の' | 'で' | '、') {
                found(offset + ch.len_utf8());
            }
        }
    }
}

/// Builds `text` in paragraphs in `language`, under `hyphens: auto`, and
/// `word-break: auto-phrase` for Japanese, half of each paragraph in a
/// span.
fn prose(layout: &mut Layout, cx: &mut Context, language: &str, text: &str) {
    let root = ComputedStyle {
        text: TextGroup {
            language: Language::parse(language).ok(),
            hyphens: Hyphens::Auto,
            word_break: if language == "ja" {
                WordBreak::AutoPhrase
            } else {
                WordBreak::Normal
            },
            ..TextGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut b = layout.builder(
        cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    for (at, paragraph) in (0..).zip(text.split('\n')) {
        let half = paragraph
            .char_indices()
            .nth(paragraph.chars().count() / 2)
            .map_or(paragraph.len(), |(offset, _)| offset);
        let (first, second) = paragraph.split_at(half);
        b.text(NodeKey(4 * at), first);
        b.open_box(NodeKey(4 * at + 1), &root, None);
        b.text(NodeKey(4 * at + 2), second);
        b.close_box();
        b.line_break(NodeKey(4 * at + 3));
    }
    assert!(b.finish().is_complete());
}

/// Building and breaking again with a hyphenator and a phrase segmenter
/// allocates nothing.
///
/// The words, the offsets found and the points kept live in the analysis
/// scratch, and the opportunities that draw a hyphen in the layout's
/// clusters, which keep their capacity.
#[test]
fn hyphenating_and_phrasing_allocate_nothing_warm() {
    let documents = [
        (
            "en",
            "Hyphenation breaks extraordinarily long words, (parenthetically) and \u{201C}quoted\u{201D}.",
        ),
        (
            "ja",
            "日本語の文章は、漢字と仮名で書かれる。私の東京での暮らしは楽しい。",
        ),
    ];
    let fonts = test_fonts::collection(&[han()], han_fallback("Test Han"));
    let mut cx = Context::new(fonts);
    cx.set_hyphenator(Some(Box::new(EveryThird)));
    cx.set_phrase_segmenter(Some(Box::new(Particles)));
    let widths = [37.0, 81.25, 12.0, 250.0];
    for (language, line) in documents {
        let mut layout = Layout::new();
        let same = text(line, 8);
        let cold = count_allocations(|| {
            prose(&mut layout, &mut cx, language, &same);
            relayout(&mut layout, &mut cx, &widths);
        });
        assert!(cold > 0, "a cold layout grows");
        let warm = count_allocations(|| prose(&mut layout, &mut cx, language, &same));
        assert_eq!(warm, 0, "{language}: rebuilding allocated");
        relayout(&mut layout, &mut cx, &widths);
        let warm = count_allocations(|| {
            assert!(relayout(&mut layout, &mut cx, &widths) > 0);
        });
        assert_eq!(warm, 0, "{language}: breaking again allocated");
    }
}
