//! Line-break opportunities in Latin-1 text from a pair table, which the
//! line stream tries before the line engine.
//!
//! The table is the engine's UAX #14 rules restricted to the Latin-1
//! characters they decide by pairs and a few states, so it gives the
//! engine's answers, not Blink's. Blink's own ASCII rules are in `ascii`,
//! which the writer asks first and which overrides the stream where it
//! decides. The tests hold the table to the engine for every pair of covered
//! characters and every string of their classes up to four long.
//!
//! The rules restart at each opportunity. For these classes they need a few
//! states beside the classes:
//! - an opening punctuation and its following spaces, after which no line
//!   breaks (LB14);
//! - a hyphen starting a word, after which no line breaks before a letter
//!   (LB20a);
//! - a symbol, an infix separator or a closing punctuation inside a number,
//!   after which no line breaks before a prefix or postfix (LB25).
//!
//! The table gives no answer where the rules read further ahead: an infix
//! separator after a space (LB15c), and an opening punctuation after a
//! prefix or postfix (LB25). It leaves out every other character the rules
//! keep state for: the French quotation marks (LB15a, LB15b, LB19a),
//! combining marks (LB9), classes the options remap, the characters
//! `line-break: loose` treats specially in Japanese and Chinese (`¤`, `°`,
//! `±`), and every control. At any of them the table gives no answer and the
//! stream asks the engine from the last opportunity.

use crate::work;

/// A covered character's Line_Break class as the engine reads it (AI as
/// AL), numbered as a bit of a table row.
type Class = u8;

const AL: Class = 0;
const NU: Class = 1;
const SP: Class = 2;
const BA: Class = 3;
const HY: Class = 4;
const IS: Class = 5;
const SY: Class = 6;
const EX: Class = 7;
const QU: Class = 8;
const OP: Class = 9;
const CP: Class = 10;
const CL: Class = 11;
const GL: Class = 12;
const PR: Class = 13;
const PO: Class = 14;
const BB: Class = 15;
const HH: Class = 16;

/// A character the table does not cover.
const NONE: Class = 0xFF;

// The states that are not a class.

/// An opening punctuation and one or more spaces, where no line breaks
/// before the next character (LB14).
const OP_SP: u8 = 17;
/// A hyphen-minus starting a word: at the start, or after a space or a
/// no-break space (LB20a). A space after an opening punctuation is a
/// space.
const HY_WORD: u8 = 18;
/// A soft hyphen starting a word (LB20a).
const HH_WORD: u8 = 19;
/// A symbol in a number: `NU (SY | IS)*` (LB25).
const SY_NU: u8 = 20;
/// An infix separator in a number: `NU (SY | IS)*` (LB25).
const IS_NU: u8 = 21;
/// A closing punctuation after a number (LB25).
const CL_NU: u8 = 22;
/// A closing parenthesis after a number (LB25).
const CP_NU: u8 = 23;

/// The number of states, each a row of the table.
const STATES: usize = 24;

/// Returns the class of the Latin-1 code point `cp`, or [`NONE`].
const fn class(cp: u8) -> Class {
    match cp {
        b'a'..=b'z'
        | b'A'..=b'Z'
        | b'#'
        | b'&'
        | b'*'
        | b'<'
        | b'='
        | b'>'
        | b'@'
        | b'^'
        | b'_'
        | b'`'
        | b'~'
        | 0xA6..=0xAA
        | 0xAC
        | 0xAE
        | 0xAF
        | 0xB2
        | 0xB3
        | 0xB5
        | 0xB6
        | 0xB8..=0xBA
        | 0xBC..=0xBE
        | 0xC0..=0xFF => AL,
        b'0'..=b'9' => NU,
        b' ' => SP,
        // A tab and the vertical line.
        b'\t' | b'|' => BA,
        // The soft hyphen, HH since Unicode 18.0.
        0xAD => HH,
        b'-' => HY,
        // And the middle dot, IS since Unicode 18.0.
        b',' | b'.' | b':' | b';' | 0xB7 => IS,
        b'/' => SY,
        b'!' | b'?' => EX,
        b'"' | b'\'' => QU,
        // And the inverted exclamation and question marks.
        b'(' | b'[' | b'{' | 0xA1 | 0xBF => OP,
        b')' | b']' => CP,
        b'}' => CL,
        // The no-break space.
        0xA0 => GL,
        // And the pound and yen signs.
        b'$' | b'+' | b'\\' | 0xA3 | 0xA5 => PR,
        // And the cent sign.
        b'%' | 0xA2 => PO,
        // The acute accent.
        0xB4 => BB,
        _ => NONE,
    }
}

/// Every Latin-1 code point's class.
static CLASSES: [Class; 256] = {
    let mut classes = [NONE; 256];
    let mut cp = 0;
    while cp < 256 {
        classes[cp] = class(cp as u8);
        cp += 1;
    }
    classes
};

/// Returns a table row with a bit set for each class in `classes`, which a
/// line may break before.
const fn before(classes: &[Class]) -> u32 {
    let mut bits = 0;
    let mut at = 0;
    while at < classes.len() {
        bits |= 1 << classes[at];
        at += 1;
    }
    bits
}

/// The rules' answers for these classes, one row per state.
///
/// Bit `c` of a row is set where a line may break between the state and a
/// following character of class `c`.
static BREAKS: [u32; STATES] = [
    /* AL */ before(&[BB]),
    /* NU */ before(&[BB]),
    /* SP */ before(&[AL, NU, BA, HH, HY, QU, OP, GL, PR, PO, BB]),
    /* BA */ before(&[AL, NU, OP, PR, PO, BB]),
    /* HY */ before(&[AL, OP, GL, PR, PO, BB]),
    /* IS */ before(&[OP, PR, PO, BB]),
    /* SY */ before(&[AL, NU, OP, PR, PO, BB]),
    /* EX */ before(&[AL, NU, OP, PR, PO, BB]),
    /* QU */ before(&[]),
    /* OP */ before(&[]),
    /* CP */ before(&[OP, PR, PO, BB]),
    /* CL */ before(&[AL, NU, OP, PR, PO, BB]),
    /* GL */ before(&[]),
    /* PR */ before(&[PR, PO, BB]),
    /* PO */ before(&[PR, PO, BB]),
    /* BB */ before(&[]),
    /* HH */ before(&[AL, NU, OP, GL, PR, PO, BB]),
    /* OP_SP */ before(&[]),
    /* HY_WORD */ before(&[OP, GL, PR, PO, BB]),
    /* HH_WORD */ before(&[NU, OP, GL, PR, PO, BB]),
    /* SY_NU */ before(&[AL, OP, BB]),
    /* IS_NU */ before(&[OP, BB]),
    /* CL_NU */ before(&[AL, NU, OP, BB]),
    /* CP_NU */ before(&[OP, BB]),
];

/// Returns whether the rules need more than the state and the class to
/// decide: an infix separator after a space (LB15c), or an opening
/// punctuation after a prefix or postfix (LB25).
const fn reads_ahead(state: u8, class: Class) -> bool {
    (state == SP && class == IS) || ((state == PR || state == PO) && class == OP)
}

/// Returns the state after a character of class `class` that follows the
/// state `state`.
const fn next_state(state: u8, class: Class) -> u8 {
    let number = state == NU || state == SY_NU || state == IS_NU;
    let word_start = state == SP || state == OP_SP || state == GL;
    match class {
        SP if state == OP || state == OP_SP => OP_SP,
        HY if word_start => HY_WORD,
        HH if word_start => HH_WORD,
        SY if number => SY_NU,
        IS if number => IS_NU,
        CL if number => CL_NU,
        CP if number => CP_NU,
        _ => class,
    }
}

/// Returns the class and byte length of the character at byte `at`, or
/// `None` where the table does not cover it.
///
/// Only an ASCII byte or a two-byte sequence for U+0080 to U+00FF can be
/// covered.
#[inline]
fn table_class(bytes: &[u8], at: usize) -> Option<(Class, usize)> {
    let lead = *bytes.get(at)?;
    let (cp, len) = if lead < 0x80 {
        (usize::from(lead), 1)
    } else if lead & 0xFE == 0xC2 {
        let trail = *bytes.get(at + 1)?;
        (
            (usize::from(lead & 0x1F) << 6) | usize::from(trail & 0x3F),
            2,
        )
    } else {
        return None;
    };
    let class = *CLASSES.get(cp)?;
    (class != NONE).then_some((class, len))
}

/// Returns the next opportunity after byte `from` of `text`, segmenting
/// from `from` as from the text's start.
///
/// Returns the text's end if no opportunity comes first, and `None` if an
/// uncovered character starts at `from` or comes before the opportunity, or
/// the rules read ahead before it.
pub(super) fn next_break(text: &str, from: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let (first, len) = table_class(bytes, from)?;
    // The start of a text starts a word.
    let mut state = match first {
        HY => HY_WORD,
        HH => HH_WORD,
        class => class,
    };
    let mut at = from + len;
    while at < bytes.len() {
        work::step();
        let (class, len) = table_class(bytes, at)?;
        if reads_ahead(state, class) {
            return None;
        }
        let row = BREAKS.get(usize::from(state)).copied().unwrap_or(0);
        if row & (1 << class) != 0 {
            return Some(at);
        }
        state = next_state(state, class);
        at += len;
    }
    Some(bytes.len())
}

/// Returns whether the character at byte `at` of `text` may be covered,
/// judged by its first byte.
///
/// It is cheap enough to test at every opportunity before [`next_break`],
/// even in text the table never covers, such as ideographs.
#[inline]
pub(super) fn may_cover(text: &str, at: usize) -> bool {
    text.as_bytes().get(at).is_some_and(|&lead| lead < 0xC4)
}

/// Returns whether the table covers `ch`.
#[cfg(test)]
pub(super) fn covers(ch: char) -> bool {
    u8::try_from(u32::from(ch)).is_ok_and(|cp| class(cp) != NONE)
}
