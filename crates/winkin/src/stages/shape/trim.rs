//! `text-spacing-trim` at shaping: which full-width punctuation gives back
//! its blank half, and how.
//!
//! The rule follows Blink's `HanKerning` (`han_kerning.cc`). The tests pin
//! it with Yu Gothic, which has `halt`, and MS Gothic, which does not.
//! - Under any value but `space-all`, font selection turns on `chws`. A font
//!   with contextual half-width spacing collapses adjacent punctuation
//!   itself.
//! - Where the font has `halt`, this module finds the pairs the text-spacing
//!   classes make. The font's `halt` applies to each mark that gives its
//!   blank back, one character at a time. An opening mark kerns after an
//!   opening, middle or closing mark, or a narrow opening bracket
//!   ([`should_kern`]). A closing mark kerns before a closing, middle or
//!   narrow closing mark ([`should_kern_last`]).
//! - Where the font also has `chws`, it handles pairs inside a run itself,
//!   and only the run's edges are found here.
//! - The characters either side of a run are the text's, across items and
//!   fonts. A neighbour whose class depends on the font takes it from the
//!   font it is shaped in ([`class_beside`]).
//! - A kerned mark is unsafe to break before, so a line breaking inside a
//!   pair reshapes. A line starting at an opening mark is shaped as a start
//!   and takes nothing from before it.
//! - Where the font has no `halt`, nothing is trimmed.
//!   [`Config::punctuation_trim`] set to [`PunctuationTrim::Always`] trims
//!   there too, by halving the mark's own advance as Blink's unshipped
//!   fallback (`ApplyKerning`) does. An opening mark's glyph moves back into
//!   the half it keeps.
//! - Five classes depend on the font: its dots, colon, semicolon and curly
//!   quotes. [`TrimFont`] reads them once from its glyphs, shaped in the
//!   text's language, by which half of the advance each glyph's ink is in.
//! - At a line's start, a paragraph under `trim-start` trims its opening
//!   mark ([`ShapingEdges::trim_start`]). So does every wrapped line under
//!   `space-first` or `trim-start`, which the breaker reshapes.
//! - At a line's end, a closing mark that fits only when trimmed is trimmed,
//!   and the breaker reshapes it too ([`ShapingEdges::trim_end`]).
//!
//! [`Config::punctuation_trim`]: crate::config::Config::punctuation_trim
//! [`PunctuationTrim::Always`]: crate::config::PunctuationTrim::Always

use alloc::vec::Vec;

use crate::data::heap_bytes;
use crate::unicode::{self, TextSpacingClass};
use crate::work;

/// A character's class for the rule, with the font's answer for the five
/// classes that depend on it.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(super) enum TrimClass {
    #[default]
    Other,
    Open,
    Close,
    Middle,
    OpenNarrow,
    CloseNarrow,
}

/// What a font says to the rule (Blink's `HanKerning::FontData`), read once
/// per font, language and direction.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(super) struct TrimFont {
    /// Whether it has `halt` (`vhal` upright in a vertical line), so it can
    /// trim.
    halt: bool,
    /// Whether it has `chws` (`vchw`), so it collapses the pairs inside a
    /// run itself.
    chws: bool,
    /// Whether its curly quotes are full width, as Chinese fonts draw them;
    /// a Japanese font's are commonly proportional.
    quotes_full: bool,
    /// The class of its ideographic and fullwidth comma and full stop.
    dot: TrimClass,
    /// The class of its fullwidth colon.
    colon: TrimClass,
    /// The class of its fullwidth semicolon.
    semicolon: TrimClass,
}

/// The ten characters whose glyphs a font's classes are read off, in
/// Blink's order: the four dots, the colon, the semicolon, and the opening
/// then the closing curly quotes.
pub(super) const TRIM_PROBES: [char; 10] = [
    '\u{3001}', '\u{3002}', '\u{FF0C}', '\u{FF0E}', '\u{FF1A}', '\u{FF1B}', '\u{201C}', '\u{2018}',
    '\u{201D}', '\u{2019}',
];

/// A glyph of [`TRIM_PROBES`] as a font shapes it, in font units.
///
/// It records whether the font draws it, its advance along the line, and
/// where its ink starts and ends along the line from its origin. The
/// context measures each probe into one, and [`TrimFont::from_probes`]
/// reads the classes from them. Only the resulting [`TrimFont`] is cached.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct TrimProbe {
    pub(super) drawn: bool,
    pub(super) advance: f32,
    pub(super) left: f32,
    pub(super) right: f32,
}

impl TrimFont {
    /// Returns what a font with the given `halt` and `chws` says, from its
    /// [`TRIM_PROBES`] shaped as `probes`, one glyph each.
    ///
    /// Where `probes` is `None`, because the font does not draw them one
    /// glyph each, its `halt` goes unused and its classes are `Other`, as
    /// Blink's `FontData` gives up. The classes are read even without
    /// `halt`, for the halving trim.
    pub(super) fn from_probes(halt: bool, chws: bool, probes: Option<&[TrimProbe; 10]>) -> Self {
        let Some(probes) = probes else {
            return Self {
                halt: false,
                chws,
                ..Self::default()
            };
        };
        let group = |from: usize, to: usize| group_class(probes.get(from..to).unwrap_or_default());
        Self {
            halt,
            chws,
            quotes_full: group(6, 8) == TrimClass::Open && group(8, 10) == TrimClass::Close,
            dot: group(0, 4),
            colon: single_class(probes[4]),
            semicolon: single_class(probes[5]),
        }
    }

    /// Returns whether it has `halt`, so it trims even where the config does
    /// not halve advances.
    pub(super) fn has_halt(&self) -> bool {
        self.halt
    }

    /// Returns whether it has `chws`, so it handles pairs inside a run itself
    /// and the rule finds only the run's edges.
    pub(super) fn has_chws(&self) -> bool {
        self.chws
    }

    /// Returns the class in this font of a character of text-spacing class
    /// `class`.
    pub(super) fn class(&self, class: TextSpacingClass) -> TrimClass {
        match class {
            TextSpacingClass::Other => TrimClass::Other,
            TextSpacingClass::Open => TrimClass::Open,
            TextSpacingClass::Close => TrimClass::Close,
            TextSpacingClass::Middle => TrimClass::Middle,
            TextSpacingClass::OpenNarrow => TrimClass::OpenNarrow,
            TextSpacingClass::CloseNarrow => TrimClass::CloseNarrow,
            TextSpacingClass::Dot => self.dot,
            TextSpacingClass::Colon => self.colon,
            TextSpacingClass::Semicolon => self.semicolon,
            TextSpacingClass::OpenQuote if self.quotes_full => TrimClass::Open,
            TextSpacingClass::OpenQuote => TrimClass::OpenNarrow,
            TextSpacingClass::CloseQuote if self.quotes_full => TrimClass::Close,
            TextSpacingClass::CloseQuote => TrimClass::CloseNarrow,
        }
    }
}

/// Returns the class a glyph's ink makes (Blink's `CharTypeFromBounds`).
///
/// Ink in the right half of the advance makes an opening mark, in the left
/// half a closing one, and within the middle half a middle one.
fn bounds_class(half: f32, left: f32, right: f32) -> TrimClass {
    if right <= half {
        TrimClass::Close
    } else if left >= half {
        TrimClass::Open
    } else if right - left <= half && left >= half / 2.0 {
        TrimClass::Middle
    } else {
        TrimClass::Other
    }
}

/// Returns the class of one probe, or `Other` where the font has no glyph
/// for it.
fn single_class(probe: TrimProbe) -> TrimClass {
    if !probe.drawn {
        return TrimClass::Other;
    }
    bounds_class(probe.advance / 2.0, probe.left, probe.right)
}

/// Returns the class of a group of probes that should be drawn alike.
///
/// That is the first drawn probe's class where every other drawn probe has
/// the same advance and class, and `Other` where they differ or none is
/// drawn.
fn group_class(probes: &[TrimProbe]) -> TrimClass {
    let mut drawn = probes.iter().filter(|probe| probe.drawn);
    let Some(first) = drawn.next() else {
        return TrimClass::Other;
    };
    let half = first.advance / 2.0;
    let class = bounds_class(half, first.left, first.right);
    for probe in drawn {
        if probe.advance != first.advance || bounds_class(half, probe.left, probe.right) != class {
            return TrimClass::Other;
        }
    }
    class
}

/// Returns whether an opening mark of class `class` after one of `last`
/// gives back its blank (Blink's `HanKerning::ShouldKern`).
fn should_kern(class: TrimClass, last: TrimClass) -> bool {
    class == TrimClass::Open
        && matches!(
            last,
            TrimClass::Open | TrimClass::Middle | TrimClass::Close | TrimClass::OpenNarrow
        )
}

/// Returns whether a closing mark of class `last` before one of `class`
/// gives back its blank (Blink's `HanKerning::ShouldKernLast`).
fn should_kern_last(class: TrimClass, last: TrimClass) -> bool {
    last == TrimClass::Close
        && matches!(
            class,
            TrimClass::Close | TrimClass::Middle | TrimClass::CloseNarrow
        )
}

/// Returns the class of `ch`, a character beside a shaped range, for a
/// range in the font `own`.
///
/// For the five classes that depend on the font, `theirs`, the font `ch` is
/// shaped in, decides where it has `halt`, as Blink's `HanKerning` caches
/// each character's class in its own font, fallback fonts included.
/// Otherwise `own` decides.
pub(super) fn class_beside(
    ch: char,
    own: &TrimFont,
    theirs: impl FnOnce() -> Option<TrimFont>,
) -> TrimClass {
    let class = unicode::rare_props(ch).text_spacing();
    let depends = matches!(
        class,
        TextSpacingClass::Dot
            | TextSpacingClass::Colon
            | TextSpacingClass::Semicolon
            | TextSpacingClass::OpenQuote
            | TextSpacingClass::CloseQuote
    );
    match depends.then(theirs).flatten().filter(TrimFont::has_halt) {
        Some(theirs) => theirs.class(class),
        None => own.class(class),
    }
}

/// Returns whether `ch` may be an opening mark in some font.
///
/// It checks the classes Blink's `MaybeHanKerningOpen` checks before it
/// reshapes a line's start.
pub(crate) fn maybe_opening_mark(ch: char) -> bool {
    maybe_open_or_close(ch)
        && matches!(
            unicode::rare_props(ch).text_spacing(),
            TextSpacingClass::Open | TextSpacingClass::OpenQuote
        )
}

/// Returns whether `ch` may be a closing mark in some font.
///
/// It checks the classes Blink's `MaybeHanKerningClose` checks before it
/// trims a line's end.
pub(crate) fn maybe_closing_mark(ch: char) -> bool {
    maybe_open_or_close(ch)
        && matches!(
            unicode::rare_props(ch).text_spacing(),
            TextSpacingClass::Close | TextSpacingClass::CloseQuote
        )
}

/// Returns whether `ch` is in the ranges that hold every opening or closing
/// mark.
///
/// This is Blink's `MaybeHanKerningOpenOrCloseFast`, a gate before any
/// table is read.
fn maybe_open_or_close(ch: char) -> bool {
    matches!(ch, '\u{2018}'..='\u{301F}' | '\u{FF08}'..='\u{FF60}')
}

/// Returns whether `text` may hold a mark to trim at all.
///
/// This is Blink's `HanKerning::MayApply`, which declines at once on 8-bit
/// text. Every character in range is U+2018 or later, whose UTF-8 starts
/// with a byte of 0xE2 or more, so text with no such byte, such as all of
/// Latin-1, is scanned byte by byte and never decoded.
pub(super) fn may_hold_mark(text: &str) -> bool {
    text.bytes().any(|byte| byte >= 0xE2) && text.chars().any(maybe_open_or_close)
}

/// How a shaped range treats a line's edges.
///
/// The shaping pass sets it for a paragraph's start and for bidi controls,
/// and the breaker for the edges it reshapes. The default, all false,
/// shapes a range inside a line, with its neighbours.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct ShapingEdges {
    /// Whether the range starts a line, so what precedes it takes no part in
    /// a pair.
    pub(crate) line_start: bool,
    /// Whether the first character gives its blank back whatever precedes
    /// it.
    ///
    /// It does for an opening mark starting a paragraph under `trim-start`,
    /// or a wrapped line under `space-first` or `trim-start`.
    pub(crate) trim_start: bool,
    /// Whether the last character gives its blank back whatever follows it,
    /// as a closing mark ending a line that fits only when trimmed does.
    pub(crate) trim_end: bool,
    /// Whether a bidi control stands before the range, so the text before it
    /// is no context. Blink's text holds the control there, which ends
    /// joining.
    pub(crate) cut_before: bool,
    /// Whether a bidi control stands after the range.
    pub(crate) cut_after: bool,
}

/// A mark that gives its blank back.
///
/// It records the mark's bytes in the text and its class in the font, which
/// says which half is blank.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct TrimMark {
    start: u32,
    end: u32,
    class: TrimClass,
}

impl TrimMark {
    /// Returns the byte of the text it starts at, as the shaper labels it.
    pub(super) fn start(&self) -> u32 {
        self.start
    }

    /// Returns the byte of the text after it, where its `halt` range ends
    /// for harfrust.
    pub(super) fn end(&self) -> u32 {
        self.end
    }

    /// Returns its class in the font: an opening mark's blank is before its
    /// ink, a closing one's after.
    pub(super) fn class(&self) -> TrimClass {
        self.class
    }
}

/// The marks of a shaped range that give their blanks back.
///
/// It is one call's scratch, which the shaping context keeps for its
/// capacity.
#[derive(Default, Debug)]
pub(super) struct TrimMarks {
    /// The marks, in text order, each once.
    marks: Vec<TrimMark>,
    /// The byte each character starts at where breaking before it is unsafe
    /// because a pair meets there, in text order.
    unsafe_before: Vec<u32>,
}

impl TrimMarks {
    /// Empties it, keeping its capacity.
    pub(super) fn clear(&mut self) {
        self.marks.clear();
        self.unsafe_before.clear();
    }

    /// Returns the marks found, in text order, each once.
    pub(super) fn marks(&self) -> &[TrimMark] {
        &self.marks
    }

    /// Returns the byte each character starts at where breaking before it is
    /// unsafe because a pair meets there, in text order.
    pub(super) fn unsafe_before(&self) -> &[u32] {
        &self.unsafe_before
    }

    /// Finds the marks of `chars` (Blink's `HanKerning::AppendFontFeatures`).
    ///
    /// `chars` start at byte `base` of the text, between characters of the
    /// classes `before` and `after` ([`class_beside`]), `None` where there is
    /// no character. They are shaped in a font described by `font`, with
    /// `edges`.
    pub(super) fn find(
        &mut self,
        chars: &str,
        base: u32,
        before: Option<TrimClass>,
        after: Option<TrimClass>,
        font: &TrimFont,
        edges: ShapingEdges,
    ) {
        let mut all = chars.char_indices();
        let Some((_, first)) = all.next() else {
            return;
        };
        let classify = |ch: char| font.class(unicode::rare_props(ch).text_spacing());
        let byte = |offset: usize| base.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
        let mark = |offset: usize, ch: char, class: TrimClass| {
            let start = byte(offset);
            TrimMark {
                start,
                end: start.saturating_add(u32::try_from(ch.len_utf8()).unwrap_or(0)),
                class,
            }
        };
        let first_class = classify(first);
        let mut last = first_class;
        if edges.trim_start {
            self.mark(mark(0, first, first_class), true);
        } else if let Some(before) = before.filter(|_| !edges.line_start)
            && should_kern(first_class, before)
        {
            self.mark(mark(0, first, first_class), true);
        }
        // The last character's offset, character and class, for the edge
        // after it.
        let mut end = (0, first, first_class);
        if font.has_chws() {
            // The font's `chws` handles the pairs inside the run, so only
            // its end is found here.
            if let Some((offset, ch)) = chars.char_indices().next_back()
                && offset > 0
            {
                last = classify(ch);
                end = (offset, ch, last);
            }
        } else {
            for (offset, ch) in all {
                work::step();
                let class = classify(ch);
                if should_kern_last(class, last) {
                    self.mark(mark(end.0, end.1, end.2), false);
                    self.unsafe_before.push(byte(offset));
                } else if should_kern(class, last) {
                    self.mark(mark(offset, ch, class), true);
                }
                last = class;
                end = (offset, ch, class);
            }
        }
        let closes = edges.trim_end || after.is_some_and(|after| should_kern_last(after, last));
        if closes {
            self.mark(mark(end.0, end.1, end.2), false);
        }
    }

    /// Records `mark` once, and breaking before it as unsafe where
    /// `unsafe_before` says it is kerned against what precedes it.
    fn mark(&mut self, mark: TrimMark, unsafe_before: bool) {
        if self
            .marks
            .last()
            .is_none_or(|last| last.start != mark.start)
        {
            self.marks.push(mark);
        }
        if unsafe_before {
            self.unsafe_before.push(mark.start);
        }
    }
}

heap_bytes! {
    TrimMarks { marks, unsafe_before }
}

#[cfg(test)]
mod tests {
    use alloc::string::String;

    use super::*;

    /// The byte gate skips no character that may be a mark: for every
    /// character after Latin-1 text, it answers as decoding does.
    #[test]
    fn the_byte_gate_keeps_every_mark() {
        let mut text = String::new();
        for ch in (0..=0x10_FFFF_u32).filter_map(char::from_u32) {
            text.clear();
            text.push_str("Tomorrow, é ");
            text.push(ch);
            assert_eq!(may_hold_mark(&text), maybe_open_or_close(ch), "{ch:?}");
        }
    }
}
