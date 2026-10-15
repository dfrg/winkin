//! Tests of the breaks a host's hyphenator and phrase segmenter find. They
//! pin:
//! - the words a hyphenator is asked about: trimmed, long enough, in the
//!   text's language, not capitalized in English, never without a language
//!   or under `hyphens: manual`;
//! - the opportunities kept: two characters on each side, and none of the
//!   offsets a hyphenator gets wrong;
//! - lines breaking there with the `hyphenate-character`, at min-content
//!   too, and the text's last word kept whole where an earlier break fits;
//! - `auto-phrase` breaking Japanese only between phrases, as `normal`
//!   where no phrase fits or with no segmenter, and holding soft hyphens
//!   back.
//!
//! The expected lines follow Chrome's line breaker, whose `Hyphenation`
//! and phrase segmentation these traits stand in for. In Ahem at 10px
//! every character, the hyphen included, is 10px wide.

use alloc::boxed::Box;
use std::sync::Mutex;

use super::*;
use crate::segment::{Hyphenator, PhraseSegmenter};
use crate::stages::analysis::ClusterId;
use crate::style::{Hyphens, Language};
use crate::tests::{ahem, han, han_fallback, texts};

/// A hyphenator with a fixed list of words and their breaks, for English
/// and German, that logs every word it is asked about.
#[derive(Default)]
struct Toy {
    asked: Log,
}

/// The words a [`Toy`] is asked about, each with its language.
type Log = Arc<Mutex<Vec<(String, String)>>>;

impl Hyphenator for Toy {
    fn hyphenate(&self, word: &str, language: Language, found: &mut dyn FnMut(usize)) {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push((String::from(language.as_str()), String::from(word)));
        }
        if !matches!(language.language(), "en" | "de") {
            return;
        }
        let offsets: &[usize] = match word {
            "hyphenation" => &[2, 6],
            // Too near either end, then fine.
            "abcdefgh" => &[1, 3, 7],
            // Out of order, repeated, past the end and at zero.
            "badbreaks" => &[5, 3, 5, 0, 9, 40, 4],
            // Inside a two-byte character, or on a boundary.
            "\u{E9}t\u{E9}t\u{E9}s" => &[1, 2, 4, 6, 8],
            "Hyphenation" => &[2, 6],
            _ => &[],
        };
        for &offset in offsets {
            found(offset);
        }
    }
}

/// Returns a context with Ahem and `hyphenator`, and the log it keeps.
fn hyphenating() -> (Context, Log) {
    let toy = Toy::default();
    let asked = toy.asked.clone();
    let mut cx = context();
    cx.set_hyphenator(Some(Box::new(toy)));
    (cx, asked)
}

/// Returns Ahem at 10px under `hyphens: auto` in `language`.
fn auto(language: &str) -> ComputedStyle<'static> {
    let mut style = ahem(10.0);
    style.text.hyphens = Hyphens::Auto;
    style.text.language = Language::parse(language).ok();
    style
}

/// Returns the words asked about since the last call.
fn asked(log: &Mutex<Vec<(String, String)>>) -> Vec<(String, String)> {
    log.lock()
        .map(|mut log| log.drain(..).collect())
        .unwrap_or_default()
}

/// Returns the byte offsets where a break draws a hyphen.
fn hyphen_offsets(layout: &Layout) -> Vec<usize> {
    let analysis = layout.stages().analysis;
    let clusters = &analysis.clusters;
    (0..clusters.len())
        .map(ClusterId::new)
        .filter(|&cluster| {
            (clusters.attrs(cluster))
                .is_some_and(|attrs| analysis.breaks_with_hyphen(cluster, attrs))
        })
        .map(|cluster| clusters.range(cluster).end.get())
        .collect()
}

/// Builds `text` in `style` and breaks it `width` wide.
fn lay_out(
    cx: &mut Context,
    layout: &mut Layout,
    style: &ComputedStyle<'_>,
    text: &str,
    width: f32,
) {
    build(cx, layout, &ComputedBlockStyle::new(style), |b| {
        b.text(NodeKey(1), text);
    });
    layout.break_lines(cx, Area::new(width), &mut NoExclusions);
}

/// A word breaks where the hyphenator finds, and the line draws a hyphen.
///
/// "hyphen" and its hyphen take 70px, so `hyphenation is` at 70px breaks
/// after it, as Chrome's `ShapingLineBreaker::Hyphenate` takes the last
/// opportunity that fits.
#[test]
fn a_word_breaks_where_the_hyphenator_finds_with_a_hyphen() {
    let (mut cx, log) = hyphenating();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &auto("en"), "hyphenation is", 70.0);
    assert_eq!(texts(&layout), ["hyphen", "ation ", "is"]);
    let hyphenated: Vec<_> = layout.lines().map(|line| line.is_hyphenated()).collect();
    assert_eq!(hyphenated, [true, false, false]);
    let line = layout.line(0).expect("a line");
    let hyphen: Vec<_> = line
        .items()
        .filter_map(|item| match item {
            Item::Generated(run) => Some(run.advance()),
            _ => None,
        })
        .collect();
    assert_eq!(hyphen, [10.0]);
    // "is" is too short to ask about.
    let words = asked(&log);
    assert_eq!(words, [(String::from("en"), String::from("hyphenation"))]);
    assert_eq!(hyphen_offsets(&layout), [2, 6]);
}

/// The hyphen drawn is the `hyphenate-character`, and the line pays for
/// it: at 70px, "hyphen" and a hyphen two characters wide take 80px, so the
/// first line ends after "hy".
#[test]
fn a_hyphenation_opportunity_draws_the_hyphenate_character() {
    let (mut cx, _) = hyphenating();
    let mut layout = Layout::new();
    let mut style = auto("en");
    style.text.hyphenate_character = Some("==");
    lay_out(&mut cx, &mut layout, &style, "hyphenation", 70.0);
    assert_eq!(texts(&layout), ["hy", "phen", "ation"]);
    let hyphens: Vec<f32> = layout
        .lines()
        .flat_map(|line| {
            line.items()
                .filter_map(|item| match item {
                    Item::Generated(run) => Some(run.advance()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(hyphens, [20.0, 20.0]);
}

/// Two characters stay on each side of a break, and offsets that are out
/// of order, repeated, out of range or inside a character are dropped.
#[test]
fn hyphenation_keeps_two_characters_on_each_side_and_drops_bad_offsets() {
    let (mut cx, _) = hyphenating();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &auto("en"), "abcdefgh", 200.0);
    assert_eq!(hyphen_offsets(&layout), [3]);
    lay_out(&mut cx, &mut layout, &auto("en"), "badbreaks", 200.0);
    assert_eq!(hyphen_offsets(&layout), [5]);
    // In `ététés`, 1 and 4 fall inside an `é`, 2 leaves one character
    // before it and 8 one after.
    lay_out(
        &mut cx,
        &mut layout,
        &auto("en"),
        "\u{E9}t\u{E9}t\u{E9}s",
        200.0,
    );
    assert_eq!(hyphen_offsets(&layout), [6]);
}

/// A word is trimmed of the opening punctuation, quotes and closing
/// punctuation around it before it is asked about, as Chrome's
/// `HyphenationMinikin::WordToHyphenate` trims it, and a word under five
/// characters is not asked about.
#[test]
fn a_word_is_trimmed_and_long_enough_before_it_is_asked_about() {
    let (mut cx, log) = hyphenating();
    let mut layout = Layout::new();
    lay_out(
        &mut cx,
        &mut layout,
        &auto("en"),
        "(\u{201C}hyphenation\u{201D}), word",
        300.0,
    );
    let words = asked(&log);
    assert_eq!(words, [(String::from("en"), String::from("hyphenation"))]);
    // `(“` is four bytes.
    assert_eq!(hyphen_offsets(&layout), [6, 10]);
}

/// The hyphenator is asked in the text's language. A capitalized word is
/// asked about except in English, as in Chrome's
/// `Hyphenation::ShouldHyphenateWord`. Text with no language, or under
/// `hyphens: manual`, is never asked about.
#[test]
fn the_hyphenator_is_asked_in_the_language_of_the_text() {
    let (mut cx, log) = hyphenating();
    let mut layout = Layout::new();
    let word = (String::from("hyphenation"), String::from("Hyphenation"));
    lay_out(
        &mut cx,
        &mut layout,
        &auto("de-CH"),
        "hyphenation Hyphenation",
        300.0,
    );
    assert_eq!(
        asked(&log),
        [
            (String::from("de-CH"), word.0.clone()),
            (String::from("de-CH"), word.1.clone())
        ]
    );
    assert_eq!(hyphen_offsets(&layout), [2, 6, 14, 18]);
    lay_out(
        &mut cx,
        &mut layout,
        &auto("en-GB"),
        "hyphenation Hyphenation",
        300.0,
    );
    assert_eq!(asked(&log), [(String::from("en-GB"), word.0.clone())]);
    assert_eq!(hyphen_offsets(&layout), [2, 6]);
    // A language the toy has no words for gets none.
    lay_out(&mut cx, &mut layout, &auto("fr"), "hyphenation", 300.0);
    assert_eq!(asked(&log), [(String::from("fr"), word.0.clone())]);
    assert_eq!(hyphen_offsets(&layout), []);
    let mut unknown = auto("en");
    unknown.text.language = None;
    lay_out(&mut cx, &mut layout, &unknown, "hyphenation", 300.0);
    let mut manual = auto("en");
    manual.text.hyphens = Hyphens::Manual;
    lay_out(&mut cx, &mut layout, &manual, "hyphenation", 300.0);
    assert_eq!(asked(&log), []);
    assert_eq!(hyphen_offsets(&layout), []);
}

/// With no hyphenator, `hyphens: auto` breaks at soft hyphens alone, as
/// `manual` does.
#[test]
fn without_a_hyphenator_hyphens_auto_breaks_at_soft_hyphens_only() {
    let mut cx = context();
    let mut layout = Layout::new();
    lay_out(
        &mut cx,
        &mut layout,
        &auto("en"),
        "hyphenation hy\u{AD}phen",
        50.0,
    );
    assert_eq!(texts(&layout), ["hyphenation ", "hy\u{AD}", "phen"]);
    assert_eq!(hyphen_offsets(&layout), [16]);
}

/// The text's last word breaks only where nothing else fits, as Chrome's
/// `ShouldHyphenate` keeps it whole unless it starts the line.
///
/// At 100px `aa hyphen` and its hyphen would fit, but `aa` ends the line.
/// The word then starts a line it overflows, and breaks there. A word that
/// is all the text breaks as any other.
#[test]
fn the_last_word_breaks_only_where_nothing_else_fits() {
    let (mut cx, _) = hyphenating();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &auto("en"), "aa hyphenation", 100.0);
    assert_eq!(texts(&layout), ["aa ", "hyphen", "ation"]);
    lay_out(
        &mut cx,
        &mut layout,
        &auto("en"),
        "aa hyphenation aa",
        100.0,
    );
    assert_eq!(texts(&layout), ["aa hyphen", "ation aa"]);
    lay_out(&mut cx, &mut layout, &auto("en"), "hyphenation", 100.0);
    assert_eq!(texts(&layout), ["hyphen", "ation"]);
}

/// Min-content is the widest part of a hyphenated word with its hyphen, as
/// Chrome's `FastMinTextContext::AddHyphenated` measures it: `phen` and its
/// hyphen, and `ation`, 50px each.
#[test]
fn min_content_takes_the_widest_hyphenated_part() {
    let (mut cx, _) = hyphenating();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &auto("en"), "hyphenation", 300.0);
    assert_eq!(layout.intrinsic_sizes().min_content, 50.0);
    assert_eq!(layout.intrinsic_sizes().max_content, 110.0);
}

/// A phrase segmenter that starts a phrase after each `は`, `に` and `を`,
/// and logs the text it is asked about.
#[derive(Default)]
struct Particles {
    asked: Arc<Mutex<Vec<String>>>,
}

impl PhraseSegmenter for Particles {
    fn segment(&self, text: &str, _: Language, found: &mut dyn FnMut(usize)) {
        if let Ok(mut asked) = self.asked.lock() {
            asked.push(String::from(text));
        }
        for (offset, ch) in text.char_indices() {
            if matches!(ch, 'は' | 'に' | 'を') {
                found(offset + ch.len_utf8());
            }
        }
    }
}

/// The Han test font, in which each ideograph and kana is 10px at 20px.
const HAN: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed("Test Han"))];

/// Returns a context with the Han test font and, where `segments`, the
/// particle segmenter, with its log.
fn phrasing(segments: bool) -> (Context, Arc<Mutex<Vec<String>>>) {
    let segmenter = Particles::default();
    let asked = segmenter.asked.clone();
    let mut cx = Context::new(collection(&[han()], han_fallback("Test Han")));
    if segments {
        cx.set_phrase_segmenter(Some(Box::new(segmenter)));
    }
    (cx, asked)
}

/// Returns the Han test font at 20px under `word-break: auto-phrase` in
/// `language`.
fn phrases(language: &str) -> ComputedStyle<'static> {
    let mut style = sized(&HAN, 20.0);
    style.text.word_break = WordBreak::AutoPhrase;
    style.text.language = Language::parse(language).ok();
    style
}

/// Japanese under `auto-phrase` breaks only between phrases. A phrase wider
/// than the line breaks as under `normal`, as Chrome retries a line that
/// overflows without phrases.
#[test]
fn auto_phrase_breaks_japanese_between_phrases() {
    let (mut cx, log) = phrasing(true);
    let mut layout = Layout::new();
    let text = "私は東京に行きます";
    lay_out(&mut cx, &mut layout, &phrases("ja"), text, 40.0);
    assert_eq!(texts(&layout), ["私は", "東京に", "行きます"]);
    let asked = log.lock().map(|log| log.clone()).unwrap_or_default();
    assert_eq!(asked, [String::from(text)]);
    lay_out(&mut cx, &mut layout, &phrases("ja"), text, 30.0);
    assert_eq!(texts(&layout), ["私は", "東京に", "行きま", "す"]);
    // Min-content is the widest phrase.
    assert_eq!(layout.intrinsic_sizes().min_content, 40.0);
}

/// `auto-phrase` breaks as `normal` without a segmenter, and in a language
/// other than Japanese, which the segmenter is not asked about.
#[test]
fn auto_phrase_breaks_as_normal_without_japanese_or_a_segmenter() {
    let text = "私は東京に行きます";
    let normal = ["私は東京", "に行きま", "す"];
    let (mut cx, log) = phrasing(false);
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &phrases("ja"), text, 40.0);
    assert_eq!(texts(&layout), normal);
    let (mut cx, log_zh) = phrasing(true);
    lay_out(&mut cx, &mut layout, &phrases("zh"), text, 40.0);
    assert_eq!(texts(&layout), normal);
    for log in [log, log_zh] {
        assert!(log.lock().is_ok_and(|log| log.is_empty()));
    }
}

/// Under `auto-phrase` with a segmenter, a soft hyphen breaks only where
/// nothing else fits, as Chrome turns soft hyphens off under phrases until
/// a line overflows. `cc aaaa` and its hyphen fit at 80px; `cc` ends the
/// line instead.
#[test]
fn auto_phrase_holds_soft_hyphens_back() {
    let mut style = auto("en");
    style.text.hyphens = Hyphens::Manual;
    style.text.word_break = WordBreak::AutoPhrase;
    let text = "cc aaaa\u{AD}bbbb";
    let mut cx = context();
    let mut layout = Layout::new();
    lay_out(&mut cx, &mut layout, &style, text, 80.0);
    assert_eq!(texts(&layout), ["cc aaaa\u{AD}", "bbbb"]);
    cx.set_phrase_segmenter(Some(Box::new(Particles::default())));
    lay_out(&mut cx, &mut layout, &style, text, 80.0);
    assert_eq!(texts(&layout), ["cc ", "aaaa\u{AD}bbbb"]);
    // Where nothing else fits, it breaks there with its hyphen.
    lay_out(&mut cx, &mut layout, &style, text, 60.0);
    assert_eq!(texts(&layout), ["cc ", "aaaa\u{AD}", "bbbb"]);
    assert!(layout.line(1).is_some_and(|line| line.is_hyphenated()));
}

/// A hyphenator and phrase segmenter that report offsets from a fixed
/// pseudo-random sequence: in and out of range, on and off character
/// boundaries, in any order.
struct Scattered;

impl Scattered {
    /// Reports offsets around `text` to `found`.
    fn scatter(text: &str, found: &mut dyn FnMut(usize)) {
        let mut state = (text.len() as u64).wrapping_mul(2_654_435_761) | 1;
        for _ in 0..24 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            found((state >> 33) as usize % (text.len() + 4));
        }
        found(usize::MAX);
    }
}

impl Hyphenator for Scattered {
    fn hyphenate(&self, word: &str, _: Language, found: &mut dyn FnMut(usize)) {
        Self::scatter(word, found);
    }
}

impl PhraseSegmenter for Scattered {
    fn segment(&self, text: &str, _: Language, found: &mut dyn FnMut(usize)) {
        Self::scatter(text, found);
    }
}

/// Whatever offsets the host reports, every build and break gives lines
/// that cover the text in order, over mixed scripts, marks, emoji, soft
/// hyphens and boxes.
#[test]
fn any_host_offsets_give_valid_lines() {
    let mut cx = Context::new(collection(&[han()], han_fallback("Test Han")));
    cx.set_hyphenator(Some(Box::new(Scattered)));
    cx.set_phrase_segmenter(Some(Box::new(Scattered)));
    let texts = [
        "extraordinarily long words, hyphen\u{AD}ated (and) \u{201C}quoted\u{201D}",
        "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301} \u{1F469}\u{200D}\u{1F4BB}\u{1F469}abcdef",
        "\u{5D0}\u{5D1}\u{5D2}\u{5D3}\u{5D4}\u{5D5} abcdefgh \u{5D0}\u{5D1}\u{5D2}",
        "私は東京に行きます。日本語の文章は、漢字と仮名で書かれる。",
        "",
    ];
    let mut layout = Layout::new();
    for language in ["en", "ja", "he"] {
        let mut style = sized(&HAN, 20.0);
        style.text.hyphens = Hyphens::Auto;
        style.text.language = Language::parse(language).ok();
        for word_break in [WordBreak::Normal, WordBreak::AutoPhrase] {
            style.text.word_break = word_break;
            for text in texts {
                let half = text
                    .char_indices()
                    .nth(text.chars().count() / 2)
                    .map_or(text.len(), |(offset, _)| offset);
                let (first, second) = text.split_at(half);
                build(
                    &mut cx,
                    &mut layout,
                    &ComputedBlockStyle::new(&style),
                    |b| {
                        b.text(NodeKey(1), first);
                        b.open_box(NodeKey(2), &style, None);
                        b.text(NodeKey(3), second);
                        b.close_box();
                    },
                );
                for width in [0.0, 13.0, 37.0, 100.0, 1000.0] {
                    layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
                    let mut end = 0;
                    for line in layout.lines() {
                        let range = line.text_range();
                        let case = (language, word_break, text, width);
                        assert_eq!(range.start, end, "{case:?}");
                        end = range.end;
                    }
                    assert_eq!(end, layout.content().text.len(), "{text:?}");
                }
            }
        }
    }
}
