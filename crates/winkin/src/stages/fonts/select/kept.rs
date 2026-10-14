//! Keeps a cluster's candidate for the clusters after it that the list would
//! give the same candidate, without asking the list.

#[cfg(test)]
use core::cell::Cell;

use fontwich::{Charset, CharsetPage};

use super::pick::{Pages, SpanText};
use crate::data::{Id, IdRange, Table};
use crate::stages::analysis::{ClusterClass, ClusterId};
use crate::stages::fonts::coverage;
use crate::stages::fonts::lists::{CandidateId, CandidateList};
use crate::work;

/// Returns whether the run loop takes its shortcut, [`KeptCandidate`].
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
    pub(crate) static PER_CLUSTER: Cell<bool> = const { Cell::new(false) };
}

/// The clusters after one that the text list just set in a candidate, which
/// keep that candidate without asking the list.
///
/// The list gives a cluster the first candidate that the memo does not rule
/// out and that covers it. Where every earlier candidate is ruled out until
/// `limit` (or is unusable), and this one covers a cluster as written, the
/// list would give the cluster this one. A found candidate memoizes nothing,
/// so walking the list would change nothing either. So text in its requested
/// font costs a bit test per character. A cluster that draws nothing takes
/// the font before it, as in the walk.
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
    /// The first cluster at which an earlier candidate may be asked again.
    limit: ClusterId,
    /// Whether the style's `font-variant-emoji` forces a presentation on a
    /// keycap base: `#`, `*` or a digit, whose cluster is text.
    keycaps: bool,
}

impl<'l> KeptCandidate<'l> {
    /// Returns the kept clusters after `cluster`, which `list` gave candidate
    /// `at`, in a span ending at `end`.
    ///
    /// `memo` and `pages` are the list's memo and pages. `keycaps` says the
    /// style's `font-variant-emoji` forces a presentation on a keycap base.
    /// Returns `None` where no cluster would be kept, or where the memo does
    /// not rule out an earlier candidate past `cluster`. The memo leaves in
    /// a font whose colour the presentation passed over, and one that
    /// covered only a sequence's base.
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
            // The list never tries such a candidate.
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
    ///
    /// The clusters before it keep the font of the cluster before `from`.
    pub(super) fn kept_end(&self, kept: &mut KeptCandidate<'_>, from: ClusterId) -> ClusterId {
        let clusters = self.clusters;
        // A grapheme's later part divided from the part before it by an item
        // boundary. It starts an item, so none falls inside a span's kept
        // clusters.
        let divided = |cluster: ClusterId| clusters.is_continuation(cluster);
        // Each cluster starts where the one before it ends, so each end is
        // read once, with its selector flag.
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
            // variation selector for a presentation or a sequence. Only the
            // list answers those.
            if matches!(class, ClusterClass::Emoji | ClusterClass::Symbol)
                || end.is_variation_selector()
            {
                return cluster;
            }
            // A cluster of one ASCII character, most of Latin text, is read
            // as a byte. No default-ignorable is ASCII, so the font covers it
            // as written exactly where it maps it, as `covers_as_written`
            // would say.
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
        // Where an item boundary divides a grapheme at `limit`, the cluster
        // before it chooses with the later part, so the list is asked again
        // there.
        if divided(kept.limit) {
            return ClusterId::new(kept.limit.get() - 1).max(from);
        }
        kept.limit
    }
}
