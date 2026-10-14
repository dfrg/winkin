//! The room a line makes for its ruby annotations and emphasis marks, and
//! where its annotations stand.
//!
//! Annotation positions belong to the line, not the column. The first level
//! over every base on a line shares one baseline, which sets the tallest
//! level em box on the tallest base em box. Each outer level sits on the one
//! inside it; levels under the bases stack the same way. This is Chrome's
//! `RubyBlockPositionCalculator::PlaceLines`. The breaker and line layout
//! both call [`LevelBands::stack`].
//!
//! The room follows Chrome's `ComputeAnnotationOverflow` and
//! `SetAnnotationOverflow`:
//! - The line's content is each text item's em box, each atomic inline's
//!   margin box, the emphasis marks and the annotations.
//! - Content past the line box's top moves the line down, less the room the
//!   line before left under its content (`Area::room_above` for the first).
//! - Content past the bottom moves what follows down; the next line moves
//!   back up as far as the room over its own content allows.
//! - Both fold into the line's extent, so `text-box-trim` trims them with the
//!   line. The last line's carry is `Layout::room_below`.
//!
//! Under `Config::emphasis_room = Uniform`, beyond Chrome, marks take no part
//! in that. Each marked line grows instead by what its marks and their text
//! need past its line box, split over and under in proportion to the marks,
//! so lines stay evenly spaced.
//!
//! Only content with ruby or emphasis makes room line by line, as Chrome
//! checks `Node().HasRuby() || has_text_emphasis`. Other blocks measure the
//! last line's carry when `Lines::room_below` is read
//! ([`LineStages::plain_carry`]).

use super::{ColumnPart, RubyLines};
use crate::stages::measure::{ColumnWalk, RubyLevelId};
use alloc::vec::Vec;
use core::ops::Range;

use crate::config::EmphasisRoom;
use crate::data::{Id, RunCursor, heap_bytes};
use crate::stages::LineStages;
use crate::stages::analysis::ClusterId;
use crate::stages::content::{
    Content, ContentFlags, Item, ItemFlags, ItemId, ItemKind, NodeId, TextFactsId, TextFlags,
};
use crate::stages::fonts::FontRunId;
use crate::stages::measure::{Extent, Measured, RubyColumnId, RubyColumns, RubySide, em_box};
use crate::style::EmphasisSide;
use crate::style::FirstLineVariant;
use crate::unit::LayoutUnit;
use crate::work;

/// One level of a line's annotations on one side.
///
/// `extent` unites the em boxes of the level's annotations. `raise` is its
/// baseline's height above the line's, negative below.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct LevelBand {
    side: RubySide,
    extent: Extent,
    raise: LayoutUnit,
}

/// How far a line's annotations reach over and under its baseline; `None` on
/// a side with none.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct AnnotationReach {
    over: Option<LayoutUnit>,
    under: Option<LayoutUnit>,
}

/// The levels of one line's annotations and where each stands.
///
/// Over levels come first, innermost first, then under levels. The breaker
/// and line layout each keep one as scratch, cleared without freeing.
pub(crate) struct LevelBands {
    bands: Vec<LevelBand>,
    /// The number of over levels, which precede the under levels.
    over: usize,
}

impl LevelBands {
    /// Returns an empty set of levels, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            bands: Vec::new(),
            over: 0,
        }
    }

    /// Stacks the annotation levels of a line's columns, and returns how far
    /// they reach.
    ///
    /// The columns are those `parts` continue from before `on_line`, then
    /// `on_line`, whose bases start on the line. Empty levels of split columns
    /// are skipped. The first level sits on the bases' em box, which the line
    /// sets with in-flow content ending at `end`; where every base on a side is
    /// empty, on `line`, the line box's extent, as in Chrome. Linear in the
    /// levels.
    pub(crate) fn stack(
        &mut self,
        rubies: &RubyColumns,
        on_line: Range<RubyColumnId>,
        (line, end): (Extent, ClusterId),
        parts: Option<(&RubyLines, &[ColumnPart])>,
    ) -> AnnotationReach {
        let out = &mut self.bands;
        out.clear();
        self.over = 0;
        let mut reach = AnnotationReach::default();
        let open = parts
            .map_or(&[][..], |(_, parts)| parts)
            .iter()
            .map(|part| part.column)
            .filter(|&column| column < on_line.start);
        let columns = open.chain((on_line.start.get()..on_line.end.get()).map(RubyColumnId::new));
        for side in [RubySide::Over, RubySide::Under] {
            let from = out.len();
            let mut base = Extent::NONE;
            for id in columns.clone() {
                let Some(column) = rubies.get(id) else {
                    continue;
                };
                let mut index = from;
                let part = parts
                    .and_then(|(_, on_line)| on_line.iter().rev().find(|part| part.column == id));
                for (offset, level) in rubies.levels(column).iter().enumerate() {
                    if let Some(part) = part {
                        let empty = parts
                            .and_then(|(parts, _)| {
                                parts.level(
                                    part,
                                    RubyLevelId::new(column.levels.start.get() + offset),
                                )
                            })
                            .is_none_or(|level| level.clusters.is_empty());
                        if empty {
                            continue;
                        }
                    }
                    work::step();
                    if level.side != side {
                        continue;
                    }
                    index = from + level.depth as usize;
                    while out.len() <= index {
                        out.push(LevelBand {
                            side,
                            extent: Extent::NONE,
                            raise: LayoutUnit::ZERO,
                        });
                    }
                    // No extent is a zero-height level at the baseline, as Chrome takes an
                    // empty annotation line.
                    let extent = level.extent.zero_if_none();
                    match out.get_mut(index) {
                        Some(band) => band.extent = band.extent.unite(extent),
                        None => out.push(LevelBand {
                            side,
                            extent,
                            raise: LayoutUnit::ZERO,
                        }),
                    }
                    index += 1;
                }
                if index > from {
                    base = base.unite(rubies.line_base_em(id, column, end));
                }
            }
            if side == RubySide::Over {
                self.over = out.len();
            }
            if out.len() == from {
                continue;
            }
            // Where every base is empty, the levels stand on the line box, as in
            // Chrome (`node_metrics = line_box_metrics`).
            if base.is_none() || base.height() == LayoutUnit::ZERO {
                base = line;
            }
            // A depth no level on the line fills, as under a split column, takes no
            // room: Chrome stacks only the annotation lines a line has.
            let bands = out.get_mut(from..).unwrap_or_default();
            match side {
                RubySide::Over => {
                    let mut top = base.ascent();
                    for band in bands {
                        if band.extent.is_none() {
                            band.raise = top;
                            continue;
                        }
                        band.raise = top + band.extent.descent();
                        top = band.raise + band.extent.ascent();
                    }
                    reach.over = Some(top);
                }
                RubySide::Under => {
                    let mut bottom = base.descent();
                    for band in bands {
                        if band.extent.is_none() {
                            band.raise = -bottom;
                            continue;
                        }
                        band.raise = -(bottom + band.extent.ascent());
                        bottom = -band.raise + band.extent.descent();
                    }
                    reach.under = Some(bottom);
                }
            }
        }
        reach
    }

    /// Returns the baseline height of level `level` on `side` above the line's,
    /// negative below.
    ///
    /// Levels count from zero at the bases. Returns `None` past the side's last
    /// level.
    #[inline]
    pub(crate) fn raise(&self, side: RubySide, level: usize) -> Option<LayoutUnit> {
        let at = match side {
            RubySide::Over => level,
            RubySide::Under => self.over.checked_add(level)?,
        };
        self.bands
            .get(at)
            .filter(|band| band.side == side)
            .map(|band| band.raise)
    }
}

heap_bytes! {
    LevelBands { bands; over }
}

impl Default for LevelBands {
    fn default() -> Self {
        Self::new()
    }
}

/// The rules for a line's annotation and mark room, shared by both variants.
pub(super) struct AnnotationRoom<'a> {
    /// The measurements with each text's mark em box, where some text sets
    /// marks.
    marks: Option<&'a Measured>,
    rule: EmphasisRoom,
}

/// What a line leaves the next at its block-end side, as Chrome's
/// `BlockStartAnnotationSpace`.
///
/// Positive is room under the content; negative is how far the annotations
/// reach past the line box.
pub(super) type Carry = LayoutUnit;

/// The inputs a line's room reads beyond its own items.
pub(super) struct AnnotationContinuation<'a> {
    pub(super) carry: Carry,
    /// The columns whose bases start on the line.
    pub(super) columns: Range<RubyColumnId>,
    /// The line's parts of split columns, with their level table.
    pub(super) parts: Option<(&'a RubyLines, &'a [ColumnPart])>,
    /// The `text-fit` scale of the line's text.
    pub(super) scale: f32,
}

impl<'a> AnnotationRoom<'a> {
    /// Returns whether the content has ruby or marks, so every line makes room.
    ///
    /// As with Chrome's `Node().HasRuby()`, ruby anywhere counts for the whole
    /// block. The answer is the same for both variants.
    pub(super) fn wanted(content: &Content, measured: &Measured) -> bool {
        let flags = content.flags;
        (flags.contains(ContentFlags::RUBY)
            && !measured
                .text(FirstLineVariant::Standard)
                .ruby_columns()
                .is_empty())
            || flags.contains(ContentFlags::EMPHASIS)
    }

    /// Returns the room rules for `content`, with marks under `rule`.
    ///
    /// Built only where [`wanted`](Self::wanted) holds.
    pub(super) fn new(content: &Content, measured: &'a Measured, rule: EmphasisRoom) -> Self {
        let marks = content
            .flags
            .contains(ContentFlags::EMPHASIS)
            .then_some(measured);
        Self { marks, rule }
    }

    /// Returns the extent of the line over `clusters` with annotation and mark
    /// room folded in, and its carry.
    ///
    /// `natural` is the line's own extent; `ems` is what its em boxes and marks
    /// reach. The module doc gives the rules. Out of line, so that a line with
    /// no annotation keeps its extent code small.
    #[inline(never)]
    pub(super) fn room(
        &self,
        stages: &LineStages<'_>,
        bands: &mut LevelBands,
        ems: &LineEms,
        clusters: Range<ClusterId>,
        natural: Extent,
        continuation: AnnotationContinuation<'_>,
    ) -> (Extent, Carry) {
        let AnnotationContinuation {
            carry: before,
            columns: line_columns,
            parts,
            scale,
        } = continuation;
        if natural.is_none() {
            return (natural, LayoutUnit::ZERO);
        }
        let start = clusters.start;
        let (ascent, descent) = (natural.ascent(), natural.descent());
        let rubies = stages.measured.ruby_columns();
        let reach = if rubies.is_empty() {
            AnnotationReach::default()
        } else {
            let end = stages.analysis.clusters.drawn_end(clusters.clone());
            bands.stack(rubies, line_columns.clone(), (natural, end), parts)
        };
        // What the content reaches from the baseline.
        let mut over = reach.over.map_or(ems.over, |reach| ems.over.max(reach));
        let mut under = reach.under.map_or(ems.under, |reach| ems.under.max(reach));
        let (mut annotated_over, mut annotated_under) =
            (reach.over.is_some(), reach.under.is_some());
        // The marked text comes in text order, as the columns do.
        let mut columns = ColumnWalk::from_column(rubies, line_columns.start, start);
        for mark in &ems.marks {
            // A mark on its column's annotation side is set past the annotation.
            let past = annotation_past(rubies, &mut columns, bands, mark.first, mark.side);
            let em = mark.em;
            match mark.side {
                EmphasisSide::Over => {
                    over = over.max(past.unwrap_or(em.ascent()) + mark.height);
                    annotated_over = true;
                }
                EmphasisSide::Under => {
                    under = under.max(past.unwrap_or(em.descent()) + mark.height);
                    annotated_under = true;
                }
            }
        }
        let (mut over, mut under) = stages.letter_room(natural, over, under, scale);
        // Content past the line box counts only on a side with annotations or
        // marks; a font's letters may reach past it anyway.
        if !annotated_over {
            over = over.min(ascent);
        }
        if !annotated_under {
            under = under.min(descent);
        }
        let overflow_over = (over - ascent).max(LayoutUnit::ZERO);
        let space_over = (ascent - over).max(LayoutUnit::ZERO);
        let overflow_under = (under - descent).max(LayoutUnit::ZERO);
        let space_under = (descent - under).max(LayoutUnit::ZERO);
        // The line moves down by what reaches over it, less the room the line
        // before left. After a line that reached past its bottom, it moves up as
        // far as the room over its content allows.
        let mut shift = overflow_over;
        if before < LayoutUnit::ZERO && space_over > LayoutUnit::ZERO {
            shift = -space_over.min(-before);
        }
        if overflow_over > LayoutUnit::ZERO && before > LayoutUnit::ZERO {
            shift = (overflow_over - before).max(LayoutUnit::ZERO);
        }
        let mut extent = Extent::new(ascent + shift, descent + overflow_under);
        let carry = if overflow_under > LayoutUnit::ZERO {
            -overflow_under
        } else {
            space_under
        };
        // Uniform mark room, split over and under in proportion to the marks. The
        // first line counts the room above the block.
        if self.rule == EmphasisRoom::Uniform && !ems.marked.is_none() {
            let (marks_over, marks_under) = (ems.marks_over, ems.marks_under);
            let wanted = marks_over + ems.marked.height() + marks_under;
            let lent = if start == ClusterId::new(0) {
                before.max(LayoutUnit::ZERO)
            } else {
                LayoutUnit::ZERO
            };
            let short = (wanted - extent.height() - lent).max(LayoutUnit::ZERO);
            let marks = marks_over + marks_under;
            if short > LayoutUnit::ZERO && marks > LayoutUnit::ZERO {
                let up = LayoutUnit::from_px(short.to_px() * marks_over.to_px() / marks.to_px());
                extent = Extent::new(extent.ascent() + up, extent.descent() + (short - up));
            }
        }
        (extent, carry)
    }
}

/// The prepared data a line's room reads, in the line's variant.
impl LineStages<'_> {
    /// Returns the block's computed font size in pixels, or 0 for content never
    /// built.
    pub(super) fn block_size(&self) -> f32 {
        let (content, block) = (self.content, NodeId::BLOCK);
        content.nodes.kind(block).map_or(0.0, |_| {
            let request = content.facts.text_request(self.text_facts(block));
            content.facts.request(request).font.computed_size()
        })
    }

    /// Returns the carry of a line without ruby or marks, for
    /// [`Lines::room_below`](super::Lines::room_below).
    ///
    /// It matches what [`AnnotationRoom::room`] gives such a line with nothing
    /// left by the line before. It walks the em boxes once from item `from`,
    /// scaled by the line's `text-fit` `scale`.
    pub(super) fn plain_carry(
        &self,
        from: ItemId,
        clusters: Range<ClusterId>,
        natural: Extent,
        scale: f32,
    ) -> Carry {
        debug_assert!(
            self.measured.ruby_columns().is_empty(),
            "a line with ruby makes its room"
        );
        if natural.is_none() {
            return LayoutUnit::ZERO;
        }
        let mut ems = LineEms::new();
        let mut walk = EmWalk::new(self, None, &mut ems, clusters.clone());
        let items = &self.content.items;
        let item_clusters = &self.analysis.item_clusters;
        let mut id = from;
        while items.get(id).is_some() {
            work::step();
            let on_line = item_clusters.range(id);
            if on_line.start >= clusters.end {
                break;
            }
            if on_line.end > clusters.start {
                walk.visit(self, id, on_line);
            }
            id = ItemId::new(id.get() + 1);
        }
        // As `room` does with nothing past the line box: content under the line
        // box counts to its bottom.
        if scale != 1.0 {
            ems.scale(scale);
        }
        let (_, under) = self.letter_room(natural, ems.over, ems.under, scale);
        let descent = natural.descent();
        (descent - under.min(descent)).max(LayoutUnit::ZERO)
    }

    /// Returns how far a line's content reaches over and under its baseline,
    /// keeping room for a letter of the block's size.
    ///
    /// As in Chrome, a nearly empty line reaches its line box `natural` less
    /// half the leading such a letter leaves.
    fn letter_room(
        &self,
        natural: Extent,
        over: LayoutUnit,
        under: LayoutUnit,
        scale: f32,
    ) -> (LayoutUnit, LayoutUnit) {
        let (ascent, descent) = (natural.ascent(), natural.descent());
        let size = self.block_size();
        let size = LayoutUnit::from_px(if scale == 1.0 { size } else { size * scale });
        if over == LayoutUnit::MIN || under == LayoutUnit::MIN || over + under < size {
            let leading = ((ascent + descent - size).half()).max(LayoutUnit::ZERO);
            return (ascent - leading, descent - leading);
        }
        (over, under)
    }

    /// Returns the em box of item `id` over its `clusters` on a line.
    ///
    /// Text gives its em box, from the font runs where it has no single box; an
    /// atomic inline gives its margin box; anything else gives none.
    fn em(
        &self,
        id: ItemId,
        item: &Item,
        text: TextFactsId,
        clusters: Range<ClusterId>,
        runs: &mut Option<RunCursor<FontRunId, ClusterId>>,
    ) -> Extent {
        match item.kind {
            ItemKind::Text => self.measured.em_boxes().get(id).unwrap_or_else(|| {
                let (fonts, variant) = (self.fonts, self.variant);
                let (table, end) = (fonts.runs(variant), self.analysis.clusters.end_id());
                let Some(mut run) = runs.or_else(|| table.cursor_containing(clusters.start, end))
                else {
                    return Extent::NONE;
                };
                table.step_to(&mut run, clusters.start, end);
                *runs = Some(run);
                let request = self.content.facts.text_request(text);
                em_box(fonts, request, clusters, variant, run.id()).0
            }),
            ItemKind::Atomic => self.measured.extents.get(id),
            _ => Extent::NONE,
        }
    }
}

/// What a line's em boxes and emphasis marks reach, gathered by an
/// [`EmWalk`] during the extent walk.
///
/// [`AnnotationRoom::room`] then does only arithmetic over it. The breaker
/// keeps one in its scratch.
pub(super) struct LineEms {
    /// How far the em boxes reach over and under the baseline; `MIN` on a line
    /// with none.
    over: LayoutUnit,
    under: LayoutUnit,
    /// Under uniform mark room, the tallest marks each side and the text they
    /// mark.
    marks_over: LayoutUnit,
    marks_under: LayoutUnit,
    marked: Extent,
    /// Otherwise, the marked text items in text order.
    marks: Vec<EmMark>,
}

/// A marked text item on a line.
#[derive(Copy, Clone, Debug)]
struct EmMark {
    /// Its first cluster, which names the column whose annotation its marks may
    /// be set past.
    first: ClusterId,
    /// Its em box on the line.
    em: Extent,
    side: EmphasisSide,
    /// Its marks' height.
    height: LayoutUnit,
}

impl LineEms {
    /// Returns an empty record, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            over: LayoutUnit::MIN,
            under: LayoutUnit::MIN,
            marks_over: LayoutUnit::ZERO,
            marks_under: LayoutUnit::ZERO,
            marked: Extent::NONE,
            marks: Vec::new(),
        }
    }

    /// Scales the reaches by `scale`, on a line whose text `text-fit` scales.
    pub(super) fn scale(&mut self, scale: f32) {
        let scaled = |length: LayoutUnit| {
            if length == LayoutUnit::MIN {
                length
            } else {
                length.times_truncated(scale)
            }
        };
        let scaled_extent = |extent: Extent| {
            if extent.is_none() {
                extent
            } else {
                Extent::new(scaled(extent.ascent()), scaled(extent.descent()))
            }
        };
        self.over = scaled(self.over);
        self.under = scaled(self.under);
        self.marks_over = scaled(self.marks_over);
        self.marks_under = scaled(self.marks_under);
        self.marked = scaled_extent(self.marked);
        for mark in &mut self.marks {
            mark.em = scaled_extent(mark.em);
            mark.height = scaled(mark.height);
        }
    }

    /// Clears the record, keeping the marks' allocation.
    fn clear(&mut self) {
        self.over = LayoutUnit::MIN;
        self.under = LayoutUnit::MIN;
        self.marks_over = LayoutUnit::ZERO;
        self.marks_under = LayoutUnit::ZERO;
        self.marked = Extent::NONE;
        self.marks.clear();
    }
}

heap_bytes! {
    LineEms { marks; over, under, marks_over, marks_under, marked }
}

/// The gathering of one line's [`LineEms`], item by item.
///
/// It skips the initial letter's node, and carries one cursor over the font
/// runs for text items with no single em box.
pub(super) struct EmWalk<'w, 'a> {
    /// The mark rules, where the line makes room for marks.
    room: Option<&'w AnnotationRoom<'a>>,
    ems: &'w mut LineEms,
    letter: Option<NodeId>,
    runs: Option<RunCursor<FontRunId, ClusterId>>,
    line: Range<ClusterId>,
}

impl<'w, 'a> EmWalk<'w, 'a> {
    /// Starts gathering the line over `line` into `ems`, with marks under
    /// `room`.
    pub(super) fn new(
        stages: &LineStages<'_>,
        room: Option<&'w AnnotationRoom<'a>>,
        ems: &'w mut LineEms,
        line: Range<ClusterId>,
    ) -> Self {
        ems.clear();
        Self {
            room,
            ems,
            letter: stages.measured.initial_letter().map(|letter| letter.node),
            runs: None,
            line,
        }
    }

    /// Adds item `id`, which contains `clusters`, some on the line.
    ///
    /// Skips annotations, which belong to their column, and items with no
    /// cluster. Skips the initial letter too: as in Chrome's
    /// `ComputeAnnotationOverflow`, it adds no height to the line.
    pub(super) fn visit(
        &mut self,
        stages: &LineStages<'_>,
        id: ItemId,
        clusters: Range<ClusterId>,
    ) {
        let Some(item) = stages.content.items.get(id) else {
            return;
        };
        if clusters.is_empty() || item.flags.contains(ItemFlags::ANNOTATION) {
            return;
        }
        let content = stages.content;
        if self
            .letter
            .is_some_and(|letter| content.nodes.contains(letter, item.node))
        {
            return;
        }
        let text = stages.text_facts(item.node);
        let on_line = clusters.start.max(self.line.start)..clusters.end.min(self.line.end);
        let em = stages.em(id, item, text, on_line, &mut self.runs);
        if em.is_none() {
            return;
        }
        let ems = &mut *self.ems;
        ems.over = ems.over.max(em.ascent());
        ems.under = ems.under.max(em.descent());
        let Some(room) = self.room else {
            return;
        };
        let Some(measured) = room.marks else {
            return;
        };
        let row = content.facts.text(text);
        if item.kind != ItemKind::Text || !row.has(TextFlags::EMPHASIS) {
            return;
        }
        let Some(metrics) = measured.text_metrics(text) else {
            return;
        };
        let (side, height) = (row.emphasis_side, metrics.mark.height());
        if room.rule == EmphasisRoom::Uniform {
            ems.marked = ems.marked.unite(em);
            match side {
                EmphasisSide::Over => ems.marks_over = ems.marks_over.max(height),
                EmphasisSide::Under => ems.marks_under = ems.marks_under.max(height),
            }
            return;
        }
        let first = clusters.start;
        ems.marks.push(EmMark {
            first,
            em,
            side,
            height,
        });
    }
}

/// Returns how far the outermost annotation on `side` over `cluster`'s base
/// reaches from the baseline.
///
/// An emphasis mark in that base is set past it, as Chrome's
/// `SetTextEmphasisAnnotationMetrics` does. The annotation's em box stands on
/// its level's baseline (`UpdateColumnLayoutAnnotationMetrics`).
fn annotation_past(
    rubies: &RubyColumns,
    columns: &mut ColumnWalk<'_>,
    bands: &LevelBands,
    cluster: ClusterId,
    side: EmphasisSide,
) -> Option<LayoutUnit> {
    let side = RubySide::from(side);
    let outermost = columns.outermost(cluster, side)?;
    let level = rubies.level(outermost)?;
    let extent = level.extent.zero_if_none();
    let raise = bands.raise(side, level.depth as usize)?;
    Some(match side {
        RubySide::Over => raise + extent.ascent(),
        RubySide::Under => -raise + extent.descent(),
    })
}
