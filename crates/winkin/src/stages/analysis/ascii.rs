//! Blink's own line-break rules for ASCII, which it applies before it asks
//! ICU.
//!
//! Blink decides a boundary by three rules before asking its ICU iterator
//! (`LazyLineBreakIterator::NextBreakablePosition` and
//! `Context::ShouldBreakFast`, `platform/text/text_break_iterator.cc`):
//!
//! - **After a run of spaces** a line may break whatever follows, and never
//!   before a space (`BreakSpaceType::kAfterSpaceRun`). A space is U+0020, a
//!   tab or a line feed (`IsBreakableSpace`). CSS Text 3, section 4.1.3 puts
//!   an opportunity after every run of kept spaces; Blink puts one after
//!   every run, collapsed or kept, so `Bonjour !` may break before its `!`.
//! - **A hyphen before a digit** breaks only after an ASCII letter or digit,
//!   so `ABCD-1234` and `1234-5678` in long URLs break and a minus sign does
//!   not.
//! - **Between two printable ASCII characters**, Blink's pair table decides
//!   (`LineBreakData::FillAscii`, generated into `kFastLineBreakTable`): a
//!   break before `(`, `<`, `[` and `{` after most punctuation, after `-`
//!   and `?` before nearly anything, and nowhere else. So nothing breaks
//!   after `/` between letters or digits, where ICU breaks `example.com/a`
//!   after its slash.
//!
//! Blink leaves everything else to ICU:
//! - a pair with a character past ASCII. Blink's table reaches U+00FF, but
//!   past ASCII it holds ICU's answers for the pair alone; the stream gives
//!   them with their context;
//! - a hyphen before a character past ASCII;
//! - a control character other than DEL, which the table holds. The
//!   analysis decides a control's breaks by its own rules.
//!
//! Measured in 10 px Ahem, each string in a zero-width column: all 8,836 pairs
//! of printable ASCII characters break as the table does; the 94 `?-1` strings
//! break before the digit only after a letter or digit; and a space breaks
//! before every printable character and before `»`, `«`, `、`, `」`, `）`,
//! U+2060, U+00A0, `é`, U+0F0B, `—`, `°` and `¢` alike.
//!
//! Nothing here allocates or panics.

/// Returns whether a line may break between `before` and `after` where
/// Blink's ASCII rules decide it, or `None` where Blink asks ICU.
///
/// `before_before` finds the character before `before`; only a hyphen
/// before a digit reads it.
pub(super) fn breaks(
    before_before: impl FnOnce() -> Option<char>,
    before: char,
    after: char,
) -> Option<bool> {
    if is_breakable_space(after) {
        return Some(false);
    }
    if is_breakable_space(before) {
        return Some(true);
    }
    let (before, after) = (u32::from(before), u32::from(after));
    if before == u32::from(b'-') && after > 0x7F {
        return None;
    }
    let printable = 0x21..=0x7F;
    if !printable.contains(&before) || !printable.contains(&after) {
        return None;
    }
    // Both are ASCII, so both fit a byte.
    let (before, after) = (u8::try_from(before).ok()?, u8::try_from(after).ok()?);
    if before == b'-' && after.is_ascii_digit() {
        return Some(before_before().is_some_and(|ch| ch.is_ascii_alphanumeric()));
    }
    Some(pair_breaks(before, after))
}

/// Returns whether a line may break between `before` and `after` under
/// `word-break: break-all` where Blink's ASCII rules decide it, or `None`
/// where Blink asks ICU.
///
/// Blink's break-all table adds opportunities to the pair table and takes
/// none away (`ShouldBreakAfterBreakAll`). Its ICU iterator is never asked
/// between two ASCII characters.
pub(super) fn breaks_all(
    before_before: impl FnOnce() -> Option<char>,
    before: char,
    after: char,
) -> Option<bool> {
    let breaks = breaks(before_before, before, after)?;
    let adds = u8::try_from(before)
        .ok()
        .zip(u8::try_from(after).ok())
        .is_some_and(|(before, after)| break_all_adds(before, after));
    Some(breaks || adds)
}

/// Returns whether Blink's break-all table adds an opportunity between two
/// printable ASCII characters.
///
/// The table is indexed by line-break class, and Blink's data puts `+` with
/// the letters rather than with `$` and `\`. Measured in 10 px Ahem, each pair
/// alone in a zero-width column, it adds these (after each group, before the
/// listed characters):
/// - a letter, a digit, `/`, `<`, `@`, `^`, `_` or `` ` ``: a letter or `#`,
///   `&`, `*`, `+`, `-`, `=`, `>`, `@`, `^`, `_`, `` ` ``, `|` or `~` (call
///   them letter-like), `$`, `\`, a digit, or an opening `(`, `<`, `[`, `{`;
/// - `!` or `%`: the letter-like, `$`, `\`, `%` or a digit;
/// - `#`, `&`, `)`, `*`, `+`, `=`, `>`, `]`, `|`, `}` or `~`: the
///   letter-like, `$`, `\` or a digit;
/// - `,`, `.`, `:` or `;`: the letter-like or a digit;
/// - `$` or `\`: `%`;
/// - `-`: a digit;
/// - and after `"`, `'`, `(`, `?`, `[`, `{` and DEL, before nothing.
fn break_all_adds(before: u8, after: u8) -> bool {
    let letter = matches!(
        after,
        b'#' | b'&'
            | b'*'
            | b'+'
            | b'-'
            | b'='
            | b'>'
            | b'@'
            | b'A'..=b'Z'
            | b'^'..=b'`'
            | b'a'..=b'z'
            | b'|'
            | b'~'
    );
    let digit = after.is_ascii_digit();
    let currency = matches!(after, b'$' | b'\\');
    let opening = matches!(after, b'(' | b'<' | b'[' | b'{');
    match before {
        b'/' | b'0'..=b'9' | b'<' | b'@' | b'A'..=b'Z' | b'^'..=b'`' | b'a'..=b'z' => {
            letter || currency || digit || opening
        }
        b'!' | b'%' => letter || currency || digit || after == b'%',
        b'#' | b'&' | b')' | b'*' | b'+' | b'=' | b'>' | b']' | b'|' | b'}' | b'~' => {
            letter || currency || digit
        }
        b',' | b'.' | b':' | b';' => letter || digit,
        b'$' | b'\\' => after == b'%',
        b'-' => digit,
        _ => false,
    }
}

/// Returns whether `ch` is a space Blink breaks after a run of and never
/// before: U+0020, a tab or a line feed.
fn is_breakable_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n')
}

/// Returns Blink's pair table answer between two printable ASCII
/// characters, U+0021 to U+007F, as `FillAscii`'s calls combine.
///
/// Nothing breaks before `!`, `)`, `,`, `.`, `/`, `:`, `;`, `?`, `]` or `}`.
/// Otherwise a line breaks: after `-`, before anything but `$` (a digit is
/// the context rule's); after `?`, before anything but a quotation mark;
/// after a letter, a digit, `$`, `'`, `(`, `/`, `<`, `@`, `[`, `^`, `_`,
/// `` ` ``, `{` or DEL, never; after the rest, before `(`, `<`, `[` and `{`
/// only.
fn pair_breaks(before: u8, after: u8) -> bool {
    if matches!(
        after,
        b'!' | b')' | b',' | b'.' | b'/' | b':' | b';' | b'?' | b']' | b'}'
    ) {
        return false;
    }
    match before {
        b'-' => !matches!(after, b'$' | b'0'..=b'9'),
        b'?' => !matches!(after, b'"' | b'\''),
        b'$'
        | b'\''
        | b'('
        | b'/'
        | b'0'..=b'9'
        | b'<'
        | b'@'
        | b'A'..=b'Z'
        | b'['
        | b'^'..=b'`'
        | b'a'..=b'z'
        | b'{'
        | 0x7F => false,
        _ => matches!(after, b'(' | b'<' | b'[' | b'{'),
    }
}
