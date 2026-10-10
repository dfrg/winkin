//! The `local()` index of a layer with no loader.
//!
//! `src: local(...)` names a font by its full or PostScript name. A layer
//! built from bytes or files has no loader to ask, so it reads its fonts'
//! `name` tables once and keeps a sorted table of name hashes.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::sort;

use super::{FamilyId, Layer};
#[cfg(feature = "std")]
use crate::font::{FileFont, TABLE_LIMIT, standalone_name_table};
use crate::font::{Font, Source, local_names, name_table};

/// A layer's `local()` index and the generation it was built at.
pub(super) type LocalNames = (u64, Arc<[LocalName]>);

/// One name a font answers to for `local()`, hashed, and where the font is.
///
/// Sixteen bytes a name. A system has thousands of fonts, and the hash is
/// enough to find the few worth reading.
#[derive(Copy, Clone, Debug)]
pub(super) struct LocalName {
    hash: u64,
    family: FamilyId,
    font: u32,
}

impl Layer {
    /// Finds a font of this layer by its full or PostScript name.
    ///
    /// Builds the index on first use, and again after the layer changes.
    /// Clones share the index until one of them changes.
    pub(super) fn local_here(&self, name: &str) -> Option<Font> {
        let wanted = local_hash(name);
        let names = self.local_index();
        let at = names.partition_point(|local| local.hash < wanted);
        for local in names[at..].iter().take_while(|local| local.hash == wanted) {
            let fonts = self.fonts(local.family);
            let font = fonts.get(local.font as usize)?;
            if reads_as(font, name) {
                return Some(font.clone());
            }
        }
        None
    }

    /// Returns the `local()` index for this layer's generation, building it if
    /// the one held is missing or stale.
    ///
    /// The lock is held only to read or replace the index, never while names
    /// are read, since that loads families and opens files. Two threads
    /// building one index both build it, and the first to finish keeps
    /// theirs: both read the same fonts.
    fn local_index(&self) -> Arc<[LocalName]> {
        if let Some((generation, names)) = &*self.locals.lock()
            && *generation == self.generation
        {
            return names.clone();
        }
        let built: Arc<[LocalName]> = self.local_names().into();
        let mut locals = self.locals.lock();
        match &*locals {
            Some((generation, names)) if *generation == self.generation => names.clone(),
            _ => {
                *locals = Some((self.generation, built.clone()));
                built
            }
        }
    }

    /// Returns every name this layer's fonts answer to, hashed and sorted.
    fn local_names(&self) -> Vec<LocalName> {
        let mut names = Vec::new();
        let (mut full, mut postscript) = (String::new(), String::new());
        let mut family = FamilyId::FIRST;
        while self.holds(family) {
            for (at, font) in self.fonts(family).iter().enumerate() {
                if !read_names(font, &mut full, &mut postscript) {
                    continue;
                }
                for read in [&full, &postscript] {
                    if read.is_empty() {
                        continue;
                    }
                    names.push(LocalName {
                        hash: local_hash(read),
                        family,
                        font: at as u32,
                    });
                }
            }
            family = family.next();
        }
        sort::by_key(&mut names, |local| local.hash);
        names
    }
}

/// Hashes a name as the index keys it: lowercased, so a page's spelling and
/// the font's need not agree on case.
fn local_hash(name: &str) -> u64 {
    use core::hash::{BuildHasher, Hasher};

    let mut hasher = crate::hash::Build.build_hasher();
    for byte in name.bytes() {
        hasher.write_u8(byte.to_ascii_lowercase());
    }
    hasher.finish()
}

/// Reads the full name and PostScript name of `font` into the buffers.
///
/// Reads bytes in hand where they are. Opens a file for its `name` table
/// alone, which needs `std`.
fn read_names(font: &Font, full: &mut String, postscript: &mut String) -> bool {
    full.clear();
    postscript.clear();
    match font.source() {
        Source::Data(bytes) => name_table(bytes.data(), font.index())
            .is_some_and(|name| local_names(&name, full, postscript)),
        #[cfg(feature = "std")]
        Source::Path(path) => {
            let Some(mut file) = FileFont::opened(path) else {
                return false;
            };
            if !file.seat(font.index()) {
                return false;
            }
            let mut table = Vec::new();
            file.table_into(b"name", TABLE_LIMIT, &mut table)
                && standalone_name_table(&table)
                    .is_some_and(|name| local_names(&name, full, postscript))
        }
        Source::Pending => false,
    }
}

/// Whether `font`'s full name or PostScript name matches `name`, compared as
/// family names are.
fn reads_as(font: &Font, name: &str) -> bool {
    let (mut full, mut postscript) = (String::new(), String::new());
    read_names(font, &mut full, &mut postscript)
        && (crate::names_match(&full, name) || crate::names_match(&postscript, name))
}
