//! Language tags, and the Han tradition they decide.
//!
//! Han is unified in Unicode but not in type: one character wants a
//! different glyph in Simplified Chinese, Traditional Chinese, Japanese and
//! Korean text. A language tag decides which tradition text is set in.

use parlance::{Language, Script};

use crate::fallback::key::Han;
use crate::script as sc;

/// Parses a BCP-47 language tag.
///
/// Accepts extlang forms such as `zh-yue-HK` by reducing them to `yue-HK`.
/// Returns `None` if the tag cannot be parsed. Does not allocate.
pub fn parse_language(tag: &str) -> Option<Language> {
    if let Ok(language) = Language::parse(tag) {
        return Some(language);
    }
    let separator = tag.find(['-', '_'])?;
    let rest = tag.get(separator + 1..)?;
    let extlang = match rest.find(['-', '_']) {
        Some(end) => &rest[..end],
        None => rest,
    };
    if extlang.len() != 3 || !extlang.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    Language::parse(rest).ok()
}

/// Language subtags that decide a Han tradition on their own.
///
/// The Chinese entries are the bare forms. [`parse_language`] has already
/// rewritten `zh-yue` to `yue`.
const LANG_TO_HAN: &[(&str, Han)] = &[
    ("cdo", Han::Hans),
    ("cjy", Han::Hans),
    ("cmn", Han::Hans),
    ("cpx", Han::Hans),
    ("czh", Han::Hans),
    ("czo", Han::Hans),
    ("gan", Han::Hans),
    ("hak", Han::Hant),
    ("hsn", Han::Hans),
    ("ja", Han::Jpan),
    ("ko", Han::Kore),
    ("lzh", Han::Hant),
    ("mnp", Han::Hans),
    ("nan", Han::Hant),
    ("wuu", Han::Hans),
    ("yue", Han::Hant),
    ("zh", Han::Hans),
];

/// Regions that decide a Han tradition. Hong Kong and Macau are Traditional
/// here; [`tradition`](crate::fallback::key::tradition) separates them.
const REGION_TO_HAN: &[(&str, Han)] = &[
    ("CN", Han::Hans),
    ("HK", Han::Hant),
    ("JP", Han::Jpan),
    ("KR", Han::Kore),
    ("MO", Han::Hant),
    ("SG", Han::Hans),
    ("TW", Han::Hant),
];

/// Languages written in a script other than Latin, by their usual script.
///
/// This is what a language is written in when its tag does not say, from
/// Blink's `LocaleToScriptCodeForFontSelection`, for the languages whose
/// script has fonts of its own. [`LANG_TO_HAN`] holds the Chinese, Japanese
/// and Korean languages.
const LANG_TO_SCRIPT: &[(&str, [u8; 4])] = &[
    ("am", *b"Ethi"),
    ("ar", *b"Arab"),
    ("as", *b"Beng"),
    ("ba", *b"Cyrl"),
    ("be", *b"Cyrl"),
    ("bg", *b"Cyrl"),
    ("bho", *b"Deva"),
    ("bn", *b"Beng"),
    ("bo", *b"Tibt"),
    ("ce", *b"Cyrl"),
    ("chr", *b"Cher"),
    ("ckb", *b"Arab"),
    ("cv", *b"Cyrl"),
    ("dv", *b"Thaa"),
    ("dz", *b"Tibt"),
    ("el", *b"Grek"),
    ("fa", *b"Arab"),
    ("gu", *b"Gujr"),
    ("he", *b"Hebr"),
    ("hi", *b"Deva"),
    ("hy", *b"Armn"),
    ("ii", *b"Yiii"),
    ("iu", *b"Cans"),
    ("ka", *b"Geor"),
    ("kk", *b"Cyrl"),
    ("km", *b"Khmr"),
    ("kn", *b"Knda"),
    ("kok", *b"Deva"),
    ("ks", *b"Arab"),
    ("ky", *b"Cyrl"),
    ("lo", *b"Laoo"),
    ("mai", *b"Deva"),
    ("mk", *b"Cyrl"),
    ("ml", *b"Mlym"),
    ("mn", *b"Cyrl"),
    ("mr", *b"Deva"),
    ("my", *b"Mymr"),
    ("ne", *b"Deva"),
    ("new", *b"Deva"),
    ("nqo", *b"Nkoo"),
    ("or", *b"Orya"),
    ("os", *b"Cyrl"),
    ("pa", *b"Guru"),
    ("ps", *b"Arab"),
    ("ru", *b"Cyrl"),
    ("sa", *b"Deva"),
    ("sat", *b"Olck"),
    ("sd", *b"Arab"),
    ("si", *b"Sinh"),
    ("sr", *b"Cyrl"),
    ("syr", *b"Syrc"),
    ("ta", *b"Taml"),
    ("te", *b"Telu"),
    ("tg", *b"Cyrl"),
    ("th", *b"Thai"),
    ("ti", *b"Ethi"),
    ("tt", *b"Cyrl"),
    ("ug", *b"Arab"),
    ("uk", *b"Cyrl"),
    ("ur", *b"Arab"),
    ("vai", *b"Vaii"),
    ("yi", *b"Hebr"),
];

/// Returns the script `lang` is written in, for choosing a generic family's
/// font.
///
/// Reads the tag's script subtag first. Chinese is Traditional where the
/// region is Taiwan, Hong Kong or Macau. Otherwise the language's usual
/// script applies, and Latin for a language not known to use another. Han
/// comes back as its tradition: `Hans`, `Hant`, `Jpan` or `Kore`.
pub(super) fn locale_script(lang: Language) -> Script {
    let explicit = lang.script().and_then(|s| Script::parse(s).ok());
    if let Some(script) = explicit {
        return match resolve_han(script, Some(lang)) {
            Some(han) => han.script(),
            None => script,
        };
    }
    let language = lang.language();
    if let Some(han) = lookup(LANG_TO_HAN, language) {
        let region = lang.region().and_then(|r| lookup(REGION_TO_HAN, r));
        return match (han, region) {
            (Han::Hans, Some(Han::Hant)) => Han::Hant.script(),
            (han, _) => han.script(),
        };
    }
    lookup(LANG_TO_SCRIPT, language).map_or(sc::LATN, Script::from_bytes)
}

fn lookup<T: Copy>(table: &[(&str, T)], key: &str) -> Option<T> {
    table
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .map(|(_, v)| *v)
}

/// Returns the Han tradition a script is written in on its own, if any.
fn script_han(script: Script) -> Option<Han> {
    match script {
        sc::HANS => Some(Han::Hans),
        sc::HANT | sc::BOPO => Some(Han::Hant),
        sc::JPAN | sc::HIRA | sc::KANA | sc::HRKT => Some(Han::Jpan),
        sc::KORE | sc::HANG => Some(Han::Kore),
        _ => None,
    }
}

/// Returns the Han tradition for text in `script` under `lang`, or `None` if
/// the text is not Han at all.
///
/// Never returns [`Han::HantHK`]. An unambiguous script decides alone.
/// Otherwise, for `Hani` or a script-less query, the tag decides: its script
/// subtag, then its region, then its language. This follows Blink's
/// `ScriptCodeForHan`, with script before region stated outright.
///
/// Returns `None` for an unresolved `Hani`. The caller picks the default.
pub(crate) fn resolve_han(script: Script, lang: Option<Language>) -> Option<Han> {
    if let Some(han) = script_han(script) {
        return Some(han);
    }
    if script != sc::HANI && script != Script::COMMON && script != Script::UNKNOWN {
        return None;
    }
    let lang = lang?;
    if let Some(han) = lang
        .script()
        .and_then(|s| Script::parse(s).ok())
        .and_then(script_han)
    {
        return Some(han);
    }
    if let Some(han) = lang.region().and_then(|r| lookup(REGION_TO_HAN, r)) {
        return Some(han);
    }
    lookup(LANG_TO_HAN, lang.language())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn han(script: &str, lang: &str) -> Option<Han> {
        resolve_han(Script::parse(script).unwrap(), parse_language(lang))
    }

    #[test]
    fn script_subtag_wins_over_region() {
        assert_eq!(han("Hani", "zh-Hant-CN"), Some(Han::Hant));
        assert_eq!(han("Hani", "zh-Hans-TW"), Some(Han::Hans));
    }

    #[test]
    fn region_wins_over_language() {
        // Sites emit lang="en-JP" when Japanese is the preferred language.
        assert_eq!(han("Hani", "en-JP"), Some(Han::Jpan));
        assert_eq!(han("Hani", "zh-TW"), Some(Han::Hant));
        assert_eq!(han("Hani", "zh-CN"), Some(Han::Hans));
    }

    #[test]
    fn a_language_is_written_in_a_script() {
        let script = |lang: &str| locale_script(parse_language(lang).unwrap()).to_bytes();
        assert_eq!(&script("en"), b"Latn");
        assert_eq!(&script("en-JP"), b"Latn");
        assert_eq!(&script("fr-CA"), b"Latn");
        assert_eq!(&script("ar"), b"Arab");
        assert_eq!(&script("km"), b"Khmr");
        assert_eq!(&script("ja"), b"Jpan");
        assert_eq!(&script("ko-KR"), b"Kore");
        assert_eq!(&script("zh"), b"Hans");
        assert_eq!(&script("zh-TW"), b"Hant");
        assert_eq!(&script("yue"), b"Hant");
        assert_eq!(&script("zh-Hant-CN"), b"Hant");
        assert_eq!(&script("sr-Latn"), b"Latn");
        assert_eq!(&script("sr"), b"Cyrl");
        assert_eq!(&script("uz-Arab"), b"Arab");
    }

    #[test]
    fn language_only() {
        assert_eq!(han("Hani", "ja"), Some(Han::Jpan));
        assert_eq!(han("Hani", "ko"), Some(Han::Kore));
        assert_eq!(han("Hani", "zh"), Some(Han::Hans));
        assert_eq!(han("Hani", "yue"), Some(Han::Hant));
        assert_eq!(han("Hani", "wuu"), Some(Han::Hans));
    }

    #[test]
    fn extlang_canonicalization_drops_the_primary_subtag() {
        // The reason this helper exists: parlance rejects the form outright.
        assert!(Language::parse("zh-yue").is_err());
        // BCP-47 §4.5. Everything after the first separator is kept verbatim,
        // including a `_` separator, which parlance then normalizes.
        for (tag, expected) in [
            ("zh-yue-HK", "yue-HK"),
            ("zh-cmn", "cmn"),
            ("zh_hak_TW", "hak-TW"),
        ] {
            assert_eq!(parse_language(tag).unwrap().as_str(), expected);
        }
    }

    #[test]
    fn extlang_forms_survive_canonicalization() {
        // parlance rejects these outright; without `parse_language`
        // they would silently degrade to no locale signal at all, and
        // Cantonese would get Simplified glyph forms.
        assert_eq!(han("Hani", "zh-yue"), Some(Han::Hant));
        assert_eq!(han("Hani", "zh-hak"), Some(Han::Hant));
        assert_eq!(han("Hani", "zh-cmn"), Some(Han::Hans));
        assert_eq!(han("Hani", "zh-yue-HK"), Some(Han::Hant));
    }

    #[test]
    fn unambiguous_script_ignores_locale() {
        assert_eq!(han("Hant", "ja-JP"), Some(Han::Hant));
        assert_eq!(han("Kana", "zh-CN"), Some(Han::Jpan));
        assert_eq!(han("Bopo", "en-US"), Some(Han::Hant));
    }

    #[test]
    fn non_han_text_has_no_tradition() {
        assert_eq!(han("Deva", "hi-IN"), None);
        assert_eq!(han("Latn", "ja-JP"), None);
        assert_eq!(han("Hani", "en-US"), None);
    }

    #[test]
    fn an_unparseable_tag_degrades_rather_than_failing() {
        // Fallback always has to answer, so a bad tag means no language,
        // never an error. The list includes shapes the `extlang` retry must
        // not accept.
        for tag in ["", "!!", "e", "en1", "en-La1n", "zh-yu3"] {
            assert_eq!(parse_language(tag), None, "{tag} should not parse");
        }
        // The same as no tag at all. The key then applies the default
        // tradition for unresolved Han.
        assert_eq!(han("Hani", "!!"), None);
        assert_eq!(resolve_han(sc::HANI, None), None);
    }

    #[test]
    fn trailing_subtags_are_discarded_not_rejected() {
        assert_eq!(
            parse_language("zh-Latn-pinyin").unwrap().as_str(),
            "zh-Latn"
        );
        assert_eq!(parse_language("zh_Hant_TW").unwrap().as_str(), "zh-Hant-TW");
    }
}
