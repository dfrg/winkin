//! A missed character's own key: what a character no family of its run maps
//! asks before the Common key.
//!
//! - **Emoji.** A character with the Emoji property asks an emoji key: the
//!   color fonts where it is emoji by default (Emoji_Presentation) or a
//!   selector asks for them (VS16), the text emoji fonts otherwise (VS15, or a
//!   symbol such as `☺` in text). A character without the property, such as
//!   `★`, ignores its selector.
//! - **Its script.** The Script property's, with the fullwidth forms
//!   U+FF01–FF5E as Han. Han and the kana, hangul and bopomofo keys take
//!   their tradition as [`FallbackKey::new`] gives it.
//! - **A shared character** (Common or Inherited) asks one of its
//!   Script_Extensions: `。` Han in the page's tradition, `ー` Japanese, `।`
//!   Devanagari. Without any, a math alphanumeric asks the math key and a
//!   symbol the symbol key.
//!
//! Anything else asks nothing of its own and goes to the Common key, which on
//! Windows ends with Blink's catch-all lists (`platform::windows::tail`).
//!
//! No rule here needs a table per Unicode block. Blink's Windows block rule
//! sends arrows and math operators to Cambria Math, and `①` reaches
//! MS PGothic through `kCommonFonts`; here both ask the symbol key. Blink's
//! Linux fallback walks the page language's whole list for shared
//! punctuation; this answers it from the character's scripts' lists.

use parlance::{Language, Script};

use crate::fallback::emoji::Presentation;
use crate::fallback::key::{BackendFacts, FallbackKey, FallbackRequest, GenericClass};
use crate::fallback::unicode;
use crate::script as sc;

/// The key a character no family of its run maps asks before the Common key,
/// if it has one of its own.
///
/// `presentation` is the cluster's, `language` the content language (the
/// collection's default where the run has none), `generic` the run's class
/// and `facts` the fallback layers'. Never the Common key itself; nothing
/// allocated.
pub(crate) fn key(
    c: char,
    presentation: Presentation,
    language: Option<Language>,
    generic: GenericClass,
    facts: BackendFacts,
) -> Option<FallbackKey> {
    let request = match ask(u32::from(c), presentation)? {
        Ask::Emoji(presentation) => FallbackRequest::Emoji(presentation),
        Ask::Script(script) => FallbackRequest::Text {
            script,
            language,
            generic,
        },
    };
    let common = FallbackRequest::Text {
        script: Script::COMMON,
        language,
        generic,
    };
    let key = FallbackKey::new(&request, facts);
    (key != FallbackKey::new(&common, facts)).then_some(key)
}

/// What a missed character asks, before canonicalization.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Ask {
    /// A script's key, a pseudo-script's included.
    Script(Script),
    /// The emoji fonts for a presentation.
    Emoji(Presentation),
}

/// What `c` asks in `presentation`, if anything.
fn ask(c: u32, presentation: Presentation) -> Option<Ask> {
    if let Some(by_default) = emoji(c) {
        let color = match presentation {
            Presentation::Emoji | Presentation::EmojiOnly => true,
            Presentation::TextOnly => false,
            Presentation::Text => by_default,
        };
        let presentation = if color {
            Presentation::Emoji
        } else {
            Presentation::Text
        };
        return Some(Ask::Emoji(presentation));
    }
    if (0xFF01..=0xFF5E).contains(&c) {
        return Some(Ask::Script(sc::HANI));
    }
    if let Some(script) = unicode::script(c) {
        return Some(Ask::Script(Script::from_bytes(script)));
    }
    if let Some(script) = extension(unicode::extensions(c)) {
        return Some(Ask::Script(script));
    }
    if (0x1D400..=0x1D7FF).contains(&c) {
        return Some(Ask::Script(sc::ZMTH));
    }
    if SYMBOLS.iter().any(|range| range.contains(&c)) {
        return Some(Ask::Script(sc::ZSYM));
    }
    None
}

/// Whether `c` has the Emoji property and, if so, whether it is emoji by
/// default (Emoji_Presentation).
pub(super) fn emoji(c: u32) -> Option<bool> {
    let at = EMOJI.partition_point(|&(_, end, _)| end < c);
    match EMOJI.get(at) {
        Some(&(start, _, by_default)) if start <= c => Some(by_default),
        _ => None,
    }
}

/// Which of a shared character's Script_Extensions to ask, if any.
///
/// Han first, since the language decides its forms, then kana, hangul and
/// bopomofo for the same reason; then Latin, Devanagari and Arabic, the
/// blocks the marks, the danda and the Arabic punctuation come from. Below
/// those the first, which is arbitrary but stable: Script_Extensions is in
/// code order.
fn extension(extensions: &[[u8; 4]]) -> Option<Script> {
    let has = |script: &Script| extensions.contains(&script.to_bytes());
    [
        sc::HANI,
        sc::HIRA,
        sc::KANA,
        sc::HANG,
        sc::BOPO,
        sc::LATN,
        sc::DEVA,
        sc::ARAB,
    ]
    .into_iter()
    .find(has)
    .or_else(|| extensions.first().map(|&tag| Script::from_bytes(tag)))
}

/// Where the symbols are: Letterlike Symbols through Miscellaneous Symbols
/// and Arrows, and the supplementary planes' symbol blocks.
const SYMBOLS: [core::ops::RangeInclusive<u32>; 2] = [0x2100..=0x2BFF, 0x1F000..=0x1FBFF];

include!("classify_table.rs");
