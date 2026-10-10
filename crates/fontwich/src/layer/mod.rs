//! One layer of fonts: a complete set of family names, immutable once
//! published.
//!
//! A layer lists every family name up front. A family's fonts load on first
//! use, through the layer's [`LoadFamily`], or are already in hand for a
//! layer built from bytes.
//!
//! Start at [`Layer`]. Children:
//! - `builder`: [`LayerBuilder`], which makes and edits layers of fonts the
//!   caller supplies.
//! - `local`: the index `src: local(...)` searches in a layer of bytes.

mod builder;
mod local;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use crate::sort;
use crate::sync::{Mutex, Once};

use core::cmp::Ordering;

use hashbrown::HashTable;

use crate::backend::Backend;
use crate::fallback::FallbackOverride;
use crate::family::compare_names;
#[cfg(feature = "std")]
use crate::font::FontFile;
use crate::font::{Charset, Face, Font};

pub use builder::{AddError, LayerBuilder};
use local::LocalNames;

/// A layer's role in font lookup and fallback.
///
/// Layer order determines name lookup. The role determines which layers
/// participate in fallback; see
/// [`Collection::fallback_family`](crate::Collection::fallback_family).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Role {
    /// System fonts, used for fallback.
    System,
    /// Application-supplied fonts.
    ///
    /// Used for fallback only if no system layer is present.
    Application,
    /// Document faces, excluded from fallback.
    Document,
}

/// Loads font metadata on demand.
///
/// Used by layers that list family names before loading their fonts. Each
/// family is loaded on first use.
pub trait LoadFamily: Send + Sync + core::fmt::Debug {
    /// Loads the fonts belonging to `name`.
    ///
    /// Returns an empty vector if the family has no readable fonts.
    fn load(&self, name: &str) -> Vec<Font>;

    /// Finds a font by its full or PostScript name.
    ///
    /// The default implementation returns `None`.
    fn local(&self, name: &str) -> Option<Font> {
        let _ = name;
        None
    }
}

/// Indexes a layer's table of families.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct FamilyId(u32);

impl FamilyId {
    /// The first family a layer lists.
    pub(crate) const FIRST: Self = Self(0);

    /// The family listed after this one.
    pub(crate) fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }

    /// The id of the family at `at` in the table.
    fn at(at: usize) -> Self {
        Self(u32::try_from(at).unwrap_or(u32::MAX))
    }

    /// The family's place in the table.
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// One family of a layer: its names and its fonts.
#[derive(Debug)]
pub(crate) struct FamilyRecord {
    /// The name as first seen. Later fonts may spell it in another case.
    name: Box<str>,
    /// Other names for the family. DirectWrite gives one per language.
    aliases: Vec<Box<str>>,
    /// The fonts. A layer with a loader fills this on first use. A layer
    /// built from bytes fills it when the family is created.
    fonts: Once<Vec<Font>>,
    /// Whether this is a secondary family. See [`Layer::is_secondary`].
    secondary: bool,
}

// `Once` is not `Clone`, and copy-on-write needs a clone. A clone keeps what
// was loaded and loads the rest on its own.
impl Clone for FamilyRecord {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            aliases: self.aliases.clone(),
            fonts: match self.fonts.get() {
                Some(fonts) => Once::with_value(fonts.clone()),
                None => Once::new(),
            },
            secondary: self.secondary,
        }
    }
}

impl FamilyRecord {
    /// Returns the family's own name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Returns the family's aliases.
    pub(crate) fn aliases(&self) -> impl Iterator<Item = &str> {
        self.aliases.iter().map(|alias| &**alias)
    }

    /// Whether the family's fonts have loaded.
    pub(crate) fn is_loaded(&self) -> bool {
        self.fonts.get().is_some()
    }

    /// Returns one of the family's names: `0` is its own, `n` is alias
    /// `n - 1`.
    fn nth_name(&self, which: u32) -> &str {
        match which {
            0 => &self.name,
            n => &self.aliases[n as usize - 1],
        }
    }
}

/// One entry in a layer's name index: a family, and which of its names.
#[derive(Copy, Clone, Debug)]
struct NameEntry {
    family: FamilyId,
    name: u32,
}

/// A collection of font families.
///
/// Contains a complete set of family names. Font metadata may be loaded
/// lazily through a [`LoadFamily`] implementation.
///
/// Clones share family records and the name index. Changes use
/// copy-on-write; unchanged families retain their identity across
/// snapshots.
#[derive(Clone)]
pub struct Layer {
    role: Role,
    /// The families, in the order they were first added.
    families: Vec<Arc<FamilyRecord>>,
    /// Every name of every family, aliases included, sorted by
    /// [`compare_names`]. A binary search finds a name without hashing or
    /// lowercasing it. Each distinct name appears once; the first family to
    /// claim it keeps it.
    by_name: Arc<Vec<NameEntry>>,
    loader: Option<Arc<dyn LoadFamily>>,
    /// The platform's fallback, asked when fallback reads this layer. A
    /// system layer holds its platform's backend, so its fonts and its
    /// fallback come from one view of the system.
    backend: Option<Arc<Backend>>,
    /// A caller's own fallback, asked before the backend.
    fallback_override: Option<Arc<dyn FallbackOverride>>,
    /// One copy of each distinct charset and of each file. Clones share it,
    /// since it only grows.
    shared: Arc<Mutex<SharedFontData>>,
    /// How many times the builder has changed the layer. See
    /// [`generation`](Self::generation).
    generation: u64,
    /// The id the next face added takes.
    next_face: u32,
    /// The `local()` index for a layer with no loader. See
    /// [`local`](Self::local).
    locals: Arc<Mutex<Option<LocalNames>>>,
}

/// What a layer's fonts share.
///
/// Fonts that map the same characters hold one charset. Fonts from one file,
/// such as the members of a `.ttc`, share its path and its loaded bytes.
#[derive(Default)]
struct SharedFontData {
    /// The distinct charsets, found by a hash of what they map.
    charsets: HashTable<Arc<Charset>>,
    /// Each path once, with the file every font from it shares.
    #[cfg(feature = "std")]
    files: crate::hash::HashMap<Arc<std::path::Path>, Arc<FontFile>>,
}

impl SharedFontData {
    /// Points `font` at the layer's copy of its charset and file cell, or
    /// registers its own if the layer has none yet.
    fn share(&mut self, font: &mut Font) {
        let hash = font.charset.hash();
        match self.charsets.find(hash, |held| **held == *font.charset) {
            Some(held) => font.charset = held.clone(),
            None => {
                self.charsets
                    .insert_unique(hash, font.charset.clone(), |held| held.hash());
            }
        }
        // The file cell exists from the moment its path is named, so a font
        // has its key before it has its bytes.
        #[cfg(feature = "std")]
        if let crate::font::Source::Path(path) = &font.source {
            let (path, file) = match self.files.get_key_value(path) {
                Some((held, file)) => (held.clone(), file.clone()),
                None => {
                    let file = Arc::new(FontFile::new());
                    self.files.insert(path.clone(), file.clone());
                    (path.clone(), file)
                }
            };
            font.source = crate::font::Source::Path(path);
            font.file = Some(file);
        }
    }
}

impl core::fmt::Debug for Layer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Layer")
            .field("role", &self.role)
            .field("families", &self.families.len())
            .field("loaded", &self.loaded())
            .field("backend", &self.backend)
            .field("fallback_override", &self.fallback_override.is_some())
            .finish()
    }
}

impl Layer {
    /// Creates a layer of system fonts.
    ///
    /// Lists installed families and attaches the platform's fallback
    /// backend. Family metadata is loaded on first use; whole font files
    /// are loaded separately.
    ///
    /// Returns an empty layer if the platform font source is unavailable.
    /// Reuse the layer to avoid repeating enumeration.
    #[cfg(all(any(windows, unix), feature = "system"))]
    pub fn system() -> Self {
        crate::platform::system_layer()
    }

    /// Creates a layer from family names and a loader.
    ///
    /// Each name may have aliases. Font metadata is loaded on first use.
    /// Duplicate names resolve to the first family listed.
    pub fn from_names<I, A>(role: Role, families: I, loader: Arc<dyn LoadFamily>) -> Self
    where
        I: IntoIterator<Item = (String, A)>,
        A: IntoIterator<Item = String>,
    {
        Self::listed(role, families, usize::MAX, loader)
    }

    /// Sets the fallback backend and returns the layer.
    ///
    /// The backend is consulted only if the layer participates in fallback.
    /// See
    /// [`Collection::fallback_family`](crate::Collection::fallback_family).
    #[must_use]
    pub fn with_fallback(mut self, backend: Backend) -> Self {
        self.backend = Some(Arc::new(backend));
        self
    }

    /// Sets a fallback override and returns the layer.
    ///
    /// The override is consulted before the backend, if any.
    #[must_use]
    pub fn with_fallback_override(mut self, fallback: impl FallbackOverride + 'static) -> Self {
        self.fallback_override = Some(Arc::new(fallback));
        self
    }

    /// Returns the layer's role.
    pub fn role(&self) -> Role {
        self.role
    }

    /// Returns the number of families.
    pub fn len(&self) -> usize {
        self.families.len()
    }

    /// Returns `true` if the layer contains no families.
    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }

    /// Returns family names in insertion order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.families.iter().map(|family| &*family.name)
    }

    /// Finds a font by its full or PostScript name.
    ///
    /// Delegates to [`LoadFamily::local`] when a loader is present.
    /// Otherwise, builds an index from the fonts' name tables on first use.
    /// Cached name hashes are verified against the font before returning a
    /// match.
    pub fn local(&self, name: &str) -> Option<Font> {
        if let Some(loader) = &self.loader {
            return loader.local(name);
        }
        self.local_here(name)
    }

    /// Returns the layer's generation.
    ///
    /// Incremented when its builder adds fonts or declares, loads, or
    /// removes faces. Use it to invalidate layout caches after a change.
    /// Layers without a builder have generation zero.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the number of families whose metadata has been loaded.
    pub fn loaded(&self) -> usize {
        self.families
            .iter()
            .filter(|family| family.is_loaded())
            .count()
    }

    /// Returns the estimated heap usage in bytes.
    ///
    /// Includes names, aliases, the name index, loaded font metadata, and
    /// supplied font bytes. Shared allocations are counted once within this
    /// layer. Unloaded font metadata is excluded.
    ///
    /// Counts requested allocation sizes, excluding allocator overhead.
    /// Other snapshots count shared allocations separately. The loader and
    /// fallback backend are excluded.
    pub fn heap_usage(&self) -> usize {
        self.heap(&mut crate::heap::Seen::default())
    }

    pub(crate) fn new(role: Role) -> Self {
        Self {
            role,
            families: Vec::new(),
            by_name: Arc::new(Vec::new()),
            loader: None,
            backend: None,
            fallback_override: None,
            shared: Arc::default(),
            generation: 0,
            next_face: 0,
            locals: Arc::default(),
        }
    }

    /// Creates a layer like [`from_names`](Self::from_names), where the
    /// families from the `primaries`th on are secondary.
    ///
    /// DirectWrite, fontconfig and Core Text list secondary families after
    /// the primary ones. Android's layer is built font by font instead.
    #[cfg(all(feature = "system", any(windows, unix)))]
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub(crate) fn with_secondary<I, A>(
        role: Role,
        families: I,
        primaries: usize,
        loader: Arc<dyn LoadFamily>,
    ) -> Self
    where
        I: IntoIterator<Item = (String, A)>,
        A: IntoIterator<Item = String>,
    {
        Self::listed(role, families, primaries, loader)
    }

    fn listed<I, A>(role: Role, families: I, primaries: usize, loader: Arc<dyn LoadFamily>) -> Self
    where
        I: IntoIterator<Item = (String, A)>,
        A: IntoIterator<Item = String>,
    {
        let listed: Vec<(String, Vec<Box<str>>)> = families
            .into_iter()
            .map(|(name, aliases)| {
                let aliases = aliases.into_iter().map(String::into_boxed_str).collect();
                (name, aliases)
            })
            .collect();

        // Drop a family whose own name an earlier one already has. Equal
        // names keep the order they were listed in, so the first comes first.
        let mut order: Vec<usize> = (0..listed.len()).collect();
        sort::by(&mut order, |&a, &b| {
            compare_names(&listed[a].0, &listed[b].0).then(a.cmp(&b))
        });
        let mut keep = alloc::vec![true; listed.len()];
        for pair in order.windows(2) {
            if compare_names(&listed[pair[0]].0, &listed[pair[1]].0) == Ordering::Equal {
                keep[pair[1]] = false;
            }
        }

        let mut layer = Self::new(role);
        layer.loader = Some(loader);
        layer.families = listed
            .into_iter()
            .zip(keep)
            .enumerate()
            .filter(|(_, (_, keep))| *keep)
            .map(|(at, ((name, aliases), _))| {
                Arc::new(FamilyRecord {
                    name: name.into_boxed_str(),
                    aliases,
                    fonts: Once::new(),
                    secondary: at >= primaries,
                })
            })
            .collect();
        layer.by_name = Arc::new(index(&layer.families));
        layer
    }

    pub(crate) fn set_fallback(&mut self, backend: Backend) {
        self.backend = Some(Arc::new(backend));
    }

    pub(crate) fn set_fallback_override(&mut self, fallback: Arc<dyn FallbackOverride>) {
        self.fallback_override = Some(fallback);
    }

    /// Returns the backend fallback asks when it reads this layer, if any.
    pub(crate) fn backend(&self) -> Option<&Backend> {
        self.backend.as_deref()
    }

    /// Returns the caller's fallback, asked before the backend, if any.
    pub(crate) fn fallback_override(&self) -> Option<&dyn FallbackOverride> {
        self.fallback_override.as_deref()
    }

    /// Returns the family `id`'s record.
    pub(crate) fn record(&self, id: FamilyId) -> &Arc<FamilyRecord> {
        &self.families[id.index()]
    }

    /// Whether the layer has a family `id`.
    pub(crate) fn holds(&self, id: FamilyId) -> bool {
        id.index() < self.families.len()
    }

    /// Whether the family `id` is secondary.
    ///
    /// A secondary family's fonts all belong to a primary family. Its name
    /// is a font's legacy family name, such as "Arial Black" for Arial's
    /// black weight. Lookup finds it like any family. The fallback walk
    /// skips it, since it adds no coverage.
    pub(crate) fn is_secondary(&self, id: FamilyId) -> bool {
        self.record(id).secondary
    }

    pub(crate) fn heap(&self, seen: &mut crate::heap::Seen) -> usize {
        use crate::heap::ARC;
        use core::mem::size_of;
        let mut total = self.families.capacity() * size_of::<Arc<FamilyRecord>>();
        for record in &self.families {
            total += seen.once(Arc::as_ptr(record), || {
                let mut own = ARC + size_of::<FamilyRecord>();
                own += record.name.len();
                own += record.aliases.capacity() * size_of::<Box<str>>();
                own += record
                    .aliases
                    .iter()
                    .map(|alias| alias.len())
                    .sum::<usize>();
                own
            });
            if let Some(fonts) = record.fonts.get() {
                total += seen.once(fonts.as_ptr(), || fonts.capacity() * size_of::<Font>());
                for font in fonts {
                    total += font.heap(seen);
                }
            }
        }
        total += seen.once(Arc::as_ptr(&self.by_name), || {
            ARC + size_of::<Vec<NameEntry>>() + self.by_name.capacity() * size_of::<NameEntry>()
        });
        // The tables of distinct charsets and files, roughly: their entries.
        // The fonts holding them count what the entries point to.
        total += seen.once(Arc::as_ptr(&self.shared), || {
            let shared = self.shared.lock();
            #[cfg(feature = "std")]
            let files =
                shared.files.capacity() * (size_of::<(Arc<std::path::Path>, Arc<FontFile>)>() + 1);
            #[cfg(not(feature = "std"))]
            let files = 0;
            ARC + size_of::<Mutex<SharedFontData>>()
                + shared.charsets.capacity() * (size_of::<Arc<Charset>>() + 1)
                + files
        });
        total
    }

    /// Returns the family that answers to `name`.
    pub(crate) fn find(&self, name: &str) -> Option<FamilyId> {
        let at = self
            .by_name
            .binary_search_by(|entry| compare_names(self.entry_name(entry), name))
            .ok()?;
        Some(self.by_name[at].family)
    }

    /// Gives the family called `to` another name, and says whether there
    /// was such a family.
    ///
    /// Android's `fonts.xml` aliases desktop names (`arial` to
    /// `sans-serif`). DirectWrite and fontconfig give aliases with the
    /// family. An alias finds the same fonts and adds nothing to a walk of
    /// the layer.
    ///
    /// Does nothing if `to` is absent or `alias` already names some family,
    /// this one included, so applying the same aliases twice changes
    /// nothing.
    pub(crate) fn add_alias(&mut self, alias: &str, to: &str) -> bool {
        let Some(family) = self.find(to) else {
            return false;
        };
        if self.find(alias).is_some() {
            return false;
        }
        let record = Arc::make_mut(&mut self.families[family.index()]);
        record.aliases.push(Box::from(alias));
        let name = record.aliases.len() as u32;
        self.index_name(NameEntry { family, name });
        true
    }

    /// Adds `font` to the family `name`, creating it if need be, and returns
    /// the family.
    ///
    /// For layers built from bytes, which have no loader. The fonts are
    /// already in hand, so the family is loaded the moment it exists.
    pub(crate) fn insert(&mut self, name: String, font: Font) -> FamilyId {
        self.insert_into(name, font, false)
    }

    /// Adds `font` to the family its legacy name gives it, creating it as a
    /// secondary family if need be.
    pub(crate) fn insert_secondary(&mut self, name: String, font: Font) -> FamilyId {
        self.insert_into(name, font, true)
    }

    fn insert_into(&mut self, name: String, mut font: Font, secondary: bool) -> FamilyId {
        debug_assert!(self.loader.is_none(), "a loader layer is built whole");
        self.shared.lock().share(&mut font);
        if let Some(family) = self.find(&name) {
            // Copies this one family if a snapshot still shares it.
            let record = Arc::make_mut(&mut self.families[family.index()]);
            match record.fonts.get_mut() {
                Some(fonts) => fonts.push(font),
                None => record.fonts = Once::with_value(alloc::vec![font]),
            }
            return family;
        }
        let family = FamilyId::at(self.families.len());
        self.families.push(Arc::new(FamilyRecord {
            name: name.into_boxed_str(),
            aliases: Vec::new(),
            fonts: Once::with_value(alloc::vec![font]),
            secondary,
        }));
        self.index_name(NameEntry { family, name: 0 });
        family
    }

    /// Inserts `entry` into the name index, in order. The name must be new.
    fn index_name(&mut self, entry: NameEntry) {
        let families = &self.families;
        let name_of = |entry: &NameEntry| families[entry.family.index()].nth_name(entry.name);
        let name = name_of(&entry);
        let index = Arc::make_mut(&mut self.by_name);
        if let Err(slot) = index.binary_search_by(|held| compare_names(name_of(held), name)) {
            index.insert(slot, entry);
        }
    }

    /// Returns the name an index entry stands for.
    fn entry_name(&self, entry: &NameEntry) -> &str {
        self.families[entry.family.index()].nth_name(entry.name)
    }

    /// Records a change, for [`generation`](Self::generation).
    pub(crate) fn changed(&mut self) {
        self.generation += 1;
    }

    /// Adds a face to the family `name`, creating the family if need be, and
    /// returns its id and the family.
    pub(crate) fn insert_face(
        &mut self,
        name: String,
        descriptors: crate::FaceDescriptors,
        font: Option<Font>,
    ) -> (crate::FaceId, FamilyId) {
        let id = crate::FaceId(self.next_face);
        self.next_face += 1;
        let face = Face::new(id, descriptors);
        let font = match font {
            Some(font) => font.with_face(face),
            None => Font::pending(face),
        };
        (id, self.insert(name, font))
    }

    /// Returns where the face `id` is: its family, and its place among the
    /// family's fonts.
    fn find_face(&self, id: crate::FaceId) -> Option<(usize, usize)> {
        self.families
            .iter()
            .enumerate()
            .find_map(|(family, record)| {
                let at = record
                    .fonts
                    .get()?
                    .iter()
                    .position(|font| font.face_id() == Some(id))?;
                Some((family, at))
            })
    }

    /// Replaces the face `id`'s font with `font`, keeping the face's
    /// descriptors. Returns `false` if there is no such face.
    pub(crate) fn replace_face(&mut self, id: crate::FaceId, font: Font) -> bool {
        let Some((family, at)) = self.find_face(id) else {
            return false;
        };
        let record = Arc::make_mut(&mut self.families[family]);
        let Some(fonts) = record.fonts.get_mut() else {
            return false;
        };
        let Some(face) = fonts[at].face.clone() else {
            return false;
        };
        let mut font = font.with_face(face);
        self.shared.lock().share(&mut font);
        fonts[at] = font;
        true
    }

    /// Removes the face `id`, and its family if it was the last face.
    ///
    /// A name no rule declares any more is not the document's to answer.
    /// Returns `false` if there is no such face.
    pub(crate) fn remove_face(&mut self, id: crate::FaceId) -> bool {
        let Some((family, at)) = self.find_face(id) else {
            return false;
        };
        let record = Arc::make_mut(&mut self.families[family]);
        let Some(fonts) = record.fonts.get_mut() else {
            return false;
        };
        fonts.remove(at);
        if fonts.is_empty() {
            self.families.remove(family);
            self.by_name = Arc::new(index(&self.families));
        }
        true
    }

    /// Returns the family `id`'s fonts, loading them on first use.
    pub(crate) fn fonts(&self, id: FamilyId) -> &[Font] {
        let record = self.record(id);
        record.fonts.get_or_init(|| {
            let Some(loader) = &self.loader else {
                return Vec::new();
            };
            let mut fonts = loader.load(&record.name);
            // The lock is held only while pointers are swapped. The slow
            // load is done, and other threads load their families meanwhile.
            let mut shared = self.shared.lock();
            for font in &mut fonts {
                shared.share(font);
            }
            // A loader collects from a filter and the vector grows by
            // doubling. Unshrunk, a family's fonts would take nearly twice
            // their size.
            fonts.shrink_to_fit();
            fonts
        })
    }
}

/// Builds the name index for `families`: every name, sorted. Where two
/// families answer to one name, the first family's entry stays.
fn index(families: &[Arc<FamilyRecord>]) -> Vec<NameEntry> {
    let name = |entry: &NameEntry| families[entry.family.index()].nth_name(entry.name);
    let mut index: Vec<NameEntry> = families
        .iter()
        .enumerate()
        .flat_map(|(family, record)| {
            (0..=record.aliases.len()).map(move |name| NameEntry {
                family: FamilyId::at(family),
                name: name as u32,
            })
        })
        .collect();
    // Equal names keep the entries' order, family by family, so among them
    // the first family's entry comes first and `dedup_by` keeps it.
    sort::by(&mut index, |a, b| {
        compare_names(name(a), name(b))
            .then((a.family.index(), a.name).cmp(&(b.family.index(), b.name)))
    });
    index.dedup_by(|later, earlier| compare_names(name(later), name(earlier)) == Ordering::Equal);
    index
}
