//! The system layer on Apple platforms: the fonts Core Text can reach.
//!
//! Unlike DirectWrite and fontconfig, Core Text has no call that lists
//! everything a page can name. `CTFontCollectionCreateFromAvailableFonts`
//! answers with the fonts the system considers *activated* — 183 families on
//! a stock macOS 26 — while a great many more resolve perfectly well the
//! moment a name is asked for: Times, Courier, STIXGeneral, Seravek, and the
//! ninety-odd Noto script fonts in `/System/Library/Fonts/Supplemental` are
//! all missing from the collection, all report `kCTFontEnabledAttribute` of
//! 1, and all come back with a real file when matched by family name. No
//! collection option changes that — `IncludeDisabledFonts`,
//! `RemoveDuplicates` and `DisallowAutoActivation` each list the same 183 —
//! and matching a name does not add it to later listings. Apple's own
//! private families, the ones whose names begin with a dot, are absent from
//! the collection too.
//!
//! So the names come from the font directories instead, read the way
//! [`crate::Scanned`] reads a file's family: the `name` table alone, no cmap
//! and no outlines. That is not the more expensive half of the two. A
//! name-only walk of every font on this machine takes ~16 ms against Core
//! Text's own ~11 ms for a third of the answer, and it finds 461 families
//! against 183.
//!
//! It is also not a *different* answer. Matching every font Core Text does
//! list back to its file and comparing the family Core Text filed it under
//! against the one its `name` table gives: 485 agree, none differ. Where
//! both can see a font they see the same family, so reading the files is the
//! same grouping arrived at more completely — which is what lets the two be
//! mixed at all.
//!
//! Core Text still answers for what a directory walk cannot see: fonts
//! delivered through MobileAsset or held inside a private framework, which
//! is where PingFang SC/TC/HK/MO, STHeiti and STFangsong live. Asked by
//! name, it gives their files, and the files say which of their fonts the
//! family is — see [`family_fonts`], and the twenty-eight fonts of
//! `PingFangUI.ttc` that answer is what keeps out.
//!
//! So Core Text is asked only while the layer is being built. Every listed
//! family knows where its fonts are by the time there is a layer, and
//! loading one reads its files and calls nothing. `local()` is the
//! exception, and has to be: a full name or PostScript name is not a family
//! and the listing does not index them.
//!
//! The layer's fallback source is the Core Text backend, so listing the
//! fonts and choosing fallback read one view of the system.

use alloc::boxed::Box;
use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::vec::Vec;
use std::ffi::OsStr;

use crate::sort;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use objc2_core_foundation::{
    CFArray, CFDictionary, CFNumber, CFRetained, CFSet, CFString, CFType, CFURL,
    kCFTypeSetCallBacks,
};
use objc2_core_text::{
    CTFont, CTFontCollection, CTFontCollectionCopyOptions, CTFontDescriptor, CTFontUIFontType,
    kCTFontDisplayNameAttribute, kCTFontFamilyNameAttribute, kCTFontNameAttribute,
    kCTFontURLAttribute, kCTFontVariationAttribute,
};

use crate::family::compare_names;
use crate::font::{
    FileFont, Font, TABLE_LIMIT, postscript_name, read_family_names, standalone_name_table,
};
use crate::hash::HashMap;
use crate::{Layer, LoadFamily, Role};

/// The system layer; see [`Layer::system`].
pub(crate) fn layer() -> Layer {
    let listing = listing();
    Layer::with_secondary(
        Role::System,
        listing.names,
        listing.primaries,
        Arc::new(listing.families),
    )
    .with_fallback(crate::backend::Backend::platform())
}

/// One font of a file.
#[derive(Clone, PartialEq, Eq, Debug)]
struct FontInFile {
    path: Arc<Path>,
    index: u32,
}

/// A family name as a map key, hashed and compared the way the collection
/// compares family names, so that two files spelling one family differently
/// — `Helvetica` and `helvetica` — land on one entry rather than two the
/// layer would then throw one of them away.
#[derive(Clone, Default, Debug)]
struct Name(String);

impl PartialEq for Name {
    fn eq(&self, other: &Self) -> bool {
        crate::names_match(&self.0, &other.0)
    }
}

impl Eq for Name {}

impl core::hash::Hash for Name {
    /// Lowercased character by character, which is exactly what
    /// [`compare_names`] compares, so names it makes equal hash together.
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        for c in self.0.chars().flat_map(char::to_lowercase) {
            state.write_u32(c as u32);
        }
    }
}

/// One family: its name, and where its fonts are.
#[derive(Debug)]
struct ListedFamily {
    name: Box<str>,
    fonts: Box<[FontInFile]>,
}

/// Where every listed family's fonts are: the layer's loader, for a
/// family's fonts and `local()`'s.
///
/// Worked out at listing, for the families a directory walk found and for
/// the handful only Core Text names alike, so loading one reads its files
/// and calls nothing.
///
/// A sorted slice rather than a map, because [`Layer`] hands the loader a
/// family's own name back and a binary search by [`compare_names`] finds it
/// without hashing it and without lowercasing it into a buffer — which is
/// what the layer's own name index does, and for the same reason. The walk
/// that builds it uses a hash map instead: that one is all lookups and
/// inserts and reads nothing in order.
#[derive(Debug)]
struct ListedFamilies(Box<[ListedFamily]>);

impl ListedFamilies {
    /// Where the fonts of the family called `name` are; empty for a name
    /// this does not have.
    fn fonts(&self, name: &str) -> &[FontInFile] {
        match self
            .0
            .binary_search_by(|family| compare_names(&family.name, name))
        {
            Ok(at) => &self.0[at].fonts,
            Err(_) => &[],
        }
    }
}

/// What [`listing`] works out for [`Layer::with_secondary`].
struct Listing {
    /// Every name to list, each with its aliases, primaries first. A vector
    /// of pairs because that is what `with_secondary` takes.
    names: Vec<(String, Vec<String>)>,
    /// How many of `names` are a family in their own right; the rest are
    /// secondary.
    primaries: usize,
    /// Where each of those names' fonts are.
    families: ListedFamilies,
}

/// The buffers a walk reuses from one font to the next.
///
/// A directory walk reads one `name` table per font — some nine hundred of
/// them on a stock macOS — and every one of them wants a table, a family
/// name, a legacy name and a list of other-language names. Held here, that
/// is four allocations for the whole walk rather than four per font.
#[derive(Default)]
struct Scratch {
    /// The `name` table of the font being read.
    table: Vec<u8>,
    /// What it calls its family.
    own: String,
    /// The legacy family name beside it, empty where it has none.
    legacy: String,
    /// The family's name in the font's other languages.
    others: Vec<String>,
    /// A key to look a family up with, so that a family already seen costs
    /// nothing to find.
    probe: Name,
}

impl Scratch {
    /// Reads what the font seated in `file` calls itself, into the buffers.
    fn read(&mut self, file: &mut FileFont) -> bool {
        file.table_into(b"name", TABLE_LIMIT, &mut self.table)
            && standalone_name_table(&self.table).is_some_and(|name| {
                read_family_names(&name, &mut self.own, &mut self.legacy, &mut self.others)
            })
    }

    /// `name` as a key, without allocating one.
    fn key(&mut self, name: &str) -> &Name {
        self.probe.0.clear();
        self.probe.0.push_str(name);
        &self.probe
    }
}

/// A walk of the font directories: what it has read, and the buffers it
/// reuses while reading.
#[derive(Default)]
struct Walk {
    /// Where each family's fonts are, by the name its fonts give it.
    families: HashMap<Name, Vec<FontInFile>>,
    /// Where each *legacy* family name's fonts are.
    legacy: HashMap<Name, Vec<FontInFile>>,
    /// The other names each family writes itself under.
    aliases: HashMap<Name, Vec<String>>,
    scratch: Scratch,
}

impl Walk {
    /// Files `font` under every name the scratch says it has.
    fn add(&mut self, font: FontInFile) {
        let Scratch {
            own,
            legacy,
            others,
            probe,
            ..
        } = &mut self.scratch;
        // Fonts of one family can write different sets of other-language
        // names, so every font filed under one name contributes.
        if !others.is_empty() {
            let held = entry(&mut self.aliases, probe, own);
            for other in others.drain(..) {
                if !held.iter().any(|seen| crate::names_match(seen, &other)) {
                    held.push(other);
                }
            }
        }
        entry(&mut self.families, probe, own).push(font.clone());
        if !legacy.is_empty() {
            entry(&mut self.legacy, probe, legacy).push(font);
        }
    }
}

/// The entry for `name`, made if it is not there.
///
/// `probe` rather than `HashMap::entry`, which would need an owned key and
/// so a `String` per font even for a family already seen — five hundred of
/// them wasted on a stock macOS. Only a family's first font allocates its
/// name.
fn entry<'a, V: Default>(map: &'a mut HashMap<Name, V>, probe: &mut Name, name: &str) -> &'a mut V {
    probe.0.clear();
    probe.0.push_str(name);
    // Two lookups where the family is already there, which is the common
    // case, and three where it is not. `entry` would be one either way and
    // allocate the key every time.
    if !map.contains_key(probe) {
        map.insert(probe.clone(), V::default());
    }
    map.get_mut(probe).expect("just inserted")
}

/// Every family name a page may ask for, with how many of them are a family
/// in their own right; the rest are secondary.
///
/// Sorted, both halves: the order they are listed in is the layer's, and a
/// directory walk arrives in whatever order the file system answers in.
fn listing() -> Listing {
    let mut walk = scanned();

    // What a directory walk cannot see: a font delivered on demand, or one
    // inside a framework. Core Text names the file; which fonts of it are
    // the family comes from the file, the same as for every other family.
    for name in listed() {
        let name = Name(name);
        if walk.families.contains_key(&name) {
            continue;
        }
        let mut aliases = Vec::new();
        let fonts = family_fonts(&name.0, &mut walk.scratch, &mut aliases);
        // A name resolving to nothing is not a family. Core Text answers for
        // `.AppleSystemUIFont` with descriptors that carry no file at all,
        // and that one is an alias below rather than a family of its own.
        if fonts.is_empty() {
            continue;
        }
        if !aliases.is_empty() {
            walk.aliases
                .entry(name.clone())
                .or_default()
                .extend(aliases);
        }
        walk.families.insert(name, fonts);
    }

    // The system font has a family of its own — `.AppleSystemUIFont` — that
    // Core Text answers to and no file declares, since the file behind it
    // calls itself `.SF NS`. The backend names it for `system-ui` and as a
    // last resort, so it has to resolve. An alias of whichever family holds
    // its file, asked of Core Text rather than written down, so it follows
    // the system font wherever Apple moves it.
    if let Some((name, path)) = system_ui_font()
        && let Some((family, _)) = walk
            .families
            .iter()
            .find(|(_, fonts)| fonts.iter().any(|font| *font.path == *path))
    {
        let family = family.clone();
        walk.aliases.entry(family).or_default().push(name);
    }

    // A font's legacy family name is a family of its own where nothing else
    // claims that name: "Helvetica Neue Condensed Black" beside "Helvetica
    // Neue", which a page asks for and Core Text answers with those fonts
    // alone. Secondary, since its fonts are already a listed family's.
    walk.legacy
        .retain(|name, _| !walk.families.contains_key(name));

    let Walk {
        families: primary,
        legacy,
        mut aliases,
        mut scratch,
    } = walk;
    // A name that is another family's own is that family, not an alias of
    // this one.
    for others in aliases.values_mut() {
        others.retain(|other| !primary.contains_key(scratch.key(other)));
    }

    let primaries = primary.len();
    let mut names = Vec::with_capacity(primaries + legacy.len());
    let mut families = Vec::with_capacity(names.capacity());
    for (source, secondary) in [(primary, false), (legacy, true)] {
        let first = families.len();
        for (name, fonts) in source {
            // A secondary family is another view of a family already
            // listed, and answers to that one name alone.
            let others = match secondary {
                true => Vec::new(),
                false => aliases.remove(&name).unwrap_or_default(),
            };
            names.push((name.0.clone(), others));
            families.push(ListedFamily {
                name: name.0.into_boxed_str(),
                fonts: fonts.into_boxed_slice(),
            });
        }
        // Sorted within each half, so that two runs list the same order
        // however the file system answered.
        sort::by(&mut names[first..], |a, b| {
            compare_names(&a.0, &b.0).then_with(|| a.0.cmp(&b.0))
        });
    }
    // `names` keeps its two halves, primaries first, which marks the
    // secondary ones. `ListedFamilies` is searched, not walked, so it sorts
    // whole. The two halves share no name, so the order is certain.
    sort::by(&mut families, |a, b| compare_names(&a.name, &b.name));
    Listing {
        names,
        primaries,
        families: ListedFamilies(families.into_boxed_slice()),
    }
}

/// Every family in the font directories, by the name its fonts' `name`
/// tables give, and separately by the legacy name they give beside it.
///
/// Reads one table per font and nothing else: what is wanted here is the
/// name, and the file around it can be twenty megabytes of outlines. One
/// open per *file*, not per font — `PingFangUI.ttc` alone holds thirty-two
/// — and one set of buffers for the whole walk.
fn scanned() -> Walk {
    let mut walk = Walk::default();
    let mut directories = directories();
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `file_type` rather than `path.is_dir`, which follows a link and
            // could walk a directory twice, or forever.
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    directories.push(path);
                    continue;
                }
                Ok(_) => {}
                Err(_) => continue,
            }
            if !is_font(&path) {
                continue;
            }
            let Some(mut file) = FileFont::opened(&path) else {
                continue;
            };
            let path: Arc<Path> = Arc::from(path);
            for index in 0..file.fonts() {
                if file.seat(index) && walk.scratch.read(&mut file) {
                    walk.add(FontInFile {
                        path: path.clone(),
                        index,
                    });
                }
            }
        }
    }
    walk
}

/// The fonts of a family no font directory holds, through Core Text.
///
/// Core Text names each font's file and never its place in the file, so the
/// file says that: the fonts of `family` in it are the ones whose own `name`
/// table files them under that family, which is the same question the
/// directory walk answers and the same answer.
///
/// Reading the file rather than trusting the descriptor matters here.
/// `PingFangUI.ttc` holds thirty-two fonts, of which Core Text exposes four
/// — PingFang SC, TC, HK and MO. The other twenty-eight are Apple's internal
/// UI and watchOS cuts, `.PingFang UI Watch SC` and the like, which nothing
/// should ever name. fontique takes the URLs from the same descriptors and
/// scans the whole file, so it lists all thirty-two; filing each font under
/// the family its own `name` table gives keeps the four Core Text meant.
///
/// The family's names in the font's other languages go into `aliases`, as
/// the directory walk gathers them for every other family. Without that,
/// PingFang SC is reachable by that name and not as 蘋方-簡, which is what
/// Chrome reports having drawn with.
fn family_fonts(family: &str, scratch: &mut Scratch, aliases: &mut Vec<String>) -> Vec<FontInFile> {
    let mut fonts: Vec<FontInFile> = Vec::new();
    for matched in matching(unsafe { kCTFontFamilyNameAttribute }, family) {
        // One file however many of its fonts matched: it is walked once.
        if fonts.iter().any(|font| font.path == matched.path) {
            continue;
        }
        let Some(mut file) = FileFont::opened(&matched.path) else {
            continue;
        };
        for index in 0..file.fonts() {
            if !file.seat(index)
                || !scratch.read(&mut file)
                || !crate::names_match(&scratch.own, family)
            {
                continue;
            }
            for other in scratch.others.drain(..) {
                if !aliases.iter().any(|seen| crate::names_match(seen, &other)) {
                    aliases.push(other);
                }
            }
            fonts.push(FontInFile {
                path: matched.path.clone(),
                index,
            });
        }
    }
    fonts
}

/// Whether `path` is named like a font file.
fn is_font(path: &Path) -> bool {
    let Some(extension) = path.extension().and_then(OsStr::to_str) else {
        return false;
    };
    ["ttf", "otf", "ttc", "otc"]
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

/// Where macOS keeps fonts: the system's, the machine's, and this user's.
///
/// `/System/Library/Fonts` holds `Supplemental` under it, which the walk
/// reaches on its own. `/Network/Library/Fonts` is left out: it is a mount
/// that may not answer, and a directory walk that blocks on a server is
/// worse than a font not listed.
fn directories() -> Vec<PathBuf> {
    let mut directories = alloc::vec![
        PathBuf::from("/System/Library/Fonts"),
        PathBuf::from("/Library/Fonts"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        directories.push(Path::new(&home).join("Library/Fonts"));
    }
    directories
}

/// Every family name Core Text lists as available.
///
/// Incomplete — see this module's own documentation — but it is the only
/// place a font outside the font directories is named at all.
fn listed() -> Vec<String> {
    unsafe {
        let collection = CTFontCollection::from_available_fonts(None);
        let names = collection.font_attribute(
            kCTFontFamilyNameAttribute,
            CTFontCollectionCopyOptions::Unique,
        );
        let names: CFRetained<CFArray<CFType>> = CFRetained::cast_unchecked(names);
        names
            .iter()
            .filter_map(|name| Some(name.downcast_ref::<CFString>()?.to_string()))
            .collect()
    }
}

/// The family Core Text files the system font under, and the file behind it.
fn system_ui_font() -> Option<(String, Arc<Path>)> {
    unsafe {
        let font = CTFont::new_ui_font_for_language(CTFontUIFontType::System, 12.0, None)?;
        let descriptor = font.font_descriptor();
        let name = string_attribute(&descriptor, kCTFontFamilyNameAttribute)?;
        Some((name, descriptor_path(&descriptor)?))
    }
}

impl LoadFamily for ListedFamilies {
    /// Every font the listing filed under `name`, read from its file.
    ///
    /// A variable font is one font here however many named instances Core
    /// Text lists of it — PingFang SC is six descriptors of one font, Skia
    /// ten — because the listing counts the fonts of a file rather than
    /// descriptors, so the instances have nowhere to fold in from. Its axes
    /// give its range, as they do on the other two backends.
    fn load(&self, name: &str) -> Vec<Font> {
        self.fonts(name)
            .iter()
            .filter_map(|font| Font::from_path(font.path.clone(), font.index))
            .collect()
    }

    fn local(&self, name: &str) -> Option<Font> {
        // The PostScript name, then the full name, as CSS says `local()`
        // matches. Either may be a named instance's — "Skia Bold" is a point
        // on Skia's weight axis — and Core Text says where in the font that
        // is, in the variation dictionary it matched with. Asked for here,
        // where the descriptor is already in hand, and kept nowhere.
        let matched = unsafe { [kCTFontNameAttribute, kCTFontDisplayNameAttribute] }
            .into_iter()
            .find_map(|attribute| matching(attribute, name).into_iter().next())?;
        let index = self.matched_index(&matched)?;
        let mut font = Font::from_path(matched.path, index)?;
        // Only where the name is an instance's: a name that is the font's
        // own leaves it whole, with every axis it varies along, which is
        // what a rule declaring a range over `local()` needs.
        if font.is_instance(&matched.variation) {
            font.pin(&matched.variation);
        }
        Some(font)
    }
}

impl ListedFamilies {
    /// Which font of its file `matched` is.
    ///
    /// Core Text gives a font's file and never its index, so this is asked
    /// of what the listing already worked out: the fonts of the matched
    /// font's family that live in that file. Usually one — a `.ttc` holding
    /// several families gives each of them one font — and then there is
    /// nothing to choose and no file to open.
    ///
    /// Where a family has several fonts in one file, as Helvetica does, the
    /// PostScript name says which. A named instance's PostScript name is in
    /// `fvar` and not in `name`, so it matches none of them; the instance
    /// must then be a point of whichever of the candidates varies at all,
    /// and only if exactly one does.
    fn matched_index(&self, matched: &Matched) -> Option<u32> {
        let family = self.fonts(matched.family.as_deref()?);
        let here = || family.iter().filter(|font| font.path == matched.path);
        let mut found = here().map(|font| font.index);
        let first = found.next()?;
        if found.next().is_none() {
            return Some(first);
        }

        let mut file = FileFont::opened(&matched.path)?;
        let mut table = Vec::new();
        if let Some(postscript) = matched.postscript.as_deref() {
            let mut read = String::new();
            for font in here() {
                if file.seat(font.index)
                    && file.table_into(b"name", TABLE_LIMIT, &mut table)
                    && standalone_name_table(&table)
                        .is_some_and(|name| postscript_name(&name, &mut read))
                    && read == postscript
                {
                    return Some(font.index);
                }
            }
        }
        let mut varying = None;
        for font in here() {
            if file.seat(font.index)
                && file.locate(b"fvar").is_some()
                && varying.replace(font.index).is_some()
            {
                // More than one varies, so nothing says which the instance
                // is a point of.
                return None;
            }
        }
        varying
    }
}

/// A font Core Text matched: the file behind it, the name that says which
/// font of that file it is, and where in its design space the match sits.
struct Matched {
    path: Arc<Path>,
    /// What Core Text files it under, which says where in the listing to
    /// look for the rest of its family's fonts in the same file.
    family: Option<String>,
    postscript: Option<String>,
    variation: Vec<([u8; 4], f32)>,
}

/// Every font whose `attribute` is `value`, those with no file on this
/// machine passed over.
///
fn matching(attribute: &CFString, value: &str) -> Vec<Matched> {
    unsafe {
        let value = CFString::from_str(value);
        let attributes: CFRetained<CFDictionary<CFString, CFType>> =
            CFDictionary::from_slices(&[attribute], &[&*value as &CFType]);
        let attributes = CFRetained::cast_unchecked::<CFDictionary>(attributes);
        let descriptor = CTFontDescriptor::with_attributes(&attributes);
        // The attribute is mandatory, so a font that does not have it is not
        // a match: `CTFontDescriptorCreateMatchingFontDescriptors` with
        // nothing mandatory is free to answer with a substitute.
        let mut values = [attribute as *const CFString as *const core::ffi::c_void];
        let Some(mandatory) = CFSet::new(
            None,
            values.as_mut_ptr(),
            values.len() as isize,
            &kCFTypeSetCallBacks,
        ) else {
            return Vec::new();
        };
        let Some(found) = descriptor.matching_font_descriptors(Some(&mandatory)) else {
            return Vec::new();
        };
        let found: CFRetained<CFArray<CTFontDescriptor>> = CFRetained::cast_unchecked(found);
        found
            .iter()
            .filter_map(|descriptor| {
                Some(Matched {
                    path: descriptor_path(&descriptor)?,
                    family: string_attribute(&descriptor, kCTFontFamilyNameAttribute),
                    postscript: string_attribute(&descriptor, kCTFontNameAttribute),
                    variation: descriptor_variation(&descriptor),
                })
            })
            .collect()
    }
}

/// The file behind `descriptor`, if it is a file on this machine.
///
/// A font with no URL is one no reader outside Core Text can open —
/// `.AppleSystemUIFont` is one, a family made of routing rather than of
/// glyphs — and is left out rather than listed as a name resolving to
/// nothing.
fn descriptor_path(descriptor: &CTFontDescriptor) -> Option<Arc<Path>> {
    unsafe {
        let url = descriptor.attribute(kCTFontURLAttribute)?;
        let url = url.downcast::<CFURL>().ok()?;
        // The file system representation, not the path string: a path on
        // Unix need not be valid UTF-8, and going through a `String` would
        // replace what it could not decode and name a file that is not
        // there. `PATH_MAX` on Darwin, so a path this does not fit is one
        // the kernel would not open either.
        let mut buffer = [0u8; 1024];
        if !url.file_system_representation(true, buffer.as_mut_ptr(), buffer.len() as isize) {
            return None;
        }
        let end = buffer.iter().position(|&byte| byte == 0)?;
        Some(Arc::from(Path::new(OsStr::from_bytes(&buffer[..end]))))
    }
}

/// `attribute` of `descriptor`, as a string.
fn string_attribute(descriptor: &CTFontDescriptor, attribute: &CFString) -> Option<String> {
    unsafe {
        let value = descriptor.attribute(attribute)?;
        Some(value.downcast::<CFString>().ok()?.to_string())
    }
}

/// Where `descriptor` sits in its font's design space: each axis the match
/// holds and its value there. Empty for a font that does not vary.
///
/// Core Text keys the dictionary by the axis's four-character tag packed
/// into an integer, which is what `fvar` holds and what [`Font::axis`] is
/// asked for.
fn descriptor_variation(descriptor: &CTFontDescriptor) -> Vec<([u8; 4], f32)> {
    unsafe {
        let Some(value) = descriptor.attribute(kCTFontVariationAttribute) else {
            return Vec::new();
        };
        let Ok(variation) = value.downcast::<CFDictionary>() else {
            return Vec::new();
        };
        let variation: CFRetained<CFDictionary<CFNumber, CFNumber>> =
            CFRetained::cast_unchecked(variation);
        let (tags, values) = variation.to_vecs();
        tags.into_iter()
            .zip(values)
            .filter_map(|(tag, value)| {
                Some(((tag.as_i64()? as u32).to_be_bytes(), value.as_f64()? as f32))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback::{BackendFacts, FallbackKey};
    use crate::{Collection, FallbackRequest, GenericClass, Source};
    use parlance::Script;

    /// The system layer. Nothing here skips on a family being absent, the
    /// way the fontconfig tests have to: every family these name is bundled
    /// with macOS rather than an optional install, so an absent one is a
    /// real failure and should say so. A test that returns early prints
    /// `ok`, and one of these spent a while doing exactly that.
    fn layer() -> Arc<Layer> {
        Arc::new(super::layer())
    }

    fn query(tag: &[u8; 4], lang: Option<&'static str>) -> FallbackRequest {
        FallbackRequest::Text {
            script: Script::from_bytes(*tag),
            language: lang.and_then(crate::parse_language),
            generic: GenericClass::Plain,
        }
    }

    #[test]
    fn listing_the_system_reads_no_font() {
        // Every Mac has fonts, so unlike the fontconfig tests nothing here
        // skips on an empty list: an empty layer is a failure.
        let layer = layer();
        assert!(layer.len() > 10, "listed {} families", layer.len());
        assert_eq!(layer.loaded(), 0, "building the layer loaded a family");
    }

    #[test]
    fn helvetica_loads_its_own_files() {
        // Helvetica has shipped with every Mac OS X.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let helvetica = collection.family("helvetica").expect("installed");
        assert_eq!(helvetica.name(), "Helvetica");
        assert!(!helvetica.is_loaded());
        let fonts = helvetica.fonts();
        assert!(!fonts.is_empty(), "Helvetica loaded no fonts");
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
    fn every_listed_family_has_fonts_that_load() {
        // The invariant a name set has to keep: a family listed by name
        // resolves to fonts. A name that loads nothing is a name the
        // collection would answer to and then hand back nothing — the bug
        // `tests/directwrite.rs` caught on Windows.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let mut empty = Vec::new();
        for name in layer.names() {
            let family = collection.family(name).expect("listed");
            if family.fonts().is_empty() {
                empty.push(String::from(name));
            }
        }
        assert!(empty.is_empty(), "families listed with no fonts: {empty:?}");
    }

    #[test]
    fn every_font_is_a_font_once_and_reads_the_same_either_way() {
        // Every family on the system, loaded. Core Text lists a variable
        // font once per named instance -- PingFang SC is six descriptors of
        // one font -- so those must have folded into it, leaving each
        // (file, index) once. And the tables read from a file's directory
        // must say what the whole font says.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let (mut checked, mut variable) = (0, 0);
        for name in layer.names() {
            let family = collection.family(name).expect("listed");
            let fonts = family.fonts();
            for (at, font) in fonts.iter().enumerate() {
                let Source::Path(path) = font.source() else {
                    unreachable!()
                };
                assert!(
                    !fonts[..at].iter().any(|other| {
                        matches!(other.source(), Source::Path(other) if other == path)
                            && other.index() == font.index()
                    }),
                    "{name}: {} #{} twice",
                    path.display(),
                    font.index()
                );
                let Ok(data) = std::fs::read(path) else {
                    // A font inside a sealed volume or delivered on demand
                    // may be readable by its table directory and not as a
                    // whole; that is Core Text's business, not a failure
                    // here.
                    continue;
                };
                let whole = Font::from_data(Arc::<[u8]>::from(data), font.index());
                assert_eq!(
                    (whole.attributes(), whole.axes(), whole.charset()),
                    (font.attributes(), font.axes(), font.charset()),
                    "{} #{}",
                    path.display(),
                    font.index()
                );
                checked += 1;
                variable += usize::from(!font.axes().is_empty());
            }
        }
        std::eprintln!("{checked} fonts checked, {variable} variable");
        assert!(checked >= layer.len());
    }

    #[test]
    fn every_family_core_text_falls_back_to_is_listed() {
        // The point of the layer owning the backend: one view of the system.
        // Every family the Core Text backend names must be one the listing
        // found. The static tail is included: on macOS those families are
        // bundled with the OS rather than optional installs, which is the
        // same assumption `tests/coretext.rs` makes.
        let layer = layer();
        let collection = Collection::new().with_layer(layer);
        let source = super::super::MacOs::new();
        let mut missing = Vec::new();
        for (tag, lang) in [
            (b"Latn", Some("en-US")),
            (b"Arab", Some("ar")),
            (b"Deva", Some("hi-IN")),
            (b"Hani", Some("ja-JP")),
            (b"Hani", Some("zh-Hans")),
            (b"Thai", None),
            (b"Hebr", None),
        ] {
            let key = FallbackKey::new(&query(tag, lang), BackendFacts::default());
            source.families(&key, &mut |name| {
                if collection.family(name).is_none() {
                    missing.push(String::from(name));
                }
            });
        }
        missing.dedup();
        assert!(
            missing.is_empty(),
            "Core Text falls back to families the listing does not have: {missing:?}"
        );
    }

    #[test]
    fn a_family_chain_on_the_system_names_only_what_loads() {
        let layer = layer();
        let collection = Collection::new().with_layer(layer);
        let families = collection.fallback(&collection.key(&query(b"Latn", Some("en-US"))));
        assert!(!families.is_empty(), "no fallback for Latin at all");
        for family in families.iter() {
            assert_eq!(family.role(), Role::System);
            assert!(!family.fonts().is_empty(), "{} has no fonts", family.name());
        }
    }

    #[test]
    fn the_system_ui_family_resolves() {
        // `.AppleSystemUIFont` is what the backend names for `system-ui` and
        // as its last resort, and it is a family Core Text routes rather
        // than one any file declares: no descriptor of it has a URL. It has
        // to reach the system font's own file all the same.
        let layer = layer();
        let collection = Collection::new().with_layer(layer);
        let family = collection
            .family(".AppleSystemUIFont")
            .expect("the system font's family");
        let fonts = family.fonts();
        assert!(!fonts.is_empty(), "{} has no fonts", family.name());
        let (_, expected) = system_ui_font().expect("Core Text has a system font");
        assert!(
            fonts
                .iter()
                .any(|font| matches!(font.source(), Source::Path(path) if **path == *expected)),
            "{} does not hold {}",
            family.name(),
            expected.display()
        );
    }

    #[test]
    fn families_only_core_text_knows_are_listed_and_load() {
        // A font delivered through MobileAsset or held inside a private
        // framework is in no font directory, so the directory walk cannot
        // see it and the collection is the only thing that names it. PingFang
        // is the one the backend cares about: it is what `system-ui` and
        // Chinese sans-serif resolve to for a caller that rasterizes through
        // Core Text.
        let layer = layer();
        let collection = Collection::new().with_layer(layer);
        let family = collection.family("PingFang SC").expect("bundled");
        let fonts = family.fonts();
        assert!(!fonts.is_empty(), "PingFang SC loaded no fonts");
        // Six named instances of one font, and one font of the file.
        assert_eq!(fonts.len(), 1, "PingFang SC's instances did not fold");
        assert!(!fonts[0].axes().is_empty(), "PingFang SC does not vary");
        let Source::Path(path) = fonts[0].source() else {
            panic!("named by path")
        };
        assert!(
            !directories().iter().any(|dir| path.starts_with(dir)),
            "{} is in a font directory after all",
            path.display()
        );
    }

    #[test]
    fn a_family_only_core_text_knows_still_answers_to_its_other_names() {
        // Gathering a family's names in the font's other languages is the
        // directory walk's job, and the families Core Text names instead
        // went without: PingFang SC was reachable by that name and not as
        // 蘋方-簡, which is the name Chrome reports having drawn with. Found
        // by the browser comparison, which could not tell the two apart.
        let layer = layer();
        let collection = Collection::new().with_layer(layer);
        let family = collection.family("PingFang SC").expect("bundled");
        assert!(
            family.aliases().next().is_some(),
            "PingFang SC has no other names"
        );
        for alias in ["蘋方-簡", "PingFang SC"] {
            let found = collection.family(alias).expect(alias);
            assert_eq!(found.name(), family.name(), "{alias} found another family");
        }
    }

    #[test]
    fn a_files_other_fonts_are_not_listed_because_it_was_opened() {
        // `PingFangUI.ttc` holds thirty-two fonts and Core Text exposes four
        // of them. The rest are Apple's internal UI and watchOS cuts, whose
        // `name` tables file them under `.PingFang UI Watch SC` and the
        // like. Opening the file to find PingFang SC must not drag those in
        // -- fontique scans the whole file and lists all thirty-two.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let family = collection.family("PingFang SC").expect("bundled");
        let Source::Path(shared) = family.fonts()[0].source() else {
            panic!("named by path")
        };
        let shared = shared.clone();
        let from_that_file: Vec<&str> = layer
            .names()
            .filter(|name| {
                collection.family(name).is_some_and(|family| {
                    family
                        .fonts()
                        .iter()
                        .any(|font| matches!(font.source(), Source::Path(path) if *path == shared))
                })
            })
            .collect();
        assert_eq!(
            from_that_file,
            ["PingFang HK", "PingFang MO", "PingFang SC", "PingFang TC"],
            "{} was listed as more than the families Core Text names",
            shared.display()
        );
    }

    #[test]
    fn a_legacy_family_name_finds_that_font_alone() {
        // A Core Text family holds every style, so a font whose own family
        // record names it apart -- "Avenir Black" -- is a family of its own
        // that a page asks for, and it must answer with that font rather
        // than with all of Avenir. Chrome resolves this one too.
        let layer = layer();
        let collection = Collection::new().with_layer(layer.clone());
        let family = collection
            .family("Avenir Black")
            .expect("Avenir ships with macOS");
        let fonts = family.fonts();
        assert_eq!(fonts.len(), 1, "a legacy name should name one font");
        let whole = collection.family("Avenir").expect("installed");
        assert!(
            whole.fonts().len() > fonts.len(),
            "the family should hold more than the one font"
        );
        let at = layer.find("Avenir Black").expect("listed");
        assert!(
            layer.is_secondary(at),
            "a legacy name is a secondary family"
        );
    }

    #[test]
    fn local_finds_a_font_by_its_postscript_and_full_names() {
        let layer = layer();
        for name in ["Helvetica-Bold", "Helvetica Bold"] {
            let font = layer.local(name).unwrap_or_else(|| panic!("{name}"));
            let Source::Path(path) = font.source() else {
                panic!("named by path")
            };
            assert!(path.exists(), "{}", path.display());
            assert_eq!(font.attributes().weight, crate::FontWeight::BOLD, "{name}");
        }
        assert!(layer.local("Not A Font At All").is_none());
    }

    #[test]
    fn local_holds_a_named_instance_where_its_name_points() {
        // Skia is one variable font whose named instances Core Text lists as
        // ten fonts of the family. `local()` names one point of it, and the
        // font comes back with that axis narrowed to that point, while a
        // name that is the font's own leaves every axis its full range.
        let layer = layer();
        let whole = layer
            .local("Skia-Regular")
            .expect("Skia ships with macOS, and varies");
        let weight = whole.axis(b"wght").expect("Skia varies by weight");
        assert!(weight.min < weight.max, "Skia's weight axis is a point");
        let bold = layer.local("Skia Bold").expect("a named instance");
        let pinned = bold.axis(b"wght").expect("the same axis");
        assert_eq!(pinned.min, pinned.max, "the instance was not pinned");
        assert!(
            pinned.min > weight.default,
            "Skia Bold should be heavier than the default"
        );
        // Skia is one of Apple's pre-OpenType GX fonts, where `wght` and
        // `wdth` are multipliers -- 0.48 to 3.2, and 0.62 to 1.3 -- rather
        // than a weight class and a percentage. Held at the point, but the
        // attributes stay the font's own: read as CSS values they would
        // make Skia Bold weight 1.9 and Skia Condensed 0.6% of normal.
        assert!(
            (100.0..=1000.0).contains(&bold.attributes().weight.value()),
            "Skia Bold came back at weight {}",
            bold.attributes().weight.value()
        );
        let condensed = layer.local("Skia Condensed").expect("a named instance");
        let width = condensed.axis(b"wdth").expect("Skia varies by width");
        assert_eq!(width.min, width.max, "the instance was not pinned");
        assert!(
            (0.5..=2.0).contains(&condensed.attributes().width.ratio()),
            "Skia Condensed came back at width {}",
            condensed.attributes().width.ratio()
        );
    }

    #[test]
    fn a_name_keys_by_the_same_rule_the_collection_compares_by() {
        // A `Hash` that disagreed with the `Eq` would file two spellings of
        // one family in different buckets, and the walk would keep only
        // whichever the layer's own index then threw away. Silent, and
        // exactly the bug the key exists to prevent.
        use core::hash::BuildHasher;
        let hash = |name: &str| crate::hash::Build.hash_one(Name(String::from(name)));
        for (a, b) in [
            ("Helvetica", "helvetica"),
            ("Helvetica", "HELVETICA"),
            ("Apple SD Gothic Neo", "apple sd gothic neo"),
            ("ヒラギノ角ゴシック", "ヒラギノ角ゴシック"),
            ("Ångström", "ångström"),
            ("ÉCOLE", "école"),
        ] {
            assert!(crate::names_match(a, b), "{a:?} and {b:?} name one family");
            assert_eq!(Name(String::from(a)), Name(String::from(b)));
            assert_eq!(hash(a), hash(b), "{a:?} and {b:?} hash apart");
        }
        for (a, b) in [("Helvetica", "Helvetica Neue"), ("Arial", "Ariel")] {
            assert_ne!(Name(String::from(a)), Name(String::from(b)));
        }
    }

    #[test]
    fn a_name_the_system_does_not_have_is_absent() {
        let layer = layer();
        let collection = Collection::new().with_layer(layer);
        assert!(collection.family("No Such Family Exists Here").is_none());
    }
}
