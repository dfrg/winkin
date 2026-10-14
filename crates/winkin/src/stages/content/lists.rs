//! Interned style lists, and the [`Lookup`] every interned content table
//! uses.
//!
//! Each kind of list a style names has a table that stores each distinct
//! list once and names it by id.
//!
//! - [`Lists`] lives in the context's content scratch. Font selection reads
//!   its feature and variation settings and hyphen strings; measurement
//!   reads the hyphen strings for generated text. Each build clears it and
//!   keeps the capacity, so a warm rebuild allocates nothing.
//! - The content keeps only its [`Languages`].
//! - A `font-family` list is named by the font context's id for it.
//! - A table refuses a list that does not fit instead of panicking; the
//!   style then takes the initial list, and the build reports the count.
//!
//! Start at [`Lists::key`], which interns a style's lists and returns a
//! [`StyleKey`]. The writer lowers the key into the facts and keeps it
//! nowhere.

use alloc::boxed::Box;

use core::hash::{Hash, Hasher};
use core::ops::Range;
use parlance::{FontFeature, FontVariation, Language};

use super::memo::{FontKey, StyleKey, TextKey};
use crate::data::{
    FxHasher, HashIndex, HeapBytes, Id, IdRange, Table, define_id, hash_one, heap_bytes,
};
use crate::stages::fonts::{FontContext, NamedListId};
use crate::style::ComputedStyle;
use crate::style::{Same, same_by_value};

define_id! {
    /// Names a distinct `font-feature-settings` list in a build.
    pub(crate) struct FeatureSettingsId(u16);
}

define_id! {
    /// Names a distinct `font-variation-settings` list in a build.
    pub(crate) struct VariationSettingsId(u16);
}

define_id! {
    /// Names a distinct content language in a layout.
    ///
    /// Script runs name languages by this id too, which keeps a
    /// `ScriptRun` at 12 bytes.
    pub(crate) struct LanguageId(u16);
}

define_id! {
    /// Names a distinct `hyphenate-character` string in a build.
    pub(crate) struct HyphenStringId(u16);
}

define_id! {
    /// Names one setting in the shared value table of [`FeatureSettings`].
    pub(crate) struct FontFeatureId(u32);
}

define_id! {
    /// Names one setting in the shared value table of [`VariationSettings`].
    pub(crate) struct FontVariationId(u32);
}

define_id! {
    /// Names one byte in the shared byte table of [`HyphenStrings`].
    struct HyphenByteId(u32);
}

same_by_value!(
    FeatureSettingsId,
    VariationSettingsId,
    LanguageId,
    HyphenStringId
);

impl FeatureSettingsId {
    /// The empty list, stored only once another list is (see
    /// [`InternedLists::intern`]).
    pub(super) const EMPTY: Self = Self(0);
}

impl VariationSettingsId {
    /// The empty list, stored only once another list is (see
    /// [`InternedLists::intern`]).
    pub(super) const EMPTY: Self = Self(0);
}

impl LanguageId {
    /// `und`, for no language, stored only once another language is (see
    /// [`Languages::intern`]).
    pub(super) const UNDETERMINED: Self = Self(0);
}

/// Returns `id`, or `initial` counted in `replaced` when `id` is `None`.
fn or_initial<T>(id: Option<T>, initial: T, replaced: &mut usize) -> T {
    id.unwrap_or_else(|| {
        *replaced = replaced.saturating_add(1);
        initial
    })
}

/// The most values a table holds before it is searched by hash.
const FEW: usize = 8;

/// Finds values already held by the build's interned tables.
///
/// A table of up to [`FEW`] values is scanned newest first; most builds
/// hold only a few of each, and comparing a few is cheaper than hashing.
/// A larger table uses one [`HashIndex`] shared by every table, which it
/// fills with its earlier values when it first passes [`FEW`]. The lookup
/// lives in the context's content scratch, so no layout keeps an index. The
/// style memo has a `Lookup` of its own.
pub(super) struct Lookup {
    index: HashIndex,
}

impl Lookup {
    /// Creates a lookup with no index, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            index: HashIndex::new(),
        }
    }

    /// Forgets every value, keeping the index's capacity.
    pub(super) fn clear(&mut self) {
        self.index.clear();
    }

    /// Finds the id, among a table's first `held` values, that `same`
    /// accepts.
    ///
    /// On a miss, returns the hash computed by `hash` for
    /// [`add`](Self::add), or `None` if nothing was hashed.
    pub(super) fn find<I: Id>(
        &self,
        held: usize,
        hash: impl FnOnce() -> u64,
        same: impl Fn(I) -> bool,
    ) -> Result<I, Option<u64>> {
        if held <= FEW {
            return (I::new(0)..I::new(held))
                .ids()
                .rev()
                .find(|&id| same(id))
                .ok_or(None);
        }
        let hash = hash();
        self.index.find(hash, same).ok_or(Some(hash))
    }

    /// Records `id`, the value a table has just added.
    ///
    /// `hash` is the hash from [`find`](Self::find), if any. Nothing is
    /// indexed until the table passes [`FEW`] values; then every earlier
    /// value is indexed too, hashed by `hash_of`.
    pub(super) fn add<I: Id>(&mut self, id: I, hash: Option<u64>, hash_of: impl Fn(I) -> u64) {
        let held = id.get() + 1;
        if held <= FEW {
            return;
        }
        if held == FEW + 1 {
            for before in (I::new(0)..id).ids() {
                let h = hash_of(before);
                self.index.insert(h, before);
            }
        }
        let h = hash.unwrap_or_else(|| hash_of(id));
        self.index.insert(h, id);
    }

    /// Returns the id of `value` in `values`, adding it if new, or `None` if
    /// the table is full.
    pub(super) fn intern<I: Id, T: PartialEq + Hash>(
        &mut self,
        values: &mut Table<I, T>,
        value: T,
    ) -> Option<I> {
        let held = &*values;
        let found = self.find(
            held.len(),
            || hash_one(&value),
            |id| held.get(id) == Some(&value),
        );
        let hash = match found {
            Ok(id) => return Some(id),
            Err(hash) => hash,
        };
        let id = values.push(value)?;
        let held = &*values;
        self.add(id, hash, |id| held.get(id).map_or(0, hash_one));
        Some(id)
    }
}

heap_bytes! {
    Lookup { index }
}

/// Lists of `Copy` values, each distinct list stored once.
///
/// Backs [`FeatureSettings`], [`VariationSettings`] and [`HyphenStrings`].
/// `I` names a list; `V` names a value in the one table all lists share.
pub(crate) struct InternedLists<I, V, T> {
    /// Each list's range of `values`.
    lists: Table<I, Range<V>>,
    values: Table<V, T>,
}

/// The build's `font-feature-settings` lists.
type FeatureSettings = InternedLists<FeatureSettingsId, FontFeatureId, FontFeature>;

/// The build's `font-variation-settings` lists.
type VariationSettings = InternedLists<VariationSettingsId, FontVariationId, FontVariation>;

impl<I: Id, V: Id, T: Same + Copy> InternedLists<I, V, T> {
    /// Creates an empty table, allocating nothing.
    const fn new() -> Self {
        Self {
            lists: Table::new(),
            values: Table::new(),
        }
    }

    /// Removes every list and keeps the allocations.
    fn clear(&mut self) {
        self.lists.clear();
        self.values.clear();
    }

    fn hash(list: &[T]) -> u64 {
        let mut fx = FxHasher::new();
        fx.write_usize(list.len());
        for value in list {
            value.feed(&mut fx);
        }
        fx.finish()
    }

    /// Returns the id of `list`, interning it if new, or `None` if it does
    /// not fit.
    ///
    /// The empty list is id 0 and is stored only when the first other list
    /// is. Until then [`get`](Self::get) returns it as an id past the end.
    fn intern(&mut self, list: &[T], lookup: &mut Lookup) -> Option<I> {
        if list.is_empty() {
            return Some(I::new(0));
        }
        let found = lookup.find(
            self.lists.len(),
            || Self::hash(list),
            |id| {
                let stored = self.get(id);
                stored.len() == list.len() && stored.iter().zip(list).all(|(a, b)| a.same(b))
            },
        );
        let hash = match found {
            Ok(id) => return Some(id),
            Err(hash) => hash,
        };
        // A list is stored whole or not at all, after the empty one.
        let first = self.lists.is_empty();
        if self.lists.remaining() < 1 + usize::from(first) {
            return None;
        }
        if first {
            let none = self.values.next_id();
            self.lists
                .push_bounded(none..none, "the room for two lists was checked above");
        }
        let values = self.values.extend_from_slice(list)?;
        let id = self.lists.push(values)?;
        let Self { lists, values } = self;
        lookup.add(id, hash, |held| {
            let stored = lists
                .get(held)
                .and_then(|range| values.get_slice(range.clone()))
                .unwrap_or(&[]);
            Self::hash(stored)
        });
        Some(id)
    }

    /// Returns the values of list `id`; empty past the end.
    pub(crate) fn get(&self, id: I) -> &[T] {
        self.lists
            .get(id)
            .and_then(|values| self.values.get_slice(values.clone()))
            .unwrap_or(&[])
    }
}

impl<I, V, T> HeapBytes for InternedLists<I, V, T> {
    fn heap_bytes(&self) -> usize {
        let Self { lists, values } = self;
        lists.heap_bytes() + values.heap_bytes()
    }
}

/// The layout's content languages, each stored once.
///
/// Font requests and script runs name them by [`LanguageId`]. The table is
/// boxed at the first language other than `und`, then cleared and kept.
pub(crate) struct Languages {
    languages: Option<Box<Table<LanguageId, Language>>>,
}

impl Languages {
    /// Creates an empty table, allocating nothing.
    pub(super) const fn new() -> Self {
        Self { languages: None }
    }

    /// Removes every language and keeps the allocations.
    pub(super) fn clear(&mut self) {
        if let Some(languages) = &mut self.languages {
            languages.clear();
        }
    }

    /// Returns the id of `language` (`None` is `und`), interning it if new,
    /// or `None` if the table is full.
    ///
    /// `und` is id 0 and is stored only when the first other language is.
    /// Until then [`get`](Self::get) returns it as an id past the end.
    fn intern(&mut self, language: Option<Language>, lookup: &mut Lookup) -> Option<LanguageId> {
        let language = language.unwrap_or(Language::UND);
        if language == Language::UND {
            return Some(LanguageId::UNDETERMINED);
        }
        let languages = self.languages.get_or_insert_with(Box::default);
        if languages.is_empty() {
            languages.push_bounded(Language::UND, "an empty table has room");
        }
        lookup.intern(languages, language)
    }

    /// Returns language `id`; `und` past the end.
    pub(crate) fn get(&self, id: LanguageId) -> Language {
        self.languages
            .as_deref()
            .and_then(|languages| languages.get(id))
            .copied()
            .unwrap_or(Language::UND)
    }
}

heap_bytes! {
    Languages { languages }
}

/// The build's `hyphenate-character` strings, each stored once.
///
/// Text facts name them by [`HyphenStringId`]. Each is interned as its
/// UTF-8 bytes.
pub(crate) struct HyphenStrings {
    strings: InternedLists<HyphenStringId, HyphenByteId, u8>,
}

impl HyphenStrings {
    /// Creates an empty table, allocating nothing.
    const fn new() -> Self {
        Self {
            strings: InternedLists::new(),
        }
    }

    /// Removes every string and keeps the allocations.
    fn clear(&mut self) {
        self.strings.clear();
    }

    /// Returns the id of `string`, interning it if new, or `None` if it does
    /// not fit.
    fn intern(&mut self, string: &str, lookup: &mut Lookup) -> Option<HyphenStringId> {
        self.strings.intern(string.as_bytes(), lookup)
    }

    /// Returns string `id`; empty past the end. The bytes always came from
    /// a `str`, so they are valid UTF-8.
    pub(crate) fn get(&self, id: HyphenStringId) -> &str {
        str::from_utf8(self.strings.get(id)).unwrap_or_default()
    }
}

heap_bytes! {
    HyphenStrings { strings }
}

/// The build's interned style lists.
///
/// [`key`](Self::key) is the entry for every style's lists.
pub(crate) struct Lists {
    /// The lookup for these tables and for the content's facts and
    /// languages.
    lookup: Lookup,
    feature_settings: FeatureSettings,
    variation_settings: VariationSettings,
    hyphen_strings: HyphenStrings,
}

impl Lists {
    /// Creates empty tables, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            lookup: Lookup::new(),
            feature_settings: InternedLists::new(),
            variation_settings: InternedLists::new(),
            hyphen_strings: HyphenStrings::new(),
        }
    }

    /// Returns the `font-feature-settings` lists font requests name.
    pub(crate) fn feature_settings(&self) -> &FeatureSettings {
        &self.feature_settings
    }

    /// Returns the `font-variation-settings` lists font requests name.
    pub(crate) fn variation_settings(&self) -> &VariationSettings {
        &self.variation_settings
    }

    /// Returns the `hyphenate-character` strings text facts name.
    pub(crate) fn hyphen_strings(&self) -> &HyphenStrings {
        &self.hyphen_strings
    }

    /// Empties every table, keeping its capacity.
    pub(super) fn clear(&mut self) {
        self.lookup.clear();
        self.feature_settings.clear();
        self.variation_settings.clear();
        self.hyphen_strings.clear();
    }

    /// Returns `style` with each list replaced by its interned id.
    ///
    /// The language is interned in `languages` and the family list in
    /// `fonts`. A list that does not fit becomes the initial one and is
    /// counted in `replaced`.
    pub(super) fn key(
        &mut self,
        style: &ComputedStyle<'_>,
        languages: &mut Languages,
        fonts: &mut FontContext,
        replaced: &mut usize,
    ) -> StyleKey {
        let (font, text) = (&style.font, &style.text);
        let lookup = &mut self.lookup;
        let families = fonts.family_list(font.families);
        let language = languages.intern(text.language, lookup);
        let hyphenate_character = match text.hyphenate_character {
            // `auto`, which needs no room.
            None => None,
            Some(string) => or_initial(
                self.hyphen_strings.intern(string, lookup).map(Some),
                None,
                replaced,
            ),
        };
        let features = self.feature_settings.intern(font.features, lookup);
        let variations = self.variation_settings.intern(font.variations, lookup);
        StyleKey {
            font: FontKey::new(
                font,
                or_initial(families, NamedListId::INITIAL, replaced),
                or_initial(features, FeatureSettingsId::EMPTY, replaced),
                or_initial(variations, VariationSettingsId::EMPTY, replaced),
            ),
            text: TextKey::new(
                text,
                or_initial(language, LanguageId::UNDETERMINED, replaced),
                hyphenate_character,
            ),
            line: style.line,
            bidi: style.bidi,
            orientation: style.orientation,
            edges: style.edges,
            ruby: style.ruby,
            paints: style.paints,
            outlines: style.outlines,
            decorates: style.decorates,
        }
    }

    /// Returns the lookup the writer interns fact rows with.
    pub(super) fn lookup(&mut self) -> &mut Lookup {
        &mut self.lookup
    }
}

heap_bytes! {
    Lists { lookup, feature_settings, variation_settings, hyphen_strings }
}
