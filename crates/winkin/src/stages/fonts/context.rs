//! What font selection keeps between layouts.
//!
//! Everything here is keyed by content, never by a width or a first-line
//! cut, so every layout the context serves shares it. The keys are what a
//! family list names, a run's script and language, and a style's matching
//! attributes. There are five caches, each a type of its own:
//! - the **named** and **candidate lists** (`lists`): the families and fonts
//!   to try, in order, and the characters no font maps;
//! - **instances** (`instance`), keyed by font, variations, synthesis and
//!   features, with the faces they are cut from;
//! - **offers** (`features`): which features that are synthesized where
//!   missing a font has under a script;
//! - **feature coverages** (`positions`): what a font's `sups` or `subs`
//!   covers under a script.
//!
//! The candidate lists hold fontwich family handles, so a collection with
//! other layer snapshots drops them. The instances and faces of fonts the
//! new collection lacks go too, which releases their bytes. The rest are
//! keyed by names or by the bytes' id, which fontwich keeps stable. A cache
//! whose ids are all taken refuses new entries rather than panicking; the
//! feature coverages clear and start again.
//!
//! **All of it is a cache.** A layout's font requests name their family
//! lists by id, but only font selection reads those ids, during the build
//! that named them. No layout holds any other id into the context, since
//! its used fonts share what they shape and draw with. So any of it may be
//! dropped between calls, and the next build finds or makes it again. Each
//! cache keeps at most its capacity: [`FontContext::trim`] drops the entries
//! used longest ago before every build, when no id is held.

use core::ptr;
use fontwich::{Collection, Family, FontFamilyName, Presentation};
use parlance::{Language, Script};

use super::features::ScriptOffers;
use super::instance::Instances;
use super::lists::{
    CandidateId, CandidateListId, CandidateLists, ListKey, MatchRequest, NamedListId, NamedLists,
};
use super::positions::FeatureCoverages;
use super::shared::SharedFonts;
use crate::context::CacheLimits;
use crate::data::heap_bytes;
use crate::stages::content::FontRequest;

/// The script a style's primary font is asked for where no language says.
///
/// The primary font is chosen by the element's locale, or the default
/// locale, not by the text. fontwich resolves a generic by the language's
/// script wherever there is a language. So this counts only where the style
/// names no language and the config's default is `und`. It is Latin, the
/// script of an English default locale. The primary's small capitals are
/// also read under it.
pub(super) const PRIMARY_SCRIPT: Script = Script::from_bytes(*b"Latn");

/// A change to the context's caches that the run loop's walk stops for.
///
/// The walk reads the lists without changing them. Where it needs a change,
/// it stops at the cluster and names the change. [`FontContext::apply`]
/// makes it, and the walk goes on from that cluster.
#[derive(Copy, Clone, Debug)]
pub(super) enum CacheChange {
    /// The candidate list needs its next family matched.
    Match(CandidateListId),
    /// The candidate list needs fallback walked for a character it misses,
    /// to add the family that maps it.
    Miss {
        list: CandidateListId,
        c: char,
        presentation: Presentation,
    },
    /// A candidate's bytes could not be had.
    Unusable(CandidateListId, CandidateId),
    /// The walk needs what a candidate's position feature covers under a
    /// script.
    Coverage {
        list: CandidateListId,
        candidate: CandidateId,
        /// The OpenType script the span's fonts are asked under.
        script: [u8; 4],
        /// `sups` or `subs`.
        feature: [u8; 4],
    },
    /// The span needs its candidate list for emoji presentation, which its
    /// walk finds or makes and holds.
    EmojiList,
}

/// The caches that turn a chosen font into an instance and a used font.
///
/// They are the instances and the offers, borrowed apart from the rest of
/// the context, so a font is chosen while the walk borrows the candidate
/// lists.
pub(super) struct FontCaches<'a> {
    pub(super) instances: &'a mut Instances,
    pub(super) offers: &'a mut ScriptOffers,
}

/// Font selection's caches, which outlive every layout.
pub(crate) struct FontContext {
    collection: Collection,
    names: NamedLists,
    candidates: CandidateLists,
    instances: Instances,
    offers: ScriptOffers,
    /// What each font's `sups` and `subs` cover. The run loop reads them
    /// while it makes instances.
    coverages: FeatureCoverages,
}

impl FontContext {
    /// Makes empty caches for text set in `collection`, each keeping what
    /// `limits` says, allocating nothing.
    ///
    /// The instances take the faces they lack from `shared` where given.
    pub(crate) fn new(
        collection: Collection,
        limits: &CacheLimits,
        shared: Option<SharedFonts>,
    ) -> Self {
        let instances = limits.font_instances;
        Self {
            collection,
            names: NamedLists::new(limits.family_lists),
            candidates: CandidateLists::new(limits.fallback_lists),
            instances: Instances::new(instances, shared),
            offers: ScriptOffers::new(instances),
            coverages: FeatureCoverages::new(instances),
        }
    }

    /// Sets how many entries each cache keeps from the next trim on.
    ///
    /// The offers and coverages, read per font, keep as many as the
    /// instances.
    pub(crate) fn set_limits(&mut self, limits: &CacheLimits) {
        let instances = limits.font_instances;
        self.names.set_capacity(limits.family_lists);
        self.candidates.set_capacity(limits.fallback_lists);
        self.instances.set_capacity(instances);
        self.offers.set_capacity(instances);
        self.coverages.set_capacity(instances);
    }

    /// Drops the entries used longest ago from each cache down to its
    /// capacity.
    ///
    /// Runs between calls, when no id is held: before each build and when
    /// the limits change. The candidate lists keyed by a dropped family list
    /// go with it, since their key names it by id.
    pub(crate) fn trim(&mut self) {
        if self.names.trim() {
            self.candidates.retain_named(&self.names);
        }
        self.candidates.trim();
        self.instances.trim();
        self.offers.trim();
        self.coverages.trim();
    }

    /// Sets text in `collection` from now on.
    ///
    /// Where the stack of snapshots changes, the candidate lists, which
    /// hold family handles, are dropped. So are the faces and instances of
    /// fonts `collection` lacks, which releases their bytes. The named
    /// lists, offers and coverages stay, as do the instances of fonts both
    /// collections have, so a rebuild reads none of those fonts again.
    ///
    /// Returns whether the stack of snapshots changed.
    pub(crate) fn set_collection(&mut self, collection: Collection) -> bool {
        let changed = !same_layers(&self.collection, &collection);
        if changed {
            self.candidates.clear();
            self.instances.retain_collection(&collection);
        }
        self.collection = collection;
        changed
    }

    /// Drops every cache, keeping the collection and the shared fonts.
    ///
    /// Each cache keeps what `limits` says from now on.
    pub(crate) fn clear(&mut self, limits: &CacheLimits) {
        let instances = limits.font_instances;
        self.names = NamedLists::new(limits.family_lists);
        self.candidates = CandidateLists::new(limits.fallback_lists);
        self.instances.clear(instances);
        self.offers = ScriptOffers::new(instances);
        self.coverages = FeatureCoverages::new(instances);
    }

    /// Returns whether a face of the font `key` names is held.
    pub(crate) fn has_face(&self, key: fontwich::FontKey) -> bool {
        self.instances.has_face(key)
    }

    /// Returns the bytes of the fonts the faces hold, which the collection
    /// shares.
    pub(crate) fn face_bytes(&self) -> usize {
        self.instances.face_bytes()
    }

    /// Returns the fonts text is set in.
    pub(crate) fn collection(&self) -> &Collection {
        &self.collection
    }

    /// Returns the font instances, for tests.
    #[cfg(test)]
    pub(crate) fn instances(&self) -> &Instances {
        &self.instances
    }

    /// Returns whether family list `id` is `families`, for tests to see a
    /// list named once.
    #[cfg(test)]
    pub(crate) fn is_named(&self, id: NamedListId, families: &[FontFamilyName<'_>]) -> bool {
        self.names.is_named(id, families)
    }

    /// Returns the candidate lists, for tests to see which a collection
    /// drops.
    #[cfg(test)]
    pub(super) fn candidate_lists(&self) -> &CandidateLists {
        &self.candidates
    }

    /// Returns the feature coverages, for tests to see each made once.
    #[cfg(test)]
    pub(super) fn coverages(&self) -> &FeatureCoverages {
        &self.coverages
    }

    /// Returns how many family lists, candidate lists, instances, faces,
    /// offers and coverages are held, in that order, for tests.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> [usize; 6] {
        [
            self.names.len(),
            self.candidates.len(),
            self.instances.len(),
            self.instances.face_count(),
            self.offers.len(),
            self.coverages.len(),
        ]
    }

    /// Returns the context's id for the `font-family` list `families`, or
    /// `None` where every id is taken.
    ///
    /// The builder names each style's list as it keys the style. Nothing
    /// trims the lists until the next build starts, so the id holds for the
    /// whole build.
    pub(crate) fn family_list(&mut self, families: &[FontFamilyName<'_>]) -> Option<NamedListId> {
        self.names.find_or_make(families)
    }

    /// Returns `request`'s first available font, as its family and its
    /// index there.
    ///
    /// Looks in family list `list` under `language`, the language the
    /// request chooses its fonts in, and under [`PRIMARY_SCRIPT`]. Returns
    /// `None` where the collection has no font for it.
    pub(super) fn primary_font(
        &mut self,
        list: NamedListId,
        request: &FontRequest,
        language: Language,
    ) -> Option<(Family, usize)> {
        let id = self.primary_list(list, request, language)?;
        self.candidates.first_available(id)
    }

    /// Returns the first font that maps U+0020 among those
    /// [`primary_font`](Self::primary_font) looks through: the font a space
    /// falls back to. `None` where no listed font maps one.
    pub(super) fn space_font(
        &mut self,
        list: NamedListId,
        request: &FontRequest,
        language: Language,
    ) -> Option<(Family, usize)> {
        let id = self.primary_list(list, request, language)?;
        self.candidates.space_font(id)
    }

    /// Finds or makes the candidate list a primary font of `request` is
    /// looked for in.
    fn primary_list(
        &mut self,
        list: NamedListId,
        request: &FontRequest,
        language: Language,
    ) -> Option<CandidateListId> {
        let key = ListKey {
            list,
            script: PRIMARY_SCRIPT,
            language,
            request: MatchRequest::from(request),
            emoji: false,
        };
        self.candidates
            .find_or_make(&self.collection, &self.names, key)
    }

    /// Finds or makes the candidate list `key` names, which a span's
    /// clusters try. Returns `None` where every id is taken.
    pub(super) fn span_list(&mut self, key: ListKey) -> Option<CandidateListId> {
        self.candidates
            .find_or_make(&self.collection, &self.names, key)
    }

    /// Makes the change the run loop's walk stopped for.
    ///
    /// Each change is made at most once per context, so a warm build stops
    /// only where a span first meets emoji presentation. The span's walk
    /// makes that change itself, so `EmojiList` never reaches here.
    pub(super) fn apply(&mut self, change: CacheChange) {
        match change {
            CacheChange::Match(list) => self.candidates.match_next_family(list),
            CacheChange::Miss {
                list,
                c,
                presentation,
            } => self
                .candidates
                .miss(list, &self.collection, c, presentation),
            CacheChange::Unusable(list, candidate) => {
                self.candidates.mark_unusable(list, candidate);
            }
            CacheChange::Coverage {
                list,
                candidate,
                script,
                feature,
            } => {
                let candidate = self
                    .candidates
                    .get(list)
                    .and_then(|list| list.get(candidate));
                if let Some(candidate) = candidate
                    && let Some(font) = candidate.font()
                {
                    self.coverages.make(
                        candidate.family(),
                        font,
                        &mut self.instances,
                        script,
                        feature,
                    );
                }
            }
            CacheChange::EmojiList => debug_assert!(false, "a span's walk makes its emoji list"),
        }
    }

    /// Borrows the caches that turn a font into an instance and a used font.
    pub(super) fn font_caches(&mut self) -> FontCaches<'_> {
        FontCaches {
            instances: &mut self.instances,
            offers: &mut self.offers,
        }
    }

    /// Borrows the context apart for the run loop's walk.
    ///
    /// The walk reads the candidate lists and the feature coverages without
    /// changing them, and fills the font caches as it chooses fonts. So a
    /// candidate's charset stays borrowed from its list while instances are
    /// made.
    pub(super) fn walk_caches(&mut self) -> (&CandidateLists, &FeatureCoverages, FontCaches<'_>) {
        (
            &self.candidates,
            &self.coverages,
            FontCaches {
                instances: &mut self.instances,
                offers: &mut self.offers,
            },
        )
    }
}

heap_bytes! {
    /// Its caches' tables. The collection, which the host holds, does not
    /// count. Family handles and character sets count at their own size,
    /// without what fontwich keeps inside them.
    FontContext {
        names, candidates, instances, offers, coverages;
        collection
    }
}

/// Returns whether two collections are the same stack of layer snapshots.
///
/// A snapshot never changes, so the caches holding its families stay good.
/// The old collection is still held during the comparison, so no address
/// can have been reused.
fn same_layers(a: &Collection, b: &Collection) -> bool {
    let (mut a, mut b) = (a.layers(), b.layers());
    loop {
        match (a.next(), b.next()) {
            (None, None) => return true,
            (Some(x), Some(y)) if ptr::eq(x, y) => {}
            _ => return false,
        }
    }
}
