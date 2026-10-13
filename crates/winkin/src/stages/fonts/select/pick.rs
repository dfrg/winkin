//! Tries a candidate list on a grapheme: what the grapheme asks of a font,
//! the memo's first pass, the second pass and the partial cover.

use alloc::boxed::Box;

use fontwich::{Charset, CharsetPage, FaceId, Font, Presentation};

use super::ItemGraphemes;
use crate::data::{Id, IdRange, Table, TextOffset};
use crate::stages::analysis::{ClusterClass, ClusterEnd, ClusterId, Clusters};
use crate::stages::content::TextCursor;
use crate::stages::fonts::WantedFaces;
use crate::stages::fonts::context::CacheChange;
use crate::stages::fonts::coverage;
use crate::stages::fonts::lists::{
    Candidate, CandidateId, CandidateList, CandidateListId, CandidateLists,
    is_private_or_noncharacter,
};
use crate::style::FontVariantEmoji;
use crate::unicode;
use crate::unicode::is_variation_selector;

/// How many candidates of a list keep their last [`CharsetPage`] while a
/// span is walked. Past these a candidate searches its charset once a
/// character; the first covers nearly everything.
const PAGES: usize = 8;

/// The charset page each of a list's first [`PAGES`] candidates last found.
///
/// Kept while a span is walked, so text in one candidate costs a bit test
/// per character, and a search only where the text leaves the page.
pub(super) struct Pages<'a>([Option<CharsetPage<'a>>; PAGES]);

impl<'a> Pages<'a> {
    /// None found yet.
    pub(super) const fn new() -> Self {
        Self([None; PAGES])
    }

    /// Where `candidate` keeps the page it last found, or `None` for a
    /// candidate past the first [`PAGES`], which keeps none.
    pub(super) fn get_mut(
        &mut self,
        candidate: CandidateId,
    ) -> Option<&mut Option<CharsetPage<'a>>> {
        self.0.get_mut(candidate.get())
    }

    /// The page `candidate` last found, if it keeps one and found one.
    pub(super) fn get(&self, candidate: CandidateId) -> Option<CharsetPage<'a>> {
        self.0.get(candidate.get()).copied().flatten()
    }
}

/// Whether pending `font` wants a character of `text`.
fn wants(font: &Font, text: &str) -> bool {
    text.chars().any(|ch| font.wants(ch))
}

/// Adds pending `face`, which wants a character of a cluster, to the faces
/// the host fetches. Each face goes in once.
fn want(wanted: &mut WantedFaces, face: FaceId) {
    let faces = wanted.get_or_insert_with(Box::default);
    if !faces.contains(&face) {
        faces.push(face);
    }
}

/// What a grapheme asks of each candidate it tries.
///
/// A grapheme, to font selection, is a cluster and the continuation
/// clusters that join it ([`ItemGraphemes`]).
#[derive(Copy, Clone)]
pub(super) struct GraphemeNeeds<'t> {
    /// The grapheme's text.
    text: &'t str,
    /// The presentation it asks for.
    pub(super) presentation: Presentation,
    /// Whether it holds a variation sequence cluster matching asks a font
    /// for: the first pass then takes a font only with every such sequence.
    sequences: bool,
    /// The sequence whose presence in a font's `cmap` accepts the font for
    /// the presentation whatever its colour, for a forced one.
    presented: Option<(char, char)>,
    /// It starts with a private-use character or a noncharacter in text,
    /// which only the listed families and the Standard font draw.
    listed_only: bool,
}

impl<'t> GraphemeNeeds<'t> {
    /// Returns what a grapheme of `text` asks under `font-variant-emoji`
    /// `variant`.
    ///
    /// `class` is its first cluster's class, and `selector` says whether it
    /// holds a variation selector. A grapheme without one costs nothing beyond
    /// its presentation.
    pub(super) fn new(
        class: ClusterClass,
        text: &'t str,
        selector: bool,
        variant: FontVariantEmoji,
    ) -> Self {
        let presentation = coverage::presentation(class, text, variant);
        Self {
            text,
            presentation,
            sequences: selector && coverage::asks_sequences(text),
            presented: coverage::presentation_sequence(text, presentation),
            listed_only: presentation.base() == Presentation::Text
                && text.chars().next().is_some_and(is_private_or_noncharacter),
        }
    }

    /// The same needs in presentation `p`: a forced form's base, tried
    /// where no font takes the form.
    fn with_presentation(self, p: Presentation) -> Self {
        Self {
            presentation: p,
            presented: coverage::presentation_sequence(self.text, p),
            ..self
        }
    }

    /// Whether `font` may draw the grapheme in its presentation.
    ///
    /// It may if its colour agrees, or if its `cmap` has the sequence that
    /// asks for the presentation. Blink likewise accepts a monochrome font
    /// with a VS16 sequence for emoji.
    fn accepts(&self, font: &Font) -> bool {
        self.presentation.accepts(font)
            || self
                .presented
                .is_some_and(|(base, selector)| font.maps_variation_sequence(base, selector))
    }

    /// Whether `font`, which covers the grapheme, has every variation sequence
    /// of it cluster matching asks for: always, where it asks for none.
    fn has_sequences(&self, font: &Font) -> bool {
        !self.sequences || coverage::has_sequences(font, self.text)
    }

    /// Returns how `candidate` fits the grapheme, with `page` its charset
    /// page last found.
    ///
    /// The walk and generated text both test candidates here. A pending face
    /// that wants the grapheme goes in `wanted`.
    #[inline]
    pub(super) fn fit<'l>(
        &self,
        candidate: &'l Candidate,
        wanted: &mut WantedFaces,
        page: &mut Option<CharsetPage<'l>>,
    ) -> CandidateFit<'l> {
        if candidate.is_unusable() {
            return CandidateFit::Passed;
        }
        let Some(font) = candidate.font() else {
            return CandidateFit::Passed;
        };
        if font.is_pending() {
            // Nothing to draw with yet: the face to fetch, if it wants any
            // of this, and the list goes on to draw something meanwhile.
            if wants(font, self.text)
                && let Some(face) = font.face_id()
            {
                want(wanted, face);
            }
            return CandidateFit::Pending(font);
        }
        if !self.accepts(font) {
            return CandidateFit::Passed;
        }
        if !coverage::covers(font, self.text, page) {
            return CandidateFit::Misses(font);
        }
        if self.has_sequences(font) {
            CandidateFit::Covers
        } else {
            CandidateFit::CoversBase
        }
    }
}

/// How a candidate fits a grapheme's [`GraphemeNeeds`].
pub(super) enum CandidateFit<'l> {
    /// It is not asked: its bytes are lost, or its colour is not the
    /// presentation's.
    Passed,
    /// It is a face still pending, which draws nothing yet: its font, for
    /// the walk's memo.
    Pending(&'l Font),
    /// It covers the grapheme, with every variation sequence of it cluster
    /// matching asks for: the first pass's answer.
    Covers,
    /// It covers the grapheme without every sequence: the second pass's answer,
    /// used where no candidate of the whole list has them.
    CoversBase,
    /// It does not cover the grapheme: its font, for the walk's memo.
    Misses(&'l Font),
}

/// What trying a candidate list on a cluster found.
pub(super) enum ListOutcome {
    /// This candidate covers it.
    Found(CandidateId),
    /// The list's next family is to be matched first.
    Match,
    /// Nothing the list has so far covers it with every sequence it asks
    /// for, and every family the list has is matched: the first
    /// that covers it without them, if any, cluster matching's second pass.
    Exhausted(Option<CandidateId>),
}

/// What `list`, none of whose candidates covers a grapheme with every sequence
/// it asks for, says, `base` the first that covers it without them: that
/// it has matched every family, or that the next is to be matched first.
pub(super) fn second_pass(list: &CandidateList, base: Option<CandidateId>) -> ListOutcome {
    if list.is_exhausted() {
        ListOutcome::Exhausted(base)
    } else {
        ListOutcome::Match
    }
}

/// Returns the candidate of `list` (named `list_id`) that a grapheme with
/// `needs` is set in, using `pick` to try the list.
///
/// It tries, in order:
/// 1. the first candidate that covers the grapheme;
/// 2. the family that fallback finds for a missing character, walked once
///    per list and character;
/// 3. the second pass, which ignores variation sequences;
/// 4. for a forced presentation that nothing takes, the base form;
/// 5. the best partial cover.
///
/// Returns `None` where nothing draws any of it, and `Err` with the change
/// to make where a family must be matched or a fallback walked first.
pub(super) fn resolve(
    lists: &CandidateLists,
    list: &CandidateList,
    list_id: CandidateListId,
    needs: &GraphemeNeeds<'_>,
    mut pick: impl FnMut(&GraphemeNeeds<'_>) -> ListOutcome,
) -> Result<Option<CandidateId>, CacheChange> {
    let base = match pick(needs) {
        ListOutcome::Found(at) => return Ok(Some(at)),
        ListOutcome::Match => return Err(CacheChange::Match(list_id)),
        ListOutcome::Exhausted(base) => base,
    };
    if let Some(c) = lists.unmissed(list, needs.text) {
        return Err(CacheChange::Miss {
            list: list_id,
            c,
            presentation: needs.presentation,
        });
    }
    if let Some(at) = base {
        return Ok(Some(at));
    }
    if needs.listed_only {
        return Ok(None);
    }
    let p = needs.presentation;
    let base = if p == p.base() {
        ListOutcome::Exhausted(None)
    } else {
        pick(&needs.with_presentation(p.base()))
    };
    Ok(match base {
        ListOutcome::Found(at) | ListOutcome::Exhausted(Some(at)) => Some(at),
        _ => best_partial(list, needs.text),
    })
}

/// Whether the position feature covering `chars` has a glyph for every
/// drawn character of `text`.
///
/// `page` is the page of `chars` last found. Default-ignorables are skipped:
/// the shaper hides them, so there is no glyph to vary. Returns `false`
/// where the font has no key, so its coverage could not be read.
pub(super) fn feature_covers<'a>(
    chars: Option<&'a Charset>,
    text: &str,
    page: &mut Option<CharsetPage<'a>>,
) -> bool {
    let Some(chars) = chars else {
        return false;
    };
    text.chars()
        .all(|ch| unicode::rare_props(ch).is_default_ignorable() || coverage::maps(chars, ch, page))
}

/// What trying a candidate list on a cluster reads, besides the list.
pub(super) struct SpanText<'w> {
    pub(super) source: &'w TextCursor<'w>,
    pub(super) clusters: &'w Clusters,
    /// The span's end, which the memo's scans stop at.
    pub(super) end: ClusterId,
    /// Where its item's graphemes cross the item's edges.
    pub(super) graphemes: ItemGraphemes,
}

impl<'w> SpanText<'w> {
    /// The text `cluster` is chosen for with: its own, and that of the later
    /// parts of its grapheme that join it.
    fn grapheme_text(&self, cluster: ClusterId) -> &'w str {
        let start = self.clusters.range(cluster).start;
        let end = self
            .clusters
            .range(self.graphemes.grapheme_last(cluster))
            .end;
        self.source.slice(start..end)
    }

    /// Returns the grapheme's text, as [`grapheme_text`](Self::grapheme_text) gives it,
    /// and whether it holds a variation selector.
    ///
    /// `start` and `end` are the cluster's start and end, which the walk has
    /// read. The analysis flags a selector in the cluster's own characters
    /// in `end`. A joined later part may start with one, so its text is
    /// scanned.
    pub(super) fn grapheme(
        &self,
        cluster: ClusterId,
        start: TextOffset,
        end: ClusterEnd,
    ) -> (&'w str, bool) {
        let own = start..end.end();
        let mut selector = end.is_variation_selector();
        let last = self.graphemes.grapheme_last(cluster);
        if last == cluster {
            return (self.source.slice(own), selector);
        }
        let grapheme_end = self.clusters.range(last).end;
        let joined = self.source.slice(own.end..grapheme_end);
        selector |= joined.chars().any(is_variation_selector);
        (self.source.slice(own.start..grapheme_end), selector)
    }

    /// Tries `list` for the grapheme at `cluster`, memoizing each candidate
    /// that misses or is pending.
    ///
    /// Finds the first candidate that `memo` does not rule out, that accepts
    /// the presentation and that covers the grapheme. A pending face that wants
    /// the grapheme goes in `wanted`.
    ///
    /// A grapheme with a variation sequence wants a candidate that also has the
    /// sequence, searched over the whole list as far as matching and
    /// fallback go. Only if none has it does the first that covers the bases
    /// win, drawing them and hiding the selectors. These are Blink's two
    /// passes. Such a base-only candidate is not memoized, since it covers
    /// the cluster.
    pub(super) fn pick<'l>(
        &self,
        list: &'l CandidateList,
        memo: &mut Table<CandidateId, ClusterId>,
        pages: &mut Pages<'l>,
        wanted: &mut WantedFaces,
        cluster: ClusterId,
        needs: &GraphemeNeeds<'_>,
    ) -> ListOutcome {
        // The first candidate covering the grapheme without every sequence: the
        // second pass's answer.
        let mut base: Option<CandidateId> = None;
        let tried = if needs.listed_only {
            list.listed_candidates()
        } else {
            usize::MAX
        };
        for (at, candidate) in list.iter().take(tried) {
            if memo.get(at).is_some_and(|&next| next > cluster) {
                continue;
            }
            let mut spare: Option<CharsetPage<'l>> = None;
            let page = pages.get_mut(at).unwrap_or(&mut spare);
            match needs.fit(candidate, wanted, page) {
                CandidateFit::Covers => return ListOutcome::Found(at),
                CandidateFit::CoversBase => {
                    base.get_or_insert(at);
                }
                CandidateFit::Misses(font) => {
                    let next = self.next_covered(font, page, cluster);
                    memo.set_growing(at, next, ClusterId::new(0));
                }
                // Skipped like a miss until the next grapheme that wants it,
                // which wants it again.
                CandidateFit::Pending(font) => {
                    let next = self.next_wanting(font, cluster);
                    memo.set_growing(at, next, ClusterId::new(0));
                }
                CandidateFit::Passed => {}
            }
        }
        // Every listed family is matched, and no other is tried.
        if tried != usize::MAX {
            return ListOutcome::Exhausted(base);
        }
        second_pass(list, base)
    }

    /// The next cluster of the span after `cluster` that `font` covers, or
    /// the span's end: until then the font is skipped. Clusters that draw
    /// nothing are passed over, since they never ask.
    fn next_covered<'l>(
        &self,
        font: &'l Font,
        page: &mut Option<CharsetPage<'l>>,
        cluster: ClusterId,
    ) -> ClusterId {
        let after = ClusterId::new(cluster.get() + 1);
        for cluster in (after..self.end).ids() {
            if self.asks(cluster) && coverage::covers(font, self.grapheme_text(cluster), page) {
                return cluster;
            }
        }
        self.end
    }

    /// The next cluster of the span after `cluster` whose grapheme pending
    /// `font` wants a character of, or the span's end: until then the font
    /// is skipped, as it draws nothing and is wanted for nothing new.
    fn next_wanting(&self, font: &Font, cluster: ClusterId) -> ClusterId {
        let after = ClusterId::new(cluster.get() + 1);
        for cluster in (after..self.end).ids() {
            if self.asks(cluster) && wants(font, self.grapheme_text(cluster)) {
                return cluster;
            }
        }
        self.end
    }

    /// Whether `cluster` asks the list for a font: it draws, and is not the
    /// later part of a grapheme that takes the part before it.
    fn asks(&self, cluster: ClusterId) -> bool {
        !self.graphemes.joins(cluster)
            && self
                .clusters
                .class(cluster)
                .is_some_and(|class| !coverage::draws_nothing(class))
    }
}

/// Returns the candidate of `list` that draws the most of `chars`.
///
/// Covering the base character ranks first, then the count. `None` where
/// none draws any of it.
fn best_partial(list: &CandidateList, chars: &str) -> Option<CandidateId> {
    let mut best: Option<(CandidateId, coverage::Score)> = None;
    for (at, candidate) in list.iter() {
        if candidate.is_unusable() {
            continue;
        }
        let Some(score) = candidate
            .font()
            .filter(|font| !font.is_pending())
            .and_then(|font| coverage::score(font, chars))
        else {
            continue;
        };
        if best.is_none_or(|(_, held)| score.beats(held)) {
            best = Some((at, score));
        }
    }
    best.map(|(at, _)| at)
}
