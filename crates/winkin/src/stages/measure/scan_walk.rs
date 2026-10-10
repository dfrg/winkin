//! The scan's walk over the text, segment by segment.
//!
//! It crosses the items at each boundary into the segment after them. It
//! answers what those items take along the line, what each cluster adds,
//! and the room of an autospace seam after a cluster.

use super::autospace::{SeamContext, Seams};
use super::edges::logical_edges;
use super::metrics::normal_extent;
use super::scan::{BoundaryRoom, EdgeAmounts, Scan, ScanWalk};
use super::scan_overhang::TextBeside;
use super::spacing::LetterWordSpacing;
use super::{Extent, ItemExtents, autospace};
use crate::data::Id;
use crate::stages::analysis::{ClusterClass, ClusterId, RunOrientation, ScriptRun};
use crate::stages::content::{
    Atomic, AtomicId, BoxFlags, FloatId, Item, ItemFlags, ItemId, ItemKind, NodeId, NodeKind,
    TextLineHeight,
};
use crate::stages::fonts::UsedFontId;
use crate::stages::{Segment, Segments, Step};
use crate::unit::{InlineLayoutUnit, TextUnit};
use crate::work;

impl<'a> ScanWalk<'a> {
    /// Returns the walk over `scan`'s text from its start.
    ///
    /// No boundary is crossed yet, so it holds no segment, and the block's
    /// text facts are in hand.
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

    /// Returns whether the current item is annotation text.
    ///
    /// Annotation clusters add nothing to the base's line.
    #[inline]
    pub(super) fn annotation(&self) -> bool {
        self.item
            .is_some_and(|item| item.flags.contains(ItemFlags::ANNOTATION))
    }

    /// Returns whether the current clusters pay only their advances.
    ///
    /// They do when they are not annotation text, not spaced, and inside no
    /// cloned box. The plain fast path may then run over them.
    pub(super) fn is_plain(&self) -> bool {
        !self.annotation() && self.spacing.is_none() && self.cloned.is_empty()
    }

    /// Returns whether the clusters of the script run `run` are combined
    /// text.
    ///
    /// Combined text is set as one character. It takes no letter- or
    /// word-spacing, inside or after it. Blink's `LayoutTextCombine` sets its
    /// own `letter-spacing` to zero, and is an atomic inline in its line,
    /// which takes none. No autospace seam is beside it either, as none is
    /// beside an atomic inline.
    fn combined(scan: &Scan<'a>, run: &ScriptRun) -> bool {
        scan.has_combined && run.orientation == RunOrientation::Combined
    }

    /// Returns the text a ruby column starting at boundary `at` may
    /// overhang.
    ///
    /// It is the last text passed, with only tags after it. Its width on a
    /// max-content line runs from its first cluster to `sum`, the running
    /// sum after the boundary's leading closes. `prefix` holds the scan's
    /// sums before the boundary.
    pub(super) fn leaf_before(
        &self,
        scan: &Scan<'a>,
        prefix: &[InlineLayoutUnit],
        at: ClusterId,
        sum: InlineLayoutUnit,
    ) -> Option<TextBeside> {
        let OverhangNeighbour::Text(item) = self.leaf else {
            return None;
        };
        let first = scan.input.analysis.item_clusters.start(item);
        let start = prefix.get(first.get()).copied().unwrap_or(sum);
        Some(TextBeside {
            item,
            width: sum - start,
            cluster: ClusterId::new(at.get().checked_sub(1)?),
            end: sum,
        })
    }

    /// Returns what the cluster `at` of class `class` adds to the base line.
    ///
    /// It adds its shaped `advance`, or an atomic inline's margin box for its
    /// U+FFFC, plus the spacing after it. Annotation text adds nothing, since
    /// its clusters take no room on the base line.
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

    /// Returns the spacing after the cluster `at` of class `class`, on
    /// whichever line its text is set.
    ///
    /// [`LetterWordSpacing::after`] finds it, with the glyphs saying whether
    /// the cluster continues a ligature. It answers for an annotation's text
    /// on its own line too, though the prefix gives that text none.
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
        let text = scan.input.analysis.clusters.text(scan.source, at);
        let continuation = scan.shaped.glyphs.word(at).is_continuation();
        // The text's first cluster is the one at its offset 0.
        let first = at == ClusterId::new(0);
        InlineLayoutUnit::from_text(spacing.after(class, text, continuation, first))
    }

    /// Returns the room an autospace seam after cluster `at` takes, and
    /// moves `seams` past the cluster.
    ///
    /// The cluster after `at` is read one ahead. It is in the same segment,
    /// or past its end in the segment the walk yields next. In that case the
    /// items between are checked for one that ends the seam. Each side is
    /// read at its script run's level.
    pub(super) fn seam_after(&self, scan: &Scan<'a>, seams: &mut Seams, at: ClusterId) -> TextUnit {
        let content = scan.input.content;
        let none = TextUnit::from_raw(0);
        let Some(segment) = &self.segment else {
            return none;
        };
        let after = ClusterId::new(at.get() + 1);
        let text_end = scan.input.analysis.clusters.end_id();
        // Find the item and script run holding the cluster after, and
        // whether an item with no text at the boundary ends the seam. Where
        // the segment goes on past the cluster, there is nothing to look at.
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

    /// Crosses boundary `at`: takes the items the walk yields there, and the
    /// segment after them.
    ///
    /// It pushes each item's extent to `extents` where given; a look-ahead
    /// passes none. Returns what the items at the boundary take along the
    /// line, and the edges of the cloned boxes open across a break there.
    /// Also returns whether it entered a segment starting at `at`.
    ///
    /// Most boundaries in prose fall inside a segment and have no item. One
    /// comparison passes them. Walking items at every boundary costs 7% of
    /// preparing an article.
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

    /// Crosses the items at boundary `at`, where the current segment ends,
    /// and takes the segment after them, as [`cross`](Self::cross) says.
    // Out of line, so that the boundaries with no item keep a tight loop.
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
                // The text item before this one is passed, so its extent is
                // final.
                self.flush(extents);
                let (extent, _) = self.extent(scan, item, false, None);
                extents.push(extent);
                // A box's strut is its opening item's extent. Count the box
                // open where its strut reaches past the block's, and closed
                // again where the box ends.
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
                // The cloned boxes open where a break here falls are across
                // it.
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
            // Nothing holds the cluster after: this is the text's end, or a
            // missing row ended the walk, which is a bug in an earlier
            // stage.
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

    /// Takes `segment` from the walk. It starts at the boundary the scan
    /// crosses.
    ///
    /// It reads the segment's paragraph, and whether the scan looks for
    /// seams there. Where the segment starts a new item, it enters the item:
    /// its extent, its text's facts, its spacing and, for an atomic inline,
    /// the width of its U+FFFC. Where the segment continues the current
    /// item, because a shaping run or a paragraph ended inside it, it widens
    /// the item's extent by the segment's font.
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
            // The item holding the cluster at the boundary counts as passed
            // only once the scan is past that cluster.
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
    /// paragraph, and returns whether it did.
    ///
    /// That happens where a shaping run ends inside the item. No item sits
    /// at the boundary between, so the plain fast path goes on past it.
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

    /// Pushes the pending extent of the text item the scan has left to
    /// `extents`.
    pub(super) fn flush(&mut self, extents: &mut ItemExtents) {
        if let Some((extent, _)) = self.pending.take() {
            extents.push(extent);
        }
    }

    /// Updates the edges of the cloned boxes open, past `item`.
    ///
    /// A box that clones adds both its edges where it opens, and removes
    /// them where it closes.
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

    /// Returns the atomic inline whose item is `id`, moving the cursor to
    /// it.
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

    /// Returns the width along the line of the float whose anchor is `id`:
    /// its margin box. Moves the cursor to it.
    ///
    /// Returns zero for an anchor with no float, which is a bug in an
    /// earlier stage.
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

    /// Returns how far `item` reaches either side of its baseline, and
    /// whether the fonts of its clusters widen it.
    ///
    /// `holds` says the item holds clusters. `atomic` is its row where it is
    /// an atomic inline. Fonts widen text under `line-height: normal`, as
    /// Blink accumulates used fonts; the scan widens it segment by segment.
    ///
    /// - Text takes its text's strut, from the text metrics.
    /// - A `<br>` and a box's opening edge take their text's strut: every box
    ///   has a strut, as in Blink's standards mode.
    /// - An atomic inline takes its margin box against its baseline.
    /// - What holds none of the line's text, and every closing edge, takes
    ///   nothing.
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
        // An initial letter stands on no line: what it holds takes no room
        // in the line it opens (CSS Inline 3, "initial-letter").
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
    /// Returns the `normal` leaded extent of the used font `font`.
    ///
    /// It widens the extent of text under `line-height: normal`. It is
    /// [`Extent::NONE`] where `font` names no used font.
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
/// Chrome's `CanApplyStartOverhang` and `CommitPendingEndOverhang` look for
/// text past the tags of boxes and ruby containers, which hold positions,
/// not widths. The scan tracks it only where the content has ruby.
#[derive(Copy, Clone, Debug, Default)]
pub(super) enum OverhangNeighbour {
    /// Nothing yet.
    #[default]
    None,
    /// Text of the content's own, not an annotation's: its item.
    Text(ItemId),
    /// Anything else that holds a place: an atomic inline, a float, a forced
    /// break, an annotation, a ruby column.
    Other,
}
