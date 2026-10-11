//! Stored paragraphs from text analysis.

use core::ops::Range;

use crate::data::{Id, Run, RunCursor, Runs, define_flags, heap_bytes};
use crate::work;

use super::{BidiLevel, ClusterId, ParagraphId, RunOrientation};

define_flags! {
    /// What a paragraph contains, which a later stage checks to skip work.
    ///
    /// The flags are per paragraph because a relayout touches paragraphs.
    /// Gating at that grain takes the CJK passes from 310 to 30 µs.
    pub(crate) struct ParagraphFlags(u8) {
        /// A tab.
        pub(crate) const HAS_TABS = 1 << 0;
        /// A soft hyphen, whose hyphen font selection chooses a font for and
        /// measurement shapes.
        pub(crate) const HAS_SOFT_HYPHEN = 1 << 1;
        /// East Asian text: Han, kana, Hangul, Bopomofo, Yi and Tangut,
        /// their punctuation, and the fullwidth and compatibility forms, as
        /// the writer's `is_east_asian` lists them.
        pub(crate) const HAS_EAST_ASIAN = 1 << 2;
        /// Text set upright in a vertical line, or combined across it: runs
        /// whose glyphs the shaper sets down the line, or that take one em
        /// of it.
        pub(crate) const HAS_UPRIGHT = 1 << 3;
        /// Text combined across a vertical line (`text-combine-upright`): a
        /// unit of one em, which shaping fits to it and measurement spaces as
        /// one character.
        pub(crate) const HAS_COMBINED = 1 << 4;
        /// Clusters at other levels than the paragraph's own.
        ///
        /// The runs are split where the level changes, and a cluster's level
        /// is its run's. Without it every cluster is at the paragraph's level.
        pub(crate) const MIXED_LEVELS = 1 << 5;
        /// Something in it reads right to left: its level is odd, or some
        /// of its clusters' is.
        ///
        /// `MIXED_LEVELS` alone may hold only even levels, as a left-to-right
        /// override around right-to-left text sets them, so this flag says
        /// bidi is at work.
        pub(crate) const RIGHT_TO_LEFT = 1 << 6;
        /// A shaping stop inside the paragraph other than its final separator
        /// and the items' edges: a tab, atomic inline, generated break
        /// opportunity or preserved ZWNJ. Without it shaping can follow run
        /// endpoints, which include the items'.
        pub(crate) const HAS_SHAPING_STOPS = 1 << 7;
    }
}

impl From<RunOrientation> for ParagraphFlags {
    /// Returns the flags a run set as `orientation` gives its paragraph:
    /// upright text, combined text, or neither.
    fn from(orientation: RunOrientation) -> Self {
        match orientation {
            RunOrientation::Horizontal | RunOrientation::Sideways => Self::NONE,
            RunOrientation::Upright => Self::HAS_UPRIGHT,
            RunOrientation::Combined => Self::HAS_UPRIGHT.union(Self::HAS_COMBINED),
        }
    }
}

/// A paragraph, in 8 bytes.
///
/// This holds only what analysis found. Measurement keeps its own facts
/// about a paragraph in its own table. A paragraph ends where the next
/// starts, or at the text's end, which [`Paragraphs`] keeps
/// ([`Paragraphs::clusters`]).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Paragraph {
    /// Its first cluster.
    pub(crate) start: ClusterId,
    /// The base level: the block's direction, or where that is `auto`, the
    /// first strong character's (rules P2 and P3).
    pub(crate) level: BidiLevel,
    /// What it contains.
    pub(crate) flags: ParagraphFlags,
}

impl Run for Paragraph {
    type Position = ClusterId;

    #[inline]
    fn start(&self) -> ClusterId {
        self.start
    }
}

/// The paragraphs, in text order: which paragraph a cluster is in, and what
/// each is.
///
/// They tile the clusters, each ending after its separator where the next
/// starts. So a paragraph is found by its first cluster: by search for a
/// lookup of one cluster, or by a cursor for a walk forward. An analyzed
/// content always has one paragraph, and a cleared analysis none.
#[derive(Debug)]
pub(crate) struct Paragraphs {
    pub(super) paragraphs: Runs<ParagraphId, Paragraph>,
    /// Where the last paragraph ends: the end of the clusters written when
    /// it was pushed, which is the text's.
    end: ClusterId,
}

impl Paragraphs {
    pub(super) const fn new() -> Self {
        Self {
            paragraphs: Runs::new(),
            end: ClusterId(0),
        }
    }

    pub(super) fn clear(&mut self) {
        self.paragraphs.clear();
        self.end = ClusterId(0);
    }

    /// Appends the paragraph after the last, ending at `end`.
    ///
    /// Returns `None` past what a `ParagraphId` names, which there are too
    /// few clusters to reach.
    pub(super) fn push(&mut self, paragraph: Paragraph, end: ClusterId) -> Option<ParagraphId> {
        self.end = end;
        self.paragraphs.push(paragraph)
    }

    /// How many paragraphs there are.
    pub(crate) fn len(&self) -> usize {
        self.paragraphs.len()
    }

    /// Whether there are none, which happens only in a cleared analysis.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.paragraphs.is_empty()
    }

    /// Paragraph `id`, or `None` past the last.
    pub(crate) fn get(&self, id: ParagraphId) -> Option<&Paragraph> {
        self.paragraphs.get(id)
    }

    /// What paragraph `id` contains: nothing past the last.
    #[inline]
    pub(crate) fn flags(&self, id: ParagraphId) -> ParagraphFlags {
        self.paragraphs
            .get(id)
            .map_or(ParagraphFlags::NONE, |paragraph| paragraph.flags)
    }

    /// Every paragraph with its id, in text order.
    pub(crate) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (ParagraphId, &Paragraph)> + ExactSizeIterator {
        self.paragraphs.iter()
    }

    /// Paragraph `id`'s clusters, its separator last if it has one.
    ///
    /// They run to where the next starts, or the text's end. A text ending in
    /// a separator has an empty last paragraph, where the caret sits after a
    /// final `<br>`. Past the last paragraph, this is empty at the text's end.
    #[inline]
    pub(crate) fn clusters(&self, id: ParagraphId) -> Range<ClusterId> {
        self.paragraphs.span(id, self.end)
    }

    /// The first paragraph's clusters, or `None` in an analysis cleared.
    pub(super) fn first_clusters(&self) -> Option<Range<ClusterId>> {
        let first = ParagraphId::new(0);
        self.paragraphs.get(first).map(|_| self.clusters(first))
    }

    /// The paragraph holding `cluster`, by search: the last whose first
    /// cluster is not past it. The text's end is in the last paragraph.
    pub(crate) fn containing(&self, cluster: ClusterId) -> Option<ParagraphId> {
        self.paragraphs.containing(cluster)
    }

    /// The clusters of the paragraph holding `cluster`, by search, or
    /// `None` in a cleared analysis.
    pub(crate) fn clusters_containing(&self, cluster: ClusterId) -> Option<Range<ClusterId>> {
        self.containing(cluster).map(|id| self.clusters(id))
    }

    /// Whether some paragraph reads left to right, and whether some reads
    /// right to left, by their base levels.
    ///
    /// Only under `auto` do paragraphs differ from the block's direction.
    pub(crate) fn directions(&self) -> (bool, bool) {
        let (mut ltr, mut rtl) = (false, false);
        for (_, paragraph) in self.paragraphs.iter() {
            work::step();
            if paragraph.level.is_rtl() {
                rtl = true;
            } else {
                ltr = true;
            }
            if ltr && rtl {
                break;
            }
        }
        (ltr, rtl)
    }

    /// Whether the text's end is on a line: its last paragraph holds a
    /// cluster. A text ending in a separator ends in an empty paragraph,
    /// which draws no line, and a cleared analysis has none.
    pub(crate) fn ends_on_a_line(&self) -> bool {
        self.paragraphs
            .last()
            .is_some_and(|last| last.start < self.end)
    }
}

/// The cursor a walk over the paragraphs holds ([`Segments`]).
///
/// [`Segments`]: crate::stages::Segments
impl Paragraphs {
    /// A cursor at paragraph `id`, from a link to it: a line's.
    #[inline]
    pub(crate) fn cursor(&self, id: ParagraphId) -> RunCursor<ParagraphId, ClusterId> {
        self.paragraphs.cursor(id, self.end)
    }

    /// A cursor at the paragraph holding `cluster`, by search; `None` only
    /// in a cleared analysis.
    pub(crate) fn cursor_containing(
        &self,
        cluster: ClusterId,
    ) -> Option<RunCursor<ParagraphId, ClusterId>> {
        self.paragraphs.cursor_containing(cluster, self.end)
    }

    /// Moves `cursor` forward to the paragraph holding boundary `at`, never
    /// past the last, which holds the text's end ([`Runs::step_to`]).
    #[inline]
    pub(crate) fn step_to(&self, cursor: &mut RunCursor<ParagraphId, ClusterId>, at: ClusterId) {
        self.paragraphs.step_to(cursor, at, self.end);
    }
}

heap_bytes! {
    Paragraphs { paragraphs; end }
}
