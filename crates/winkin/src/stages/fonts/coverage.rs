//! Whether a font covers a cluster, and which presentation a cluster asks for.
//!
//! **Coverage is all or nothing per cluster.** A font covers a cluster where
//! harfrust, shaping the cluster in it, draws no `.notdef`. Three tests ask
//! this against fontwich's [`Charset`], each only where the one before cannot
//! say:
//! - **As written:** every character is mapped, or is a default-ignorable
//!   that harfrust hides ([`is_default_ignorable`]). Each
//!   candidate keeps the last [`CharsetPage`] it found, so most characters
//!   cost one bit test.
//! - **A lone character with no decomposition:** harfrust draws it only by
//!   faking it ([`faked_from`]). This catches every ideograph a Latin font
//!   sends to fallback in one lookup.
//! - **harfrust's normalizer** (`_hb_ot_shape_normalize`), replayed against
//!   the charset in its three rounds: decompose, reorder marks, recompose.
//!   A cluster the font draws in a mix of forms is covered. For example, `ǘ`
//!   in a font with `ü` and U+0301 but neither `ǘ` nor U+0308 draws as `ü`
//!   and U+0301. Hangul follows harfrust's Hangul shaper instead.
//!
//! The replay skips the Indic, Khmer, Myanmar, USE and Hebrew shapers' own
//! compositions and the Thai shaper's sara am. It allocates nothing: a
//! cluster of more than [`MAX_NORMALIZED_CHARS`] characters is tested as
//! written only.
//!
//! **A variation sequence** is covered as its base, since harfrust draws the
//! base where the font lacks the sequence. Cluster matching asks
//! [`has_sequences`] in Blink's two passes: first the first font with every
//! sequence of the cluster, then the first font that covers it.

use fontwich::{Charset, CharsetPage, Font, Presentation};

use crate::stages::analysis::ClusterClass;
use crate::style::FontVariantEmoji;
use crate::unicode;
use crate::unicode::{
    MAX_DECOMPOSITION, compose, decompose, is_default_ignorable, is_mark, is_variation_selector,
    modified_combining_class,
};

/// The longest cluster harfrust's normalizer is replayed on, in characters.
/// Past it a cluster -- a pile of combining marks -- is tested as written
/// only, which keeps the replay's buffers inline and its work bounded.
const MAX_NORMALIZED_CHARS: usize = 16;

/// The most jamo harfrust's Hangul shaper writes a character as: a
/// syllable's leading consonant, vowel and trailing consonant.
const MAX_JAMO: usize = 3;

/// The longest run of marks harfrust reorders, `MAX_COMBINING_MARKS`
/// (HarfBuzz's `HB_OT_SHAPE_MAX_COMBINING_MARKS`): a longer run is left in
/// the order it was written.
const MAX_COMBINING_MARKS: usize = 32;

/// Whether a cluster of `class` puts nothing on the page of its own.
///
/// These are the classes shaping skips: a tab, whose advance is the line's,
/// a separator, an atomic inline's U+FFFC, and a U+200B the builder made.
/// They also include the default-ignorables shaped with the text around them
/// but hidden by the shaper: a soft hyphen, a U+200B the caller wrote, a lone
/// joiner or a bidi control.
///
/// Such a cluster takes the font of the text around it rather than asking
/// the list. So a joiner in a fallback run does not end it, and a kern across
/// a word joiner is shaped in one font. A `<br>` does not send the walk to
/// fallback for a character no font maps.
pub(super) fn draws_nothing(class: ClusterClass) -> bool {
    matches!(
        class,
        ClusterClass::Tab
            | ClusterClass::Separator
            | ClusterClass::Object
            | ClusterClass::ZeroWidthSpace
            | ClusterClass::BreakOpportunity
            | ClusterClass::SoftHyphen
            | ClusterClass::Control
    )
}

/// Returns the presentation a cluster asks for.
///
/// Three kinds of cluster ask, as Chrome asks them; everything else is text:
/// - an emoji cluster: an Emoji_Presentation character, a flag, or a
///   modifier, tag or ZWJ sequence;
/// - a pictograph in text presentation;
/// - a keycap base (`#`, `*` or a digit). It is an emoji character and one of
///   CSS's Emoji Presentation Participating Code Points, though no
///   pictograph.
///
/// VS16 asks for emoji only, and VS15 for text only. Without one, a keycap
/// asks for emoji only, an emoji cluster for emoji first, and anything else
/// for text first.
///
/// `font-variant-emoji` forces a form on a cluster whose first character is
/// an emoji (Emoji=Yes), a keycap's base among them, unless a selector in the
/// text overrides it. Chrome 155 sets a keycap as text under `text` and
/// `unicode`, and a keycap after VS15 as text under `emoji`. Chrome's
/// glyph callback (`HarfBuzzGetGlyph`) does the same by supplying VS15 or
/// VS16 to every such character. `text` forces the text form, `emoji` the
/// emoji form, and `unicode` the form Emoji_Presentation says. Like Chrome,
/// this does not limit it to the spec's 371 participating bases. A
/// pictograph that is no emoji is only preferred in the form asked for, as
/// Chrome's fallback prefers it (`ApplyFontVariantEmojiOnFallbackPriority`).
pub(super) fn presentation(
    class: ClusterClass,
    text: &str,
    variant: FontVariantEmoji,
) -> Presentation {
    let first = text.chars().next();
    let keycap = class == ClusterClass::Text && first.is_some_and(is_keycap_base);
    if !matches!(class, ClusterClass::Emoji | ClusterClass::Symbol) && !keycap {
        return Presentation::Text;
    }
    let (mut text_selector, mut emoji_selector, mut keycap_mark) = (false, false, false);
    for ch in text.chars() {
        match ch {
            '\u{FE0E}' => text_selector = true,
            '\u{FE0F}' => emoji_selector = true,
            '\u{20E3}' => keycap_mark = true,
            _ => {}
        }
    }
    if emoji_selector {
        return Presentation::EmojiOnly;
    }
    if text_selector {
        return Presentation::TextOnly;
    }
    let props = first.map(unicode::core_props);
    let emoji = props.is_some_and(|props| props.is_emoji());
    match variant {
        FontVariantEmoji::Normal if keycap_mark => Presentation::EmojiOnly,
        FontVariantEmoji::Text if emoji => Presentation::TextOnly,
        FontVariantEmoji::Emoji if emoji => Presentation::EmojiOnly,
        FontVariantEmoji::Unicode if emoji => {
            if props.is_some_and(|props| props.is_emoji_presentation()) {
                Presentation::EmojiOnly
            } else {
                Presentation::TextOnly
            }
        }
        FontVariantEmoji::Text => Presentation::Text,
        FontVariantEmoji::Emoji => Presentation::Emoji,
        FontVariantEmoji::Normal | FontVariantEmoji::Unicode => {
            if class == ClusterClass::Emoji {
                Presentation::Emoji
            } else {
                Presentation::Text
            }
        }
    }
}

/// Whether `ch` is a keycap base: `#`, `*` or an ASCII digit.
///
/// A keycap base is an emoji character that no pictograph class takes in.
pub(super) fn is_keycap_base(ch: char) -> bool {
    matches!(ch, '#' | '*' | '0'..='9')
}

/// Returns the variation sequence that accepts a font for presentation `p`
/// of `text`, whatever the font's colour.
///
/// Blink's glyph callback asks for it in the font's `cmap` before it tests
/// the font's colour tables. It is VS16 after its base for emoji alone, and
/// VS15 for text alone. The base is the character before the written
/// selector, or the first character where `font-variant-emoji` or a keycap
/// forced the form. Returns `None` for a presentation that accepts every
/// font.
pub(super) fn presentation_sequence(text: &str, p: Presentation) -> Option<(char, char)> {
    let selector = match p {
        Presentation::EmojiOnly => '\u{FE0F}',
        Presentation::TextOnly => '\u{FE0E}',
        Presentation::Text | Presentation::Emoji => return None,
    };
    let mut before = None;
    for ch in text.chars() {
        if ch == selector {
            return before.map(|base| (base, selector));
        }
        before = Some(ch);
    }
    text.chars().next().map(|first| (first, selector))
}

/// Whether `ch` is a variation selector that cluster matching asks a font
/// for with its base.
///
/// These are VS1 to VS256 except VS15 and VS16, which ask for a
/// presentation instead. Every such selector counts, where Blink counts only
/// the sequences Unicode defines.
fn is_sequence_selector(ch: char) -> bool {
    is_variation_selector(ch) && !matches!(ch, '\u{FE0E}' | '\u{FE0F}')
}

/// Returns the variation sequences of the cluster `text` that cluster
/// matching asks a font for.
///
/// Each is a character other than a selector, with the selector after it,
/// where [`is_sequence_selector`] says. harfrust looks them up the same way
/// (`handle_variation_selector_cluster`): it passes over a selector after a
/// selector. Where the font lacks a sequence, harfrust draws the base and
/// hides the selector.
fn sequences(text: &str) -> impl Iterator<Item = (char, char)> + '_ {
    text.chars()
        .zip(text.chars().skip(1))
        .filter(|&(base, selector)| is_sequence_selector(selector) && !is_variation_selector(base))
}

/// Whether the cluster `text` holds a variation sequence that cluster
/// matching asks a font for.
///
/// Where it does, the first pass wants a font with every one of them.
pub(super) fn asks_sequences(text: &str) -> bool {
    sequences(text).next().is_some()
}

/// Whether `font` has every variation sequence of `text` that cluster
/// matching asks for.
///
/// fontwich reads the font's `cmap` format 14. A Default UVS entry counts
/// where the font maps the base, as harfrust's `Charmap::map_variant` has
/// it. The first pass asks this of a font that covers the cluster.
pub(super) fn has_sequences(font: &Font, text: &str) -> bool {
    sequences(text).all(|(base, selector)| font.maps_variation_sequence(base, selector))
}

/// Whether `charset` maps `ch`, with `page` the page last found for it: one
/// bit test while characters stay on that page.
pub(super) fn maps<'a>(charset: &'a Charset, ch: char, page: &mut Option<CharsetPage<'a>>) -> bool {
    match *page {
        Some(held) if held.holds(ch) => held.contains(ch),
        _ => {
            *page = charset.page(ch);
            page.is_some_and(|found| found.contains(ch))
        }
    }
}

/// The character whose glyph harfrust draws `ch` with in a font that does
/// not map it, where it fakes one: U+0020's for the space separators it
/// fakes, U+2010's for U+2011.
pub(super) fn faked_from(ch: char) -> Option<char> {
    match ch {
        '\u{A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => Some(' '),
        '\u{2011}' => Some('\u{2010}'),
        _ => None,
    }
}

/// The characters [`faked_from`] fakes, each once.
pub(super) const FAKED: [char; 16] = [
    '\u{A0}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}', '\u{2006}',
    '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{202F}', '\u{205F}', '\u{3000}', '\u{2011}',
];

/// Whether harfrust draws `ch` with a font that does not map it, as written:
/// fakes it, or hides it.
fn allowed<'a>(charset: &'a Charset, ch: char, page: &mut Option<CharsetPage<'a>>) -> bool {
    match faked_from(ch) {
        Some(from) => maps(charset, from, page),
        None => is_default_ignorable(ch),
    }
}

/// Whether `charset` draws `text` as written, with `page` its last page.
///
/// Every character must be mapped or hidden by harfrust. [`covers`] asks
/// this first. The run loop asks only this of text in the font it last set
/// text in. A character harfrust fakes is left to [`covers`], which knows
/// where harfrust may fake it.
pub(super) fn covers_as_written<'a>(
    charset: &'a Charset,
    text: &str,
    page: &mut Option<CharsetPage<'a>>,
) -> bool {
    text.chars()
        .all(|ch| maps(charset, ch, page) || is_default_ignorable(ch))
}

/// Whether harfrust draws `text`, a cluster, in `font` with no `.notdef`,
/// with `page` its last page (see the module documentation).
pub(super) fn covers<'a>(font: &'a Font, text: &str, page: &mut Option<CharsetPage<'a>>) -> bool {
    let charset = font.charset();
    if covers_as_written(charset, text, page) {
        return true;
    }
    let mut chars = text.chars();
    if let (Some(ch), None) = (chars.next(), chars.next())
        && decompose(ch).is_none()
    {
        return faked_from(ch).is_some_and(|from| maps(charset, from, page));
    }
    let mut input = Inline::<char, MAX_NORMALIZED_CHARS>::new('\0');
    for ch in text.chars() {
        input.push(ch);
    }
    !input.overflowed && Normalizer::new(charset, page).draws(input.as_slice())
}

/// How much of a cluster a font draws, for when no font draws it all.
///
/// It records whether the font draws the base (the first visible character)
/// and how many visible characters it draws. Default-ignorables count in
/// neither, since they are invisible whether or not a font has them.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct Score {
    base: bool,
    drawn: u32,
}

impl Score {
    /// Whether this is the better of two scores for one cluster.
    ///
    /// The base counts first, since losing the base loses the word and
    /// losing a mark loses a nuance. The count decides the rest.
    pub(super) fn beats(self, other: Self) -> bool {
        if self.base != other.base {
            return self.base;
        }
        self.drawn > other.drawn
    }
}

/// How much of `text` `font` draws as written, or `None` where it draws
/// nothing visible.
pub(super) fn score(font: &Font, text: &str) -> Option<Score> {
    let charset = font.charset();
    let mut page = None;
    let mut score = Score {
        base: false,
        drawn: 0,
    };
    let mut seen_base = false;
    for ch in text.chars() {
        if is_default_ignorable(ch) {
            continue;
        }
        let drawn = maps(charset, ch, &mut page) || allowed(charset, ch, &mut page);
        score.drawn = score.drawn.saturating_add(u32::from(drawn));
        if !seen_base {
            seen_base = true;
            score.base = drawn;
        }
    }
    (score.drawn > 0).then_some(score)
}

/// A replay of harfrust's normalizer on one cluster against one charset.
///
/// It builds what harfrust would put in its buffer, and whether the font
/// draws each character.
struct Normalizer<'a, 'p> {
    charset: &'a Charset,
    page: &'p mut Option<CharsetPage<'a>>,
    /// The cluster as harfrust's rounds leave it.
    out: Inline<NormalizedChar, { MAX_NORMALIZED_CHARS * MAX_DECOMPOSITION }>,
}

/// One character of a cluster being normalized.
#[derive(Copy, Clone, Debug)]
struct NormalizedChar {
    ch: char,
    /// Whether the font draws it: maps it, or fakes it.
    drawn: bool,
    /// Whether it is a mark.
    mark: bool,
    /// The class harfrust reorders it by and tests blocking with.
    ///
    /// It is the modified combining class for a mark and zero otherwise, as
    /// harfrust's glyph info keeps it.
    class: u8,
}

impl NormalizedChar {
    /// Makes the entry for `ch`, drawn or not.
    fn new(ch: char, drawn: bool) -> Self {
        let mark = is_mark(ch);
        Self {
            ch,
            drawn,
            mark,
            class: if mark {
                modified_combining_class(ch)
            } else {
                0
            },
        }
    }
}

impl<'a, 'p> Normalizer<'a, 'p> {
    /// Starts an empty replay against `charset`, with `page` its last page.
    fn new(charset: &'a Charset, page: &'p mut Option<CharsetPage<'a>>) -> Self {
        Self {
            charset,
            page,
            out: Inline::new(NormalizedChar::new('\0', false)),
        }
    }

    /// Whether the font maps `ch`: harfrust's nominal glyph lookup.
    fn maps(&mut self, ch: char) -> bool {
        maps(self.charset, ch, self.page)
    }

    /// Whether harfrust draws the cluster `input` with no `.notdef`.
    ///
    /// Runs the three rounds, or the Hangul shaper's, then checks that every
    /// character is drawn or hidden.
    fn draws(mut self, input: &[char]) -> bool {
        if input.first().is_some_and(|&ch| is_hangul(ch)) {
            // The Hangul shaper's normalizer takes every character as it
            // stands where the font maps it, and composes nothing.
            let mut jamo = Inline::<char, { MAX_NORMALIZED_CHARS * MAX_JAMO }>::new('\0');
            self.compose_hangul(input, &mut jamo);
            self.decompose_all(jamo.as_slice(), true);
        } else if self.decompose_all(input, false) {
            self.reorder();
            self.recompose();
        }
        !self.out.overflowed
            && self
                .out
                .as_slice()
                .iter()
                .all(|entry| entry.drawn || is_default_ignorable(entry.ch))
    }

    /// Runs the first round: decomposes each character of `input` into `out`
    /// as harfrust's normalizer does.
    ///
    /// A character no mark follows stands alone, taken as written where the
    /// font maps it. A character and the marks after it decompose together,
    /// as far as the font allows unless `marks_shortest`. They stay as
    /// written where they hold a variation selector. Returns whether any
    /// character took marks, since only then do the other two rounds run.
    fn decompose_all(&mut self, input: &[char], marks_shortest: bool) -> bool {
        let mut marked = false;
        let mut at = 0;
        while at < input.len() {
            // The characters before the last one before a mark stand alone.
            let mut end = at + 1;
            while input.get(end).is_some_and(|&ch| !is_mark(ch)) {
                end += 1;
            }
            if end < input.len() {
                end -= 1;
            }
            for &ch in input.get(at..end).unwrap_or_default() {
                self.decompose_current(ch, true);
            }
            at = end;
            if at >= input.len() {
                break;
            }
            // One character and its marks.
            marked = true;
            end = at + 1;
            while input.get(end).is_some_and(|&ch| is_mark(ch)) {
                end += 1;
            }
            let cluster = input.get(at..end).unwrap_or_default();
            if cluster.iter().any(|&ch| is_variation_selector(ch)) {
                for &ch in cluster {
                    let drawn = self.maps(ch);
                    self.out.push(NormalizedChar::new(ch, drawn));
                }
            } else {
                for &ch in cluster {
                    self.decompose_current(ch, marks_shortest);
                }
            }
            at = end;
        }
        marked
    }

    /// Writes `ch` as harfrust's `decompose_current_character` does.
    ///
    /// In order of preference, `ch` is:
    /// - as written, where the font maps it and `shortest`;
    /// - decomposed as far as the font needs;
    /// - as written, where the font maps it or harfrust fakes it;
    /// - a `.notdef` the third round may yet compose away.
    fn decompose_current(&mut self, ch: char, shortest: bool) {
        let mapped = self.maps(ch);
        if mapped && shortest {
            self.out.push(NormalizedChar::new(ch, true));
            return;
        }
        if self.decompose(ch, shortest, 0) > 0 {
            return;
        }
        let drawn = mapped || faked_from(ch).is_some_and(|from| self.maps(from));
        self.out.push(NormalizedChar::new(ch, drawn));
    }

    /// Decomposes `ab` into characters the font maps, as harfrust's
    /// `decompose` does.
    ///
    /// It decomposes one step, then the first half further. Returns how many
    /// characters it wrote, zero where it cannot. `depth` counts the steps
    /// so far, which no decomposition exceeds.
    fn decompose(&mut self, ab: char, shortest: bool, depth: usize) -> usize {
        if depth >= MAX_DECOMPOSITION {
            return 0;
        }
        let Some((a, b)) = decompose(ab) else {
            return 0;
        };
        // No point going on where the font cannot take the second half.
        if let Some(b) = b
            && !self.maps(b)
        {
            return 0;
        }
        let a_mapped = self.maps(a);
        if !(a_mapped && shortest) {
            let written = self.decompose(a, shortest, depth + 1);
            if written > 0 {
                return written + self.push_second(b);
            }
            if !a_mapped {
                return 0;
            }
        }
        self.out.push(NormalizedChar::new(a, true));
        1 + self.push_second(b)
    }

    /// Writes the second half of a decomposition, where there is one; how
    /// many characters that wrote.
    fn push_second(&mut self, b: Option<char>) -> usize {
        match b {
            Some(b) => {
                self.out.push(NormalizedChar::new(b, true));
                1
            }
            None => 0,
        }
    }

    /// Runs the second round: sorts each run of marks stably by class.
    ///
    /// It sorts as harfrust's insertion sort does, and only runs no longer
    /// than [`MAX_COMBINING_MARKS`].
    fn reorder(&mut self) {
        let chars = self.out.as_mut_slice();
        let mut at = 0;
        while at < chars.len() {
            if chars.get(at).is_none_or(|entry| entry.class == 0) {
                at += 1;
                continue;
            }
            let mut end = at + 1;
            while chars.get(end).is_some_and(|entry| entry.class != 0) {
                end += 1;
            }
            if end - at <= MAX_COMBINING_MARKS
                && let Some(run) = chars.get_mut(at..end)
            {
                for next in 1..run.len() {
                    let class = run.get(next).map_or(0, |entry| entry.class);
                    let mut to = next;
                    while to > 0 && run.get(to - 1).is_some_and(|entry| entry.class > class) {
                        to -= 1;
                    }
                    if let Some(moved) = run.get_mut(to..=next) {
                        moved.rotate_right(1);
                    }
                }
            }
            // The character after a run has a class of zero.
            at = end + 1;
        }
    }

    /// Runs the third round: composes each mark onto the starter before it.
    ///
    /// A mark composes where the characters between have lower classes and
    /// the font maps the composite, as harfrust composes in its
    /// composed-diacritics mode. Anything left with a class of zero becomes
    /// the next starter.
    fn recompose(&mut self) {
        let len = self.out.as_slice().len();
        let mut starter = 0;
        let mut kept = 1;
        for at in 1..len {
            let Some(current) = self.out.get(at) else {
                break;
            };
            let Some(before) = self.out.get(kept - 1) else {
                break;
            };
            if current.mark
                && (starter == kept - 1 || before.class < current.class)
                && let Some(first) = self.out.get(starter)
                && let Some(composed) = compose(first.ch, current.ch)
                && self.maps(composed)
            {
                self.out.set(starter, NormalizedChar::new(composed, true));
                continue;
            }
            self.out.set(kept, current);
            kept += 1;
            if current.class == 0 {
                starter = kept - 1;
            }
        }
        self.out.truncate(kept);
    }

    /// Composes `input` into `out` as harfrust's Hangul shaper does before
    /// its normalizer runs (`preprocess_text_hangul`).
    ///
    /// - Jamo compose into a syllable where the font maps the syllable.
    /// - A precomposed syllable composes with a trailing consonant after it
    ///   where the font maps the result.
    /// - Otherwise a precomposed syllable becomes its jamo, where the font
    ///   maps them and either lacks the syllable or a trailing consonant it
    ///   cannot take follows.
    /// - Everything else stays as written.
    ///
    /// The shaper moves tone marks before their syllable; this does not,
    /// since their place does not matter to coverage.
    fn compose_hangul<const N: usize>(&mut self, input: &[char], out: &mut Inline<char, N>) {
        let mut at = 0;
        while let Some(&ch) = input.get(at) {
            let next = input.get(at + 1).copied();
            let u = u32::from(ch);
            if is_leading(ch)
                && let Some(vowel) = next
                && is_vowel(vowel)
            {
                // A leading consonant and a vowel, and a trailing consonant
                // where one follows.
                let trailing = input.get(at + 2).copied().filter(|&t| is_trailing(t));
                let v = u32::from(vowel);
                let t = trailing.map_or(T_BASE, u32::from);
                let length = 2 + usize::from(trailing.is_some());
                if (L_BASE..L_BASE + L_COUNT).contains(&u)
                    && (V_BASE..V_BASE + V_COUNT).contains(&v)
                    && (trailing.is_none() || is_combining_trailing(t))
                    && let Some(syllable) = char::from_u32(
                        S_BASE + (u - L_BASE) * N_COUNT + (v - V_BASE) * T_COUNT + (t - T_BASE),
                    )
                    && self.maps(syllable)
                {
                    out.push(syllable);
                } else {
                    for &jamo in input.get(at..at + length).unwrap_or_default() {
                        out.push(jamo);
                    }
                }
                at += length;
                continue;
            }
            if (S_BASE..S_BASE + S_COUNT).contains(&u) {
                let mapped = self.maps(ch);
                let index = u - S_BASE;
                let t_index = index % T_COUNT;
                let before_trailing = t_index == 0 && next.is_some_and(is_trailing);
                if t_index == 0
                    && let Some(t) = next.map(u32::from)
                    && is_combining_trailing(t)
                    && let Some(syllable) = char::from_u32(u + t - T_BASE)
                    && self.maps(syllable)
                {
                    out.push(syllable);
                    at += 2;
                    continue;
                }
                if !mapped || before_trailing {
                    let leading = char::from_u32(L_BASE + index / N_COUNT);
                    let vowel = char::from_u32(V_BASE + index % N_COUNT / T_COUNT);
                    let trailing = char::from_u32(T_BASE + t_index).filter(|_| t_index != 0);
                    if let (Some(l), Some(v)) = (leading, vowel)
                        && self.maps(l)
                        && self.maps(v)
                        && trailing.is_none_or(|t| self.maps(t))
                    {
                        out.push(l);
                        out.push(v);
                        if let Some(t) = trailing {
                            out.push(t);
                        }
                        at += 1;
                        // A syllable written as jamo before a trailing
                        // consonant takes it into the syllable.
                        if mapped
                            && t_index == 0
                            && let Some(t) = next
                        {
                            out.push(t);
                            at += 1;
                        }
                        continue;
                    }
                }
            }
            out.push(ch);
            at += 1;
        }
    }
}

// Hangul, as harfrust's Hangul shaper counts it: the conjoining jamo and
// the precomposed syllables they compose to.
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const S_BASE: u32 = 0xAC00;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT;
const S_COUNT: u32 = L_COUNT * N_COUNT;

/// Whether harfrust shapes a cluster that starts with `ch` with its Hangul
/// shaper.
///
/// That holds for a conjoining jamo or a precomposed syllable. A
/// compatibility or halfwidth jamo composes with nothing, so the default
/// shaper's normalizer draws it the same way.
fn is_hangul(ch: char) -> bool {
    is_leading(ch)
        || is_vowel(ch)
        || is_trailing(ch)
        || (S_BASE..S_BASE + S_COUNT).contains(&u32::from(ch))
}

/// A leading consonant jamo, a syllable's first.
fn is_leading(ch: char) -> bool {
    matches!(ch, '\u{1100}'..='\u{115F}' | '\u{A960}'..='\u{A97C}')
}

/// A vowel jamo, a syllable's second.
fn is_vowel(ch: char) -> bool {
    matches!(ch, '\u{1160}'..='\u{11A7}' | '\u{D7B0}'..='\u{D7C6}')
}

/// A trailing consonant jamo, a syllable's last.
fn is_trailing(ch: char) -> bool {
    matches!(ch, '\u{11A8}'..='\u{11FF}' | '\u{D7CB}'..='\u{D7FB}')
}

/// Whether `t` is a trailing consonant a precomposed syllable takes.
fn is_combining_trailing(t: u32) -> bool {
    (T_BASE + 1..T_BASE + T_COUNT).contains(&t)
}

/// An inline buffer of up to `N` items of `T`, which records overflow.
///
/// No cluster within [`MAX_NORMALIZED_CHARS`] overflows it.
struct Inline<T, const N: usize> {
    items: [T; N],
    len: usize,
    overflowed: bool,
}

impl<T: Copy, const N: usize> Inline<T, N> {
    /// Makes an empty buffer, its room filled with `empty`.
    fn new(empty: T) -> Self {
        Self {
            items: [empty; N],
            len: 0,
            overflowed: false,
        }
    }

    /// Adds `item`, or marks the buffer overflowed where it is full.
    fn push(&mut self, item: T) {
        match self.items.get_mut(self.len) {
            Some(slot) => {
                *slot = item;
                self.len += 1;
            }
            None => self.overflowed = true,
        }
    }

    /// Returns the item at `at`.
    fn get(&self, at: usize) -> Option<T> {
        self.as_slice().get(at).copied()
    }

    /// Puts `item` at `at`, where there is one.
    fn set(&mut self, at: usize, item: T) {
        if let Some(slot) = self.as_mut_slice().get_mut(at) {
            *slot = item;
        }
    }

    /// Keeps the first `len`.
    fn truncate(&mut self, len: usize) {
        self.len = self.len.min(len);
    }

    fn as_slice(&self) -> &[T] {
        self.items.get(..self.len).unwrap_or_default()
    }

    fn as_mut_slice(&mut self) -> &mut [T] {
        self.items.get_mut(..self.len).unwrap_or_default()
    }
}
