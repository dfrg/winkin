//! The word boundary rules of UAX #29, WB3 to WB999, as one table of a
//! state and the class after it.
//!
//! A state is the class of the last character that is not attached to the
//! one before it (WB4), or one of four contexts that a rule reads further
//! back: a letter and then a middle letter (WB7), a number and then a middle
//! number (WB11), a Hebrew letter and then a double quote (WB7c), and an
//! even run of regional indicators (WB15, WB16). WB3c reads whether a
//! zero width joiner comes last, which the scan keeps apart.

/// The word classes: Word_Break, with U+002E FULL STOP apart from MidNumLet
/// and the ALetter of Han and Hiragana apart from ALetter, as the generator
/// numbers them.
pub(super) const OTHER: u8 = 0;
pub(super) const CR: u8 = 1;
pub(super) const LF: u8 = 2;
pub(super) const NEWLINE: u8 = 3;
pub(super) const EXTEND: u8 = 4;
pub(super) const ZWJ: u8 = 5;
pub(super) const REGIONAL_INDICATOR: u8 = 6;
pub(super) const FORMAT: u8 = 7;
pub(super) const KATAKANA: u8 = 8;
pub(super) const HEBREW_LETTER: u8 = 9;
pub(super) const LETTER: u8 = 10;
pub(super) const SINGLE_QUOTE: u8 = 11;
pub(super) const DOUBLE_QUOTE: u8 = 12;
pub(super) const MID_NUM_LETTER: u8 = 13;
pub(super) const MID_LETTER: u8 = 14;
pub(super) const MID_NUMBER: u8 = 15;
pub(super) const NUMERIC: u8 = 16;
pub(super) const EXTEND_NUM_LETTER: u8 = 17;
pub(super) const SEGMENT_SPACE: u8 = 18;
/// U+002E FULL STOP, which the scan reads as MidNumLet, or as MidNum under
/// the POSIX tailoring.
pub(super) const FULL_STOP: u8 = 19;
/// An ALetter of Han or Hiragana, such as U+3005 `々`, which the scan reads
/// as ALetter, or as no letter where a dictionary finds the words of Han
/// and Hiragana.
pub(super) const CHINESE_JAPANESE_LETTER: u8 = 20;
/// The number of classes.
pub(super) const CLASSES: usize = 21;

/// A letter, then a MidLetter, MidNumLet or Single_Quote (WB7).
const LETTER_MID: u8 = 21;
/// A number, then a MidNum, MidNumLet or Single_Quote (WB11).
const NUMBER_MID: u8 = 22;
/// A Hebrew letter, then a double quote (WB7c).
const HEBREW_QUOTE: u8 = 23;
/// An even run of regional indicators, which the next one does not join.
const INDICATOR_PAIR: u8 = 24;
/// The start of a text, before its first character.
pub(super) const START: u8 = 25;
/// The number of states.
const STATES: usize = 26;

/// No boundary.
pub(super) const KEEP: u8 = 0;
/// A boundary.
pub(super) const BREAK: u8 = 1;
/// No boundary if the next class that is not Extend, Format or ZWJ is a
/// letter (WB6).
pub(super) const NEEDS_LETTER: u8 = 2;
/// No boundary if that class is a Hebrew letter (WB7b).
pub(super) const NEEDS_HEBREW: u8 = 3;
/// No boundary if that class is a number (WB12).
pub(super) const NEEDS_NUMBER: u8 = 4;

/// The bits of a [`PAIRS`] entry that hold the answer.
const ANSWER_MASK: u8 = 0x7;
/// The shift of a [`PAIRS`] entry's next state.
const NEXT_SHIFT: u32 = 3;

/// Returns whether `class` is AHLetter.
const fn is_letter(class: u8) -> bool {
    matches!(class, LETTER | HEBREW_LETTER)
}

/// Returns whether `class` is MidLetter or MidNumLetQ.
const fn is_mid_letter(class: u8) -> bool {
    matches!(class, MID_LETTER | MID_NUM_LETTER | SINGLE_QUOTE)
}

/// Returns whether `class` is MidNum or MidNumLetQ.
const fn is_mid_number(class: u8) -> bool {
    matches!(class, MID_NUMBER | MID_NUM_LETTER | SINGLE_QUOTE)
}

/// Returns whether ExtendNumLet joins `class` (WB13a, WB13b).
const fn extends(class: u8) -> bool {
    is_letter(class) || class == NUMERIC || class == KATAKANA
}

/// Returns whether `class` attaches to the character before it (WB4).
pub(super) const fn is_attached(class: u8) -> bool {
    matches!(class, EXTEND | FORMAT | ZWJ)
}

/// Returns whether `class` is a line end (WB3a, WB3b).
const fn is_newline(class: u8) -> bool {
    matches!(class, CR | LF | NEWLINE)
}

/// Returns whether a boundary falls between the state `left` and a
/// character of class `right`, or what the answer waits on.
const fn decide(left: u8, right: u8) -> u8 {
    // WB3, WB3a, WB3b.
    if left == CR && right == LF {
        return KEEP;
    }
    if is_newline(left) || is_newline(right) {
        return BREAK;
    }
    // WB3d: a segment space with nothing attached to it.
    if left == SEGMENT_SPACE && right == SEGMENT_SPACE {
        return KEEP;
    }
    // WB4.
    if is_attached(right) {
        return KEEP;
    }
    // WB5, WB6, WB7a, WB7b.
    if is_letter(left) {
        if is_letter(right) {
            return KEEP;
        }
        if left == HEBREW_LETTER && right == SINGLE_QUOTE {
            return KEEP;
        }
        if is_mid_letter(right) {
            return NEEDS_LETTER;
        }
        if left == HEBREW_LETTER && right == DOUBLE_QUOTE {
            return NEEDS_HEBREW;
        }
    }
    // WB7, WB7c, WB11.
    if (left == LETTER_MID && is_letter(right))
        || (left == HEBREW_QUOTE && right == HEBREW_LETTER)
        || (left == NUMBER_MID && right == NUMERIC)
    {
        return KEEP;
    }
    // WB8, WB9, WB10.
    if (left == NUMERIC || is_letter(left)) && (right == NUMERIC || is_letter(right)) {
        return KEEP;
    }
    // WB12.
    if left == NUMERIC && is_mid_number(right) {
        return NEEDS_NUMBER;
    }
    // WB13, WB13a, WB13b.
    if left == KATAKANA && right == KATAKANA {
        return KEEP;
    }
    if (extends(left) || left == EXTEND_NUM_LETTER) && right == EXTEND_NUM_LETTER {
        return KEEP;
    }
    if left == EXTEND_NUM_LETTER && extends(right) {
        return KEEP;
    }
    // WB15, WB16: the state is an odd run of indicators.
    if left == REGIONAL_INDICATOR && right == REGIONAL_INDICATOR {
        return KEEP;
    }
    BREAK
}

/// Returns the state after a character of class `right` follows the state
/// `left`.
const fn next(left: u8, right: u8) -> u8 {
    if is_attached(right) {
        return if is_newline(left) || left == START {
            // WB4 attaches nothing to a line end or the start: the
            // character stands for itself.
            right
        } else if left == SEGMENT_SPACE {
            // WB3d reads the space alone, and no other rule reads it.
            OTHER
        } else {
            left
        };
    }
    if is_letter(left) && is_mid_letter(right) {
        LETTER_MID
    } else if left == NUMERIC && is_mid_number(right) {
        NUMBER_MID
    } else if left == HEBREW_LETTER && right == DOUBLE_QUOTE {
        HEBREW_QUOTE
    } else if left == REGIONAL_INDICATOR && right == REGIONAL_INDICATOR {
        INDICATOR_PAIR
    } else {
        right
    }
}

/// Each state's answer before each class, in the low three bits, and the
/// state after it above them.
static PAIRS: [[u8; CLASSES]; STATES] = {
    let mut table = [[0; CLASSES]; STATES];
    let mut left = 0;
    while left < STATES {
        let mut right = 0;
        while right < CLASSES {
            let answer = if left as u8 == START {
                BREAK
            } else {
                decide(left as u8, right as u8)
            };
            table[left][right] = answer | next(left as u8, right as u8) << NEXT_SHIFT;
            right += 1;
        }
        left += 1;
    }
    table
};

/// Returns the answer between the state `left` and a character of class
/// `right`, and the state after it.
#[inline(always)]
pub(super) fn pair(left: u8, right: u8) -> (u8, u8) {
    let entry = PAIRS
        .get(usize::from(left))
        .and_then(|row| row.get(usize::from(right)))
        .copied()
        .unwrap_or(BREAK | OTHER << NEXT_SHIFT);
    (entry & ANSWER_MASK, entry >> NEXT_SHIFT)
}

/// Returns whether a character of class `next`, the first after the pair
/// that no rule attaches, answers `waits` with no boundary.
#[inline]
pub(super) const fn holds(waits: u8, next: u8) -> bool {
    match waits {
        NEEDS_LETTER => is_letter(next),
        NEEDS_HEBREW => next == HEBREW_LETTER,
        NEEDS_NUMBER => next == NUMERIC,
        _ => false,
    }
}
