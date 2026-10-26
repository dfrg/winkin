//! The context's family lists: two tables, each with a hash index over it.
//!
//! - [`NamedLists`] holds each distinct `font-family` list by its names, so
//!   a list keeps one id across layouts. The builder names each style's list
//!   here as it keys the style, so no layout holds a name. It holds names,
//!   not families, so it survives a new collection.
//! - [`CandidateLists`] holds the families to try in CSS's order, per list,
//!   script, language, matching request and presentation. Their fonts are
//!   matched a family at a time as the walk reaches each, so no family loads
//!   before a cluster needs it. Beside the lists it keeps the characters no
//!   font maps.
//!
//! A candidate list's families are found the first time a span or a primary
//! font asks for it. The list grows only at its end, as families match and
//! misses add families.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::hash::{Hash, Hasher};

use fontwich::{
    Attributes, Collection, FallbackRequest, Family, Font, FontFamilyName, FontStyle, FontWeight,
    FontWidth, GenericClass, GenericFamily, Presentation,
};
use parlance::{Language, Script};

use crate::data::{FxHasher, HeapBytes, LruCache, Table, define_id, hash_one, heap_bytes};
use crate::stages::content::FontRequest;
use crate::style::{same_by_value, style_struct};

define_id! {
    /// Names a distinct `font-family` list in the context's
    /// [`NamedLists`], across layouts.
    ///
    /// A font request names its list by it. Only font selection reads it,
    /// during the build, while no trim can drop the list.
    pub(crate) struct NamedListId(u16);
}

impl NamedListId {
    /// The initial `font-family`, the empty list: the Standard font alone.
    ///
    /// The table never stores it. Its id, `u16::MAX`, is one the table never
    /// hands out.
    pub(crate) const INITIAL: Self = Self(u16::MAX);
}

same_by_value!(NamedListId);

define_id! {
    /// Names a list of fonts to try in the context's table of
    /// [`CandidateList`]s.
    pub(super) struct CandidateListId(u32);
}

define_id! {
    /// Names a font to try in one [`CandidateList`]'s table of
    /// [`Candidate`]s: its place in the order CSS tries them.
    pub(super) struct CandidateId(u32);
}

/// One entry of a stored family list, as the style named it.
#[derive(Clone, Debug)]
enum StoredName {
    Generic(GenericFamily),
    Named(Box<str>),
}

/// Returns whether `stored` holds the same entries as `names`.
fn is_named(stored: &[StoredName], names: &[FontFamilyName<'_>]) -> bool {
    stored.len() == names.len()
        && stored
            .iter()
            .zip(names)
            .all(|(stored, name)| match (stored, name) {
                (StoredName::Generic(a), FontFamilyName::Generic(b)) => a == b,
                (StoredName::Named(a), FontFamilyName::Named(b)) => **a == **b,
                _ => false,
            })
}

/// Each distinct `font-family` list the context has met, by its names.
///
/// A [`CandidateList`] is keyed by it, so every layout naming a list shares
/// its fonts. The empty list, the initial `font-family`, is never stored:
/// it is [`NamedListId::INITIAL`]. A trim before each build keeps the lists
/// used last, and the candidate lists keyed by a list it drops go with it.
/// A build only adds, so every id it hands out holds until it ends.
pub(super) struct NamedLists {
    lists: LruCache<NamedListId, Box<[StoredName]>>,
}

impl NamedLists {
    /// Makes an empty table that keeps `capacity` lists, allocating nothing.
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            lists: LruCache::new(capacity),
        }
    }

    /// Sets how many lists a trim keeps.
    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.lists.set_capacity(capacity);
    }

    /// Drops the lists used longest ago down to the capacity, and returns
    /// whether it dropped any.
    pub(super) fn trim(&mut self) -> bool {
        let mut dropped = false;
        self.lists.trim(|_, _| dropped = true);
        dropped
    }

    /// Returns whether list `id` is `names`, for tests.
    #[cfg(test)]
    pub(super) fn is_named(&self, id: NamedListId, names: &[FontFamilyName<'_>]) -> bool {
        is_named(self.get(id), names)
    }

    /// Returns how many lists are held, for tests to see them bounded.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.lists.len()
    }

    /// Returns whether list `id` is held. The initial list always is.
    fn contains(&self, id: NamedListId) -> bool {
        id == NamedListId::INITIAL || self.lists.get(id).is_some()
    }

    /// Returns the names of list `id`; empty for the initial list or an id
    /// this cache did not hand out.
    fn get(&self, id: NamedListId) -> &[StoredName] {
        self.lists.get(id).map_or(&[], |names| names)
    }

    /// Returns the id of the list `names`, storing its names if it is new.
    ///
    /// Returns `None` where every id is taken.
    pub(super) fn find_or_make(&mut self, names: &[FontFamilyName<'_>]) -> Option<NamedListId> {
        if names.is_empty() {
            return Some(NamedListId::INITIAL);
        }
        let mut fx = FxHasher::new();
        fx.write_usize(names.len());
        for name in names {
            match name {
                FontFamilyName::Generic(generic) => {
                    fx.write_u8(0);
                    generic.hash(&mut fx);
                }
                FontFamilyName::Named(name) => {
                    fx.write_u8(1);
                    name.hash(&mut fx);
                }
            }
        }
        let hash = fx.finish();
        if let Some(found) = self.lists.find(hash, |stored| is_named(stored, names)) {
            return Some(found);
        }
        if self.lists.is_full() {
            return None;
        }
        let stored = names
            .iter()
            .map(|name| match name {
                FontFamilyName::Generic(generic) => StoredName::Generic(*generic),
                FontFamilyName::Named(name) => StoredName::Named(Box::from(&**name)),
            })
            .collect();
        self.lists.insert(hash, stored)
    }
}

impl HeapBytes for NamedLists {
    /// Its table, and each list's names.
    fn heap_bytes(&self) -> usize {
        let Self { lists } = self;
        let own: usize = lists
            .iter()
            .map(|(_, names)| {
                let text: usize = names
                    .iter()
                    .map(|name| match name {
                        StoredName::Generic(_) => 0,
                        StoredName::Named(name) => name.len(),
                    })
                    .sum();
                size_of_val::<[StoredName]>(names) + text
            })
            .sum();
        lists.heap_bytes() + own
    }
}

style_struct! {
    /// What a font request asks of a family's fonts.
    ///
    /// It holds CSS matching's attributes and `font-synthesis-style`.
    /// Whether an oblique may be synthesized changes what matches.
    pub(super) struct MatchRequest {
        weight: FontWeight,
        width: FontWidth,
        style: FontStyle,
        synthesize_style: bool,
    }
}

impl From<&FontRequest> for MatchRequest {
    fn from(request: &FontRequest) -> Self {
        let font = &request.font;
        Self {
            weight: font.weight,
            width: font.width,
            style: font.style,
            synthesize_style: font.synthesis.style,
        }
    }
}

impl MatchRequest {
    /// Returns the attributes fontwich matches by.
    fn attributes(self) -> Attributes {
        Attributes {
            width: self.width,
            style: self.style,
            weight: self.weight,
        }
    }
}

/// What a list of fonts to try is for: everything its families and its
/// fonts are chosen by, and nothing else.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct ListKey {
    pub(super) list: NamedListId,
    /// The run's script, resolved as UAX #24 resolves it, with Han as
    /// `Hani`.
    pub(super) script: Script,
    /// The language the style chooses its fonts in; `und` for none.
    pub(super) language: Language,
    pub(super) request: MatchRequest,
    /// Whether the list is for clusters in emoji presentation.
    ///
    /// Such a list tries the list's families, then the emoji fonts, then the
    /// run's.
    pub(super) emoji: bool,
}

/// A font to try, as its family and its index there.
///
/// It holds the family handle and an index rather than the [`Font`], since
/// cloning a variable font allocates. The font says whether it draws in
/// color and whether it is pending.
#[derive(Clone, Debug)]
pub(super) struct Candidate {
    family: Family,
    index: usize,
    /// Whether its bytes could not be had when a cluster chose it.
    ///
    /// An unusable candidate is never tried again.
    unusable: bool,
}

impl Candidate {
    /// Returns the font, or `None` if the family lost it, which a snapshot
    /// never does.
    pub(super) fn font(&self) -> Option<&Font> {
        self.family.fonts().get(self.index)
    }

    /// Returns its family, which says whether it is a platform font.
    pub(super) fn family(&self) -> &Family {
        &self.family
    }

    /// Returns whether its bytes could not be had, so it is never tried
    /// again.
    pub(super) fn is_unusable(&self) -> bool {
        self.unusable
    }
}

/// The fonts a cluster tries, in the order CSS tries them.
///
/// The families come in this order, each once:
/// - each family of the list;
/// - the Standard font;
/// - fallback's families for the run;
/// - the families a missed character's walk added.
///
/// Each family gives every font tied for the best match. Families match one
/// at a time, when the walk first reaches past what is matched. So a family
/// loads when a cluster needs it, and then once per context.
pub(super) struct CandidateList {
    key: ListKey,
    /// The fallback request for the run: its script, language and class.
    ///
    /// A miss asks fontwich it again.
    fallback: FallbackRequest,
    /// The families to try, in order.
    families: Vec<Family>,
    /// How many of `families` are the list's own and the Standard font.
    ///
    /// The first available font is looked for among these.
    listed: usize,
    /// How many of `families` have been matched.
    matched: usize,
    candidates: Table<CandidateId, Candidate>,
    /// The characters whose miss has been walked, sorted.
    ///
    /// A list walks each character's miss once.
    missed: Vec<char>,
}

impl CandidateList {
    /// Makes the list for `key`, whose names are `names`, in `collection`.
    ///
    /// It holds each named family the collection has, with a generic
    /// resolved for the run. Then come the Standard font of the language,
    /// the emoji fonts for an emoji list, and fallback's families for the
    /// run. Nothing is matched yet.
    fn new(collection: &Collection, names: &[StoredName], key: ListKey) -> Self {
        let language = (key.language != Language::UND).then_some(key.language);
        // The first generic the list names is the class fallback reads (serif
        // fallbacks, Windows' monospace Arabic), as Blink's
        // `GenericFamily()` is the first generic keyword. With none, the
        // class is `Plain`, the Standard font's.
        let class = names
            .iter()
            .find_map(|name| match name {
                StoredName::Generic(generic) => Some(GenericClass::from(*generic)),
                StoredName::Named(_) => None,
            })
            .unwrap_or(GenericClass::Plain);
        let fallback = FallbackRequest::Text {
            script: key.script,
            language,
            generic: class,
        };
        let mut families: Vec<Family> = Vec::with_capacity(names.len() + 1);
        for name in names {
            // CSS skips a name the collection does not have.
            let family = match name {
                StoredName::Named(name) => collection.family(name),
                StoredName::Generic(generic) => {
                    collection.resolve(&FontFamilyName::Generic(*generic), &fallback)
                }
            };
            add_each(&mut families, family.iter());
        }
        // The Standard font comes after the list and before fallback's, as
        // Blink's `FontFallbackList::GetFontData` tries it. A list naming
        // its own generic still reaches that generic first.
        let standard = collection.fallback(&collection.key(&FallbackRequest::Standard(language)));
        add_each(&mut families, standard.first());
        let listed = families.len();
        if key.emoji {
            let emoji = FallbackRequest::Emoji(Presentation::Emoji);
            add_each(
                &mut families,
                collection.fallback(&collection.key(&emoji)).iter(),
            );
        }
        add_each(
            &mut families,
            collection.fallback(&collection.key(&fallback)).iter(),
        );
        Self {
            key,
            fallback,
            families,
            listed,
            matched: 0,
            candidates: Table::new(),
            missed: Vec::new(),
        }
    }

    /// Returns candidate `id`, or `None` past those matched so far.
    pub(super) fn get(&self, id: CandidateId) -> Option<&Candidate> {
        self.candidates.get(id)
    }

    /// Returns every candidate matched so far with its id, in the order CSS
    /// tries them.
    pub(super) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (CandidateId, &Candidate)> + ExactSizeIterator {
        self.candidates.iter()
    }

    /// Returns how many candidates come from the listed families and the
    /// Standard font, or `usize::MAX` while some of those are unmatched.
    ///
    /// Candidates are matched a family at a time, in order, so these come
    /// first. Only a private-use grapheme asks, so the count is not kept.
    #[cold]
    pub(super) fn listed_candidates(&self) -> usize {
        if self.matched < self.listed {
            return usize::MAX;
        }
        let listed = self.families.get(..self.listed).unwrap_or_default();
        self.candidates
            .iter()
            .take_while(|(_, candidate)| listed.contains(&candidate.family))
            .count()
    }

    /// Returns whether every family it has is matched.
    ///
    /// An exhausted list holds all its candidates until a miss adds a
    /// family.
    pub(super) fn is_exhausted(&self) -> bool {
        self.matched >= self.families.len()
    }

    /// Matches its next family that has any font for its request, or every
    /// family left where none has.
    fn match_next_family(&mut self) {
        let request = self.key.request;
        while let Some(family) = self.families.get(self.matched) {
            self.matched = self.matched.saturating_add(1);
            let before = self.candidates.len();
            // Loads the family, where a font is first read.
            let fonts = family.fonts();
            for index in family.matching_indices(request.attributes(), request.synthesize_style) {
                if fonts.get(index).is_none() {
                    continue;
                }
                let pushed = self.candidates.push(Candidate {
                    family: family.clone(),
                    index,
                    unusable: false,
                });
                // Every candidate id is taken, which no family reaches. The
                // fonts matched so far are the list.
                if pushed.is_none() {
                    break;
                }
            }
            if self.candidates.len() > before {
                return;
            }
        }
    }

    /// Walks fontwich's miss path for `c` in `presentation`.
    ///
    /// Adds the first family that maps `c` and is not already in the list,
    /// loading families as the walk reaches them. A list walks each
    /// character once. Where no family maps `c` at all, it goes into
    /// `unmapped`, and no list walks it again. Fallback is never asked for a
    /// private-use character or a noncharacter in text presentation.
    fn miss(
        &mut self,
        collection: &Collection,
        c: char,
        presentation: Presentation,
        unmapped: &mut Vec<char>,
    ) {
        let Err(at) = self.missed.binary_search(&c) else {
            return;
        };
        self.missed.insert(at, c);
        // Only the listed families draw a private-use character or a
        // noncharacter in text. Blink's `FallbackFontForCharacter` asks the
        // platform for no font for one, and CSS Fonts 4, section 5.4, asks
        // the same of private use.
        if matches!(presentation, Presentation::Text | Presentation::TextOnly)
            && is_private_or_noncharacter(c)
        {
            return;
        }
        let mut mapped = false;
        let families = &self.families;
        let found = collection
            .char_fallback(c, presentation, &self.fallback)
            .find(|family| {
                if !family.covers(c) {
                    return false;
                }
                mapped = true;
                !families.contains(family)
            });
        match found {
            Some(found) => self.families.push(found),
            None if !mapped => {
                if let Err(at) = unmapped.binary_search(&c) {
                    unmapped.insert(at, c);
                }
            }
            None => {}
        }
    }

    /// Returns CSS's first available font, as its family and its index there.
    ///
    /// It is the first font of the listed families, the Standard font
    /// included, that matches the list's request, has its data, and whose
    /// `unicode-range` holds U+0020. A face still pending, such as one
    /// downloading, is passed over.
    fn first_available(&self) -> Option<(Family, usize)> {
        let request = self.key.request;
        let listed = self.families.get(..self.listed).unwrap_or_default();
        listed.iter().find_map(|family| {
            let fonts = family.fonts();
            let at = family
                .matching_indices(request.attributes(), request.synthesize_style)
                .find(|&at| {
                    fonts
                        .get(at)
                        .is_some_and(|font| !font.is_pending() && font.serves(' '))
                })?;
            Some((family.clone(), at))
        })
    }

    /// Returns the first font of the listed families, the Standard font
    /// included, that matches the list's request and maps U+0020, as its
    /// family and its index there.
    ///
    /// Only a block whose first available font maps no space asks.
    #[cold]
    fn space_font(&self) -> Option<(Family, usize)> {
        let request = self.key.request;
        let listed = self.families.get(..self.listed).unwrap_or_default();
        listed.iter().find_map(|family| {
            let fonts = family.fonts();
            let at = family
                .matching_indices(request.attributes(), request.synthesize_style)
                .find(|&at| {
                    fonts.get(at).is_some_and(|font| {
                        !font.is_pending() && font.serves(' ') && font.charset().contains(' ')
                    })
                })?;
            Some((family.clone(), at))
        })
    }
}

/// Adds each of `families` to `list` that it does not have, in order.
fn add_each<'a>(list: &mut Vec<Family>, families: impl IntoIterator<Item = &'a Family>) {
    for family in families {
        if !list.contains(family) {
            list.push(family.clone());
        }
    }
}

/// Returns whether `c` is a variation selector or a joiner, for which alone
/// no font is chosen.
fn is_joining(c: char) -> bool {
    matches!(
        u32::from(c),
        0x200C | 0x200D | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF
    )
}

/// Returns whether `c` is a private-use character or a noncharacter, as
/// Blink's `Character::IsPrivateUse` and `Character::IsNonCharacter` test it.
pub(super) fn is_private_or_noncharacter(c: char) -> bool {
    let c = u32::from(c);
    matches!(c, 0xE000..=0xF8FF | 0xF0000..=0x10FFFF | 0xFDD0..=0xFDEF) || c & 0xFFFE == 0xFFFE
}

heap_bytes! {
    /// Its families and candidates, whose family handles count at their own
    /// size alone, and the characters walked.
    CandidateList { families, candidates, missed; key, fallback, listed, matched }
}

/// The lists of fonts to try, and the characters no font maps.
///
/// There is one list for each family list, script, language, matching
/// request and presentation a span or a primary font has asked for, trimmed
/// between builds to those asked for last. The unmapped memo holds the
/// characters no font of the collection maps, whose miss no list walks
/// again.
pub(super) struct CandidateLists {
    lists: LruCache<CandidateListId, CandidateList>,
    /// The unmapped memo, sorted.
    unmapped: Vec<char>,
}

impl CandidateLists {
    /// Makes an empty table that keeps `capacity` lists, allocating nothing.
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            lists: LruCache::new(capacity),
            unmapped: Vec::new(),
        }
    }

    /// Sets how many lists a trim keeps.
    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.lists.set_capacity(capacity);
    }

    /// Drops the lists used longest ago down to the capacity.
    pub(super) fn trim(&mut self) {
        self.lists.trim(|_, _| {});
    }

    /// Drops the lists keyed by a family list that `names` does not hold.
    pub(super) fn retain_named(&mut self, names: &NamedLists) {
        self.lists
            .retain(|_, list| names.contains(list.key.list), |_, _| {});
    }

    /// Drops every list, family handles included, and the unmapped memo,
    /// keeping the capacity.
    pub(super) fn clear(&mut self) {
        self.lists.clear();
        self.unmapped.clear();
    }

    /// Returns how many lists are held, for tests to see which a collection
    /// drops.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.lists.len()
    }

    /// Returns how many lists have walked the miss of `c`, and whether it is
    /// noted as mapped by no font, for tests to see a miss walked once.
    #[cfg(test)]
    pub(super) fn walked(&self, c: char) -> (usize, bool) {
        let lists = self.lists.iter();
        let walked = lists.filter(|(_, list)| list.missed.contains(&c)).count();
        (walked, self.unmapped.contains(&c))
    }

    /// Returns the list `id` names, or `None` for an id this cache did not
    /// hand out.
    pub(super) fn get(&self, id: CandidateListId) -> Option<&CandidateList> {
        self.lists.get(id)
    }

    /// Returns the list for `key`, or `None` where every id is taken.
    ///
    /// The first time, it is made in `collection` from the names `names`
    /// holds for its family list.
    pub(super) fn find_or_make(
        &mut self,
        collection: &Collection,
        names: &NamedLists,
        key: ListKey,
    ) -> Option<CandidateListId> {
        let hash = match self
            .lists
            .find_recent(|| hash_one(&key), |stored| stored.key == key)
        {
            Ok(id) => return Some(id),
            Err(hash) => hash,
        };
        let list = CandidateList::new(collection, names.get(key.list), key);
        self.lists.insert(hash, list)
    }

    /// Returns the first character of `text` whose miss `list` has not
    /// walked and that some font may map.
    ///
    /// Selectors and joiners are skipped. Returns `None` where there is no
    /// such character.
    pub(super) fn unmissed(&self, list: &CandidateList, text: &str) -> Option<char> {
        text.chars().filter(|&c| !is_joining(c)).find(|c| {
            list.missed.binary_search(c).is_err() && self.unmapped.binary_search(c).is_err()
        })
    }

    /// Matches the next family of list `id` that has any font for its
    /// request, or notes that none is left.
    pub(super) fn match_next_family(&mut self, id: CandidateListId) {
        if let Some(list) = self.lists.get_mut(id) {
            list.match_next_family();
        }
    }

    /// Walks the miss of `c` in `presentation` for list `id`, adding the
    /// family that maps it.
    ///
    /// This runs once per list and character, when the walk meets a cluster
    /// nothing in the list covers.
    pub(super) fn miss(
        &mut self,
        id: CandidateListId,
        collection: &Collection,
        c: char,
        presentation: Presentation,
    ) {
        if let Some(list) = self.lists.get_mut(id) {
            list.miss(collection, c, presentation, &mut self.unmapped);
        }
    }

    /// Marks candidate `candidate` of list `id` unusable, because its bytes
    /// could not be had.
    pub(super) fn mark_unusable(&mut self, id: CandidateListId, candidate: CandidateId) {
        if let Some(candidate) = self
            .lists
            .get_mut(id)
            .and_then(|list| list.candidates.get_mut(candidate))
        {
            candidate.unusable = true;
        }
    }

    /// Returns CSS's first available font of list `id`, as its family and
    /// its index there.
    ///
    /// Returns `None` where the collection has none, or the id is not one
    /// this cache handed out. The family is a handle, a count taken on its
    /// layer, so a warm build allocates nothing.
    pub(super) fn first_available(&self, id: CandidateListId) -> Option<(Family, usize)> {
        self.lists.get(id)?.first_available()
    }

    /// Returns the first font of list `id`'s listed families that maps
    /// U+0020, as its family and its index there. `None` where none does.
    pub(super) fn space_font(&self, id: CandidateListId) -> Option<(Family, usize)> {
        self.lists.get(id)?.space_font()
    }
}

impl HeapBytes for CandidateLists {
    /// Its table, each list's families, candidates and characters, and the
    /// unmapped memo.
    fn heap_bytes(&self) -> usize {
        let Self { lists, unmapped } = self;
        let own: usize = lists.iter().map(|(_, list)| list.heap_bytes()).sum();
        lists.heap_bytes() + own + unmapped.heap_bytes()
    }
}
