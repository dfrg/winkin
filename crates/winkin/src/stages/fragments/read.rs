//! Reads a text item back: its placed glyphs and its clusters' advances.
//!
//! The layout's `TextRun` wraps these walks, and the tests read positions
//! through them.
//!
//! **Where a glyph stands:** the item's exact `inline`, plus the exact
//! advances of the clusters before it, plus the glyph's own offsets.
//! - A cluster's advance is its prefix step, so its left is one subtraction
//!   from its prefix entry. A paint walk reads a word and a sum per glyph.
//! - Where reshaped line edges hold some clusters, their advances and
//!   glyphs come from the reshaped pieces, walked with a running sum. The
//!   paragraph's glyphs are never read for a reshaped cluster.
//! - Right to left, clusters are drawn from the item's right end back. Their
//!   glyphs are already in drawing order.
//!
//! **A glyph's advance** is the shaper's, except a cluster's last glyph,
//! which takes the rest of the cluster's advance. So the glyphs add up to
//! the cluster, spacing and justification included, as Chrome adds letter
//! spacing and justification room to a cluster's last glyph.
//!
//! **On a justified line** a cluster's advance adds `per` for each of its
//! opportunities, counted by the same rule line layout used. The last
//! opportunity also takes the leftover. A cluster with an opportunity before
//! it moves its glyphs by that room, as Chrome offsets an ideograph after a
//! letter. The line's item names its record; nothing is stored per cluster.
//!
//! **No item searches for its run or items.** Its record names its shaping
//! run and its content item, and its line names its paragraph.
//!
//! **An item's advance** is the same sum over all its clusters, stored on
//! the item. The walks add up to it, and a right-to-left walk starts from
//! it.
//!
//! Nothing is allocated and nothing restarts. Each walk holds a few words of
//! state and hands out each glyph or cluster once.
//!
//! **A ruby annotation's text** is on a line of its own, and the prefix sums
//! give it nothing. Its clusters step by their glyphs' own advances, which
//! shaping keeps for it, plus spacing, with a running sum.

use super::justify::{Justified, JustifiedLine};
use super::{FragmentItem, FragmentItemFlags, Fragments};
use crate::data::{Id, Table};
use crate::stages::analysis::{Analysis, ClusterId};
use crate::stages::content::{ContentFlags, ItemId, ShapingFactsId, VariantText};
use crate::stages::lines::{EdgeClusterId, EdgeShape, LineId, LineRecord, Lines};
use crate::stages::measure::{JustifyOpportunities, LetterWordSpacing, WordSpacingRule};
use crate::stages::shape::{ClusterGlyphs, GlyphWord, ShapedText, SidecarGlyph, SidecarGlyphId};
use crate::stages::{LineStages, Stages};
use crate::unit::{InlineLayoutUnit, TextUnit};
use core::fmt;

/// What a reader reads a text item back through: the prepared tables, the
/// lines and the fragment items.
///
/// A reader borrows a finished layout, which keeps no area and no config, so
/// this is not line layout's input.
pub(crate) struct ReadInput<'a> {
    stages: Stages<'a>,
    lines: &'a Lines,
    fragments: &'a Fragments,
}

impl<'a> ReadInput<'a> {
    /// Bundles `fragments` with the `lines` and `stages` they were laid out
    /// from.
    pub(crate) fn new(stages: Stages<'a>, lines: &'a Lines, fragments: &'a Fragments) -> Self {
        Self {
            stages,
            lines,
            fragments,
        }
    }
}

/// How a ruby annotation's text steps along its own line: its glyphs'
/// advances plus the letter- and word-spacing after each cluster.
///
/// Line layout places annotation text by it, and a reader reads it back by
/// it.
#[derive(Copy, Clone)]
pub(super) struct AnnotationText<'a> {
    analysis: &'a Analysis,
    /// The text its clusters are read in.
    source: VariantText<'a>,
    /// The shaping of its line's variant.
    shaped: &'a ShapedText,
    /// The spacing its style adds after each cluster, by the rule the
    /// prefix sums were built with. One applies to all its clusters.
    spacing: LetterWordSpacing,
}

impl<'a> AnnotationText<'a> {
    /// Makes the stepper for one item's or node's annotation text, in
    /// `stages`' variant, shaped as `shaping` and spaced by `words`.
    pub(super) fn new(
        stages: &LineStages<'a>,
        words: WordSpacingRule,
        shaping: ShapingFactsId,
    ) -> Self {
        Self {
            analysis: stages.analysis,
            source: stages.text(),
            shaped: stages.shaped,
            spacing: LetterWordSpacing::from_shaping(&stages.content.facts, shaping, words),
        }
    }

    /// Returns how far `cluster` steps: its glyphs' advances and the spacing
    /// after it.
    pub(super) fn step(&self, cluster: ClusterId) -> InlineLayoutUnit {
        let glyphs = &self.shaped.glyphs;
        let advance = match glyphs.glyphs(cluster) {
            ClusterGlyphs::Many(glyphs) => {
                glyphs.iter().fold(InlineLayoutUnit::ZERO, |sum, glyph| {
                    sum + InlineLayoutUnit::from_text(glyph.advance)
                })
            }
            ClusterGlyphs::One(_) | ClusterGlyphs::None => InlineLayoutUnit::ZERO,
        };
        advance + self.spacing_after(cluster)
    }

    /// Returns the spacing after `cluster`.
    ///
    /// An atomic inline steps its margin box and this.
    pub(super) fn spacing_after(&self, cluster: ClusterId) -> InlineLayoutUnit {
        let continuation = self.shaped.glyphs.word(cluster).is_continuation();
        let spacing = self
            .spacing
            .after_cluster(self.analysis, self.source, cluster, continuation);
        InlineLayoutUnit::from_text(spacing)
    }
}

impl fmt::Debug for AnnotationText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnnotationText")
            .field("spacing", &self.spacing)
            .finish_non_exhaustive()
    }
}

/// A glyph of a text item where it is drawn, in line layout's exact units.
/// The views convert it to pixels.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ExactGlyph {
    /// The glyph id, in the item's used font.
    pub(crate) id: u32,
    /// Its exact origin along the line, from the line box's left.
    pub(crate) x: InlineLayoutUnit,
    /// How far above the baseline it is drawn.
    pub(crate) y: TextUnit,
    /// The exact pen move after it: the shaper's advance, or for a cluster's
    /// last glyph the rest of the cluster's advance.
    pub(crate) advance: InlineLayoutUnit,
    /// The cluster it draws.
    pub(crate) cluster: ClusterId,
}

/// The rest of a multi-glyph cluster: glyphs still to hand out, the next
/// pen, the cluster's start and end, and the cluster.
#[derive(Copy, Clone, Debug)]
struct Pending<'a> {
    glyphs: &'a [SidecarGlyph],
    pen: InlineLayoutUnit,
    left: InlineLayoutUnit,
    end: InlineLayoutUnit,
    cluster: ClusterId,
}

/// Rebuilds a justified line's opportunities from its stored row.
///
/// The caller finds the small row first, so an unjustified run builds
/// nothing.
fn line_justification<'a>(
    stages: &LineStages<'a>,
    row: JustifiedLine,
    line: &LineRecord,
) -> Option<Justified<'a>> {
    let justification = row.justification;
    let opportunities = JustifyOpportunities::from_line(
        stages,
        line.clusters().start..row.content_end,
        Some(justification.summary),
    )?;
    Some(Justified {
        justification,
        opportunities,
    })
}

/// How one text item's clusters step along the line, in logical order.
///
/// Steps come from the prefix sums, or from reshaped line edges where they
/// hold some clusters. The cluster walk and the glyph walk share this.
#[derive(Clone, Debug)]
struct Steps<'a> {
    /// The prefix sum at the boundary before each of the item's clusters.
    prefix: &'a [InlineLayoutUnit],
    /// The line's reshaped edges that hold some of the item's clusters.
    shapes: &'a [EdgeShape],
    /// The reshaped clusters' advances, which `shapes` index.
    edge_advances: &'a Table<EdgeClusterId, TextUnit>,
    start: ClusterId,
    len: usize,
    /// The pen at the item's first cluster and at the end of its last, from
    /// the prefix sums.
    pen: InlineLayoutUnit,
    pen_end: InlineLayoutUnit,
    /// Its line's justification, where the line is justified.
    justify: Option<Justified<'a>>,
    /// A ruby annotation's text, which steps by its glyphs' own advances.
    annotation: Option<AnnotationText<'a>>,
    /// The room `ruby-align` spread on the item's left and right, which its
    /// leftmost and rightmost clusters step over, and whether the item runs
    /// right to left, which makes its last cluster the leftmost.
    ends: (InlineLayoutUnit, InlineLayoutUnit, bool),
}

impl<'a> Steps<'a> {
    /// Makes the steps of text `item` on line `id`, read in `stages`, the
    /// line's variant, whose clusters have `words` glyph words.
    ///
    /// Where the tables don't agree, the steps are empty and nothing draws.
    fn new(
        input: &ReadInput<'a>,
        stages: &LineStages<'a>,
        id: LineId,
        line: &LineRecord,
        item: &FragmentItem,
        words: usize,
    ) -> Self {
        let prefix = &stages.measured.prefix;
        let clusters = item.text_clusters();
        let (start, end) = (clusters.start.get(), clusters.end.get());
        if item.flags.contains(FragmentItemFlags::TAB) {
            // A tab is one cluster, as wide as its item. The prefix sums give
            // it nothing.
            return Self {
                prefix: &[],
                shapes: &[],
                edge_advances: &input.lines.edges.advances,
                start: clusters.start,
                len: usize::from(words == 1 && end.saturating_sub(start) == 1),
                pen: InlineLayoutUnit::ZERO,
                pen_end: item.advance(),
                justify: None,
                annotation: None,
                ends: (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO, false),
            };
        }
        if item.flags.contains(FragmentItemFlags::ANNOTATION) {
            // An annotation's text is on its own line. It steps by its
            // glyphs, with a running sum.
            return Self {
                prefix: &[],
                shapes: input.lines.edges.line_edges(line),
                edge_advances: &input.lines.edges.advances,
                start: clusters.start,
                len: if words == end.saturating_sub(start) {
                    words
                } else {
                    0
                },
                pen: InlineLayoutUnit::ZERO,
                pen_end: InlineLayoutUnit::ZERO,
                justify: None,
                annotation: Some(AnnotationText::new(
                    stages,
                    input.stages.measured.word_spacing_rule,
                    stages
                        .content
                        .facts
                        .text(stages.text_facts(item.node))
                        .shaping,
                )),
                ends: {
                    let (left, right) = input.fragments.spread_room(id, item);
                    (left, right, item.level.is_rtl())
                },
            };
        }
        let positions = prefix.positions(clusters.clone());
        let len = if words == end.saturating_sub(start) && positions.len() == words {
            words
        } else {
            0
        };
        let (pen, pen_end) = if len == 0 {
            (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO)
        } else {
            // The items at each end, walked to from the item's own, only
            // where boxes have edges, since only then does the pen read them.
            let items = &stages.analysis.item_clusters;
            let edged = stages
                .content
                .flags
                .contains(ContentFlags::BOXES_WITH_EDGES);
            let own = item.text_item().unwrap_or_default();
            let at = |at| {
                if edged {
                    items.walk_to(own, at)
                } else {
                    ItemId::default()
                }
            };
            let (start, end) = (clusters.start, clusters.end);
            let paragraph = stages.analysis.paragraphs.get(line.paragraph);
            (
                stages.pen(start, at(start)),
                stages.pen_end(end, at(end), paragraph),
            )
        };
        let edges = &input.lines.edges;
        let mut steps = Self {
            prefix: positions,
            shapes: edges.line_edges_touching(line, &clusters),
            edge_advances: &edges.advances,
            start: clusters.start,
            len,
            pen,
            pen_end,
            justify: None,
            annotation: None,
            ends: {
                let (left, right) = input.fragments.spread_room(id, item);
                (left, right, item.level.is_rtl())
            },
        };
        if len > 0
            && let Some(row) = input.fragments.line_justification(id)
        {
            steps.justify = line_justification(stages, row, line);
        }
        steps
    }

    /// Returns the pen at the item's `i`th boundary, from the prefix sums.
    #[inline]
    fn boundary_pen(&self, i: usize) -> InlineLayoutUnit {
        if i == 0 {
            self.pen
        } else if i >= self.len {
            self.pen_end
        } else {
            self.prefix.get(i).copied().unwrap_or(self.pen_end)
        }
    }

    /// Returns the item's `i`th cluster's entry in the line's edge tables,
    /// or `None` where no reshaped edge holds it.
    fn reshaped(&self, i: usize) -> Option<EdgeClusterId> {
        let cluster = ClusterId::new(self.start.get() + i);
        self.shapes.iter().find_map(|shape| shape.entry(cluster))
    }

    /// Returns the item's `i`th cluster's exact advance, with justification
    /// room.
    fn step(&self, i: usize) -> InlineLayoutUnit {
        self.step_and_shift(i).0
    }

    /// Returns the room the item's `i`th cluster's opportunities take, and
    /// the shift an opportunity before it gives its glyphs.
    ///
    /// Both are zero where the line is not justified.
    #[inline]
    fn room(&self, i: usize) -> (InlineLayoutUnit, InlineLayoutUnit) {
        match &self.justify {
            Some(justified) => {
                let (before, after) = justified.room(ClusterId::new(self.start.get() + i));
                (before + after, before)
            }
            None => (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO),
        }
    }

    /// Returns the item's `i`th cluster's advance and its glyphs' shift.
    ///
    /// On a justified line the shift is the room of an opportunity before
    /// it.
    fn step_and_shift(&self, i: usize) -> (InlineLayoutUnit, InlineLayoutUnit) {
        if let Some(text) = &self.annotation {
            let cluster = ClusterId::new(self.start.get() + i);
            let step = self
                .reshaped(i)
                .and_then(|entry| self.edge_advances.get(entry))
                .map_or_else(
                    || text.step(cluster),
                    |&advance| InlineLayoutUnit::from_text(advance),
                );
            let (extra, shift) = self.end_room(i);
            return (step + extra, shift);
        }
        let shaped = match self.reshaped(i) {
            Some(at) => self
                .edge_advances
                .get(at)
                .map_or(InlineLayoutUnit::ZERO, |&step| {
                    InlineLayoutUnit::from_text(step)
                }),
            None => self.boundary_pen(i + 1) - self.boundary_pen(i),
        };
        let (room, shift) = self.room(i);
        let (extra, end_shift) = self.end_room(i);
        (shaped + room + extra, shift + end_shift)
    }

    /// Returns the room `ruby-align` spread beside the item's `i`th cluster,
    /// and the shift the room on its left gives its glyphs.
    ///
    /// The room on the item's left widens its leftmost cluster, and the
    /// room on its right its rightmost.
    #[inline]
    fn end_room(&self, i: usize) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let (left, right, rtl) = self.ends;
        if left == InlineLayoutUnit::ZERO && right == InlineLayoutUnit::ZERO {
            return (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        }
        let (first, last) = (i == 0, i + 1 == self.len);
        let (leftmost, rightmost) = if rtl { (last, first) } else { (first, last) };
        let left = if leftmost {
            left
        } else {
            InlineLayoutUnit::ZERO
        };
        let right = if rightmost {
            right
        } else {
            InlineLayoutUnit::ZERO
        };
        (left + right, left)
    }
}

/// The clusters of one text item in logical order, each with its exact
/// left and advance.
///
/// Carets, selection and decorations read it. It is a view over the
/// prepared tables, so a clone walks on from where it was.
#[derive(Clone)]
pub(crate) struct ClusterWalk<'a> {
    steps: Steps<'a>,
    rtl: bool,
    /// How many clusters have been handed out.
    done: usize,
    /// Left to right, where the next cluster starts. Right to left, where
    /// the last one handed out starts.
    x: InlineLayoutUnit,
}

impl<'a> ClusterWalk<'a> {
    /// Walks the clusters of `item` on line `id`, read in `stages`, the
    /// line's variant; none for an item that is not text.
    ///
    /// Right to left, the walk starts at the item's right, its stored
    /// advance past its left.
    pub(crate) fn new(
        input: &ReadInput<'a>,
        stages: &LineStages<'a>,
        id: LineId,
        line: &LineRecord,
        item: &FragmentItem,
    ) -> Self {
        let clusters = item.text_clusters();
        let words = clusters.end.get().saturating_sub(clusters.start.get());
        let rtl = item.level.is_rtl();
        // The steps are built in place, so nothing is copied.
        Self {
            steps: Steps::new(input, stages, id, line, item, words),
            rtl,
            done: 0,
            x: if rtl {
                item.inline + item.advance()
            } else {
                item.inline
            },
        }
    }
}

impl Iterator for ClusterWalk<'_> {
    /// A cluster, its left along the line, and its advance.
    type Item = (ClusterId, InlineLayoutUnit, InlineLayoutUnit);

    fn next(&mut self) -> Option<Self::Item> {
        if self.done >= self.steps.len {
            return None;
        }
        let i = self.done;
        self.done += 1;
        let step = self.steps.step(i);
        let cluster = ClusterId::new(self.steps.start.get() + i);
        let left = if self.rtl {
            self.x = self.x - step;
            self.x
        } else {
            let left = self.x;
            self.x += step;
            left
        };
        Some((cluster, left, step))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.steps.len - self.done.min(self.steps.len);
        (left, Some(left))
    }
}

impl ExactSizeIterator for ClusterWalk<'_> {}

/// The glyphs of one text item in drawing order, placed exactly, for a
/// painter.
///
/// It is a view over the prepared tables, so a clone walks on from where it
/// was. A renderer that needs the glyphs twice clones the walk instead of
/// copying them.
#[derive(Clone)]
pub(crate) struct GlyphWalk<'a> {
    /// The paragraph's words for the item's clusters.
    words: &'a [GlyphWord],
    sidecar: &'a Table<SidecarGlyphId, SidecarGlyph>,
    edge_words: &'a Table<EdgeClusterId, GlyphWord>,
    edge_glyphs: &'a Table<SidecarGlyphId, SidecarGlyph>,
    steps: Steps<'a>,
    rtl: bool,
    /// Where the item starts on the line.
    origin: InlineLayoutUnit,
    /// The offset from a cluster's prefix entry to its position.
    ///
    /// Left to right, a cluster after the first starts at entry plus this.
    /// Right to left, a cluster before the last ends at this less its entry.
    base: InlineLayoutUnit,
    /// Its clusters step by the prefix sums alone: no reshaped edge holds
    /// one, it is no annotation, and `ruby-align` spread no room into it.
    prefixed: bool,
    /// Left to right, not reshaped and not justified, so a cluster's left is
    /// its prefix entry plus `base`.
    straight: bool,
    /// The next cluster may take the `straight` path. That needs `straight`,
    /// a cluster past the first, whose pen may stand past an opening edge,
    /// and no multi-glyph cluster in progress.
    fast: bool,
    /// As `straight`, on a justified line. A cluster's left is its prefix
    /// entry plus `base` and the room of the opportunities before it.
    straight_justified: bool,
    /// The next cluster may take the `straight_justified` path, as with
    /// `fast`.
    fast_justified: bool,
    /// How many clusters have been visited.
    done: usize,
    /// Where the next cluster starts, on the running-sum path.
    x: InlineLayoutUnit,
    /// On a justified line, the room the clusters visited so far took.
    room: InlineLayoutUnit,
    /// The rest of a multi-glyph cluster still being handed out.
    pending: Option<Pending<'a>>,
    /// The left and advance of the cluster of the last glyph from the
    /// general paths, which [`next_placed`](Self::next_placed) returns.
    placed: (InlineLayoutUnit, InlineLayoutUnit),
}

impl<'a> GlyphWalk<'a> {
    /// Walks the glyphs of `item` on line `id`, read in `stages`, the line's
    /// variant; none for an item that is not text.
    pub(crate) fn new(
        input: &ReadInput<'a>,
        stages: &LineStages<'a>,
        id: LineId,
        line: &LineRecord,
        item: &FragmentItem,
    ) -> Self {
        let shaped = stages.shaped;
        let lines = input.lines;
        let clusters = item.text_clusters();
        let words = shaped.glyphs.words(clusters);
        // The steps are built in place, and the rest is derived from them
        // there, so nothing is copied.
        let mut walk = Self {
            words,
            sidecar: &shaped.glyphs.sidecar,
            edge_words: &lines.edges.words,
            edge_glyphs: &lines.edges.glyphs,
            steps: Steps::new(input, stages, id, line, item, words.len()),
            rtl: item.level.is_rtl(),
            origin: item.inline,
            base: InlineLayoutUnit::ZERO,
            prefixed: false,
            straight: false,
            fast: false,
            straight_justified: false,
            fast_justified: false,
            done: 0,
            x: item.inline,
            room: InlineLayoutUnit::ZERO,
            pending: None,
            placed: (InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO),
        };
        let steps = &walk.steps;
        let base = if item.level.is_rtl() {
            item.inline + steps.pen_end
        } else {
            item.inline - steps.pen
        };
        // The item's own bit says whether room was spread into it. Reading
        // the room from the steps instead makes the compiler build them
        // apart and copy them in, which slows reading a short run by a
        // fifth.
        let prefixed = steps.shapes.is_empty() && steps.annotation.is_none() && !item.is_spread();
        let straight = prefixed && steps.justify.is_none() && !item.level.is_rtl() && steps.len > 0;
        let straight_justified =
            prefixed && steps.justify.is_some() && !item.level.is_rtl() && steps.len > 0;
        // A right-to-left annotation is walked from its right with a running
        // sum.
        let annotation_rtl = item.level.is_rtl() && steps.annotation.is_some();
        walk.base = base;
        walk.prefixed = prefixed;
        walk.straight = straight;
        walk.straight_justified = straight_justified;
        if annotation_rtl {
            walk.x = item.inline + item.advance();
        }
        walk
    }

    /// Returns the item's `i`th cluster's glyph word, the sidecar it points
    /// into, the cluster's left and advance, and its glyphs' shift.
    fn cluster(
        &mut self,
        i: usize,
    ) -> (
        GlyphWord,
        &'a Table<SidecarGlyphId, SidecarGlyph>,
        InlineLayoutUnit,
        InlineLayoutUnit,
        InlineLayoutUnit,
    ) {
        let steps = &self.steps;
        if self.prefixed {
            // One sum with the prefix entry, plus the room the clusters so
            // far took on a justified line.
            let word = self.words.get(i).copied().unwrap_or(GlyphWord::EMPTY);
            let left = if self.rtl {
                match steps.prefix.get(i + 1) {
                    Some(&after) => self.base - after,
                    None => self.origin,
                }
            } else if i == 0 {
                self.origin
            } else {
                self.base + steps.prefix.get(i).copied().unwrap_or(steps.pen_end)
            };
            let (room, shift) = steps.room(i);
            let step = steps.boundary_pen(i + 1) - steps.boundary_pen(i) + room;
            let left = left + self.room;
            self.room += room;
            return (word, self.sidecar, left, step, shift);
        }
        // A running sum, with reshaped advances where an edge holds the
        // cluster, and justification room.
        let left = self.x;
        let (step, shift) = steps.step_and_shift(i);
        self.x += step;
        match steps.reshaped(i) {
            Some(at) => {
                let word = self.edge_words.get(at).copied().unwrap_or(GlyphWord::EMPTY);
                (word, self.edge_glyphs, left, step, shift)
            }
            None => {
                let word = self.words.get(i).copied().unwrap_or(GlyphWord::EMPTY);
                (word, self.sidecar, left, step, shift)
            }
        }
    }

    /// Hands out the first of a multi-glyph cluster's `glyphs` at `pen`, and
    /// keeps the rest pending.
    ///
    /// The cluster spans `left` to `end`.
    fn expanded(
        &mut self,
        glyphs: &'a [SidecarGlyph],
        pen: InlineLayoutUnit,
        (left, end): (InlineLayoutUnit, InlineLayoutUnit),
        cluster: ClusterId,
    ) -> Option<ExactGlyph> {
        let (glyph, rest) = glyphs.split_first()?;
        let (dx, dy) = (glyph.x_offset, glyph.y_offset);
        let advance = if rest.is_empty() {
            // The cluster's last glyph takes the rest of its advance.
            end - pen
        } else {
            InlineLayoutUnit::from_text(glyph.advance)
        };
        if !rest.is_empty() {
            self.pending = Some(Pending {
                glyphs: rest,
                pen: pen + advance,
                left,
                end,
                cluster,
            });
        }
        self.placed = (left, end - left);
        Some(ExactGlyph {
            id: glyph.id(),
            x: pen + InlineLayoutUnit::from_text(dx),
            y: dy,
            advance,
            cluster,
        })
    }
}

impl GlyphWalk<'_> {
    /// Returns the next glyph from the paths `next_fast` doesn't take.
    ///
    /// These are a multi-glyph cluster's rest, right-to-left or reshaped
    /// clusters, justified clusters, and the item's first cluster, whose pen
    /// may stand past an opening edge.
    #[inline(never)]
    fn next_any(&mut self) -> Option<ExactGlyph> {
        if self.fast_justified
            && let Some(glyph) = self.next_justified()
        {
            return Some(glyph);
        }
        let glyph = self.next_slow();
        self.fast = self.straight && self.pending.is_none();
        self.fast_justified = self.straight_justified && self.pending.is_none();
        glyph
    }

    /// Returns the next single-glyph cluster's glyph on a justified,
    /// left-to-right, unreshaped item.
    ///
    /// It reads a word, a sum and the room before it, like the fast path
    /// plus justification. `None` for any other cluster.
    fn next_justified(&mut self) -> Option<ExactGlyph> {
        let (Some(&word), Some(&at)) =
            (self.words.get(self.done), self.steps.prefix.get(self.done))
        else {
            return None;
        };
        let id = word.single()?;
        let i = self.done;
        self.done += 1;
        let next = self
            .steps
            .prefix
            .get(self.done)
            .copied()
            .unwrap_or(self.steps.pen_end);
        let (room, shift) = self.steps.room(i);
        let left = self.base + at + self.room;
        let advance = next - at + room;
        self.room += room;
        self.placed = (left, advance);
        Some(ExactGlyph {
            id,
            x: left + shift,
            y: TextUnit::from_raw(0),
            advance,
            cluster: ClusterId::new(self.steps.start.get() + i),
        })
    }

    /// Returns the next glyph by the general path: a multi-glyph cluster's
    /// rest, or the next cluster's first.
    fn next_slow(&mut self) -> Option<ExactGlyph> {
        if let Some(pending) = self.pending {
            self.pending = None;
            let ends = (pending.left, pending.end);
            return self.expanded(pending.glyphs, pending.pen, ends, pending.cluster);
        }
        while self.done < self.steps.len {
            let i = if self.rtl {
                self.steps.len - 1 - self.done
            } else {
                self.done
            };
            self.done += 1;
            let (word, sidecar, left, step, shift) = self.cluster(i);
            let cluster = ClusterId::new(self.steps.start.get() + i);
            match word.glyphs(sidecar) {
                ClusterGlyphs::One(id) => {
                    self.placed = (left, step);
                    return Some(ExactGlyph {
                        id,
                        x: left + shift,
                        y: TextUnit::from_raw(0),
                        advance: step,
                        cluster,
                    });
                }
                ClusterGlyphs::Many(glyphs) => {
                    let ends = (left, left + step);
                    if let Some(glyph) = self.expanded(glyphs, left + shift, ends, cluster) {
                        return Some(glyph);
                    }
                }
                ClusterGlyphs::None => {}
            }
        }
        None
    }

    /// Returns the next glyph on the fast path: a single-glyph cluster after
    /// the first, left to right and not reshaped, as most clusters are.
    ///
    /// It reads a word and a sum. `None` for any other cluster.
    #[inline]
    fn next_fast(&mut self) -> Option<ExactGlyph> {
        if !self.fast {
            return None;
        }
        let (Some(&word), Some(&at)) =
            (self.words.get(self.done), self.steps.prefix.get(self.done))
        else {
            return None;
        };
        let id = word.single()?;
        let i = self.done;
        self.done += 1;
        let next = self
            .steps
            .prefix
            .get(self.done)
            .copied()
            .unwrap_or(self.steps.pen_end);
        Some(ExactGlyph {
            id,
            x: self.base + at,
            y: TextUnit::from_raw(0),
            advance: next - at,
            cluster: ClusterId::new(self.steps.start.get() + i),
        })
    }

    /// Returns the next glyph with its cluster's exact left and advance.
    ///
    /// Text on a path turns the cluster about these. They are `None` for a
    /// single-glyph cluster, whose glyph's place and advance are the same.
    #[inline]
    pub(crate) fn next_placed(
        &mut self,
    ) -> Option<(ExactGlyph, Option<(InlineLayoutUnit, InlineLayoutUnit)>)> {
        if let Some(glyph) = self.next_fast() {
            return Some((glyph, None));
        }
        let glyph = self.next_any()?;
        Some((glyph, Some(self.placed)))
    }
}

impl Iterator for GlyphWalk<'_> {
    type Item = ExactGlyph;

    #[inline]
    fn next(&mut self) -> Option<ExactGlyph> {
        if let Some(glyph) = self.next_fast() {
            return Some(glyph);
        }
        self.next_any()
    }
}
