//! Chrome's Windows catch-all lists, for the Windows backend's Common and Han
//! keys.
//!
//! `FontCache::GetFallbackFamilyNameFromHardcodedChoices`
//! (`font_cache_skia_win.cc`) tries one family from `GetFallbackFamily` and,
//! where it is missing or lacks the character, walks one of two lists for the
//! first family that maps it: [`CJK_FONTS`] for a character whose script is
//! still Han with no Han locale, [`COMMON_FONTS`] for every other. Only then
//! does it ask DirectWrite. These lists, not DirectWrite, answer `①`, `㍿`,
//! `㎡`, `㈱`, `ℝ`, `␀`, `₹` and a lone U+0301 (the probes of 2026-10).
//!
//! For a character with no script font, `GetFallbackFamily`'s one family is
//! [`LAST_RESORT`] (in the BMP; the planes' fonts map nothing of it), so the
//! Common key's answer is [`LAST_RESORT`] and then [`COMMON_FONTS`].
//!
//! Chrome lowercases the names; family names match without case.

/// `kLastResort`: the family `GetFallbackFamily` gives a BMP character no
/// script or block rule places.
pub(crate) static LAST_RESORT: &str = "Lucida Sans Unicode";

/// Blink's `kCommonFonts`, in its order.
///
/// `dejavu sasns` is a misspelling in that list, so it names no family; it is
/// kept so the order stays the list's, and finds nothing.
pub(super) static COMMON_FONTS: [&str; 16] = [
    "Tahoma",
    "Arial Unicode MS",
    "Lucida Sans Unicode",
    "Microsoft Sans Serif",
    "Palatino Linotype",
    // Not Microsoft's, but each covers a great deal once installed.
    "DejaVu Serif",
    "DejaVu Sasns",
    "FreeSerif",
    "FreeSans",
    "Gentium",
    "GentiumAlt",
    "MS PGothic",
    "SimSun",
    "Gulim",
    "PMingLiU",
    "Code2000",
];

/// `kCjkFonts`, in Chrome's order: for a Han character whose script no
/// locale resolved.
pub(super) static CJK_FONTS: [&str; 10] = [
    "Arial Unicode MS",
    "MS PGothic",
    "SimSun",
    "Gulim",
    "PMingLiU",
    // Partial Ext. A, but widely known to Chinese readers.
    "WenQuanYi Zen Hei",
    "AR PL ShanHeiSun Uni",
    "AR PL ZenKai Uni",
    // Complete Ext. A.
    "Han Nom A",
    "Code2000",
];

/// The color emoji font first, for emoji presentation: Chrome's
/// `GetColorEmojiFont`.
pub static EMOJI: &[&str] = &["Segoe UI Emoji", "Segoe UI Symbol"];

/// The monochrome emoji font first, for an emoji in text presentation:
/// Chrome's `GetMonoEmojiFont`.
pub static EMOJI_TEXT: &[&str] = &["Segoe UI Symbol", "Segoe UI Emoji"];

/// The symbol blocks' font, as Chrome's `GetFontBasedOnUnicodeBlock` sends
/// them to it.
pub static SYMBOLS: &[&str] = &["Segoe UI Symbol"];

/// The math fonts: Cambria Math, which Chrome sends the math blocks and the
/// math alphanumerics to, then the symbol font, then the one font that maps
/// all of them where it is installed.
pub static MATH: &[&str] = &["Cambria Math", "Segoe UI Symbol", "Code2000"];

/// Supplementary-plane CJK. Ordered for Simplified-first locales.
pub static PLANE_CJK_HANS: &[&str] = &[
    "SimSun-ExtB",
    "SimSun-ExtG",
    "MingLiU-ExtB",
    "PMingLiU-ExtB",
];

/// Supplementary-plane CJK, Traditional-first.
///
/// Ext G and I are mandated by GB18030-2022 and only SimSun-ExtG has them, so
/// it stays in the list regardless of locale.
pub static PLANE_CJK_HANT: &[&str] = &[
    "PMingLiU-ExtB",
    "MingLiU-ExtB",
    "SimSun-ExtB",
    "SimSun-ExtG",
];

#[cfg(test)]
mod tests {
    #[test]
    fn the_windows_lists_are_chromes() {
        use crate::family::names_match;
        // As `font_cache_skia_win.cc` spells them.
        let common = [
            "tahoma",
            "arial unicode ms",
            "lucida sans unicode",
            "microsoft sans serif",
            "palatino linotype",
            "dejavu serif",
            "dejavu sasns",
            "freeserif",
            "freesans",
            "gentium",
            "gentiumalt",
            "ms pgothic",
            "simsun",
            "gulim",
            "pmingliu",
            "code2000",
        ];
        let cjk = [
            "arial unicode ms",
            "ms pgothic",
            "simsun",
            "gulim",
            "pmingliu",
            "wenquanyi zen hei",
            "ar pl shanheisun uni",
            "ar pl zenkai uni",
            "han nom a",
            "code2000",
        ];
        let same = |a: &[&str], b: &[&str]| {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| names_match(a, b))
        };
        assert!(same(&common, &super::COMMON_FONTS));
        assert!(same(&cjk, &super::CJK_FONTS));
        assert!(names_match(super::LAST_RESORT, "lucida sans unicode"));
    }
}
