//! Full case mapping, with ICU4C's language tailorings.
//!
//! The mappings are Unicode's full ones (SpecialCasing.txt's unconditional
//! entries included), read from runs of characters that map alike, and the
//! conditional ones are written out here, as ICU4C's `ucase.cpp` writes them.
//! ICU4C's `CaseMap` maps case in the text's language and tailors four: Turkish and Azerbaijani, Lithuanian, Greek and Dutch. Any
//! other language maps as the root does, Armenian included.
//!
//! - Every language lowercases a capital sigma that ends a word to `ς`.
//! - Turkish and Azerbaijani case `i` and `ı` as two letters, `İ` and `I`.
//! - Lithuanian keeps the dot of an `i` under an accent.
//! - Greek drops accents from capitals, by ICU4C's `GreekUpper`.
//! - Dutch titlecases `ij` whole.
//!
//! Uppercase leaves Georgian Mkhedruli as it is, and lowercases Mtavruli,
//! following Blink's `DisableNewGeorgianCapitalLetters`.
//!
//! A mapping reads only the text it is given: a sigma at its end ends a
//! word.

mod tables;

#[cfg(test)]
mod tests;

use crate::{core_props, rare_props};
use tables::{
    ACCENTS, CASE_DELTAS, CASE_EXCEPTIONS, CASE_RUN_DATA, CASE_RUNS, CASE_STRINGS, GREEK_ACCENT,
    GREEK_DIALYTIKA, GREEK_LETTERS, GREEK_UPPER_MASK, GREEK_VOWEL, GREEK_YPOGEGRAMMENI,
    RUN_EXCEPTIONS, RUN_KIND_MASK, RUN_LENGTH_MASK, RUN_LENGTH_SHIFT, RUN_LOWER, RUN_PAIRS,
    RUN_START_SHIFT, RUN_UPPER, SOFT_DOTTED,
};

/// The rules a language maps case by.
///
/// Four languages tailor Unicode's mappings: Turkish and Azerbaijani,
/// Lithuanian, Greek and Dutch. Every other language maps as the root does.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum CaseLanguage {
    // The languages Blink's `CaseMap::Locale` picks.
    /// Unicode's mappings, untailored.
    Root,
    /// Turkish and Azerbaijani: `i` uppercases to `İ`, and `I` lowercases to
    /// `ı`.
    Turkic,
    /// Lithuanian: an `i` keeps its dot under an accent.
    Lithuanian,
    /// Greek: capitals drop their accents.
    Greek,
    /// Dutch: `ij` titlecases to `IJ`.
    Dutch,
}

impl CaseLanguage {
    /// Returns the rules of the language whose primary subtag is `language`,
    /// such as `"tr"`.
    pub fn from_language(language: &str) -> Self {
        match language {
            "tr" | "az" => Self::Turkic,
            "lt" => Self::Lithuanian,
            "el" => Self::Greek,
            "nl" => Self::Dutch,
            _ => Self::Root,
        }
    }
}

/// Which of a character's three mappings to take, as its index in an
/// exception's row.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Mapping {
    Lower = 0,
    Upper = 1,
    Title = 2,
}

const DOT_ABOVE: char = '\u{307}';
const ACUTE: char = '\u{301}';

/// Calls `push` with each character of the full uppercase of `text`.
pub fn uppercase(text: &str, language: CaseLanguage, push: &mut dyn FnMut(char)) {
    if language == CaseLanguage::Greek {
        greek_uppercase(text, push);
        return;
    }
    for (at, ch) in text.char_indices() {
        match ch {
            'i' if language == CaseLanguage::Turkic => push('\u{130}'),
            _ if ch.is_ascii() => push(ch.to_ascii_uppercase()),
            DOT_ABOVE
                if language == CaseLanguage::Lithuanian
                    && follows_soft_dotted(text.get(..at).unwrap_or_default()) => {}
            _ => upper(ch, push),
        }
    }
}

/// Calls `push` with each character of the full lowercase of `text`.
pub fn lowercase(text: &str, language: CaseLanguage, push: &mut dyn FnMut(char)) {
    let lithuanian = language == CaseLanguage::Lithuanian;
    let turkic = language == CaseLanguage::Turkic;
    for (at, ch) in text.char_indices() {
        let before = || text.get(..at).unwrap_or_default();
        let after = || text.get(at + ch.len_utf8()..).unwrap_or_default();
        match ch {
            'I' | 'J' | '\u{12E}' if lithuanian && is_followed_by_more_above(after()) => {
                map(ch, Mapping::Lower, push);
                push(DOT_ABOVE);
            }
            '\u{CC}' | '\u{CD}' | '\u{128}' if lithuanian => {
                push('i');
                push(DOT_ABOVE);
                push(match ch {
                    '\u{CC}' => '\u{300}',
                    '\u{CD}' => ACUTE,
                    _ => '\u{303}',
                });
            }
            '\u{130}' if turkic => push('i'),
            DOT_ABOVE if turkic && follows_capital_i(before()) => {}
            'I' if turkic && !is_followed_by_dot_above(after()) => push('\u{131}'),
            '\u{3A3}'
                if !is_next_cased(after().chars()) && is_next_cased(before().chars().rev()) =>
            {
                push('\u{3C2}');
            }
            _ if ch.is_ascii() => push(ch.to_ascii_lowercase()),
            _ => map(ch, Mapping::Lower, push),
        }
    }
}

/// Calls `push` with each character of `segment` with its first character
/// titlecased and the rest as they are.
///
/// Dutch titlecases a `j` after an `i` as well, with an acute on both or on
/// neither.
pub fn titlecase(segment: &str, language: CaseLanguage, push: &mut dyn FnMut(char)) {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return;
    };
    let rest = chars.as_str();
    // The titlecase where it is one character.
    let mut title = None;
    if first == 'i' && language == CaseLanguage::Turkic {
        push('\u{130}');
    } else {
        let mut count = 0;
        map(first, Mapping::Title, &mut |ch| {
            count += 1;
            title = (count == 1).then_some(ch);
            push(ch);
        });
    }
    let j = match title {
        Some(title @ ('I' | '\u{CD}')) if language == CaseLanguage::Dutch => {
            dutch_j(rest, title == '\u{CD}')
        }
        _ => None,
    };
    for (at, ch) in rest.char_indices() {
        push(if Some(at) == j { 'J' } else { ch });
    }
}

/// Returns where in `rest` the `j` is that a Dutch `I` or `Í` titlecases,
/// after the `I` (`acute` false) or `Í` (`acute` true) it follows.
///
/// A plain `I` takes a plain `j` or `J`, and one with an acute, precomposed or
/// not, takes a `j` or `J` with an acute. No other mark may follow, as in
/// ICU4C's `maybeTitleDutchIJ`.
fn dutch_j(rest: &str, mut acute: bool) -> Option<usize> {
    let mut chars = rest.char_indices();
    let (mut at, mut ch) = chars.next()?;
    if !acute && ch == ACUTE {
        acute = true;
        (at, ch) = chars.next()?;
    }
    if !matches!(ch, 'j' | 'J') {
        return None;
    }
    if acute && chars.next()?.1 != ACUTE {
        return None;
    }
    if chars
        .next()
        .is_some_and(|(_, next)| core_props(next).is_mark())
    {
        return None;
    }
    (ch == 'j').then_some(at)
}

/// Calls `push` with the full uppercase of `ch` alone, with no Georgian
/// capitals.
fn upper(ch: char, push: &mut dyn FnMut(char)) {
    match ch {
        '\u{1C90}'..='\u{1CBF}' => push(char::from_u32(u32::from(ch) - 0xBC0).unwrap_or(ch)),
        _ => map(ch, Mapping::Upper, push),
    }
}

/// Calls `push` with the full mapping of `ch` alone in the root language.
fn map(ch: char, mapping: Mapping, push: &mut dyn FnMut(char)) {
    let cp = u32::from(ch);
    let at = CASE_RUNS.partition_point(|&run| run >> RUN_START_SHIFT <= cp);
    let Some((&run, &data)) = at
        .checked_sub(1)
        .and_then(|at| CASE_RUNS.get(at).zip(CASE_RUN_DATA.get(at)))
    else {
        push(ch);
        return;
    };
    let offset = cp - (run >> RUN_START_SHIFT);
    if offset > (run >> RUN_LENGTH_SHIFT) & RUN_LENGTH_MASK {
        push(ch);
        return;
    }
    let delta = || CASE_DELTAS.get(usize::from(data)).copied().unwrap_or(0);
    let delta = match (run & RUN_KIND_MASK, mapping) {
        (RUN_LOWER, Mapping::Upper | Mapping::Title) => delta(),
        (RUN_UPPER, Mapping::Lower) => delta(),
        (RUN_PAIRS, Mapping::Lower) if offset.is_multiple_of(2) => 1,
        (RUN_PAIRS, Mapping::Upper | Mapping::Title) if !offset.is_multiple_of(2) => -1,
        (RUN_EXCEPTIONS, _) => {
            let row = (usize::from(data) + offset as usize) * 3 + mapping as usize;
            let spelled = CASE_EXCEPTIONS.get(row).copied().unwrap_or(0);
            let start = usize::from(spelled >> 2);
            let units = CASE_STRINGS
                .get(start..start + usize::from(spelled & 3))
                .unwrap_or_default();
            if units.is_empty() {
                push(ch);
            }
            for &unit in units {
                push(char::from_u32(u32::from(unit)).unwrap_or(ch));
            }
            return;
        }
        _ => 0,
    };
    push(char::from_u32(cp.wrapping_add_signed(delta)).unwrap_or(ch));
}

/// Whether the first character of `chars` that is not case-ignorable is
/// cased: ICU4C's `isFollowedByCasedLetter`, either way.
fn is_next_cased(chars: impl Iterator<Item = char>) -> bool {
    chars
        .map(rare_props)
        .find(|props| !props.is_case_ignorable())
        .is_some_and(|props| props.is_cased())
}

/// The dot type ICU4C gives a character: what Turkic and Lithuanian casing
/// read of the marks around an `i`.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Dot {
    /// A starter that is not Soft_Dotted.
    None,
    /// Soft_Dotted: `i`, `j` and the letters like them.
    SoftDotted,
    /// A mark of combining class 230, above.
    Above,
    /// A mark of any other combining class.
    OtherAccent,
}

/// Returns the dot type of `ch`.
fn dot(ch: char) -> Dot {
    let cp = u32::from(ch);
    let at = SOFT_DOTTED.partition_point(|&range| range >> 8 <= cp);
    let soft_dotted = at
        .checked_sub(1)
        .and_then(|at| SOFT_DOTTED.get(at))
        .is_some_and(|&range| cp - (range >> 8) <= range & 0xFF);
    if soft_dotted {
        return Dot::SoftDotted;
    }
    let at = ACCENTS.partition_point(|&range| range >> 8 <= cp);
    match at.checked_sub(1).and_then(|at| ACCENTS.get(at)) {
        Some(&range) if cp - (range >> 8) <= (range >> 1) & 0x7F => {
            if range & 1 != 0 {
                Dot::Above
            } else {
                Dot::OtherAccent
            }
        }
        _ => Dot::None,
    }
}

/// Whether `before` ends in a Soft_Dotted character and marks of classes
/// other than 230.
fn follows_soft_dotted(before: &str) -> bool {
    before
        .chars()
        .rev()
        .map(dot)
        .find(|&dot| dot != Dot::OtherAccent)
        == Some(Dot::SoftDotted)
}

/// Whether `before` ends in `I` and marks of classes other than 230.
fn follows_capital_i(before: &str) -> bool {
    before
        .chars()
        .rev()
        .find(|&ch| ch == 'I' || dot(ch) != Dot::OtherAccent)
        == Some('I')
}

/// Whether `after` starts with marks of classes other than 230 and then one
/// of class 230.
fn is_followed_by_more_above(after: &str) -> bool {
    after.chars().map(dot).find(|&dot| dot != Dot::OtherAccent) == Some(Dot::Above)
}

/// Whether `after` starts with marks of classes other than 230 and then
/// U+0307.
fn is_followed_by_dot_above(after: &str) -> bool {
    after
        .chars()
        .find(|&ch| ch == DOT_ABOVE || dot(ch) != Dot::OtherAccent)
        == Some(DOT_ABOVE)
}

/// A dialytika added to a Greek iota or upsilon as a combining mark.
const GREEK_COMBINING_DIALYTIKA: u32 = 1 << 14;
/// A Greek diacritic that the capital drops and that says nothing more.
const GREEK_OTHER_DIACRITIC: u32 = 1 << 15;

// What Greek uppercasing knows of the character before.
/// The last character that is not case-ignorable is cased.
const AFTER_CASED: u32 = 1;
/// The last letter is a vowel whose accent was a combining mark, dropped.
const AFTER_VOWEL_WITH_COMBINING_ACCENT: u32 = 2;
/// The last letter is a vowel whose accent was precomposed, dropped.
const AFTER_VOWEL_WITH_PRECOMPOSED_ACCENT: u32 = 4;

/// What a Greek diacritic says of the letter it is on, or zero for a
/// character that is none (`GreekUpper::getDiacriticData`).
fn greek_diacritic(ch: char) -> u32 {
    match ch {
        '\u{300}' | '\u{301}' | '\u{342}' | '\u{302}' | '\u{303}' | '\u{311}' => GREEK_ACCENT,
        '\u{308}' => GREEK_COMBINING_DIALYTIKA,
        '\u{344}' => GREEK_COMBINING_DIALYTIKA | GREEK_ACCENT,
        '\u{345}' => GREEK_YPOGEGRAMMENI,
        '\u{304}' | '\u{306}' | '\u{313}' | '\u{314}' | '\u{343}' => GREEK_OTHER_DIACRITIC,
        _ => 0,
    }
}

/// What Greek uppercasing reads of `ch`: its base letter's capital and its
/// flags, or zero where it is no Greek letter (`GreekUpper::getLetterData`).
fn greek_letter(ch: char) -> u32 {
    let index = match u32::from(ch) {
        cp @ 0x370..=0x3FF => cp - 0x370,
        cp @ 0x1F00..=0x1FFF => cp - 0x1F00 + 0x90,
        // The ohm sign is an omega.
        0x2126 => 0x3A9 - 0x370,
        _ => return 0,
    };
    GREEK_LETTERS
        .get(index as usize)
        .map_or(0, |&entry| u32::from(entry))
}

/// Calls `push` with each character of the Greek uppercase of `text`, as
/// ICU4C's `GreekUpper::toUpper` makes it.
///
/// A capital drops its accents and breathings. It keeps a dialytika, which
/// it gains where the vowel before lost an accent, and a lone `ή` keeps its
/// accent. An iota subscript becomes a capital iota after the letter.
#[cold]
#[inline(never)]
fn greek_uppercase(text: &str, push: &mut dyn FnMut(char)) {
    const EITHER_DIALYTIKA: u32 = GREEK_DIALYTIKA | GREEK_COMBINING_DIALYTIKA;
    const AFTER_ACCENT: u32 =
        AFTER_VOWEL_WITH_COMBINING_ACCENT | AFTER_VOWEL_WITH_PRECOMPOSED_ACCENT;
    let mut state = 0;
    let mut at = 0;
    while let Some(ch) = text.get(at..).and_then(|rest| rest.chars().next()) {
        let mut next = at + ch.len_utf8();
        let props = rare_props(ch);
        let mut next_state = 0;
        if props.is_case_ignorable() {
            next_state |= state & AFTER_CASED;
        } else if props.is_cased() {
            next_state |= AFTER_CASED;
        }
        let mut data = greek_letter(ch);
        if data == 0 {
            upper(ch, push);
            at = next;
            state = next_state;
            continue;
        }
        let mut capital = data & GREEK_UPPER_MASK;
        if data & GREEK_VOWEL != 0 && state & AFTER_ACCENT != 0 && matches!(capital, 0x399 | 0x3A5)
        {
            data |= if state & AFTER_VOWEL_WITH_PRECOMPOSED_ACCENT != 0 {
                GREEK_DIALYTIKA
            } else {
                GREEK_COMBINING_DIALYTIKA
            };
        }
        let mut iotas = u32::from(data & GREEK_YPOGEGRAMMENI != 0);
        let precomposed_accent = data & GREEK_ACCENT != 0;
        while let Some(mark) = text.get(next..).and_then(|rest| rest.chars().next()) {
            let flags = greek_diacritic(mark);
            if flags == 0 {
                break;
            }
            data |= flags;
            iotas += u32::from(flags & GREEK_YPOGEGRAMMENI != 0);
            next += mark.len_utf8();
        }
        if data & (GREEK_VOWEL | GREEK_ACCENT | GREEK_DIALYTIKA) == GREEK_VOWEL | GREEK_ACCENT {
            next_state |= if precomposed_accent {
                AFTER_VOWEL_WITH_PRECOMPOSED_ACCENT
            } else {
                AFTER_VOWEL_WITH_COMBINING_ACCENT
            };
        }
        let mut tonos = false;
        if capital == 0x397
            && data & GREEK_ACCENT != 0
            && iotas == 0
            && state & AFTER_CASED == 0
            && !is_next_cased(text.get(next..).unwrap_or_default().chars())
        {
            // A lone eta, the disjunctive "or", keeps its accent.
            if precomposed_accent {
                capital = 0x389;
            } else {
                tonos = true;
            }
        } else if data & GREEK_DIALYTIKA != 0 && matches!(capital, 0x399 | 0x3A5) {
            // The capital with the dialytika precomposed.
            capital = if capital == 0x399 { 0x3AA } else { 0x3AB };
            data &= !EITHER_DIALYTIKA;
        }
        push(char::from_u32(capital).unwrap_or(ch));
        if data & EITHER_DIALYTIKA != 0 {
            push('\u{308}');
        }
        if tonos {
            push(ACUTE);
        }
        for _ in 0..iotas {
            push('\u{399}');
        }
        at = next;
        state = next_state;
    }
}
