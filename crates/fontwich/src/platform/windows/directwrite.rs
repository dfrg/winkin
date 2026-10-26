//! The system layer on Windows: DirectWrite's fonts.
//!
//! The shape fontique's DirectWrite backend has, and for the same reason:
//! DirectWrite lists every family name in the system collection at once, so
//! the layer's name set is complete from the start and no font file is read. A
//! family's files are asked for the first time its fonts are wanted.
//!
//! The layer's fallback source is the Windows tables, which is what fallback on
//! Windows has always been; DirectWrite is asked only what is installed.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;
use core::ptr;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_AXIS_VALUE, DWRITE_FONT_PROPERTY,
    DWRITE_FONT_PROPERTY_ID, DWRITE_FONT_PROPERTY_ID_FULL_NAME,
    DWRITE_FONT_PROPERTY_ID_POSTSCRIPT_NAME, DWRITE_FONT_PROPERTY_ID_TYPOGRAPHIC_FAMILY_NAME,
    DWRITE_FONT_PROPERTY_ID_WIN32_FAMILY_NAME, DWRITE_FONT_SIMULATIONS_NONE,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_READING_DIRECTION, DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWriteCreateFactory,
    IDWriteFactory, IDWriteFactory2, IDWriteFactory3, IDWriteFactory6, IDWriteFont, IDWriteFont3,
    IDWriteFontCollection, IDWriteFontFaceReference, IDWriteFontFaceReference1, IDWriteFontFile,
    IDWriteFontSet, IDWriteLocalFontFileLoader, IDWriteLocalizedStrings, IDWriteNumberSubstitution,
    IDWriteTextAnalysisSource, IDWriteTextAnalysisSource_Impl,
};
use windows::core::{BOOL, HSTRING, Interface, OutRef, PCWSTR, implement, w};

use crate::font::width_from_class;
use crate::{Font, FontWeight, Layer, LoadFamily, Role};

/// The system layer; see [`Layer::system`].
pub(crate) fn layer() -> Layer {
    let (families, primaries) = families();
    Layer::with_secondary(
        Role::System,
        families,
        primaries,
        Arc::new(DirectWriteFonts),
    )
    .with_fallback(crate::backend::Backend::platform())
}

/// The system font collection.
///
/// From the *shared* factory, which DirectWrite hands out once per process and
/// whose collection it caches, so asking again costs little. Nothing here
/// holds a COM object across calls: the loader asks afresh each time a family
/// loads, which keeps it free of any question about whether a COM object may
/// cross threads, and a layer is shared across threads.
fn collection() -> Option<IDWriteFontCollection> {
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let mut collection = None;
        factory
            .GetSystemFontCollection(&mut collection, false)
            .ok()?;
        collection
    }
}

/// Every family in the system collection, as `(name, aliases)`.
///
/// DirectWrite gives a family a name per locale. The `en-us` one is its name
/// where there is one, as reading a font directly prefers English; the rest
/// are aliases, which is how MS Gothic is also found as ＭＳ ゴシック.
fn wws_families() -> Vec<(String, Vec<String>)> {
    let Some(collection) = collection() else {
        return Vec::new();
    };
    let mut families = Vec::new();
    unsafe {
        for at in 0..collection.GetFontFamilyCount() {
            let Ok(names) = collection
                .GetFontFamily(at)
                .and_then(|family| family.GetFamilyNames())
            else {
                continue;
            };
            let localized = localized(&names);
            let english = localized
                .iter()
                .position(|(locale, _)| locale.eq_ignore_ascii_case("en-us"))
                .or_else(|| {
                    localized
                        .iter()
                        .position(|(locale, _)| locale.to_ascii_lowercase().starts_with("en"))
                })
                .unwrap_or(0);
            let Some((_, name)) = localized.get(english) else {
                continue;
            };
            let mut aliases: Vec<String> = Vec::new();
            for (_, other) in &localized {
                if other != name && !aliases.contains(other) {
                    aliases.push(other.clone());
                }
            }
            families.push((name.clone(), aliases));
        }
    }
    families
}

/// Every family name a stylesheet may use, as `(name, aliases)`.
///
/// DirectWrite's collection groups fonts by weight, width and slope, so
/// Arial Black and Arial Narrow are fonts of "Arial" there, and Segoe UI
/// Semibold one of "Segoe UI". But a page asks for them by their legacy
/// names, as Win32 knew them — `font-family: "Arial Black"`, `"Segoe UI
/// Semibold"` — and each name draws only those fonts. So the system font
/// set indexes every font by its Win32 family name and its typographic
/// family name, and every one of those not already a family is a
/// family too, loaded by [`by_property`].
///
/// Returns them with how many are DirectWrite's own, which come first; the
/// rest are secondary.
fn families() -> (Vec<(String, Vec<String>)>, usize) {
    let mut families = wws_families();
    let primaries = families.len();
    let mut known: crate::hash::HashSet<String> = families
        .iter()
        .flat_map(|(name, aliases)| core::iter::once(name).chain(aliases))
        .map(|name| name.to_lowercase())
        .collect();
    let Some(set) = font_set() else {
        return (families, primaries);
    };
    for property in OTHER_NAMES {
        let Ok(names) = (unsafe { set.GetPropertyValues(property) }) else {
            continue;
        };
        let mut candidates: Vec<String> = Vec::new();
        for at in 0..unsafe { names.GetCount() } {
            let Some(name) = (unsafe {
                let len = names.GetStringLength(at).unwrap_or(0);
                wide(len, |buffer| names.GetString(at, buffer))
            }) else {
                continue;
            };
            let name = String::from_utf16_lossy(&name);
            if !name.is_empty() && known.insert(name.to_lowercase()) {
                candidates.push(name);
            }
        }
        for name in instances_removed(&set, property, candidates) {
            families.push((name, Vec::new()));
        }
    }
    (families, primaries)
}

/// The axis values a font set entry holds, which for a named instance are
/// its coordinates. Empty before Windows 10 1803, where nothing names
/// instances either.
fn axis_values(reference: &IDWriteFontFaceReference) -> Coordinates {
    let Ok(reference) = reference.cast::<IDWriteFontFaceReference1>() else {
        return Vec::new();
    };
    unsafe {
        let count = reference.GetFontAxisValueCount() as usize;
        let mut values = alloc::vec![DWRITE_FONT_AXIS_VALUE::default(); count];
        if reference.GetFontAxisValues(&mut values).is_err() {
            return Vec::new();
        }
        values
            .into_iter()
            .map(|value| (value.axisTag.0.to_le_bytes(), value.value))
            .collect()
    }
}

/// `candidates` without the names that are a variable font's named
/// instances rather than fonts of their own.
///
/// A Win32 family holds four styles, so DirectWrite gives a font that does
/// not fit one of its own — Arial Black — and does the same for each named
/// instance of a variable font — "Bahnschrift SemiBold". The first is a font
/// and the second a point on an axis, and the font set answers for both.
///
/// Which is which, without reading a font: a font's own name matches every
/// instance the set holds of it, and an instance's name matches just that
/// one. Arial Black has one entry and its name matches it; Bahnschrift
/// has fifteen and "Bahnschrift SemiBold" matches one, while "Sitka" and
/// "Segoe UI Variable", the typographic names of variable fonts, match all of
/// theirs.
fn instances_removed(
    set: &IDWriteFontSet,
    property: DWRITE_FONT_PROPERTY_ID,
    candidates: Vec<String>,
) -> Vec<String> {
    // What each name matches, and everything the set holds of each font.
    let mut matched: Vec<Vec<((PathBuf, u32), usize)>> = Vec::with_capacity(candidates.len());
    let mut whole: crate::hash::HashMap<(PathBuf, u32), crate::hash::HashSet<String>> =
        Default::default();
    for name in &candidates {
        let mut fonts = Vec::new();
        for (path, index, pinned) in by_property(set, property, &HSTRING::from(name.as_str())) {
            let font = (path, index);
            let held = whole.entry(font.clone()).or_default();
            for values in &pinned {
                held.insert(coordinates(values));
            }
            fonts.push((font, pinned.len()));
        }
        matched.push(fonts);
    }
    candidates
        .into_iter()
        .zip(matched)
        .filter(|(_, fonts)| {
            fonts
                .iter()
                .any(|(font, mine)| whole.get(font).is_none_or(|held| held.len() <= *mine))
        })
        .map(|(name, _)| name)
        .collect()
}

/// A point in a font's design space, as a key: the axes an entry holds and
/// where, in the order DirectWrite gives them.
fn coordinates(values: &Coordinates) -> String {
    use core::fmt::Write;
    let mut key = String::new();
    for (tag, value) in values {
        let _ = write!(key, "{}={value};", String::from_utf8_lossy(tag));
    }
    key
}

/// The properties besides DirectWrite's own family a font is named by.
const OTHER_NAMES: [DWRITE_FONT_PROPERTY_ID; 2] = [
    DWRITE_FONT_PROPERTY_ID_WIN32_FAMILY_NAME,
    DWRITE_FONT_PROPERTY_ID_TYPOGRAPHIC_FAMILY_NAME,
];

/// The system font set (Windows 10 and later), indexed by every name a font
/// has: the fonts installed, not those Windows would download on demand,
/// which have names and no files.
fn font_set() -> Option<IDWriteFontSet> {
    unsafe {
        if let Ok(factory) = DWriteCreateFactory::<IDWriteFactory6>(DWRITE_FACTORY_TYPE_SHARED)
            && let Ok(set) = factory.GetSystemFontSet(false)
        {
            return set.cast().ok();
        }
        // Before Windows 10 1803, a set with nothing to download.
        let factory: IDWriteFactory3 = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        factory.GetSystemFontSet().ok()
    }
}

/// Where a font set entry sits in its font's design space: each axis it
/// holds and its value there. A named instance's coordinates; for a font
/// that does not vary, whatever DirectWrite reports of its weight and slope.
type Coordinates = Vec<([u8; 4], f32)>;

/// A font of a file, and the coordinates of every entry the set holds of it.
type FontLocation = (PathBuf, u32, Vec<Coordinates>);

/// The files of the fonts whose `property` is `name`, and which font in
/// each, simulated entries passed over.
fn by_property(
    set: &IDWriteFontSet,
    property: DWRITE_FONT_PROPERTY_ID,
    name: &HSTRING,
) -> Vec<FontLocation> {
    let wanted = DWRITE_FONT_PROPERTY {
        propertyId: property,
        propertyValue: PCWSTR(name.as_ptr()),
        localeName: PCWSTR::null(),
    };
    let mut found = Vec::new();
    unsafe {
        let Ok(matched) = set.GetMatchingFonts2(&[wanted]) else {
            return found;
        };
        for at in 0..matched.GetFontCount() {
            let Ok(reference) = matched.GetFontFaceReference(at) else {
                continue;
            };
            if reference.GetSimulations() != DWRITE_FONT_SIMULATIONS_NONE {
                continue;
            }
            if let Some(path) = reference
                .GetFontFile()
                .ok()
                .and_then(|file| file_path(&file))
            {
                let index = reference.GetFontFaceIndex();
                let values = axis_values(&reference);
                // One font, however many of its instances the name matched:
                // every one of their coordinates, since it is whether any of
                // them is the font itself that says what the name means.
                match found
                    .iter_mut()
                    .find(|(seen, at, _): &&mut (PathBuf, u32, _)| *seen == path && *at == index)
                {
                    Some((_, _, pinned)) => pinned.push(values),
                    None => found.push((path, index, alloc::vec![values])),
                }
            }
        }
    }
    found
}

/// Every `(locale, string)` in `strings`.
unsafe fn localized(strings: &IDWriteLocalizedStrings) -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        for at in 0..strings.GetCount() {
            let locale = strings
                .GetLocaleNameLength(at)
                .ok()
                .and_then(|len| wide(len, |buffer| strings.GetLocaleName(at, buffer)));
            let text = strings
                .GetStringLength(at)
                .ok()
                .and_then(|len| wide(len, |buffer| strings.GetString(at, buffer)));
            if let (Some(locale), Some(text)) = (locale, text) {
                out.push((
                    String::from_utf16_lossy(&locale),
                    String::from_utf16_lossy(&text),
                ));
            }
        }
    }
    out
}

/// A UTF-16 string DirectWrite writes into a buffer of `len` characters plus
/// the terminating null it counts separately, without that null.
fn wide(len: u32, fill: impl FnOnce(&mut [u16]) -> windows::core::Result<()>) -> Option<Vec<u16>> {
    let mut buffer = vec![0u16; len as usize + 1];
    fill(&mut buffer).ok()?;
    buffer.truncate(len as usize);
    Some(buffer)
}

/// Asks DirectWrite for a family's files when its fonts are first wanted.
#[derive(Debug)]
struct DirectWriteFonts;

impl LoadFamily for DirectWriteFonts {
    fn load(&self, name: &str) -> Vec<Font> {
        let Some(collection) = collection() else {
            return Vec::new();
        };
        // Each file and index, with the weight and width DirectWrite gives it.
        let mut found: Vec<(Arc<Path>, u32, FontWeight, u16)> = Vec::new();
        unsafe {
            let mut index = 0;
            let mut exists = BOOL(0);
            let located = collection.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists);
            if located.is_err() || !exists.as_bool() {
                return other_names(name);
            }
            let Ok(family) = collection.GetFontFamily(index) else {
                return Vec::new();
            };
            for at in 0..family.GetFontCount() {
                let Ok(font) = family.GetFont(at) else {
                    continue;
                };
                // DirectWrite lists a bold or oblique it would synthesize as a
                // font of its own, backed by the same file as the real one.
                // Nobody installed it, and synthesis is matching's decision.
                if font.GetSimulations() != DWRITE_FONT_SIMULATIONS_NONE {
                    continue;
                }
                let Some((path, index)) = font_file(&font) else {
                    continue;
                };
                if !found
                    .iter()
                    .any(|(seen, at, ..)| **seen == *path && *at == index)
                {
                    found.push((
                        Arc::from(path),
                        index,
                        FontWeight::new(font.GetWeight().0 as f32),
                        font.GetStretch().0 as u16,
                    ));
                }
            }
        }
        // A variable font's named instances are all one file at one index,
        // so they have already folded into one font here, whose axes give its
        // range. Its attributes come from the file, not from whichever named
        // instance DirectWrite listed first.
        //
        // A static font takes DirectWrite's weight and width, which is what
        // every other Windows program sees. They differ from the file's where
        // the file is wrong: Franklin Gothic's Medium, Demi and Heavy all say
        // 400 in OS/2, and Copperplate Gothic's Light and Bold likewise, and
        // DirectWrite reads their weights from their style names instead.
        found
            .into_iter()
            .filter_map(|(path, index, weight, stretch)| {
                let mut font = Font::from_path(path, index)?;
                if font.axes().is_empty() && (1..=9).contains(&stretch) {
                    font.correct(weight, width_from_class(stretch));
                }
                Some(font)
            })
            .collect()
    }

    fn local(&self, name: &str) -> Option<Font> {
        local(name)
    }
}

/// The fonts of a family that is not one of DirectWrite's: those whose Win32
/// family name, or failing that typographic family name, is `name`.
///
/// A Win32 family holds four styles — regular, bold, italic, bold italic —
/// so a font that does not fit gets a family of its own: Arial Black, Arial
/// Narrow, Segoe UI Semibold. Those names are the font's own, declared in its
/// `name` table, and a page that asks for them gets exactly those fonts.
///
/// DirectWrite applies the same rule to a *variable* font's named instances,
/// which is a different thing: "Bahnschrift SemiBold" is a point on
/// Bahnschrift's weight axis, named nowhere in the file, and every instance
/// belongs to one font. Those are dropped. The font set answers for them even
/// though it does not list them, so the check is here rather than at
/// listing. Browsers resolve them inconsistently, so nothing can depend on
/// them. A page reaches a named instance through `font-weight` and the rest,
/// which vary the font, or through `local()` in `@font-face`, which CSS says
/// matches a full font name.
fn other_names(name: &str) -> Vec<Font> {
    let Some(set) = font_set() else {
        return Vec::new();
    };
    let name = HSTRING::from(name);
    OTHER_NAMES
        .into_iter()
        .map(|property| by_property(&set, property, &name))
        .find(|found| !found.is_empty())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(path, index, pinned)| {
            let font = Font::from_path(path, index)?;
            // The font's own name matches its default instance among the
            // rest; an instance's name matches only that one point.
            let itself = pinned.iter().any(|values| !font.is_instance(values));
            itself.then_some(font)
        })
        .collect()
}

/// The installed font whose full name, then PostScript name, is `name`,
/// held at that name's point of the font where it names a named instance.
///
/// Through the system font *set* (Windows 10 and later), which DirectWrite
/// indexes by both, so no family is loaded to find it. A simulated entry — a
/// bold DirectWrite would fake — is not a font anyone installed, and is
/// passed over.
fn local(name: &str) -> Option<Font> {
    let set = font_set()?;
    let name = HSTRING::from(name);
    [
        DWRITE_FONT_PROPERTY_ID_FULL_NAME,
        DWRITE_FONT_PROPERTY_ID_POSTSCRIPT_NAME,
    ]
    .into_iter()
    .find_map(|property| {
        by_property(&set, property, &name)
            .into_iter()
            .find_map(|(path, index, pinned)| {
                let mut font = Font::from_path(path, index)?;
                // A full name or PostScript name may be a named instance's —
                // "Bahnschrift SemiBold" — and the entry that matched first
                // says where in the font that is. Asked for here, where the
                // set is already open, and kept nowhere. Only where the name
                // is an instance's: a full name that is the font's own
                // leaves it whole, with every axis it varies along, which is
                // what a rule declaring a range over `local()` needs.
                if let Some(coordinates) = pinned.first()
                    && font.is_instance(coordinates)
                {
                    font.pin(coordinates);
                }
                Some(font)
            })
    })
}

/// Calls `name` with the family DirectWrite's system fallback draws `c` with
/// in text of `locale`, if it maps `c` to one that has it.
///
/// `IDWriteFontFallback::MapCharacters` as Skia's `matchFamilyStyleCharacter`
/// asks it: from the system collection, for the default family
/// (Segoe UI, the message font Skia takes), regular, upright and normal
/// width. Holds nothing across calls; a family name longer than a stack
/// buffer, or any failure, is no answer.
pub(super) fn map_character(c: char, locale: &str, name: &mut dyn FnMut(&str)) {
    let source = OneCharacter::new(c, locale);
    let len = source.len;
    let source: IDWriteTextAnalysisSource = source.into();
    unsafe {
        let Ok(factory) = DWriteCreateFactory::<IDWriteFactory2>(DWRITE_FACTORY_TYPE_SHARED) else {
            return;
        };
        let Ok(fallback) = factory.GetSystemFontFallback() else {
            return;
        };
        let mut collection = None;
        if factory
            .GetSystemFontCollection(&mut collection, false)
            .is_err()
        {
            return;
        }
        let (mut mapped, mut font, mut scale) = (0, None, 0.0);
        let asked = fallback.MapCharacters(
            &source,
            0,
            len,
            collection.as_ref(),
            w!("Segoe UI"),
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            &mut mapped,
            &mut font,
            &mut scale,
        );
        let Some(font) = font.filter(|_| asked.is_ok() && mapped == len) else {
            return;
        };
        if !font
            .HasCharacter(u32::from(c))
            .is_ok_and(|has| has.as_bool())
        {
            return;
        }
        let Ok(names) = font
            .GetFontFamily()
            .and_then(|family| family.GetFamilyNames())
        else {
            return;
        };
        let (mut at, mut exists) = (0, BOOL(0));
        if names
            .FindLocaleName(w!("en-us"), &mut at, &mut exists)
            .is_err()
            || !exists.as_bool()
        {
            at = 0;
        }
        let mut wide = [0u16; 128];
        let Ok(length) = names.GetStringLength(at) else {
            return;
        };
        let Some(buffer) = wide.get_mut(..length as usize + 1) else {
            return;
        };
        if names.GetString(at, buffer).is_err() {
            return;
        }
        let mut utf8 = [0u8; 384];
        let mut end = 0;
        for c in char::decode_utf16(wide[..length as usize].iter().copied()) {
            let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
            let Some(slot) = utf8.get_mut(end..end + c.len_utf8()) else {
                return;
            };
            end += c.encode_utf8(slot).len();
        }
        if let Ok(family) = core::str::from_utf8(&utf8[..end]) {
            name(family);
        }
    }
}

/// One character in one locale, as text for DirectWrite to analyze.
#[implement(IDWriteTextAnalysisSource)]
struct OneCharacter {
    text: [u16; 2],
    len: u32,
    /// The locale, null-terminated; a longer one is cut.
    locale: [u16; 16],
}

impl OneCharacter {
    fn new(c: char, locale: &str) -> Self {
        let mut text = [0; 2];
        let len = c.encode_utf16(&mut text).len() as u32;
        let mut wide = [0; 16];
        for (slot, unit) in wide[..15].iter_mut().zip(locale.encode_utf16()) {
            *slot = unit;
        }
        Self {
            text,
            len,
            locale: wide,
        }
    }
}

#[allow(non_snake_case)]
impl IDWriteTextAnalysisSource_Impl for OneCharacter_Impl {
    fn GetTextAtPosition(
        &self,
        position: u32,
        text: *mut *mut u16,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        // SAFETY: DirectWrite passes places to write to, and reads the text
        // only while it holds this source.
        unsafe {
            if position < self.len {
                *text = self.text.as_ptr().add(position as usize).cast_mut();
                *length = self.len - position;
            } else {
                *text = ptr::null_mut();
                *length = 0;
            }
        }
        Ok(())
    }

    fn GetTextBeforePosition(
        &self,
        position: u32,
        text: *mut *mut u16,
        length: *mut u32,
    ) -> windows::core::Result<()> {
        // SAFETY: as above.
        unsafe {
            if position > 0 && position <= self.len {
                *text = self.text.as_ptr().cast_mut();
                *length = position;
            } else {
                *text = ptr::null_mut();
                *length = 0;
            }
        }
        Ok(())
    }

    fn GetParagraphReadingDirection(&self) -> DWRITE_READING_DIRECTION {
        DWRITE_READING_DIRECTION_LEFT_TO_RIGHT
    }

    fn GetLocaleName(
        &self,
        position: u32,
        length: *mut u32,
        locale: *mut *mut u16,
    ) -> windows::core::Result<()> {
        // SAFETY: as above.
        unsafe {
            *length = self.len.saturating_sub(position);
            *locale = self.locale.as_ptr().cast_mut();
        }
        Ok(())
    }

    fn GetNumberSubstitution(
        &self,
        position: u32,
        length: *mut u32,
        substitution: OutRef<IDWriteNumberSubstitution>,
    ) -> windows::core::Result<()> {
        // SAFETY: as above.
        unsafe { *length = self.len.saturating_sub(position) };
        substitution.write(None)
    }
}

/// The file behind `font` and which font in it, if it is a file on this
/// machine.
///
/// Through a font face *reference* where DirectWrite has them (Windows 10
/// and later), which names the file without building a face: creating a face
/// maps the file and parses it, and was most of the cost of loading every
/// family.
unsafe fn font_file(font: &IDWriteFont) -> Option<(PathBuf, u32)> {
    unsafe {
        if let Ok(font) = font.cast::<IDWriteFont3>() {
            let reference = font.GetFontFaceReference().ok()?;
            let path = file_path(&reference.GetFontFile().ok()?)?;
            return Some((path, reference.GetFontFaceIndex()));
        }
        let face = font.CreateFontFace().ok()?;
        let mut count = 0;
        face.GetFiles(&mut count, None).ok()?;
        if count == 0 {
            return None;
        }
        let mut files: Vec<Option<IDWriteFontFile>> = vec![None; count as usize];
        face.GetFiles(&mut count, Some(files.as_mut_ptr())).ok()?;
        let file = files.into_iter().next()??;
        Some((file_path(&file)?, face.GetIndex()))
    }
}

/// The path of `file`, if it is a file on this machine.
///
/// A font whose loader is not DirectWrite's local-file loader — a font a
/// program registered from memory, say — has no path, and is left out.
unsafe fn file_path(file: &IDWriteFontFile) -> Option<PathBuf> {
    unsafe {
        let mut key = ptr::null_mut();
        let mut size = 0;
        file.GetReferenceKey(&mut key, &mut size).ok()?;
        let loader: IDWriteLocalFontFileLoader = file.GetLoader().ok()?.cast().ok()?;
        let len = loader.GetFilePathLengthFromKey(key, size).ok()?;
        let path = wide(len, |buffer| loader.GetFilePathFromKey(key, size, buffer))?;
        Some(PathBuf::from(OsString::from_wide(&path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback::{BackendFacts, FallbackKey};
    use crate::{Collection, FallbackRequest, GenericClass, Source};
    use parlance::Script;

    fn layer() -> Arc<Layer> {
        Arc::new(super::layer())
    }

    fn query(tag: &[u8; 4]) -> FallbackRequest {
        FallbackRequest::Text {
            script: Script::from_bytes(*tag),
            language: None,
            generic: GenericClass::Plain,
        }
    }

    #[test]
    fn listing_the_system_reads_no_font() {
        // Every Windows has fonts, so unlike the fontconfig tests nothing here
        // skips on an empty list: an empty layer is a failure.
        let layer = layer();
        assert!(
            layer.len() > 10,
            "DirectWrite listed {} families",
            layer.len()
        );
        assert_eq!(layer.loaded(), 0, "building the layer loaded a family");
    }

    #[test]
    fn segoe_ui_loads_its_own_files() {
        // Segoe UI ships with every Windows since Vista.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let segoe = collection
            .family("segoe ui")
            .expect("Segoe UI is installed");
        assert_eq!(segoe.name(), "Segoe UI");
        assert!(!segoe.is_loaded());
        let fonts = segoe.fonts();
        assert!(!fonts.is_empty(), "Segoe UI loaded no fonts");
        let mut seen = Vec::new();
        for font in fonts {
            let Source::Path(path) = font.source() else {
                panic!("a system font should be named by path");
            };
            assert!(path.exists(), "{} does not exist", path.display());
            let entry = (path.clone(), font.index());
            assert!(!seen.contains(&entry), "{} listed twice", path.display());
            seen.push(entry);
        }
        assert_eq!(layer.loaded(), 1, "loading one family loaded others");
    }

    #[test]
    fn segoe_ui_s_fonts_say_what_they_are() {
        use parlance::{FontStyle, FontWeight};
        let collection = Collection::new().with_layer(layer());
        let segoe = collection
            .family("Segoe UI")
            .expect("Segoe UI is installed");
        let fonts = segoe.fonts();
        let has = |weight: FontWeight, italic: bool| {
            fonts.iter().any(|font| {
                font.weight() == weight && (font.style() == FontStyle::Italic) == italic
            })
        };
        assert!(has(FontWeight::NORMAL, false), "no upright regular");
        assert!(has(FontWeight::NORMAL, true), "no italic regular");
        assert!(has(FontWeight::BOLD, false), "no upright bold");
        assert!(has(FontWeight::LIGHT, false), "no upright light");
    }

    #[test]
    fn a_variable_font_is_one_font_with_its_axes() {
        // Bahnschrift, a variable font since Windows 10 1709, which
        // DirectWrite lists as fifteen named instances of one file.
        let collection = Collection::new().with_layer(layer());
        let Some(bahnschrift) = collection.family("Bahnschrift") else {
            return;
        };
        let fonts = bahnschrift.fonts();
        assert_eq!(fonts.len(), 1, "the named instances did not fold");
        let font = &fonts[0];
        assert!(font.has_weight_axis() && font.has_width_axis());
        let wght = font
            .axes()
            .iter()
            .find(|axis| axis.tag.to_bytes() == *b"wght")
            .expect("a weight axis");
        assert!(wght.min < wght.default && wght.default < wght.max);
    }

    #[test]
    fn reading_tables_from_disk_agrees_with_reading_the_font() {
        // Every font of every family whose name starts with S — Segoe,
        // SimSun's .ttc, Sitka's variable collection — and the big CJK
        // collections, whose format 4 cmaps outgrow their own length field,
        // read both ways.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let mut checked = 0;
        let cjk = ["Yu Gothic", "Microsoft YaHei", "Malgun Gothic", "MS Gothic"];
        for name in layer
            .names()
            .filter(|name| name.starts_with('S') || cjk.contains(name))
        {
            for font in collection.family(name).expect("listed").fonts() {
                let Source::Path(path) = font.source() else {
                    unreachable!()
                };
                let data: Arc<[u8]> = std::fs::read(path).expect("readable").into();
                let whole = Font::from_data(data, font.index());
                assert_eq!(
                    (whole.attributes(), whole.axes(), whole.charset()),
                    (font.attributes(), font.axes(), font.charset()),
                    "{} #{}",
                    path.display(),
                    font.index()
                );
                checked += 1;
            }
        }
        assert!(checked > 20, "only {checked} fonts were checked");
    }

    #[test]
    fn a_charset_agrees_with_directwrite() {
        // DirectWrite reads the same cmap, so for every character of the BMP
        // and the emoji planes the two must agree, font by font.
        use windows::Win32::Graphics::DirectWrite::IDWriteFontFamily;
        let directwrite = super::collection().expect("the system collection");
        let collection = Collection::new().with_layer(layer());
        let mut checked = 0;
        for name in [
            "Segoe UI",
            "Segoe UI Symbol",
            "Segoe UI Emoji",
            "Yu Gothic",
            "Nirmala UI",
        ] {
            let Some(family) = collection.family(name) else {
                continue;
            };
            let dw: IDWriteFontFamily = unsafe {
                let (mut index, mut exists) = (0, BOOL(0));
                directwrite
                    .FindFamilyName(&HSTRING::from(name), &mut index, &mut exists)
                    .expect("found");
                directwrite.GetFontFamily(index).expect("a family")
            };
            // DirectWrite's first font and ours are not in the same order, so
            // compare the family's union, which both must agree on.
            for c in (0..0x3_0000).filter_map(char::from_u32) {
                let ours = family.covers(c);
                let theirs = (0..unsafe { dw.GetFontCount() }).any(|at| unsafe {
                    dw.GetFont(at)
                        .and_then(|font| font.HasCharacter(u32::from(c)))
                        .is_ok_and(|has| has.as_bool())
                });
                assert_eq!(ours, theirs, "{name}: U+{:04X}", u32::from(c));
                checked += 1;
            }
        }
        assert!(checked > 0x3_0000, "only {checked} characters were checked");
    }

    #[test]
    fn every_family_the_windows_tables_name_that_directwrite_has_is_listed() {
        // One view of the system: a family the tables name for fallback and
        // DirectWrite reports installed must be one the listing found. That is
        // the layer and DirectWrite agreeing about what exists, for every name
        // the fallback backend can produce for a spread of scripts.
        let layer = layer();
        let collection = Collection::new().with_layer(layer);
        let directwrite = super::collection().expect("the system collection");
        let installed = |name: &str| unsafe {
            let (mut index, mut exists) = (0, BOOL(0));
            directwrite
                .FindFamilyName(&HSTRING::from(name), &mut index, &mut exists)
                .is_ok()
                && exists.as_bool()
        };
        let mut checked = 0;
        for tag in [
            b"Latn", b"Arab", b"Hebr", b"Deva", b"Thai", b"Hans", b"Hant", b"Jpan", b"Kore",
            b"Ethi", b"Grek", b"Cyrl",
        ] {
            let key = FallbackKey::new(&query(tag), BackendFacts::default());
            super::super::Windows::new().families(&key, &mut |name| {
                if installed(name) {
                    checked += 1;
                    assert!(
                        collection.family(name).is_some(),
                        "DirectWrite has {name:?} but the listing did not find it"
                    );
                }
            });
        }
        assert!(
            checked > 5,
            "only {checked} installed families were checked"
        );
    }

    #[test]
    fn a_localized_name_finds_the_family() {
        // Present only where the Japanese supplemental fonts are installed, so
        // this checks what it can: every alias a family has finds it.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let mut checked = 0;
        for name in layer.names() {
            let family = collection.family(name).expect("listed");
            for alias in family.aliases() {
                assert_eq!(
                    collection.family(alias).expect("an alias finds its family"),
                    family,
                    "{alias:?} should find {name}"
                );
                checked += 1;
            }
        }
        let _ = checked;
    }

    #[test]
    fn fallback_on_the_system_names_only_what_loads() {
        let collection = Collection::new().with_layer(layer());
        let families = collection.fallback(&collection.key(&query(b"Arab")));
        assert!(!families.is_empty(), "no fallback for Arabic at all");
        for family in families.iter() {
            assert_eq!(family.role(), Role::System);
            assert!(!family.fonts().is_empty(), "{} has no fonts", family.name());
        }
    }
}
