//! What font coverage asks of canonical normalization: the steps harfrust's
//! normalizer takes, a character or a pair at a time, and the properties it
//! takes them by.
//!
//! The first three are HarfBuzz's unicode functions in their own shape:
//! `hb_unicode_decompose`, `hb_unicode_compose` and
//! `hb_unicode_combining_class`. They are pairwise steps, and a normalizer
//! to NFD or NFC is a loop over them. harfrust exposes its own since 0.14.0
//! (`harfrust::unicode`), over the tables its normalizer reads, so coverage
//! answers from the shaper's data by construction.
//!
//! Those tables are Unicode 18.0, where ICU 2.3's are 17.0. They have the
//! same decompositions and compositions, plus 37 marks Unicode 18 assigns,
//! 34 with a combining class, which ICU leaves unassigned. The tests check
//! the three against ICU's NFD and NFC over every character, every pair
//! that composes, a random corpus and Unicode's `NormalizationTest.txt`.
//! They check the classes against ICU's except for those 34.
//!
//! The rest are HarfBuzz's own rules, not Unicode's: the combining classes
//! it reorders marks by, its default-ignorables and its variation
//! selectors. harfrust (0.14.0, the workspace's) keeps these private, so
//! this module copies them, since coverage has to agree with it. Whether a
//! character is a mark, General_Category Mn, Mc or Me, comes from this
//! crate's own table ([`CoreProps::is_mark`](super::CoreProps::is_mark)).
//!
//! Nothing here allocates or panics, whatever the character.

use harfrust::unicode::{self as shaper, Decomposed};

use super::{core_props, rare_props};

/// The most characters one character decomposes to canonically, in full,
/// and the most steps of [`decompose`] that takes through the first half:
/// U+1F82 is U+03B1, U+0313, U+0300 and U+0345, in three steps. The tests
/// check both of every character.
pub(crate) const MAX_DECOMPOSITION: usize = 4;

/// One step of canonical decomposition, Hangul included: `ch` as `a` and an
/// optional `b`, or `None` where `ch` is its own decomposition.
///
/// `hb_unicode_decompose`: a singleton, U+212B to U+00C5, is `(a, None)`,
/// and a character that decomposes further does so through `a`, U+1EC7 to
/// U+1EB9 and U+0302 and U+1EB9 to `e` and U+0323. A precomposed Hangul
/// syllable takes one step too, LVT to LV and T, and LV to L and V.
#[inline]
pub(crate) fn decompose(ch: char) -> Option<(char, Option<char>)> {
    // harfrust answers in code points, every one of them a character; one
    // that were not would be no decomposition.
    match shaper::decompose(u32::from(ch))? {
        Decomposed::Singleton(a) => Some((char::from_u32(a)?, None)),
        Decomposed::Pair(a, b) => Some((char::from_u32(a)?, Some(char::from_u32(b)?))),
    }
}

/// The canonical composition of a pair, Hangul included, if one exists
/// (composition exclusions excluded, as NFC has it).
///
/// `hb_unicode_compose`: the primary composites, whose decomposition is the
/// pair and which are neither excluded from composition nor a singleton's or
/// a non-starter's decomposition, and Hangul's LV from L and V and LVT from
/// LV and T. harfrust 0.14.0 also composes an LV and U+11A7, which is no
/// trailing consonant, to the LV itself, where HarfBuzz and ICU compose
/// nothing (its bound is `T_BASE <= b`, HarfBuzz's `b > TBASE`); no replay
/// asks, since only a mark is composed onto a starter and U+11A7 is a
/// letter.
#[inline]
pub(crate) fn compose(a: char, b: char) -> Option<char> {
    char::from_u32(shaper::compose(u32::from(a), u32::from(b))?)
}

/// The canonical combining class.
///
/// `hb_unicode_combining_class`: Canonical_Combining_Class, 0 for a starter.
#[inline]
pub(super) fn combining_class(ch: char) -> u8 {
    shaper::combining_class(u32::from(ch))
}

/// Whether `ch` is a mark, General_Category Mn, Mc or Me: what harfrust's
/// normalizer clusters with the character before it, reorders, and composes
/// onto a starter.
#[inline]
pub(crate) fn is_mark(ch: char) -> bool {
    core_props(ch).is_mark()
}

/// The class harfrust reorders a mark by and tests it blocked by, its
/// canonical combining class as HarfBuzz modifies it
/// (`hb_unicode_funcs_t::modified_combining_class`): Hebrew's
/// fixed-position classes in the order the SBL Hebrew manual draws them,
/// Arabic's shadda before the other marks, Telugu's two length marks and
/// Thai's sara u and uu moved so that they do not reorder with a virama,
/// Tibetan's sign u before sign i, and three marks placed on their own. A
/// class no rule names is its own.
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

/// Whether harfrust hides `ch` where a font lacks it, as it hides a
/// default-ignorable (`hb_unicode_funcs_t::is_default_ignorable`).
///
/// Default_Ignorable_Code_Point less what HarfBuzz leaves visible: the
/// Hangul fillers U+115F, U+1160, U+3164 and U+FFA0, which fonts draw as
/// spacing glyphs as Uniscribe did, and the shorthand format controls
/// U+1BCA0 to U+1BCA3 (HarfBuzz issue 503). The tests check that these are
/// the whole of the difference. U+180F, the fourth Mongolian free variation
/// selector, is hidden, as harfrust 0.14.0 hides it.
pub(crate) fn is_default_ignorable(ch: char) -> bool {
    rare_props(ch).is_default_ignorable()
        && !matches!(
            ch,
            '\u{115F}' | '\u{1160}' | '\u{3164}' | '\u{FFA0}' | '\u{1BCA0}'..='\u{1BCA3}'
        )
}

/// Whether `ch` is a variation selector as harfrust's normalizer takes one:
/// VS1 to VS256. The Mongolian free variation selectors are its Arabic
/// shaper's.
pub(crate) fn is_variation_selector(ch: char) -> bool {
    matches!(ch, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}')
}
