//! Android.
//!
//! Behind the `android` feature, which is off by default.
//!
//! # Two halves, and why
//!
//! Android is the one platform where the obvious source of fallback data says
//! in its own first paragraph not to read it. `/system/etc/fonts.xml` opens
//! with a deprecation notice: parsing it from an app is unsupported, and the
//! file "will be renamed and/or moved out from their respective location in
//! the next Android release". It points at the NDK instead.
//!
//! The NDK does not replace it, though. `ASystemFontIterator` enumerates the
//! installed fonts with their locales and file paths, and nothing in
//! `<android/font.h>` says a word about generics — there is no way to ask it
//! what `sans-serif` is. So the split is not "new API on new devices, XML on
//! old" but:
//!
//! - **per-script families** from the NDK, keyed on `AFont_getLocale`;
//! - **generics** from `fonts.xml`, because nothing else exposes them;
//! - **everything** from `fonts.xml` where the NDK is unusable.
//!
//! Skia arrived at the same arrangement: `SkFontMgr_android_ndk.cpp` reads
//! the XML for the generic aliases and falls back to it wholesale outside an
//! app process, while `SkFontMgr_android.cpp` — still what Chrome ships —
//! only ever parses the XML.
//!
//! Two things that cost Skia scars and should not be relearned here.
//! `AFontMatcher` looks like the Core Text analogue and is not: Skia records
//! it as "broken from its introduction until at least API 33", because it
//! shapes with the default font first and so answers with that font for any
//! codepoint it happens to cover, whatever locale was asked for. And
//! `AFont_getLocale` returns nothing usable before API 30, which is the real
//! floor for the NDK half — not the API 29 the functions were introduced in.
//!
//! # State
//!
//! The XML half is done: the parse, the arrangement by script and generic,
//! the backend over it, and [`SystemFonts`] — a [`FamilyNames`] that
//! reads the names out of the fonts, for a caller with nothing better.
//!
//! The NDK enumerator is not here yet.
//!
//! [`FamilyNames`] being a trait is not scaffolding around a missing default.
//! A caller with a font collection already knows every family name on the
//! device. Opening the same two hundred files again to learn them twice is
//! waste. [`SystemFonts`] serves callers with no collection.

mod config;
mod generics;
// Against a real system image, where one has been extracted: see its docs.
#[cfg(all(test, feature = "std"))]
mod image;
mod index;
// The system layer: the fonts on disk, arranged by `fonts.xml`.
#[cfg(feature = "std")]
mod layer;
mod names;
#[cfg(test)]
mod tests;

pub use names::SystemFonts;

#[cfg(all(target_os = "android", feature = "system"))]
pub(super) use layer::layer;

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fallback::Presentation;
use crate::fallback::{FallbackKey, GenericBucket, GenericClass};
use crate::hash::HashMap;

use generics::CjkGeneric;
use index::{FontRef, Index};

/// Resolves Android font files to family names.
///
/// Implement using an existing font index to avoid reopening files.
/// [`SystemFonts`] reads names from disk when no index is available.
pub trait FamilyNames {
    /// Returns the family name for a font file and index.
    ///
    /// `path` includes the directory supplied to
    /// [`Android::from_fonts_xml`]. Return `None` for an absent or
    /// unreadable font; it will be excluded from fallback answers.
    fn family_name(&self, path: &str, index: u32) -> Option<String>;
}

impl<F: FamilyNames + ?Sized> FamilyNames for &F {
    fn family_name(&self, path: &str, index: u32) -> Option<String> {
        (**self).family_name(path, index)
    }
}

/// Resolves font names from a map keyed by path and collection index.
impl FamilyNames for BTreeMap<(String, u32), String> {
    fn family_name(&self, path: &str, index: u32) -> Option<String> {
        self.get(&(String::from(path), index)).cloned()
    }
}

/// An error encountered while reading `fonts.xml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// Malformed XML, with the parser's error message.
    Malformed(String),
    /// The root element is not `familyset`.
    NotFontsXml,
}

impl core::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Malformed(message) => write!(f, "malformed fonts.xml: {message}"),
            Self::NotFontsXml => write!(f, "not a fonts.xml: root element is not familyset"),
        }
    }
}

/// An Android font fallback backend.
///
/// Built from `fonts.xml` and a [`FamilyNames`] resolver. Can be
/// constructed on any target with the `android` feature.
#[derive(Debug)]
pub struct Android {
    index: Index,
    /// Each font's family name, read once when the backend is built. A font
    /// with none is absent.
    family_names: HashMap<FontRef, Box<str>>,
}

impl Android {
    /// Creates a backend from `fonts.xml`.
    ///
    /// `font_dir` is the directory containing the font files, usually
    /// `/system/fonts`. A trailing separator is optional. Each distinct
    /// font is resolved through `names` once.
    ///
    /// # Errors
    ///
    /// Returns an error if the XML is malformed or its root element is not
    /// `familyset`.
    pub fn from_fonts_xml(
        xml: &str,
        font_dir: &str,
        names: impl FamilyNames,
    ) -> Result<Self, ConfigError> {
        let config = config::parse(xml).map_err(|error| match error {
            config::Error::Xml(error) => {
                use core::fmt::Write;
                let mut message = String::new();
                let _ = write!(&mut message, "{error}");
                ConfigError::Malformed(message)
            }
            config::Error::NotFontsXml => ConfigError::NotFontsXml,
        })?;
        let mut font_dir = String::from(font_dir);
        if !font_dir.is_empty() && !font_dir.ends_with('/') {
            font_dir.push('/');
        }
        let index = Index::build(&config);
        let mut family_names: HashMap<FontRef, Box<str>> = HashMap::default();
        let mut absent: Vec<&FontRef> = Vec::new();
        let mut path = String::new();
        for font in index.every_font() {
            if family_names.contains_key(font) || absent.contains(&font) {
                continue;
            }
            path.clear();
            path.push_str(&font_dir);
            path.push_str(&font.file);
            match names.family_name(&path, font.index) {
                Some(name) => {
                    family_names.insert(font.clone(), name.into_boxed_str());
                }
                None => absent.push(font),
            }
        }
        drop(absent);
        Ok(Self {
            index,
            family_names,
        })
    }

    /// Returns family names and aliases from the configuration.
    ///
    /// Each pair maps a configured name to the font's family name, such as
    /// `("arial", "Roboto")`. Use with
    /// [`LayerBuilder::add_alias`](crate::LayerBuilder::add_alias).
    ///
    /// Aliases with a `weight` attribute are excluded because they identify
    /// a particular weight rather than a whole family.
    pub fn names(&self) -> Vec<(String, String)> {
        let mut resolved = Vec::new();
        // The file's own named families, which a page may name directly:
        // `casual`, `sans-serif-condensed`, `serif-monospace`. Not the four
        // that are CSS generics, which reach their fonts as generics and
        // must not be shadowed by a family of the same name.
        for (name, fonts) in self.index.named_families() {
            if index::is_generic(name) {
                continue;
            }
            if let Some(family) = fonts.iter().find_map(|font| self.family_name(font)) {
                resolved.push((String::from(name), String::from(family)));
            }
        }
        for (name, to) in self.index.aliases() {
            let fonts = self.index.named_fonts(to);
            // The family a page would land in, which is the name the first
            // font of the target family carries.
            let Some(family) = fonts.iter().find_map(|font| self.family_name(font)) else {
                continue;
            };
            resolved.push((String::from(name), String::from(family)));
        }
        resolved
    }

    /// Gives `each` the fonts of `bucket`'s Han tradition, its serif ones
    /// where `serif` says and the file marks any.
    fn tradition(
        &self,
        bucket: GenericBucket,
        serif: bool,
        each: &mut dyn FnMut(&mut dyn Iterator<Item = &FontRef>),
    ) {
        let script = match bucket {
            GenericBucket::Hans => crate::script::HANS,
            GenericBucket::Hant => crate::script::HANT,
            GenericBucket::Jpan => crate::script::JPAN,
            GenericBucket::Kore => crate::script::KORE,
            _ => return,
        };
        let fonts = self.index.script_fonts(script, serif);
        // Only a serif the file marks: without one, `serif` itself.
        if serif && fonts == self.index.script_fonts(script, false) {
            return;
        }
        each(&mut fonts.iter());
    }

    /// The family name of one font, if it has one.
    fn family_name(&self, font: &FontRef) -> Option<&str> {
        self.family_names.get(font).map(|name| &**name)
    }

    /// The families for `key`, in Skia's order:
    ///
    /// - text: the fonts for the key's Han tradition first, as Skia's
    ///   locale passes put them; then every script's fonts in the file's
    ///   order, the serif ones first for the serif class; then the symbols
    ///   and the default family. A collection keeps a script's to those
    ///   mapping its letters;
    /// - a generic: its named family, or on a CJK page the page's CJK font
    ///   as Chrome's Android rule has it; the Standard font only on a CJK
    ///   page;
    /// - emoji: the emoji fonts.
    ///
    /// Several fonts of one family name it more than once; the collection
    /// keeps the first.
    pub(crate) fn families(&self, key: &FallbackKey, name: &mut dyn FnMut(&str)) {
        let mut each = |fonts: &mut dyn Iterator<Item = &FontRef>| {
            for font in fonts {
                if let Some(family) = self.family_name(font) {
                    name(family);
                }
            }
        };
        if let Some(presentation) = key.presentation() {
            if presentation == Presentation::Emoji {
                each(&mut self.index.script_fonts(index::EMOJI, false).iter());
            }
            return;
        }
        if let Some((generic, bucket)) = key.generic() {
            match generics::cjk_generic(generic, bucket) {
                // The page's CJK font: its tradition's fonts, the first of
                // which maps the sample character.
                Some(CjkGeneric::Character(_)) => self.tradition(bucket, false, &mut each),
                // The page language's serif fallback, else `serif`.
                Some(CjkGeneric::Serif) => {
                    self.tradition(bucket, true, &mut each);
                    each(&mut self.index.named_fonts("serif").iter());
                }
                None => {
                    for named in generics::TABLE.families(generic, bucket) {
                        each(&mut self.index.named_fonts(named).iter());
                    }
                }
            }
            return;
        }
        if let Some(bucket) = key.standard() {
            if generics::standard_cjk(bucket).is_some() {
                self.tradition(bucket, false, &mut each);
            }
            return;
        }
        let serif = key.class() == GenericClass::Serif;
        if let Some(han) = key.han() {
            each(&mut self.index.script_fonts(han.script(), serif).iter());
        }
        each(&mut self.index.ordered_fonts(serif));
        each(&mut self.index.symbols().iter());
        // `fonts.xml` names no last resort, so the default family stands in.
        // AOSP's own comment marks it: "first font is default".
        each(&mut self.index.default_family().iter());
    }
}
