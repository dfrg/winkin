//! The break opportunities the host's segmenters find: hyphenation
//! opportunities in the words of `hyphens: auto` text, and phrase
//! boundaries in Japanese `word-break: auto-phrase` text.
//!
//! The writer asks once a text item starts, for the whole item, and keeps
//! the offsets found in [`HostedPoints`]. Each boundary in the item then
//! takes the point at its offset, in text order. Nothing the host reports
//! is trusted: offsets out of range, off a character boundary or out of
//! order are dropped.

use alloc::vec::Vec;
use core::ops::Range;

use cleft::Dictionaries;

use super::breaks::{LineKey, LineStream};
use crate::data::{TextOffset, heap_bytes};
use crate::segment::{Hyphenator, PhraseSegmenter};
use crate::style::Language;
use crate::unicode::{self, FirstLetterClass, ScriptId};
use crate::work;

/// What the host's segmenters change in a text item's opportunities.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(super) enum Hosted {
    /// Nothing.
    #[default]
    None,
    /// Its words also break where the hyphenator finds, with a hyphen
    /// (`hyphens: auto`).
    Hyphens,
    /// Japanese `auto-phrase` text: between ideographs and kana it breaks
    /// only at the phrase boundaries the segmenter finds. The other
    /// opportunities there, and its soft hyphens, are taken only where
    /// nothing else fits: a line that overflows is retried without phrases.
    Phrases,
    /// `auto-phrase` text in another language: only its soft hyphens wait
    /// for a line that overflows.
    Unsegmented,
}

/// The fewest characters a word has before it is hyphenated, as Blink's
/// `Hyphenation` sets by default (CSS Text 4's `hyphenate-limit-chars`).
const MIN_WORD: usize = 5;

/// The fewest characters before a hyphen.
const MIN_PREFIX: usize = 2;

/// The fewest characters after a hyphen.
const MIN_SUFFIX: usize = 2;

/// The points the host found in the text item being walked, and the next
/// one a boundary may take.
///
/// It lives in the analysis scratch, so a warm build allocates nothing.
pub(super) struct HostedPoints {
    /// Each point's byte offset in the whole text, shifted left one. Bit 0
    /// is set for a hyphenation opportunity in the text's last word, which
    /// is taken only where nothing else fits.
    points: Vec<u32>,
    /// The index of the next point a boundary may take.
    next: usize,
    /// A word's offsets as the host reported them, before they are checked.
    found: Vec<usize>,
}

impl HostedPoints {
    /// Returns no points, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            points: Vec::new(),
            next: 0,
            found: Vec::new(),
        }
    }

    /// Forgets every point, keeping the capacity.
    pub(super) fn clear(&mut self) {
        self.points.clear();
        self.next = 0;
        self.found.clear();
    }

    /// Takes the point at byte `at`, passing those before it. Returns
    /// whether it lies in the text's last word, or `None` if there is no
    /// point at `at`.
    ///
    /// Boundaries ask in increasing order.
    pub(super) fn take(&mut self, at: usize) -> Option<bool> {
        while let Some(&point) = self.points.get(self.next) {
            let offset = (point >> 1) as usize;
            if offset > at {
                return None;
            }
            self.next += 1;
            if offset == at {
                return Some(point & 1 != 0);
            }
        }
        None
    }

    /// Fills `found` with the offsets in `text` that `ask` reports: each
    /// after the one before, inside `text` and on a character boundary.
    fn ask(&mut self, text: &str, ask: impl FnOnce(&mut dyn FnMut(usize))) {
        let found = &mut self.found;
        found.clear();
        ask(&mut |offset| {
            let after = found.last().is_none_or(|&last| offset > last);
            if after && offset > 0 && offset < text.len() && text.is_char_boundary(offset) {
                found.push(offset);
            }
        });
    }
}

/// Records in `points` a point at byte `at`, unless it does not fit.
fn push_point(points: &mut Vec<u32>, at: usize, last_word: bool) {
    if let Some(shifted) = u32::try_from(at).ok().and_then(|at| at.checked_mul(2)) {
        points.push(shifted | u32::from(last_word));
    }
}

heap_bytes! {
    HostedPoints { points, found; next }
}

/// The text item and the options its words are found under.
pub(super) struct ItemWords<'t> {
    /// The whole text.
    pub(super) text: &'t str,
    /// The item's bytes in `text`.
    pub(super) range: Range<TextOffset>,
    pub(super) key: LineKey,
    /// The dictionaries that break SA runs.
    pub(super) dictionaries: &'t Dictionaries,
    pub(super) language: Language,
}

/// Records in `points` the hyphenation opportunities `hyphenator` finds in
/// the words of `item`.
///
/// A word runs from one line-break opportunity to the next, as Blink's
/// line breaker hands words to its `Hyphenation`, and is cut at the item's
/// ends. Each is trimmed as Blink's `HyphenationMinikin` trims it, and is
/// asked about only if long enough and, in English, not capitalized. An
/// opportunity in the text's last word is marked, unless that word is all
/// the text, as Blink's `ShouldHyphenate` keeps that word whole where an
/// earlier break fits.
pub(super) fn hyphenate(
    points: &mut HostedPoints,
    item: &ItemWords<'_>,
    hyphenator: &dyn Hyphenator,
) {
    let ItemWords {
        text,
        ref range,
        key,
        dictionaries,
        language,
    } = *item;
    let (start, end) = (range.start.get(), range.end.get());
    let Some(item_text) = text.get(start..end) else {
        return;
    };
    let host = (hyphenator, language.language().eq_ignore_ascii_case("en"));
    let mut stream = LineStream::new(text, start, dictionaries, key);
    let mut word_start = start;
    let mut before = None;
    for (offset, ch) in item_text.char_indices() {
        work::step();
        let at = start + offset;
        let earlier = || {
            item_text
                .get(..offset)
                .unwrap_or_default()
                .chars()
                .rev()
                .skip(1)
        };
        if let Some(last) = before
            && key.blink_or(earlier, last, ch, || stream.at(at))
        {
            let word = text.get(word_start..at).unwrap_or_default();
            hyphenate_word(points, word, word_start, false, language, host);
            word_start = at;
        }
        before = Some(ch);
    }
    if word_start < end {
        let last = text.get(end..).is_some_and(all_breakable_spaces)
            && !text.get(..word_start).is_some_and(all_breakable_spaces);
        let word = text.get(word_start..end).unwrap_or_default();
        hyphenate_word(points, word, word_start, last, language, host);
    }
}

/// Records the opportunities in `raw`, the word from one opportunity to the
/// next, starting at byte `raw_start` of the text.
///
/// `last` marks them as the text's last word's. `english` keeps a
/// capitalized word whole, as Blink's `ShouldHyphenateWord` does for
/// locales starting with `en`.
fn hyphenate_word(
    points: &mut HostedPoints,
    raw: &str,
    raw_start: usize,
    last: bool,
    language: Language,
    (hyphenator, english): (&dyn Hyphenator, bool),
) {
    let trimmed = raw.trim_start_matches(skips_before_word);
    let start = raw_start + (raw.len() - trimmed.len());
    let trimmed = trimmed.trim_end_matches(skips_after_word);
    // Blink counts UTF-16 units; this counts characters, which agree
    // outside the supplementary planes.
    let length = trimmed.chars().count();
    if length < MIN_WORD || english && trimmed.chars().next().is_some_and(char::is_uppercase) {
        return;
    }
    points.ask(trimmed, |found| {
        hyphenator.hyphenate(trimmed, language, found)
    });
    let HostedPoints {
        points: kept,
        found,
        ..
    } = points;
    let mut chars = trimmed.char_indices().map(|(offset, _)| offset).enumerate();
    for &offset in found.iter() {
        // The number of characters before the offset, which is on a boundary.
        let Some((position, _)) = chars.find(|&(_, at)| at == offset) else {
            break;
        };
        if position >= MIN_PREFIX && length - position >= MIN_SUFFIX {
            push_point(kept, start + offset, last);
        }
    }
}

/// Records in `points` the phrase boundaries `segmenter` finds in the text
/// of `range` in `text`.
pub(super) fn segment_phrases(
    points: &mut HostedPoints,
    text: &str,
    range: Range<TextOffset>,
    language: Language,
    segmenter: &dyn PhraseSegmenter,
) {
    let start = range.start.get();
    let Some(item_text) = text.get(start..range.end.get()) else {
        return;
    };
    points.ask(item_text, |found| {
        segmenter.segment(item_text, language, found)
    });
    let HostedPoints {
        points: kept,
        found,
        ..
    } = points;
    for &offset in found.iter() {
        push_point(kept, start + offset, false);
    }
}

/// Returns whether `ch` is one that ICU's phrase segmentation of Japanese
/// covers: an ideograph, a kana, or a prolonged sound or voicing mark.
pub(super) fn is_phrase_letter(ch: char) -> bool {
    let script = unicode::core_props(ch).script();
    matches!(
        script,
        ScriptId::HAN | ScriptId::HIRAGANA | ScriptId::KATAKANA
    ) || matches!(ch, '\u{30FC}' | '\u{FF70}' | '\u{FF9E}' | '\u{FF9F}')
}

/// Returns whether every character of `text` is a breakable space, as
/// Blink's `ShapingLineBreaker` counts them: a space, tab, line feed or
/// other space separator.
fn all_breakable_spaces(text: &str) -> bool {
    text.chars().all(|ch| {
        matches!(ch, ' ' | '\t' | '\n')
            || unicode::rare_props(ch).first_letter() == FirstLetterClass::Space
            || ch == '\u{3000}'
    })
}

/// Returns whether `ch` is trimmed from a word's start before it is
/// hyphenated: a space, or opening punctuation or a quote.
///
/// Blink trims Line_Break OP and QU. This reads opening punctuation (Ps)
/// and the quotation marks among the other punctuation.
fn skips_before_word(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t'
            | '\n'
            | '\u{A0}'
            | '"'
            | '\''
            | '\u{A1}'
            | '\u{AB}'
            | '\u{BB}'
            | '\u{BF}'
            | '\u{2018}'..='\u{201F}' | '\u{2039}' | '\u{203A}'
    ) || (unicode::rare_props(ch).first_letter() == FirstLetterClass::Opening && !is_dash(ch))
}

/// Returns whether `ch` is trimmed from a word's end before it is
/// hyphenated: a space separator, punctuation or a control.
fn skips_after_word(ch: char) -> bool {
    ch.is_control()
        || ch == '\u{3000}'
        || unicode::rare_props(ch).first_letter() != FirstLetterClass::Other
}

/// Returns whether `ch` is a dash (Pd), which the first-letter class files
/// with opening punctuation.
fn is_dash(ch: char) -> bool {
    matches!(
        ch,
        '-' | '\u{58A}' | '\u{5BE}' | '\u{1400}' | '\u{1806}' | '\u{2010}'
            ..='\u{2015}'
                | '\u{2E17}'
                | '\u{2E1A}'
                | '\u{2E3A}'
                | '\u{2E3B}'
                | '\u{2E40}'
                | '\u{2E5D}'
                | '\u{301C}'
                | '\u{3030}'
                | '\u{30A0}'
                | '\u{FE31}'
                | '\u{FE32}'
                | '\u{FE58}'
                | '\u{FE63}'
                | '\u{FF0D}'
                | '\u{10EAD}'
    )
}
