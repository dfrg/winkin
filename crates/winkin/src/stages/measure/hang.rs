//! `hanging-punctuation`: which marks hang past a line's edges.
//!
//! Chrome parses neither `hanging-punctuation` nor `line-padding`, so their
//! initial values give Chrome's lines. A hanging mark draws, but its width
//! doesn't count against the line (CSS Text 4, "hanging glyphs"). The scan
//! writes each hang into the line-edge costs:
//! - `first`: a negative start cost only the block's first line pays.
//! - `force-end`, and `last` at the text's end: a flex the breaker always
//!   takes (`HANG_FORCED`).
//! - `allow-end`: a flex the breaker takes only where the line doesn't fit.
//!
//! Intrinsic sizes leave out a hanging mark, as CSS Text 4 says: under
//! `allow-end` from min-content only, otherwise from both.
//!
//! A mark hangs only at the line's edge: a box edge with room in between
//! stops it. White space hangs first, so a comma before a space still hangs.
//!
//! `line-padding` comes from the innermost text at each end of the line, and
//! the breaker pays it as a line-edge cost.
//!
//! The marks are CSS Text 4's lists, written out since the crate has no
//! general categories. A test checks them against Unicode's.

use super::LineEdgeFlags;
use crate::style::{HangEnd, HangingPunctuation};

/// Returns whether `ch`, the block's first character, hangs at the start of
/// the first line under `hanging`.
pub(super) fn hangs_first(hanging: HangingPunctuation, ch: char) -> bool {
    hanging.first && opens(ch)
}

/// Returns the line-edge cost flags for `ch` hanging at a line's end, or
/// `None` where it doesn't hang.
///
/// `ch` is the line's last character before any hanging white space;
/// `at_text_end` marks a line that ends the text. [`LineEdgeFlags::HANG_FORCED`]
/// means it always hangs; no flags means it hangs only where the line
/// wouldn't fit with it (`allow-end`).
pub(super) fn hangs_end(
    hanging: HangingPunctuation,
    ch: char,
    at_text_end: bool,
) -> Option<LineEdgeFlags> {
    if hanging.last && at_text_end && closes(ch) {
        return Some(LineEdgeFlags::HANG_FORCED);
    }
    if !stop_or_comma(ch) {
        return None;
    }
    match hanging.end {
        HangEnd::None => None,
        HangEnd::Allow => Some(LineEdgeFlags::NONE),
        HangEnd::Force => Some(LineEdgeFlags::HANG_FORCED),
    }
}

/// Returns whether `ch` is an opening bracket or quote, or U+3000, which
/// `first` hangs: Unicode's Ps, Pi and Pf, and the two straight quotes.
pub(super) fn opens(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\''
            | '\u{3000}'
            | '('
            | '['
            | '{'
            | '\u{AB}'
            | '\u{BB}'
            | '\u{F3A}'
            | '\u{F3C}'
            | '\u{169B}'
            | '\u{2018}'..='\u{201F}'
            | '\u{2039}'..='\u{203A}'
            | '\u{2045}'
            | '\u{207D}'
            | '\u{208D}'
            | '\u{2308}'
            | '\u{230A}'
            | '\u{2329}'
            | '\u{2768}'
            | '\u{276A}'
            | '\u{276C}'
            | '\u{276E}'
            | '\u{2770}'
            | '\u{2772}'
            | '\u{2774}'
            | '\u{27C5}'
            | '\u{27E6}'
            | '\u{27E8}'
            | '\u{27EA}'
            | '\u{27EC}'
            | '\u{27EE}'
            | '\u{2983}'
            | '\u{2985}'
            | '\u{2987}'
            | '\u{2989}'
            | '\u{298B}'
            | '\u{298D}'
            | '\u{298F}'
            | '\u{2991}'
            | '\u{2993}'
            | '\u{2995}'
            | '\u{2997}'
            | '\u{29D8}'
            | '\u{29DA}'
            | '\u{29FC}'
            | '\u{2E02}'..='\u{2E05}'
            | '\u{2E09}'..='\u{2E0A}'
            | '\u{2E0C}'..='\u{2E0D}'
            | '\u{2E1C}'..='\u{2E1D}'
            | '\u{2E20}'..='\u{2E22}'
            | '\u{2E24}'
            | '\u{2E26}'
            | '\u{2E28}'
            | '\u{2E42}'
            | '\u{2E55}'
            | '\u{2E57}'
            | '\u{2E59}'
            | '\u{2E5B}'
            | '\u{3008}'
            | '\u{300A}'
            | '\u{300C}'
            | '\u{300E}'
            | '\u{3010}'
            | '\u{3014}'
            | '\u{3016}'
            | '\u{3018}'
            | '\u{301A}'
            | '\u{301D}'
            | '\u{FD3F}'
            | '\u{FE17}'
            | '\u{FE35}'
            | '\u{FE37}'
            | '\u{FE39}'
            | '\u{FE3B}'
            | '\u{FE3D}'
            | '\u{FE3F}'
            | '\u{FE41}'
            | '\u{FE43}'
            | '\u{FE47}'
            | '\u{FE59}'
            | '\u{FE5B}'
            | '\u{FE5D}'
            | '\u{FF08}'
            | '\u{FF3B}'
            | '\u{FF5B}'
            | '\u{FF5F}'
            | '\u{FF62}'
    )
}

/// Returns whether `ch` is a closing bracket or quote, which `last` hangs:
/// Unicode's Pe, Pi and Pf, and the two straight quotes.
pub(super) fn closes(ch: char) -> bool {
    matches!(
        ch,
        '"' | '\''
            | ')'
            | ']'
            | '}'
            | '\u{AB}'
            | '\u{BB}'
            | '\u{F3B}'
            | '\u{F3D}'
            | '\u{169C}'
            | '\u{2018}'..='\u{2019}'
            | '\u{201B}'..='\u{201D}'
            | '\u{201F}'
            | '\u{2039}'..='\u{203A}'
            | '\u{2046}'
            | '\u{207E}'
            | '\u{208E}'
            | '\u{2309}'
            | '\u{230B}'
            | '\u{232A}'
            | '\u{2769}'
            | '\u{276B}'
            | '\u{276D}'
            | '\u{276F}'
            | '\u{2771}'
            | '\u{2773}'
            | '\u{2775}'
            | '\u{27C6}'
            | '\u{27E7}'
            | '\u{27E9}'
            | '\u{27EB}'
            | '\u{27ED}'
            | '\u{27EF}'
            | '\u{2984}'
            | '\u{2986}'
            | '\u{2988}'
            | '\u{298A}'
            | '\u{298C}'
            | '\u{298E}'
            | '\u{2990}'
            | '\u{2992}'
            | '\u{2994}'
            | '\u{2996}'
            | '\u{2998}'
            | '\u{29D9}'
            | '\u{29DB}'
            | '\u{29FD}'
            | '\u{2E02}'..='\u{2E05}'
            | '\u{2E09}'..='\u{2E0A}'
            | '\u{2E0C}'..='\u{2E0D}'
            | '\u{2E1C}'..='\u{2E1D}'
            | '\u{2E20}'..='\u{2E21}'
            | '\u{2E23}'
            | '\u{2E25}'
            | '\u{2E27}'
            | '\u{2E29}'
            | '\u{2E56}'
            | '\u{2E58}'
            | '\u{2E5A}'
            | '\u{2E5C}'
            | '\u{3009}'
            | '\u{300B}'
            | '\u{300D}'
            | '\u{300F}'
            | '\u{3011}'
            | '\u{3015}'
            | '\u{3017}'
            | '\u{3019}'
            | '\u{301B}'
            | '\u{301E}'..='\u{301F}'
            | '\u{FD3E}'
            | '\u{FE18}'
            | '\u{FE36}'
            | '\u{FE38}'
            | '\u{FE3A}'
            | '\u{FE3C}'
            | '\u{FE3E}'
            | '\u{FE40}'
            | '\u{FE42}'
            | '\u{FE44}'
            | '\u{FE48}'
            | '\u{FE5A}'
            | '\u{FE5C}'
            | '\u{FE5E}'
            | '\u{FF09}'
            | '\u{FF3D}'
            | '\u{FF5D}'
            | '\u{FF60}'
            | '\u{FF63}'
    )
}

/// Returns whether `ch` is a stop or comma in CSS Text 4's list, which
/// `allow-end` and `force-end` hang.
fn stop_or_comma(ch: char) -> bool {
    matches!(
        ch,
        ',' | '.'
            | '\u{60C}'
            | '\u{6D4}'
            | '\u{3001}'
            | '\u{3002}'
            | '\u{FE50}'
            | '\u{FE51}'
            | '\u{FE52}'
            | '\u{FF0C}'
            | '\u{FF0E}'
            | '\u{FF61}'
            | '\u{FF64}'
    )
}
