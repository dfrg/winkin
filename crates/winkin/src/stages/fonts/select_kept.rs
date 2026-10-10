//! Keeps the candidate a cluster took for the clusters after it that the
//! list would give it too, without asking the list.

#[cfg(test)]
use core::cell::Cell;

use fontwich::{Charset, CharsetPage};

use super::coverage;
use super::lists::{CandidateId, CandidateList};
use super::select_pick::{Pages, SpanText};
use crate::data::{Id, IdRange, Table};
use crate::stages::analysis::{ClusterClass, ClusterId};
use crate::work;

/// Whether the run loop takes its shortcut, [`KeptCandidate`].
///
/// Always true, except in tests that compare it with the walk that asks
/// the list at every cluster.
pub(super) fn shortcuts() -> bool {
    #[cfg(test)]
    {
        !PER_CLUSTER.with(Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

#[cfg(test)]
std::thread_local! {
    /// When set, the run loop on this thread takes no shortcut and asks the
    /// list at every cluster, the first line's too.
    pub(super) static PER_CLUSTER: Cell<bool> = const { Cell::new(false) };
}

/// The clusters after one the text list just set in a candidate, which keep
/// that candidate without asking the list.
///
/// The list gives a cluster the first candidate that the memo does not rule
/// out and that covers it. If every earlier candidate is ruled out until
/// `limit` (or is unusable), and this one covers a cluster as written, the
/// list would give it this one. Walking a miss would change nothing either,
/// since a found candidate memoizes nothing. So text in its requested font
/// costs a bit test per character. A cluster that draws nothing takes the font
/// before it, as in the walk.
///
/// The walk asks the list again at the first cluster that:
/// - this candidate does not cover as written;
/// - may ask for emoji or a forced presentation;
/// - holds a variation selector, which may want a font with its sequence;
/// - is `limit`, or the cluster before it when a divided grapheme's later
///   part follows, since that cluster chooses with the part.
pub(super) struct KeptCandidate<'l> {
    /// The candidate's charset, and the page of it last found.
    charset: &'l Charset,
    page: Option<CharsetPage<'l>>,
    /// The first cluster an earlier candidate may be asked about again.
    limit: ClusterId,
    /// The style's `font-variant-emoji` forces a presentation, which a
    /// keycap base takes: `#`, `*` or a digit, whose cluster is text.
    keycaps: bool,
}

impl<'l> KeptCandidate<'l> {
    /// Returns the kept clusters after `cluster`, which `list` gave candidate
    /// `at`, in a span ending at `end`.
    ///
    /// `memo` and `pages` are the list's memo and pages. `keycaps` says the
    /// style's `font-variant-emoji` forces a presentation on a keycap base.
    /// Returns `None` where it holds nothing, or where the memo does not
    /// rule out an earlier candidate past `cluster`. That happens for a
    /// pending face or a font a forced presentation passed over.
    pub(super) fn new(
        list: &'l CandidateList,
        memo: &Table<CandidateId, ClusterId>,
        pages: &Pages<'l>,
        at: CandidateId,
        cluster: ClusterId,
        end: ClusterId,
        keycaps: bool,
    ) -> Option<Self> {
        let charset = list.get(at)?.font()?.charset();
        let mut limit = end;
        for (before, candidate) in list.iter().take(at.get()) {
            work::step();
            // Never asked: the list passes over it wherever it stands.
            if candidate.is_unusable() || candidate.font().is_none() {
                continue;
            }
            match memo.get(before) {
                Some(&next) if next > cluster => limit = limit.min(next),
                _ => return None,
            }
        }
        (limit.get() > cluster.get() + 1).then(|| Self {
            charset,
            keycaps,
            page: pages.get(at),
            limit,
        })
    }
}

impl SpanText<'_> {
    /// Returns the first cluster from `from` that `kept` does not hold.
    /// The clusters before it keep the font of the one before `from`.
    pub(super) fn kept_end(&self, kept: &mut KeptCandidate<'_>, from: ClusterId) -> ClusterId {
        let clusters = self.clusters;
        // A grapheme's later part, which an item boundary divided from the
        // part before it, and so starts a span: inside one, none is.
        let divided = |cluster: ClusterId| clusters.is_continuation(cluster);
        // Each cluster starts where the one before it ends, so each end is
        // read once, with what it says of a selector.
        let mut start = clusters.start(from);
        for cluster in (from..kept.limit).ids() {
            work::step();
            debug_assert!(
                !divided(cluster),
                "a divided grapheme's part starts an item"
            );
            let (Some(attrs), Some(end)) = (clusters.attrs(cluster), clusters.end(cluster)) else {
                return cluster;
            };
            let range = start..end.end();
            start = range.end;
            let class = attrs.class();
            if coverage::draws_nothing(class) {
                continue;
            }
            // An emoji or a pictograph asks for its presentation, and a
            // variation selector for a presentation or a sequence. The list
            // must answer those.
            if matches!(class, ClusterClass::Emoji | ClusterClass::Symbol)
                || end.is_variation_selector()
            {
                return cluster;
            }
            // One ASCII character, most of a Latin text's, read as a byte:
            // no default-ignorable is ASCII, so the font covers it as
            // written where it maps it, as `covers_as_written` would say.
            if let Some(ch) = self.source.ascii(range.clone())
                && !(kept.keycaps && coverage::is_keycap_base(ch))
                && work::fast_paths()
            {
                if coverage::maps(kept.charset, ch, &mut kept.page) {
                    continue;
                }
                return cluster;
            }
            let text = self.source.slice(range);
            if kept.keycaps && text.starts_with(coverage::is_keycap_base) {
                return cluster;
            }
            if !coverage::covers_as_written(kept.charset, text, &mut kept.page) {
                return cluster;
            }
        }
        // The part after the kept clusters may be the last cluster's own,
        // where an item boundary divides a grapheme: that cluster is chosen
        // for with it, and so is asked about again.
        if divided(kept.limit) {
            return ClusterId::new(kept.limit.get() - 1).max(from);
        }
        kept.limit
    }
}
