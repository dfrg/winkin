//! Faces several contexts share, so that each font is read once between them.
//!
//! A context's own table of faces stays the first place a face is looked
//! for, without a lock. Only a face the context lacks locks the shared
//! table, which hands back a clone of its font or makes the font under the
//! lock. A font held whole carries what harfrust reads of it lazily, so
//! every context sharing the face shares that too.

use alloc::sync::Arc;
use core::fmt;

use fontwich::{Collection, Family, FontBytes, FontKey};
use read_fonts::model::{Blob, Font};

use crate::data::{HeapBytes, LruCache, Mutex, define_id};

/// Returns the bytes of the face of `font`, of `family`, and the font held
/// whole at its default coordinates.
///
/// `hash` is the hash of `key`, the font key. Reads them from `shared`
/// where given, and otherwise loads them. Returns `None` for a font whose
/// bytes are not here or are not a font read-fonts reads.
pub(super) fn face_font(
    shared: Option<&SharedFonts>,
    family: &Family,
    font: &fontwich::Font,
    key: FontKey,
    hash: u64,
) -> Option<(FontBytes, Font)> {
    match shared {
        Some(shared) => shared.find_or_make(family, font, key, hash),
        None => load(font),
    }
}

/// Loads `font`'s bytes and holds the font whole at its default
/// coordinates.
fn load(font: &fontwich::Font) -> Option<(FontBytes, Font)> {
    let bytes = font.load()?;
    let held = Font::new(Blob::Shared(bytes.arc().clone()), font.index())?;
    Some((bytes, held))
}

/// Parsed fonts that several [`Context`](crate::Context)s share.
///
/// A font is parsed once for every context created with a clone of the same
/// `SharedFonts` by [`Context::with_shared_fonts`](crate::Context::with_shared_fonts),
/// and the tables the shaper reads from it lazily are shared too. The set
/// holds each font's bytes, so the files are not read again while it holds
/// them. Layout results are the same as with private contexts.
///
/// Clones share one set, which a lock guards: a blocking lock with the
/// `std` feature, a spin lock without it. Contexts lock it only for a font
/// they do not hold themselves, so contexts on several threads may share one
/// set. The set keeps at most the number of fonts it is created
/// with, dropping those asked for longest ago.
///
/// ```
/// use winkin::{Context, SharedFonts};
/// # use fontwich::Collection;
///
/// let shared = SharedFonts::default();
/// let first = Context::with_shared_fonts(Collection::new(), shared.clone());
/// let second = Context::with_shared_fonts(Collection::new(), shared);
/// ```
#[derive(Clone)]
pub struct SharedFonts {
    faces: Arc<Mutex<SharedFaces>>,
}

define_id! {
    /// Names a face in the shared table of faces.
    struct SharedFaceId(u32);
}

/// The shared table of faces, behind the lock.
struct SharedFaces {
    table: LruCache<SharedFaceId, SharedFace>,
    /// How many faces were made, for tests to see a font read once.
    #[cfg(test)]
    made: usize,
}

/// A font held whole at its default coordinates, with its bytes.
struct SharedFace {
    /// fontwich's key for the font, which the face is found by.
    key: FontKey,
    /// The font's family, by which a collection says whether it has the
    /// font.
    family: Family,
    /// The font's bytes, which the set keeps loaded.
    bytes: FontBytes,
    /// The font held whole at its default coordinates.
    font: Font,
}

impl SharedFonts {
    /// Creates an empty set that keeps at most `fonts` fonts.
    ///
    /// A limit of zero keeps nothing, so every context parses its fonts
    /// itself.
    pub fn new(fonts: usize) -> Self {
        Self {
            faces: Arc::new(Mutex::new(SharedFaces {
                table: LruCache::new(fonts),
                #[cfg(test)]
                made: 0,
            })),
        }
    }

    /// Drops the fonts that `fonts` does not contain, releasing their bytes
    /// once no context or layout uses them.
    ///
    /// Call it after removing a web font from every collection the sharing
    /// contexts use. [`Context::set_collection`](crate::Context::set_collection)
    /// does not change the set.
    pub fn retain(&self, fonts: &Collection) {
        self.faces
            .lock()
            .table
            .retain(|_, face| fonts.contains(&face.family), |_, _| {});
    }

    /// Returns the estimated heap usage of the set, in bytes.
    ///
    /// Counts the table of fonts. Excludes the font bytes, which the
    /// collections share, and what read-fonts keeps inside a parsed font.
    /// [`Context::heap_bytes`](crate::Context::heap_bytes) does not count
    /// the set.
    pub fn heap_bytes(&self) -> usize {
        self.faces.lock().table.heap_bytes()
    }

    /// Finds the face of `font`, of `family`, by its key `key` hashing to
    /// `hash`, or makes it under the lock, and returns its bytes and font.
    ///
    /// Returns `None` for a font whose bytes are not here
    /// or are not a font read-fonts reads.
    fn find_or_make(
        &self,
        family: &Family,
        font: &fontwich::Font,
        key: FontKey,
        hash: u64,
    ) -> Option<(FontBytes, Font)> {
        let mut faces = self.faces.lock();
        let table = &mut faces.table;
        if let Some(id) = table.find(hash, |face| face.key == key)
            && let Some(face) = table.get(id)
        {
            return Some((face.bytes.clone(), face.font.clone()));
        }
        let (bytes, held) = load(font)?;
        let face = SharedFace {
            key,
            family: family.clone(),
            bytes: bytes.clone(),
            font: held.clone(),
        };
        // A full table keeps nothing, and the context holds the face alone.
        let _ = table.insert(hash, face);
        table.trim(|_, _| {});
        #[cfg(test)]
        {
            faces.made += 1;
        }
        Some((bytes, held))
    }

    /// Returns how many fonts the set holds and how many it has parsed, for
    /// tests.
    #[cfg(all(test, feature = "std"))]
    pub(crate) fn counts(&self) -> (usize, usize) {
        let faces = self.faces.lock();
        (faces.table.len(), faces.made)
    }
}

impl Default for SharedFonts {
    /// Creates an empty set that keeps at most 64 fonts.
    fn default() -> Self {
        Self::new(64)
    }
}

impl fmt::Debug for SharedFonts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedFonts").finish_non_exhaustive()
    }
}
