//! The scan's walk over the text, segment by segment: the items at each
//! boundary, what each cluster adds, and autospace seams.

use super::overhang::TextBeside;
use super::{BoundaryRoom, EdgeAmounts, Scan, ScanPrefix, ScanWalk};
use crate::data::Id;
use crate::stages::analysis::{ClusterClass, ClusterId, RunOrientation, ScriptRun};
use crate::stages::content::{
    Atomic, AtomicId, BoxFlags, FloatId, Item, ItemFlags, ItemId, ItemKind, NodeId, NodeKind,
    TextLineHeight,
};
use crate::stages::fonts::UsedFontId;
use crate::stages::measure::autospace::{SeamContext, Seams};
use crate::stages::measure::edges::logical_edges;
use crate::stages::measure::metrics::normal_extent;
use crate::stages::measure::spacing::LetterWordSpacing;
use crate::stages::measure::{Extent, ItemExtents, autospace};
use crate::stages::{Segment, Segments, Step};
use crate::unit::{InlineLayoutUnit, TextUnit};
use crate::work;

impl<'a> ScanWalk<'a> {
    /// Returns the walk over `scan`'s text from its start, in no segment yet.
    pub(super) fn new(scan: &Scan<'a>) -> Self {
        let content = scan.input.content;
        let mut segments = Segments::from_text(scan.input.analysis, scan.shaped);
        let ahead = segments.next();
        let text = content.nodes.text_facts(NodeId::BLOCK, scan.variant);
        Self {
            segments,
            ahead,
            segment: None,
            item: None,
            paragraph: None,
            has_seams: false,
            combined: false,
            seam: None,
            until: ClusterId::new(0),
            text,
            facts: content.facts.text(text),
            spacing: LetterWordSpacing::NONE,
            atomic: None,
            cloned: EdgeAmounts::default(),
            atomics: AtomicId::new(0),
            floats: FloatId::new(0),
            pending: None,
            leaf: OverhangNeighbour::None,
            entered: None,
            past_strut: 0,
        }
    }

    /// Returns the current segment's item: its id and row.
    pub(super) fn item(&self) -> Option<(ItemId, &'a Item)> {
        Some((self.segment?.item, self.item?))
    }

    /// Returns whether the current item is annotation text, which adds nothing
    /// to the base's line.
    #[inline]
    pub(super) fn annotation(&self) -> bool {
        self.item
            .is_some_and(|item| item.flags.contains(ItemFlags::ANNOTATION))
    }

    /// Returns whether the current clusters pay only their advances: not
    /// annotation text, not spaced, in no cloned box.
    pub(super) fn is_plain(&self) -> bool {
        !self.annotation() && self.spacing.is_none() && self.cloned.is_empty()
    }

    /// Returns whether the clusters of `run` are combined text.
    ///
    /// Combined text takes no letter- or word-spacing and has no autospace seam
    /// beside it, as Blink sets `LayoutTextCombine` as an atomic inline with
    /// zero `letter-spacing`.
    fn combined(scan: &Scan<'a>, run: &ScriptRun) -> bool {
        scan.has_combined && run.orientation == RunOrientation::Combined
    }

    /// Returns the text a ruby column starting at boundary `at` may overhang:
    /// the last text passed, with only tags after it.
    ///
    /// Its width runs from its first cluster to `sum`, the running sum after
    /// the boundary's leading closes. `prefix` holds the sums before `at`.
    pub(super) fn leaf_before(
        &self,
        scan: &Scan<'a>,
        prefix: ScanPrefix<'_>,
        at: ClusterId,
        sum: InlineLayoutUnit,
    ) -> Option<TextBeside> {
        let OverhangNeighbour::Text(item) = self.leaf else {
            return None;
        };
        let first = scan.input.analysis.item_clusters.start(item);
        let start = prefix.get(first).unwrap_or(sum);
        Some(TextBeside {
            item,
            width: sum - start,
            cluster: ClusterId::new(at.get().checked_sub(1)?),
            end: sum,
        })
    }

    /// Returns what cluster `at` adds to the base line: its `advance`, or an
    /// atomic inline's margin box, plus the spacing after it. Annotation text
    /// adds nothing.
    #[inline]
    pub(super) fn step(
        &self,
        scan: &Scan<'a>,
        class: ClusterClass,
        advance: InlineLayoutUnit,
        at: ClusterId,
    ) -> InlineLayoutUnit {
        if self.annotation() {
            return InlineLayoutUnit::ZERO;
        }
        let own = match self.atomic {
            Some(width) if class == ClusterClass::Object => width,
            _ => advance,
        };
        own + self.spacing(scan, class, at)
    }

    /// Returns the spacing after cluster `at`, on whichever line its text is
    /// set. The prefix gives annotation text none, but its own line does.
    #[inline]
    pub(super) fn spacing(
        &self,
        scan: &Scan<'a>,
        class: ClusterClass,
        at: ClusterId,
    ) -> InlineLayoutUnit {
        let spacing = self.spacing;
        if spacing.is_none() || self.combined || !spacing.may_space(class) {
            return InlineLayoutUnit::ZERO;
        }
        let clusters = &scan.input.analysis.clusters;
        let text = clusters.text(scan.source, at);
        let continuation = scan.shaped.glyphs.word(at).is_continuation();
        InlineLayoutUnit::from_text(spacing.after_text(
            clusters,
            scan.source,
            class,
            text,
            at,
            continuation,
        ))
    }

    /// Returns the room of an autospace seam after cluster `at`, and moves
    /// `seams` past the cluster.
    ///
    /// Where the next cluster is past the segment's end, it is read from the
    /// segment the walk yields next, and the items between may end the seam.
    /// Each side is read at its script run's level.
    pub(super) fn seam_after(&self, scan: &Scan<'a>, seams: &mut Seams, at: ClusterId) -> TextUnit {
        let content = scan.input.content;
        let none = TextUnit::from_raw(0);
        let Some(segment) = &self.segment else {
            return none;
        };
        let after = ClusterId::new(at.get() + 1);
        let text_end = scan.input.analysis.clusters.end_id();
        // The item and script run of the next cluster, and whether an item at the
        // boundary ends the seam.
        let mut next = self.seam;
        let mut next_combined = self.combined;
        let mut ended = false;
        if after == segment.end && after < text_end {
            let (mut segments, mut step) = (self.segments, self.ahead);
            while let Some(Step::Item { id, .. }) = step {
                work::step();
                ended |= content
                    .items
                    .get(id)
                    .is_some_and(|item| autospace::ends_seams(content, item));
                step = segments.next();
            }
            next = match step {
                Some(Step::Segment(next)) => content
                    .items
                    .get(next.item)
                    .zip(scan.script(&next))
                    .and_then(|(item, run)| {
                        next_combined = Self::combined(scan, run);
                        let facts = content.nodes.text_facts(item.node, scan.variant);
                        SeamContext::new(next.item, item, facts, run.level)
                    }),
                _ => {
                    next_combined = false;
                    None
                }
            };
        }
        if self.combined || next_combined {
            seams.after(content, scan.input.fonts, None, None, true);
            return none;
        }
        let clusters = &scan.input.analysis.clusters;
        let here = seams.side(self.seam, at, clusters.text(scan.source, at), false);
        let there = if after < text_end {
            seams.side(next, after, clusters.text(scan.source, after), true)
        } else {
            None
        };
        seams.after(content, scan.input.fonts, here, there, ended)
    }

    /// Crosses boundary `at`: the items there, and the segment after them.
    ///
    /// Pushes each item's extent to `extents` where given; a look-ahead passes
    /// none. Returns the room the items take, and whether it entered a segment
    /// starting at `at`.
    ///
    /// Most boundaries fall inside a segment and pass with one comparison.
    /// Walking items at every boundary costs 7% of preparing an article.
    #[inline]
    pub(super) fn cross(
        &mut self,
        scan: &Scan<'a>,
        at: ClusterId,
        extents: Option<&mut ItemExtents>,
    ) -> (BoundaryRoom, bool) {
        if let Some((first, leaf)) = self.entered
            && first < at
        {
            self.leaf = leaf;
            self.entered = None;
        }
        if at < self.until {
            let room = BoundaryRoom {
                across: self.cloned,
                ..BoundaryRoom::default()
            };
            return (room, false);
        }
        self.cross_items(scan, at, extents)
    }

    /// Crosses the items at boundary `at`, where the current segment ends.
    // Out of line, so the boundaries with no item keep a tight loop.
    #[inline(never)]
    fn cross_items(
        &mut self,
        scan: &Scan<'a>,
        at: ClusterId,
        mut extents: Option<&mut ItemExtents>,
    ) -> (BoundaryRoom, bool) {
        let content = scan.input.content;
        let mut room = BoundaryRoom::from_boundary(scan.input.analysis, at);
        let mut across = None;
        while let Some(Step::Item { at: sits, id }) = self.ahead
            && sits == at
        {
            work::step();
            self.ahead = self.segments.next();
            let Some(item) = content.items.get(id) else {
                continue;
            };
            if let Some(extents) = extents.as_deref_mut() {
                // The text item before this one is passed; its extent is final.
                self.flush(extents);
                let (extent, _) = self.extent(scan, item, false, None);
                extents.push(extent);
                // A box's strut is its opening item's extent. Count the box while its
                // strut reaches past the block's.
                let past = |extent: Extent| extents.strut.unite(extent) != extents.strut;
                match item.kind {
                    ItemKind::Open if past(extent) => self.past_strut += 1,
                    ItemKind::Close if past(extents.get(content.nodes.items(item.node).start)) => {
                        self.past_strut = self.past_strut.saturating_sub(1);
                    }
                    _ => {}
                }
            }
            let annotation = item.flags.contains(ItemFlags::ANNOTATION);
            if scan.has_ruby {
                self.leaf = match item.kind {
                    ItemKind::Text | ItemKind::Open | ItemKind::Close | ItemKind::RubyOpen
                        if !annotation =>
                    {
                        self.leaf
                    }
                    _ => OverhangNeighbour::Other,
                };
                // A container with no annotation opens no column.
                if item.kind == ItemKind::RubyOpen
                    && !annotation
                    && room.ruby_open.is_none()
                    && item.flags.contains(ItemFlags::ANNOTATION_FOLLOWS)
                {
                    room.ruby_open = Some(id);
                }
            }
            if scan.has_edges {
                room.add(content, scan.letter.as_ref(), item);
            }
            if item.kind == ItemKind::Float {
                let width = self.float_width(scan, id);
                room.floats.sum += width;
                room.floats.widest = room.floats.widest.max(width);
            }
            if scan.has_clones {
                // The cloned boxes open where a break here falls are across it.
                if !room.split.splits() {
                    across = None;
                } else if across.is_none() {
                    across = Some(self.cloned);
                }
                self.clone_edges(scan, item);
            }
        }
        let entered = match self.ahead {
            Some(Step::Segment(segment)) if segment.start == at => {
                self.enter(scan, segment, extents);
                true
            }
            // Nothing holds the next cluster: the text's end, or a missing row,
            // which is a bug in an earlier stage.
            _ => {
                self.segment = None;
                self.item = None;
                self.paragraph = None;
                self.has_seams = false;
                self.combined = false;
                self.seam = None;
                self.until = ClusterId::new(at.get() + 1);
                false
            }
        };
        room.across = across.unwrap_or(self.cloned);
        (room, entered)
    }

    /// Enters `segment`, which starts at the boundary being crossed.
    ///
    /// A new item sets the extent, text facts, spacing and atomic width. A
    /// segment continuing the current item, after a shaping run or paragraph
    /// ends inside it, widens the item's extent by its font.
    fn enter(&mut self, scan: &Scan<'a>, segment: Segment, extents: Option<&mut ItemExtents>) {
        self.ahead = self.segments.next();
        self.until = segment.end;
        let id = segment.item;
        let same = self
            .segment
            .as_ref()
            .is_some_and(|before| before.item == id);
        self.segment = Some(segment);
        let paragraphs = &scan.input.analysis.paragraphs;
        self.paragraph = (paragraphs.get(segment.paragraph)).map(|para| (segment.paragraph, para));
        self.has_seams = self.paragraph.is_some_and(|(_, para)| scan.has_seams(para));
        self.seam = if scan.autospace.any() {
            scan.input
                .content
                .items
                .get(id)
                .zip(scan.script(&segment))
                .and_then(|(item, run)| {
                    let text = scan.input.content.nodes.text_facts(item.node, scan.variant);
                    SeamContext::new(id, item, text, run.level)
                })
        } else {
            None
        };
        self.combined = scan.has_combined
            && scan
                .script(&segment)
                .is_some_and(|run| Self::combined(scan, run));
        if same {
            // A font the item's text uses again widens nothing.
            if let Some((extent, last)) = &mut self.pending
                && let Some(font) = scan.font(&segment)
                && *last != font
            {
                *extent = extent.unite(scan.normal(font));
                *last = font;
            }
            return;
        }
        let content = scan.input.content;
        let Some(item) = content.items.get(id) else {
            self.item = None;
            return;
        };
        self.item = Some(item);
        self.text = content.nodes.text_facts(item.node, scan.variant);
        self.facts = content.facts.text(self.text);
        self.spacing = if scan.has_spacing {
            LetterWordSpacing::from_shaping(&content.facts, self.facts.shaping, scan.words)
        } else {
            LetterWordSpacing::NONE
        };
        let atomic = if item.kind == ItemKind::Atomic {
            self.atomic_row(scan, id)
        } else {
            None
        };
        self.atomic = atomic.map(|atomic| InlineLayoutUnit::from_layout(atomic.margin_inline));
        if let Some(extents) = extents {
            self.flush(extents);
            match (
                self.extent(scan, item, true, atomic.as_ref()),
                scan.font(&segment),
            ) {
                ((extent, true), Some(font)) => {
                    self.pending = Some((extent.unite(scan.normal(font)), font));
                }
                ((extent, _), _) => extents.push(extent),
            }
        }
        if scan.has_ruby {
            // The item counts as passed only once the scan is past its first cluster.
            let leaf = if item.kind == ItemKind::Text && !item.flags.contains(ItemFlags::ANNOTATION)
            {
                OverhangNeighbour::Text(id)
            } else {
                OverhangNeighbour::Other
            };
            self.entered = Some((segment.start, leaf));
        }
    }

    /// Enters the next segment where it continues the current item and
    /// paragraph, and returns whether it did. No item sits between, so the
    /// plain fast path goes on.
    pub(super) fn continues(&mut self, scan: &Scan<'a>, extents: &mut ItemExtents) -> bool {
        let next = match (&self.segment, &self.ahead) {
            (Some(segment), Some(Step::Segment(next)))
                if next.item == segment.item && next.paragraph == segment.paragraph =>
            {
                *next
            }
            _ => return false,
        };
        self.enter(scan, next, Some(extents));
        true
    }

    /// Pushes the pending extent of the text item just left to `extents`.
    pub(super) fn flush(&mut self, extents: &mut ItemExtents) {
        if let Some((extent, _)) = self.pending.take() {
            extents.push(extent);
        }
    }

    /// Updates the cloned edges past `item`: a cloning box adds both edges
    /// where it opens, and removes them where it closes.
    fn clone_edges(&mut self, scan: &Scan<'a>, item: &Item) {
        let nodes = &scan.input.content.nodes;
        if item.flags.contains(ItemFlags::ANNOTATION)
            || nodes.kind(item.node) != Some(NodeKind::Box)
        {
            return;
        }
        let facts = scan
            .input
            .content
            .facts
            .box_facts(nodes.box_facts(item.node, scan.variant));
        if !facts.has(BoxFlags::CLONES) {
            return;
        }
        let (start, end) = logical_edges(facts);
        let (start, end) = (
            InlineLayoutUnit::from_layout(start),
            InlineLayoutUnit::from_layout(end),
        );
        match item.kind {
            ItemKind::Open => {
                self.cloned.start += start;
                self.cloned.end += end;
            }
            ItemKind::Close => {
                self.cloned.start = self.cloned.start - start;
                self.cloned.end = self.cloned.end - end;
            }
            _ => {}
        }
    }

    /// Returns the atomic inline of item `id`, moving the cursor to it.
    fn atomic_row(&mut self, scan: &Scan<'a>, id: ItemId) -> Option<Atomic> {
        let atomics = scan.input.content.atomics();
        while atomics
            .get(self.atomics)
            .is_some_and(|atomic| atomic.item < id)
        {
            self.atomics = AtomicId::new(self.atomics.get() + 1);
        }
        atomics
            .get(self.atomics)
            .filter(|atomic| atomic.item == id)
            .copied()
    }

    /// Returns the margin-box width of the float anchored at item `id`, moving
    /// the cursor to it. Zero where no float is found, a bug in an earlier stage.
    fn float_width(&mut self, scan: &Scan<'a>, id: ItemId) -> InlineLayoutUnit {
        let floats = scan.input.content.floats();
        while floats.get(self.floats).is_some_and(|float| float.item < id) {
            self.floats = FloatId::new(self.floats.get() + 1);
        }
        floats
            .get(self.floats)
            .filter(|float| float.item == id)
            .map_or(InlineLayoutUnit::ZERO, |float| {
                InlineLayoutUnit::from_layout(float.margin_box.0)
            })
    }

    /// Returns how far `item` reaches either side of its baseline, and whether
    /// its fonts widen it.
    ///
    /// `holds` says the item holds clusters; `atomic` is its atomic row.
    ///
    /// - Text takes its strut, widened by its fonts under `line-height:
    ///   normal`, as Blink accumulates used fonts.
    /// - A `<br>` and a box's opening edge take their text's strut, as in
    ///   Blink's standards mode.
    /// - An atomic inline takes its margin box against its baseline.
    /// - Anything else takes nothing.
    fn extent(
        &self,
        scan: &Scan<'a>,
        item: &Item,
        holds: bool,
        atomic: Option<&Atomic>,
    ) -> (Extent, bool) {
        let none = (Extent::NONE, false);
        if item.flags.contains(ItemFlags::ANNOTATION) {
            return none;
        }
        // An initial letter stands on no line (CSS Inline 3, "initial-letter").
        if let Some(letter) = &scan.letter
            && scan.input.content.nodes.contains(letter.node, item.node)
        {
            return none;
        }
        let text = || scan.input.content.nodes.text_facts(item.node, scan.variant);
        match item.kind {
            ItemKind::Text if !holds => none,
            ItemKind::Text => {
                let normal = matches!(self.facts.line_height, TextLineHeight::Normal);
                (scan.strut(self.text), normal)
            }
            ItemKind::Atomic => (
                atomic.map_or(Extent::NONE, |atomic| {
                    Extent::from_atomic(atomic, scan.writing_mode, scan.baseline)
                }),
                false,
            ),
            kind if kind == ItemKind::Break || kind.is_open() => (scan.strut(text()), false),
            // A closing edge, a float's anchor.
            _ => none,
        }
    }
}

impl<'a> Scan<'a> {
    /// Returns the `normal` leaded extent of used font `font`, or
    /// [`Extent::NONE`] where there is no such font.
    fn normal(&self, font: UsedFontId) -> Extent {
        self.input
            .fonts
            .used
            .get(font)
            .map_or(Extent::NONE, |used| normal_extent(&used.metrics))
    }
}

/// What the scan last passed that a ruby column may overhang.
///
/// Tags of boxes and ruby containers are skipped, as in Chrome's
/// `CanApplyStartOverhang` and `CommitPendingEndOverhang`. Tracked only with
/// ruby.
#[derive(Copy, Clone, Debug, Default)]
pub(super) enum OverhangNeighbour {
    /// Nothing yet.
    #[default]
    None,
    /// Base text, by item.
    Text(ItemId),
    /// An atomic inline, a float, a forced break, an annotation or a column.
    Other,
}
