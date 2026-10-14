//! What font coverage needs of canonical normalization: the steps harfrust's
//! normalizer takes, one character or pair at a time, and the properties it
//! reads.
//!
//! [`decompose`], [`compose`] and the canonical combining class are
//! HarfBuzz's `hb_unicode_decompose`, `hb_unicode_compose` and
//! `hb_unicode_combining_class`. A normalizer to NFD or NFC is a loop over
//! these pairwise steps. They call harfrust's own (`harfrust::unicode`), so
//! coverage answers from the shaper's data.
//!
//! harfrust's tables are Unicode 18.0 and ICU 2.3's are 17.0. They agree on
//! decompositions and compositions; Unicode 18 adds 37 marks, 34 with a
//! combining class, which ICU leaves unassigned. The tests check the steps
//! against ICU's NFD and NFC over every character, every composing pair, a
//! random corpus and Unicode's `NormalizationTest.txt`, and the classes
//! against ICU's except for those 34.
//!
//! The rest are HarfBuzz's own rules: the modified combining classes, its
//! default-ignorables and its variation selectors. harfrust 0.14.0 keeps
//! these private, so this module copies them; coverage has to agree with
//! the shaper. Whether a character is a mark comes from this crate's table
//! ([`CoreProps::is_mark`](super::CoreProps::is_mark)).
//!
//! Nothing here allocates or panics, whatever the character.

use harfrust::unicode::{self as shaper, Decomposed};

use super::{core_props, rare_props};

/// A bound on the characters a character fully decomposes to, and on the
/// [`decompose`] steps that takes through `a`: U+1F82 is U+03B1, U+0313, U+0300 and
/// U+0345, in three steps. The tests check both bounds for every character.
pub(crate) const MAX_DECOMPOSITION: usize = 4;

/// Returns one step of canonical decomposition, Hangul included: `ch` as
/// `a` and an optional `b`, or `None` if `ch` is its own decomposition.
///
/// As `hb_unicode_decompose`: a singleton (U+212B to U+00C5) is
/// `(a, None)`, and a character that decomposes further does so through
/// `a` (U+1EC7 to U+1EB9 and U+0302, then U+1EB9 to `e` and U+0323). A
/// Hangul syllable takes one step too: LVT to LV and T, LV to L and V.
#[inline]
pub(crate) fn decompose(ch: char) -> Option<(char, Option<char>)> {
    // harfrust returns code points, all valid characters; an invalid one
    // would count as no decomposition.
    match shaper::decompose(u32::from(ch))? {
        Decomposed::Singleton(a) => Some((char::from_u32(a)?, None)),
        Decomposed::Pair(a, b) => Some((char::from_u32(a)?, Some(char::from_u32(b)?))),
    }
}

/// Returns the canonical composition of a pair, Hangul included, or `None`,
/// excluding composition exclusions as NFC does.
///
/// As `hb_unicode_compose`: the primary composites, and Hangul's LV from L
/// and V and LVT from LV and T. harfrust 0.14.0 also composes an LV and
/// U+11A7, which is not a trailing consonant, to the LV itself, where
/// HarfBuzz and ICU compose nothing (its bound is `T_BASE <= b`, HarfBuzz's
/// `b > TBASE`). No replay hits this: only marks compose onto a starter, and
/// U+11A7 is a letter.
#[inline]
pub(crate) fn compose(a: char, b: char) -> Option<char> {
    char::from_u32(shaper::compose(u32::from(a), u32::from(b))?)
}

/// Returns the Canonical_Combining_Class, 0 for a starter, as
/// `hb_unicode_combining_class`.
#[inline]
pub(super) fn combining_class(ch: char) -> u8 {
    shaper::combining_class(u32::from(ch))
}

/// Returns whether `ch` is a mark (General_Category Mn, Mc or Me), which
/// harfrust's normalizer clusters with the character before it, reorders
/// and composes onto a starter.
#[inline]
pub(crate) fn is_mark(ch: char) -> bool {
    core_props(ch).is_mark()
}

/// Returns the class harfrust reorders a mark by and tests blocking by: the
/// canonical combining class as HarfBuzz modifies it
/// (`hb_unicode_funcs_t::modified_combining_class`).
///
/// The changes: Hebrew's fixed-position classes in the SBL Hebrew manual's
/// order, Arabic's shadda before the other marks, Telugu's two length marks
/// and Thai's sara u and uu moved off the virama, Tibetan's sign u before
/// sign i, and three single marks. Every other class is unchanged.
pub(crate) fn modified_combining_class(ch: char) -> u8 {
    match ch {
        // Tai Tham's sakot after any tone marks, Tibetan's padma after any
        // vowel signs, and its tsa-phru before its sign u.
        '\u{1A60}' | '\u{0FC6}' => return 254,
        '\u{0F39}' => return 127,
        _ => {}
    }
    match combining_class(ch) {
        // Hebrew: sheva, hataf segol, hataf patah, hataf qamats, hiriq,
        // tsere, segol, patah, qamats, holam, qubuts, dagesh, meteg, rafe,
        // shin dot and sin dot. Point varika, 26, keeps its class.
        10 => 22,
        11 => 15,
        12 => 16,
        13 => 17,
        14 => 23,
        15 => 18,
        16 => 19,
        17 => 20,
        18 => 21,
        19 => 14,
        20 => 24,
        21 => 12,
        22 => 25,
        23 => 13,
        24 => 10,
        25 => 11,
        // Arabic: fathatan, dammatan, kasratan, fatha, damma and kasra one
        // later, and shadda first. Sukun and superscript alef keep theirs.
        27 => 28,
        28 => 29,
        29 => 30,
        30 => 31,
        31 => 32,
        32 => 33,
        33 => 27,
        // Telugu's length mark and ai length mark, the only matras of the
        // main Indic block with a class, which would otherwise reorder with
        // the virama.
        84 | 91 => 0,
        // Thai's sara u and sara uu, before phinthu (9), as Uniscribe
        // orders them.
        103 => 3,
        // Tibetan's sign i after its sign u.
        130 => 132,
        132 => 131,
        class => class,
    }
}

/// Returns whether harfrust hides `ch` where a font lacks it
/// (`hb_unicode_funcs_t::is_default_ignorable`).
///
/// That is Default_Ignorable_Code_Point less what HarfBuzz leaves visible:
/// the Hangul fillers U+115F, U+1160, U+3164 and U+FFA0, which fonts draw
/// as spacing glyphs as Uniscribe does, and the shorthand format controls
/// U+1BCA0 to U+1BCA3 (HarfBuzz issue 503). The tests check that these are
/// the whole difference. U+180F, the fourth Mongolian free variation
/// selector, is hidden, as harfrust 0.14.0 hides it.
pub(crate) fn is_default_ignorable(ch: char) -> bool {
    rare_props(ch).is_default_ignorable()
        && !matches!(
            ch,
            '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' | '\u{1BCA0}'..='\u{1BCA3}'
        )
}

/// Returns whether `ch` is a variation selector as harfrust's normalizer
/// reads one: VS1 to VS256. The Mongolian free variation selectors are left
/// to its Arabic shaper.
pub(crate) fn is_variation_selector(ch: char) -> bool {
    matches!(ch, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}
