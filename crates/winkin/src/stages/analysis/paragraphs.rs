//! Stored paragraphs from text analysis.

use core::ops::Range;

use crate::data::{Id, Run, RunCursor, Runs, define_flags, heap_bytes};
use crate::work;

use super::{BidiLevel, ClusterId, ParagraphId, RunOrientation};

define_flags! {
    /// What a paragraph contains, which later stages check to skip work.
    ///
    /// The flags are per paragraph because a relayout touches paragraphs.
    /// Gating at that grain takes the CJK passes from 310 to 30 µs.
    pub(crate) struct ParagraphFlags(u8) {
        /// A tab.
        pub(crate) const HAS_TABS = 1 << 0;
        /// A soft hyphen or a hyphenation opportunity the host found, whose
        /// hyphen needs a font and shaping. Without one, the measure scan's
        /// plain fast path skips its hyphen tests.
        pub(crate) const HAS_HYPHENS = 1 << 1;
        /// East Asian text, as `classify::is_east_asian` defines it.
        pub(crate) const HAS_EAST_ASIAN = 1 << 2;
        /// Upright or combined text in a vertical line: runs set down the
        /// line, or taking one em of it.
        pub(crate) const HAS_UPRIGHT = 1 << 3;
        /// Text combined across a vertical line (`text-combine-upright`): a
        /// one-em unit that shaping fits and measurement spaces as one
        /// character.
        pub(crate) const HAS_COMBINED = 1 << 4;
        /// Clusters at levels other than the paragraph's.
        ///
        /// Runs split where the level changes, and a cluster's level is its
        /// run's. Without it every cluster is at the paragraph's level.
        pub(crate) const MIXED_LEVELS = 1 << 5;
        /// An odd level: the paragraph's or some cluster's.
        ///
        /// `MIXED_LEVELS` alone may mean only even levels, as a left-to-right
        /// override around right-to-left text produces, so this flag says
        /// bidi is at work.
        pub(crate) const RIGHT_TO_LEFT = 1 << 6;
        /// A shaping stop other than the final separator and item edges: a
        /// tab, atomic inline, generated break opportunity or preserved ZWNJ.
        /// Without it shaping can follow run endpoints, which include the
        /// items'.
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
/// Holds only what analysis found; measurement keeps its own paragraph
/// table. A paragraph ends where the next starts, or at the text's end,
/// which [`Paragraphs`] keeps ([`Paragraphs::clusters`]).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Paragraph {
    /// Its first cluster.
    pub(crate) start: ClusterId,
    /// The base level: the block's direction, or under `auto` the first
    /// strong character's (rules P2 and P3).
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

/// The paragraphs, in text order.
///
/// They tile the clusters, each ending after its separator where the next
/// starts, so a paragraph is found by its first cluster: by search for one
/// cluster, or by a cursor walking forward. An analyzed content has at
/// least one paragraph; a cleared analysis has none.
#[derive(Debug)]
pub(crate) struct Paragraphs {
    pub(super) paragraphs: Runs<ParagraphId, Paragraph>,
    /// The end of the last paragraph: the end of the clusters written when
    /// it was pushed, which is the text's end.
    end: ClusterId,
    /// Whether the last paragraph holds no cluster but a box with an inline
    /// margin, border or padding, which makes a line of its own.
    end_line: bool,
}

impl Paragraphs {
    pub(super) const fn new() -> Self {
        Self {
            paragraphs: Runs::new(),
            end: ClusterId(0),
            end_line: false,
        }
    }

    pub(super) fn clear(&mut self) {
        self.paragraphs.clear();
        self.end = ClusterId(0);
        self.end_line = false;
    }

    /// Sets whether the last paragraph, holding no cluster, makes a line.
    pub(super) fn set_end_line(&mut self, end_line: bool) {
        self.end_line = end_line;
    }

    /// Returns whether the last paragraph holds no cluster but makes a line,
    /// because a box in it has an inline margin, border or padding.
    ///
    /// It makes a line box of the strut's height: per CSS 2,
    /// section 9.4.2, a line with such a box is no phantom line.
    pub(crate) fn has_end_line(&self) -> bool {
        self.end_line
    }

    /// Appends the paragraph after the last, ending at `end`.
    ///
    /// Returns `None` once `ParagraphId`s run out, which no text has enough
    /// clusters to reach.
    pub(super) fn push(&mut self, paragraph: Paragraph, end: ClusterId) -> Option<ParagraphId> {
        self.end = end;
        self.paragraphs.push(paragraph)
    }

    /// Returns the number of paragraphs.
    pub(crate) fn len(&self) -> usize {
        self.paragraphs.len()
    }

    /// Returns whether there are no paragraphs, as only in a cleared
    /// analysis.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.paragraphs.is_empty()
    }

    /// Returns paragraph `id`, or `None` past the last.
    pub(crate) fn get(&self, id: ParagraphId) -> Option<&Paragraph> {
        self.paragraphs.get(id)
    }

    /// Returns paragraph `id`'s flags; none past the last.
    #[inline]
    pub(crate) fn flags(&self, id: ParagraphId) -> ParagraphFlags {
        self.paragraphs
            .get(id)
            .map_or(ParagraphFlags::NONE, |paragraph| paragraph.flags)
    }

    /// Returns every paragraph with its id, in text order.
    pub(crate) fn iter(
        &self,
    ) -> impl DoubleEndedIterator<Item = (ParagraphId, &Paragraph)> + ExactSizeIterator {
        self.paragraphs.iter()
    }

    /// Returns paragraph `id`'s clusters, its separator last if it has one.
    ///
    /// They run to the next paragraph's start or the text's end. A text
    /// ending in a separator has an empty last paragraph, where the caret
    /// sits after a final `<br>`. Past the last paragraph, the range is empty
    /// at the text's end.
    #[inline]
    pub(crate) fn clusters(&self, id: ParagraphId) -> Range<ClusterId> {
        self.paragraphs.span(id, self.end)
    }

    /// Returns the first paragraph's clusters, or `None` in a cleared
    /// analysis.
    pub(super) fn first_clusters(&self) -> Option<Range<ClusterId>> {
        let first = ParagraphId::new(0);
        self.paragraphs.get(first).map(|_| self.clusters(first))
    }

    /// Returns the paragraph containing `cluster`, by search: the last whose
    /// first cluster is not past it. The text's end is in the last paragraph.
    pub(crate) fn containing(&self, cluster: ClusterId) -> Option<ParagraphId> {
        self.paragraphs.containing(cluster)
    }

    /// Returns the clusters of the paragraph containing `cluster`, by
    /// search, or `None` in a cleared analysis.
    pub(crate) fn clusters_containing(&self, cluster: ClusterId) -> Option<Range<ClusterId>> {
        self.containing(cluster).map(|id| self.clusters(id))
    }

    /// Returns whether some paragraph's base level is left to right, and
    /// whether some is right to left.
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

    /// Returns whether the text's end is on a line, which holds when the
    /// last paragraph has a cluster. A text ending in a separator ends in an
    /// empty paragraph, which draws no line; a cleared analysis has none.
    pub(crate) fn ends_on_a_line(&self) -> bool {
        self.paragraphs
            .last()
            .is_some_and(|last| last.start < self.end)
    }
}

/// Cursors for walks over the paragraphs ([`Segments`]).
///
/// [`Segments`]: crate::stages::Segments
impl Paragraphs {
    /// Returns a cursor at paragraph `id`, from a link such as a line's.
    #[inline]
    pub(crate) fn cursor(&self, id: ParagraphId) -> RunCursor<ParagraphId, ClusterId> {
        self.paragraphs.cursor(id, self.end)
    }

    /// Returns a cursor at the paragraph containing `cluster`, by search;
    /// `None` only in a cleared analysis.
    pub(crate) fn cursor_containing(
        &self,
        cluster: ClusterId,
    ) -> Option<RunCursor<ParagraphId, ClusterId>> {
        self.paragraphs.cursor_containing(cluster, self.end)
    }

    /// Moves `cursor` forward to the paragraph containing boundary `at`,
    /// never past the last, which contains the text's end
    /// ([`Runs::step_to`]).
    #[inline]
    pub(crate) fn step_to(&self, cursor: &mut RunCursor<ParagraphId, ClusterId>, at: ClusterId) {
        self.paragraphs.step_to(cursor, at, self.end);
    }
}

heap_bytes! {
    Paragraphs { paragraphs; end, end_line }
}
