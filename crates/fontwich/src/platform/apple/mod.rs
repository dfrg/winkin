//! The macOS backend, driven by Core Text at query time.
//!
//! Static tables are a poor fit here: Apple ships a lot of fonts and changes
//! them every release. This asks the system instead — once for the broad
//! fallback list, via `CTFontCopyDefaultCascadeListForLanguages`, and once
//! for the best per-script font, via `CTFontCreateForString` — so it stays
//! correct across OS versions with nothing to regenerate.
//!
//! A collection asks once per key and keeps the answer, so nothing here
//! remembers anything.
//!
//! Children:
//! - `layer`: the system layer, read from the font directories and Core Text;
//! - `scripts`: the font Core Text substitutes for each script's letters;
//! - `tail`: the emoji, symbol and last-resort fonts;
//! - `generics`: Chrome's generic families and Standard font;
//! - `tests`: Core Text's cascades, and the generics against Chrome.

mod generics;
mod layer;
mod scripts;
mod tail;
#[cfg(test)]
mod tests;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use objc2_core_foundation::{CFArray, CFRetained, CFString};
use objc2_core_text::{CTFont, CTFontDescriptor, kCTFontFamilyNameAttribute, kCTFontURLAttribute};

pub(super) use layer::layer;

use crate::fallback::Presentation;
use crate::fallback::{FallbackKey, Han};

/// A Core Text fallback backend.
///
/// Queries Core Text for script fonts and fallback cascades. Generic
/// families use the platform defaults.
#[derive(Debug, Default)]
pub struct MacOs {
    allow_hvgl_only: bool,
}

impl MacOs {
    /// Creates a Core Text fallback backend.
    ///
    /// Excludes PingFang's proprietary outlines by default. See
    /// [`allowing_hvgl_only_fonts`](Self::allowing_hvgl_only_fonts).
    pub fn new() -> Self {
        Self::default()
    }

    /// Allows fonts with proprietary `hvgl` outlines.
    ///
    /// PingFang SC and TC use Apple's `hvgl` outline format on recent macOS
    /// releases. The default backend substitutes Heiti SC and TC, which
    /// have outlines supported by other renderers.
    ///
    /// Enable this when rendering through Core Text, which supports `hvgl`.
    #[must_use]
    pub fn allowing_hvgl_only_fonts(mut self) -> Self {
        self.allow_hvgl_only = true;
        self
    }

    /// The families for `key`, best first:
    ///
    /// - a script: the font Core Text substitutes for one of its letters,
    ///   then the language-less cascade;
    /// - a Han tradition: the cascade for its language;
    /// - Common: the language-less cascade, the symbol fonts and the last
    ///   resort;
    /// - a generic, or the Standard font: Chrome's settings for its bucket,
    ///   Heiti for PingFang;
    /// - emoji: Apple Color Emoji.
    ///
    /// Asks Core Text each time; a collection asks once per key.
    pub(crate) fn families(&self, key: &FallbackKey, name: &mut dyn FnMut(&str)) {
        if let Some(presentation) = key.presentation() {
            if presentation == Presentation::Emoji {
                tail::EMOJI.iter().for_each(|family| name(family));
            }
            return;
        }
        let chrome = match (key.generic(), key.standard()) {
            (Some((generic, bucket)), _) => Some(generics::TABLE.families(generic, bucket)),
            (None, Some(bucket)) => Some(generics::TABLE.standard(bucket)),
            (None, None) => None,
        };
        if let Some(chrome) = chrome {
            // The settings' families, but for PingFang, whose outlines only
            // Core Text can draw: Heiti in its place unless the host opted
            // in.
            for family in chrome {
                match substitute(family) {
                    Some(substitute) if !self.allow_hvgl_only => name(substitute),
                    _ => name(family),
                }
            }
            return;
        }
        let Some(script) = key.script() else {
            return;
        };
        let han = match key.han() {
            Some(han) if script == crate::script::HANI => Some(han),
            _ => None,
        };
        if han.is_none()
            && !key.is_common()
            && let Some(primary) = scripts::primary(script)
        {
            name(&primary);
        }
        // A cascade from a real font names PingFang. Leaves it out, as the
        // generics substitute it, unless the caller opted in.
        for family in fetch_cascade(han) {
            if self.allow_hvgl_only || !is_hvgl_only(&family) {
                name(&family);
            }
        }
        if key.is_common() {
            tail::SYMBOLS.iter().for_each(|family| name(family));
            tail::LAST_RESORT.iter().for_each(|family| name(family));
        }
    }
}

/// Whether `family` is a PingFang family whose outlines are only in Apple's
/// proprietary `hvgl` table.
///
/// On current macOS these fonts have no `glyf`, `CFF ` or `CFF2`. No
/// open-source renderer (FreeType, skrifa, HarfBuzz's own) can draw them.
fn is_hvgl_only(family: &str) -> bool {
    matches!(
        family,
        "PingFang SC" | "PingFang TC" | "PingFang HK" | "PingFang MO"
    )
}

/// The family drawn in place of an `hvgl`-only one: Heiti, which has
/// outlines, for the Simplified and Traditional PingFangs.
fn substitute(family: &str) -> Option<&'static str> {
    match family {
        "PingFang SC" => Some("Heiti SC"),
        "PingFang TC" | "PingFang HK" | "PingFang MO" => Some("Heiti TC"),
        _ => None,
    }
}

/// Asks Core Text for the system's own fallback order.
///
/// Cascades from Helvetica Neue, a real font with a public name, never the
/// private system font `CTFontCreateUIFontForLanguage` returns. The system
/// font's cascade is full of Apple's internal routing aliases
/// (`.AppleSimplifiedChineseFont` and the like), which nothing, Core Text's
/// public API included, resolves to a font. A real font's cascade names
/// real families for the same text: PingFang, Hiragino, Geeza Pro.
fn fetch_cascade(han: Option<Han>) -> Vec<String> {
    unsafe {
        let base = CTFont::with_name(
            &CFString::from_str("Helvetica Neue"),
            0.0,
            core::ptr::null(),
        );

        let list = match han.map(cascade_key) {
            Some(language) => {
                let cf = CFString::from_str(language);
                let langs = CFArray::from_retained_objects(&[cf]);
                let langs: &CFArray = langs.as_ref();
                base.default_cascade_list_for_languages(Some(langs))
            }
            None => base.default_cascade_list_for_languages(None),
        };

        let Some(list) = list else { return Vec::new() };
        let descriptors: CFRetained<CFArray<CTFontDescriptor>> = CFRetained::cast_unchecked(list);
        descriptors
            .iter()
            .filter(names_a_file)
            .filter_map(|d| family_name(&d))
            .collect()
    }
}

/// Returns the language that steers the cascade toward `han`'s tradition.
///
/// Measured against `CTFontCopyDefaultCascadeListForLanguages`: a script
/// subtag on an unrelated language (`en-Hant`) does not reorder the Han
/// entries. Only a language Core Text ties to a tradition does: `zh-Hans`,
/// `zh-Hant`, `ja`, `ko`. Hong Kong text asks as Traditional.
///
/// Other languages move the cascade a little (`ar` promotes Geeza Pro), but
/// not to the front. [`scripts::primary`] answers those scripts directly, so
/// the cascade is asked by language only for Han.
fn cascade_key(han: Han) -> &'static str {
    match han {
        Han::Hans => "zh-Hans",
        Han::Hant | Han::HantHK => "zh-Hant",
        Han::Jpan => "ja",
        Han::Kore => "ko",
    }
}

/// Whether the descriptor names a font in a file.
///
/// Cascading from a real font rather than the private system font keeps
/// Apple's per-context routing aliases out of the list — see
/// [`fetch_cascade`] — but not quite all of them: `.Apple Symbols Fallback`
/// comes back in every cascade, for every language, and is a name nothing
/// can resolve. Core Text will not even match it back to a descriptor, and
/// no font directory holds a file declaring it, so an answer naming it
/// names a family no collection can have. It is the only entry in any cascade
/// with no file, and every real family has one, so this tells the two apart
/// without another call into Core Text.
fn names_a_file(descriptor: &CFRetained<CTFontDescriptor>) -> bool {
    unsafe { descriptor.attribute(kCTFontURLAttribute).is_some() }
}

fn family_name(descriptor: &CTFontDescriptor) -> Option<String> {
    unsafe {
        let attr = descriptor.attribute(kCTFontFamilyNameAttribute)?;
        let name = attr.downcast::<CFString>().ok()?;
        Some(name.to_string())
    }
}
