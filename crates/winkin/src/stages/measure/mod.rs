//! Measurement: everything about the text that doesn't depend on the line
//! width.
//!
//! In: [`MeasureInput`]. Out: [`Measured`], frozen until the next measure.
//! Start at [`measure_text`], then `Scan::measure` in `scan`.
//!
//! - `scan`: the prefix sums, intrinsic widths, item extents, em boxes,
//!   line-edge costs, ruby columns and initial letter.
//! - `metrics`: each text's primary-font metrics.
//! - `boxes`: the boxes' fixed shifts and extents.
//! - `edges`, `spacing`, `autospace`, `tabs`, `hang`, `justify`, `ruby`:
//!   rules shared with line breaking and line layout.
//! - `pen`: the pen position at a cluster, and what a cluster adds.
//! - `extent`: [`Extent`], the reach over and under a baseline.
//! - `prefix`, `items`, `ruby_columns`, `generated`: the stored tables.

mod autospace;
mod boxes;
mod edges;
mod extent;
mod generated;
mod hang;
mod items;
mod justify;
mod metrics;
mod pen;
mod prefix;
mod ruby;
mod ruby_columns;
mod scan;
mod spacing;
mod tabs;
#[cfg(test)]
mod tests;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::Range;

use crate::config::{
    DominantBaselines, RubyBreakWithin, RubyOverhangRule, SuperSubPosition, WordSpacing,
};
use crate::data::{Id, Keyed, SortedTable, Table, define_flags, define_id, heap_bytes};
use crate::stages::analysis::{Analysis, ClusterId, ParagraphId};
use crate::stages::content::{AtomicId, Content, NodeId, TextFactsId};
use crate::stages::fonts::{FontLineMetrics, Fonts, LineBaseline};
use crate::stages::shape::{ShapeSession, Shaped};
use crate::style::{FirstLine, FirstLineState, FirstLineVariant};
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use scan::{Scan, ScanPrefix};

pub(super) use autospace::AutospaceRules;
pub(crate) use edges::{BoundarySplit, edge_room, items_after_break};
pub(crate) use extent::Extent;
use generated::GeneratedTexts;
pub(crate) use generated::{GeneratedInput, GeneratedItems, ShapedGenerated};
pub(crate) use items::KeptBoxes;
use items::{ItemEmBoxes, ItemExtents};
pub(crate) use justify::{JustifyOpportunities, JustifySummary};
pub(super) use metrics::normal_extent;
use metrics::text_metrics;
use prefix::LineEdgeCosts;
pub(crate) use prefix::PrefixAdvances;
pub(crate) use ruby::{RubySpread, em_box};
pub(crate) use ruby_columns::{
    ColumnWalk, RubyColumn, RubyColumns, RubyLevel, RubySide, is_monolithic,
};
pub(crate) use spacing::{LetterWordSpacing, WordSpacingRule};
#[cfg(test)]
use tabs::tab_advance;
pub(crate) use tabs::tab_advance_reached;
use tabs::{TabReach, TabStops};

/// The measure stage's input: the stages before it and some config.
pub(crate) struct MeasureInput<'a> {
    pub(crate) content: &'a Content,
    pub(crate) analysis: &'a Analysis,
    pub(crate) fonts: &'a Fonts,
    pub(crate) shaped: &'a Shaped,
    pub(crate) word_spacing: WordSpacing,
    /// How far `vertical-align: super` and `sub` move a box.
    pub(crate) super_sub: SuperSubPosition,
    /// Whether `dominant-baseline` moves a box.
    pub(crate) dominant_baseline: DominantBaselines,
    /// How far a ruby annotation may reach over the text beside it.
    pub(crate) ruby_overhang: RubyOverhangRule,
    /// Whether a ruby column breaks only where every annotation has an opportunity.
    pub(crate) ruby_break_within: RubyBreakWithin,
    /// `text-indent`'s length as the intrinsic sizes read it: in an area of
    /// zero width.
    pub(crate) indent: LayoutUnit,
}

impl<'a> MeasureInput<'a> {
    /// Returns the clusters `variant` measures: all, or the first paragraph's.
    fn reach(&self, variant: FirstLineVariant) -> Range<ClusterId> {
        let all = ClusterId::new(0)..self.analysis.clusters.end_id();
        match variant {
            FirstLineVariant::Standard => all,
            FirstLineVariant::FirstLine => {
                self.analysis.first_line_reach(self.content).unwrap_or(all)
            }
        }
    }

    /// Returns the line metrics of `text`'s primary font, its request's first
    /// resolved font, or `None` where it resolved none.
    fn primary(&self, text: TextFactsId) -> Option<&'a FontLineMetrics> {
        let request = self.content.facts.text_request(text);
        self.fonts.primary_font(request).map(|used| &used.metrics)
    }

    /// Returns `text`'s computed font size.
    fn size(&self, text: TextFactsId) -> f32 {
        let facts = &self.content.facts;
        facts.request(facts.text_request(text)).font.computed_size()
    }
}

/// Measures the text of `input` into `out`, summing shaping's advances into
/// prefix sums.
///
/// With `new_fonts`, as after a build, it also measures the text metrics and
/// shapes the hyphens and ellipses with `cx`. Without, it keeps both: they
/// depend only on the fonts and the text facts. Where `::first-line`
/// restyles something, it measures the first paragraph again in those styles.
///
/// Only this writes `out`. A warm measure allocates nothing.
pub(crate) fn measure_text<'f>(
    input: &MeasureInput<'f>,
    cx: &mut ShapeSession<'_, 'f>,
    scratch: &mut MeasureScratch,
    out: &mut Measured,
    new_fonts: Option<GeneratedInput<'_>>,
) {
    out.clear_text();
    out.word_spacing_rule = WordSpacingRule::new(input.word_spacing, input.content);
    if let Some(generated_input) = new_fonts {
        out.text_metrics.clear();
        text_metrics(input, generated_input.block_space, &mut out.text_metrics);
        if let Some(generated) = &mut out.generated {
            generated.clear();
        }
        // Font selection chose fonts for some generated text.
        if generated_input.fonts.iter().next().is_some() {
            out.generated_mut().fill(input, cx, generated_input);
        }
    }
    let first = input.analysis.first_line_reach(input.content);
    let Measured {
        text,
        text_metrics,
        generated,
        intrinsic,
        word_spacing_rule,
    } = out;
    let generated = generated.as_deref().unwrap_or(&NO_GENERATED);
    let scan = |variant| Scan::new(input, variant, *word_spacing_rule, generated, text_metrics);
    let widths = scan(FirstLineVariant::Standard).measure(cx, scratch, text.text_mut());
    if first.is_none() {
        *intrinsic = widths.finish(None);
        return;
    }
    let first_widths =
        scan(FirstLineVariant::FirstLine).measure(cx, scratch, text.first_line_mut());
    *intrinsic = widths.finish(Some(&first_widths));
}

/// The scan's reusable buffers: per-level cursors that cut ruby columns.
#[derive(Default)]
pub(crate) struct MeasureScratch {
    ruby_starts: Vec<ClusterId>,
}
heap_bytes! { MeasureScratch { ruby_starts } }

define_id! {
    /// Names a [`RubyLevel`], one annotation level of a ruby column.
    pub(crate) struct RubyLevelId(u32);
}

define_id! {
    /// Names a [`RubyColumn`]. Columns are in text order.
    pub(crate) struct RubyColumnId(u32);
}

define_id! {
    /// Names an inline box in [`KeptBoxes`], which is in node order.
    pub(crate) struct KeptBoxId(u32);
}

define_id! {
    /// Names an em box in [`ItemEmBoxes`]: one per distinct font and size.
    struct EmBoxId(u8);
}

impl EmBoxId {
    /// The first box, [`Extent::NONE`], named by an item with no em box.
    const NONE: Self = Self(0);
}

define_id! {
    /// Names a [`ShapedGenerated`], one generated text.
    pub(crate) struct ShapedGeneratedId(u32);
}

define_flags! {
    /// Per-paragraph facts the breaker reads to skip work. One byte a paragraph.
    pub(crate) struct MeasureFlags(u8) {
        /// The prefix decreases from one boundary to the next.
        ///
        /// The breaker then walks the paragraph instead of searching it. A dip
        /// between boundaries doesn't count. Tabs also force a walk (analysis's
        /// `HAS_TABS`).
        pub(super) const NONMONOTONE = 1 << 0;
        /// Some line-edge cost falls in the paragraph.
        pub(super) const HAS_EDGE_COSTS = 1 << 1;
        /// `text-autospace` puts room at a seam in the paragraph. The breaker
        /// looks for seams only where this is set.
        pub(super) const HAS_AUTOSPACE = 1 << 2;
        /// An inline box open in the paragraph has a strut past the block's. The
        /// breaker unites open boxes' struts only where this is set.
        pub(super) const BOXES_PAST_STRUT = 1 << 3;
        /// Some root ruby column may break inside. The breaker fits ruby
        /// continuations only where this is set.
        pub(super) const HAS_BREAKABLE_RUBY = 1 << 4;
    }
}

define_flags! {
    /// What a line-edge cost says beyond its amounts.
    pub(crate) struct LineEdgeFlags(u8) {
        /// A line ending here hyphenates: `end` is the hyphen's width.
        pub(super) const HYPHEN = 1 << 0;
        /// The start amount gives back the first cluster's hanging mark, on the
        /// block's first line only (`hanging-punctuation: first`).
        pub(super) const FIRST_LINE_ONLY = 1 << 1;
        /// The flex always hangs (`force-end` or `last`). Without this flag, the
        /// breaker takes the flex only where the line would not fit.
        pub(super) const HANG_FORCED = 1 << 2;
    }
}

/// What a line pays or gains for starting or ending at one boundary.
///
/// Sources: a hyphenated break's hyphen, cloned box edges, `line-padding`,
/// `hanging-punctuation`, and ruby overhang with no neighbour. None depends
/// on the width, so the breaker reads them from a sparse table sorted by
/// boundary, 20 bytes a row. Plain text has none. The default pays nothing.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) struct LineEdgeCost {
    /// The boundary.
    at: ClusterId,
    /// What a line ending here pays.
    pub(super) end: LayoutUnit,
    /// What a line starting here pays.
    pub(super) start: LayoutUnit,
    /// The part of `end` the breaker gives back where the line would overflow,
    /// or always under [`LineEdgeFlags::HANG_FORCED`].
    pub(super) flex: LayoutUnit,
    /// Whether it hyphenates, applies to the first line only, or hangs always.
    pub(super) flags: LineEdgeFlags,
}

impl Keyed for LineEdgeCost {
    type Key = ClusterId;

    /// The boundary, which [`LineEdgeCosts`] is sorted by.
    #[inline]
    fn key(&self) -> ClusterId {
        self.at
    }
}

impl LineEdgeCost {
    /// Returns `end`, less the flex where it always hangs.
    pub(crate) fn charged_end(&self) -> LayoutUnit {
        if self.flags.contains(LineEdgeFlags::HANG_FORCED) {
            self.end - self.flex
        } else {
            self.end
        }
    }
}

/// The content's min-content and max-content widths, in `LayoutUnit`s.
///
/// [`IntrinsicSizes`] hands these to the host in pixels.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) struct IntrinsicWidths {
    /// Min-content: the widest content no line may break inside.
    min: LayoutUnit,
    /// Max-content: the widest paragraph.
    max: LayoutUnit,
}

/// Intrinsic content widths for shrink-to-fit boxes, table cells and flex items.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
#[non_exhaustive]
pub struct IntrinsicSizes {
    /// The min-content width in pixels: the widest unbreakable content.
    pub min_content: f32,
    /// The max-content width in pixels: the widest paragraph.
    pub max_content: f32,
}

impl From<IntrinsicWidths> for IntrinsicSizes {
    /// Converts the widths to pixels.
    fn from(intrinsic: IntrinsicWidths) -> Self {
        Self {
            min_content: intrinsic.min.to_px(),
            max_content: intrinsic.max.to_px(),
        }
    }
}

/// The facts of one [`TextFactsId`] that need its primary font.
///
/// One row per text facts, the first line's included, so one table serves
/// both variants. 48 bytes a row; a document has a few.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct TextMetrics {
    /// The primary font's ascent and descent, leaded to the line height as
    /// Blink's `CalculateLeadingSpace` does.
    ///
    /// Text, a `<br>` and a box's opening edge take this extent; the block's is
    /// every line's strut. [`Extent::NONE`] where there is no primary font.
    strut: Extent,
    /// The used line height, as Chrome's `ComputedLineHeightAsFixed` makes it.
    ///
    /// `normal` is the primary font's line spacing, a number scales the
    /// computed size, and a length rounds to the grid. A `vertical-align`
    /// percentage is of this height. Zero where there is no primary font.
    line_height: LayoutUnit,
    /// The tab stops: `tab-size` and the block's letter-spacing.
    pub(super) tab: TabStops,
    /// The em box emphasis marks are drawn in: the primary font's, at half the
    /// computed size.
    ///
    /// The size rounds to a whole pixel (`lroundf`), the ascent to 1/64, and the
    /// descent takes the rest, as in Blink's `SimpleFontData::EmphasisMarkFontData`.
    /// [`Extent::NONE`] where the text sets no marks.
    pub(crate) mark: Extent,
}

/// Measured text: the whole text's, or the first paragraph's under
/// `::first-line` styles.
pub(crate) struct MeasuredText {
    /// The prefix advances.
    pub(crate) prefix: PrefixAdvances,
    /// Each paragraph's flags ([`paragraph`](Self::paragraph)).
    paragraphs: Table<ParagraphId, MeasureFlags>,
    /// Each item's extent, and the strut.
    pub(crate) extents: ItemExtents,
    /// The tables only some content has, boxed on first use and kept after.
    /// Most layouts hold `None`.
    rare: Option<Box<RareMeasures>>,
}

/// The tables of a [`MeasuredText`] that only some content fills.
///
/// A text without them reads [`RareMeasures::NONE`].
struct RareMeasures {
    edge_costs: LineEdgeCosts,
    /// How far each box or atomic inline is raised from its parent's
    /// baseline, positive up, sorted by node. A node with no entry doesn't move.
    ///
    /// Only line-independent shifts: `vertical-align: super`, `sub`, a length
    /// or a percentage, and `dominant-baseline` where the config applies it.
    /// The breaker works out `middle`, `text-top`, `text-bottom`, `top` and
    /// `bottom` per line.
    shifts: SortedTable<(NodeId, LayoutUnit)>,
    em_boxes: ItemEmBoxes,
    rubies: RubyColumns,
    /// The boxes that keep a fragment, with their fragments' extents.
    kept_boxes: KeptBoxes,
    /// The block's initial letter, where it has one.
    initial_letter: Option<InitialLetter>,
}

impl RareMeasures {
    /// Empty tables.
    const NONE: Self = Self::new();

    const fn new() -> Self {
        Self {
            edge_costs: LineEdgeCosts::new(),
            shifts: SortedTable::new(),
            em_boxes: ItemEmBoxes::new(),
            rubies: RubyColumns::new(),
            kept_boxes: KeptBoxes::new(),
            initial_letter: None,
        }
    }

    /// Returns empty tables on the heap. Out of line, so a writer inlines only
    /// the test for the box.
    #[cold]
    #[inline(never)]
    fn boxed() -> Box<Self> {
        Box::new(Self::new())
    }

    fn clear(&mut self) {
        self.edge_costs.clear();
        self.shifts.clear();
        self.em_boxes.clear();
        self.rubies.clear();
        self.kept_boxes.clear();
        self.initial_letter = None;
    }
}

/// The rare tables a text without its box reads.
static NO_RARE_MEASURES: RareMeasures = RareMeasures::NONE;

impl MeasuredText {
    const fn new() -> Self {
        Self {
            prefix: PrefixAdvances::new(),
            paragraphs: Table::new(),
            extents: ItemExtents::new(),
            rare: None,
        }
    }

    fn clear(&mut self) {
        self.prefix.clear();
        self.paragraphs.clear();
        self.extents.clear();
        if let Some(rare) = &mut self.rare {
            rare.clear();
        }
    }

    /// Returns the rare tables, empty where the build had none.
    #[inline]
    fn rare(&self) -> &RareMeasures {
        self.rare.as_deref().unwrap_or(&NO_RARE_MEASURES)
    }

    /// Returns the rare tables for writing, boxing them on first use.
    fn rare_mut(&mut self) -> &mut RareMeasures {
        self.rare.get_or_insert_with(RareMeasures::boxed)
    }

    /// Returns the flags of `paragraph`, or none past the last paragraph.
    #[inline]
    pub(crate) fn paragraph(&self, paragraph: ParagraphId) -> MeasureFlags {
        self.paragraphs
            .get(paragraph)
            .copied()
            .unwrap_or(MeasureFlags::NONE)
    }

    /// Returns each text item's em box, where the content has ruby or marks.
    #[inline]
    pub(crate) fn em_boxes(&self) -> &ItemEmBoxes {
        &self.rare().em_boxes
    }

    /// Returns how far `node`'s baseline is raised from its parent's,
    /// independent of the line. Negative lowers it; zero where none is set.
    #[inline]
    pub(super) fn shift(&self, node: NodeId) -> LayoutUnit {
        self.shifts()
            .get(node)
            .map_or(LayoutUnit::ZERO, |&(_, shift)| shift)
    }

    /// Returns the baseline shifts, sorted by node: none where no box has one.
    #[inline]
    pub(super) fn shifts(&self) -> &SortedTable<(NodeId, LayoutUnit)> {
        &self.rare().shifts
    }

    /// Returns the line-edge costs: none where no line's edge costs anything.
    #[inline]
    pub(crate) fn edge_costs(&self) -> &LineEdgeCosts {
        &self.rare().edge_costs
    }

    /// Returns the boxes that keep a fragment, with their fragments' extents:
    /// none where every box is culled.
    #[inline]
    pub(crate) fn kept_boxes(&self) -> &KeptBoxes {
        &self.rare().kept_boxes
    }

    /// Returns the ruby columns and their levels.
    #[inline]
    pub(crate) fn ruby_columns(&self) -> &RubyColumns {
        &self.rare().rubies
    }

    /// Returns the block's initial letter, or `None` where it has none.
    #[inline]
    pub(crate) fn initial_letter(&self) -> Option<&InitialLetter> {
        self.rare().initial_letter.as_ref()
    }
}

impl FirstLineState for MeasuredText {
    fn clear(&mut self) {
        MeasuredText::clear(self);
    }

    /// Returns whether it holds no prefix: a variant the build had no use for.
    fn is_empty(&self) -> bool {
        self.prefix.is_empty()
    }
}

impl Default for MeasuredText {
    fn default() -> Self {
        Self::new()
    }
}

/// The block's initial letter, sized.
///
/// It is the box at the block's start that sets `initial-letter`: its
/// `::first-letter` or first inline box. The box fits the ink of what it
/// holds, as Chrome's `LayoutInitialLetterBox` does.
///
/// - **Along the line:** its edges take their margin, border and padding,
///   less the letter's advance outside its ink. The prefix then holds the
///   letter's advances plus exactly its margin box.
/// - **Across the lines:** it stands on no line, its baseline on the
///   `size`-th line's. A raised letter moves the first line down. The lines
///   below make room for it as an exclusion the host places.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct InitialLetter {
    /// The box.
    pub(crate) node: NodeId,
    /// The room its opening and closing edges take in the prefix
    /// ([`edge_room`](Self::edge_room)).
    start_room: InlineLayoutUnit,
    end_room: InlineLayoutUnit,
    /// Its start and end margins. A negative side bearing kerns the start
    /// margin of a box with no border or padding (Chrome's "inline kerning").
    pub(super) start_margin: LayoutUnit,
    pub(super) end_margin: LayoutUnit,
    /// Its border box around the letter's baseline.
    pub(crate) extent: Extent,
    /// How far the letter's baseline is below the top of the first line's
    /// box, before a raised letter moves the line's text down.
    pub(super) baseline: LayoutUnit,
    /// How far a raised letter moves the first line's text down: `size` less
    /// `sink` lines.
    pub(super) raise: LayoutUnit,
    /// Its margin box as an exclusion for the lines after the first: the top
    /// from the first line's top, the height across the lines, the width along.
    pub(super) exclusion_top: LayoutUnit,
    pub(super) exclusion_height: LayoutUnit,
    pub(super) exclusion_width: LayoutUnit,
}

impl InitialLetter {
    /// Returns the room the prefix gave its opening edge (`opens`) or its
    /// closing one.
    pub(crate) fn edge_room(&self, opens: bool) -> InlineLayoutUnit {
        if opens {
            self.start_room
        } else {
            self.end_room
        }
    }
}

/// The measurements: the text, its first line's variant, and what both
/// share.
pub(crate) struct Measured {
    /// The text, and the first line's measure.
    ///
    /// The first line's measure covers the first paragraph under `::first-line`
    /// styles, and is absent where they change no measurement. Once made, it
    /// is kept with its capacity.
    text: FirstLine<MeasuredText>,
    /// Each text's primary-font metrics, one row per [`TextFactsId`] of either
    /// variant.
    text_metrics: Table<TextFactsId, TextMetrics>,
    /// The shaped hyphens and ellipses of both variants, boxed on first use
    /// and kept.
    generated: Option<Box<GeneratedTexts>>,
    /// The min-content and max-content widths.
    pub(crate) intrinsic: IntrinsicWidths,
    /// Which clusters took word-spacing in the prefix. The breaker spaces a
    /// reshaped edge by the same rule.
    pub(super) word_spacing_rule: WordSpacingRule,
}

/// The empty generated texts.
static NO_GENERATED: GeneratedTexts = GeneratedTexts::new();

impl Measured {
    /// Returns empty measurements, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            text: FirstLine::new(MeasuredText::new()),
            text_metrics: Table::new(),
            generated: None,
            intrinsic: IntrinsicWidths {
                min: LayoutUnit::ZERO,
                max: LayoutUnit::ZERO,
            },
            word_spacing_rule: WordSpacingRule::CHROME,
        }
    }

    /// Empties the measurements, keeping every allocation.
    ///
    /// A layout whose builder is dropped unfinished holds this.
    pub(crate) fn clear(&mut self) {
        self.clear_text();
        self.text_metrics.clear();
        if let Some(generated) = &mut self.generated {
            generated.clear();
        }
    }

    /// Empties all but the text metrics and the generated texts, which depend
    /// only on the fonts and the text facts.
    fn clear_text(&mut self) {
        self.text.clear();
        self.intrinsic = IntrinsicWidths::default();
        self.word_spacing_rule = WordSpacingRule::CHROME;
    }

    /// Returns the generated texts for writing, boxing them on first use.
    fn generated_mut(&mut self) -> &mut GeneratedTexts {
        self.generated
            .get_or_insert_with(|| Box::new(GeneratedTexts::new()))
    }

    /// Measures atomic inline `atomic`'s extent again from `content`, in both
    /// variants.
    ///
    /// The content holds no ruby and no initial letter, so the extent reaches
    /// no other item and nothing along the line changes.
    pub(crate) fn remeasure_atomic(&mut self, content: &Content, atomic: AtomicId) {
        let Some(atomic) = content.atomics().get(atomic) else {
            return;
        };
        let baseline = LineBaseline::from_content(content);
        let extent = Extent::from_atomic(atomic, content.block.writing_mode, baseline);
        self.text.text_mut().extents.set(atomic.item, extent);
        if let Some(first_line) = self.text.kept_first_line_mut() {
            first_line.extents.set(atomic.item, extent);
        }
    }

    /// Returns the first line's measure, or `None` where `::first-line`
    /// restyles nothing measured.
    pub(crate) fn first_line(&self) -> Option<&MeasuredText> {
        self.text.first_line()
    }

    /// Returns the text as `variant` reads it. The first line falls back to the
    /// text's measure.
    #[inline]
    pub(crate) fn text(&self, variant: FirstLineVariant) -> &MeasuredText {
        self.text.get(variant)
    }

    /// Returns the [`TextMetrics`] of `text`, or `None` for facts the content
    /// doesn't have.
    #[inline]
    pub(crate) fn text_metrics(&self, text: TextFactsId) -> Option<&TextMetrics> {
        self.text_metrics.get(text)
    }

    /// Returns the shaped hyphens and ellipses, one per row of text facts,
    /// shared by both variants.
    pub(crate) fn generated(&self) -> &GeneratedTexts {
        self.generated.as_deref().unwrap_or(&NO_GENERATED)
    }

    /// Returns the heap bytes of the first line's measure, which
    /// [`heap_bytes`](crate::data::HeapBytes::heap_bytes) includes.
    pub(crate) fn first_line_heap_bytes(&self) -> usize {
        self.text.first_line_heap_bytes()
    }
}

heap_bytes! {
    /// Its texts' tables, and the generated texts' box where a build made it.
    Measured { text, text_metrics, generated; intrinsic, word_spacing_rule }
}

heap_bytes! {
    RareMeasures { edge_costs, shifts, em_boxes, rubies, kept_boxes; initial_letter }
}

heap_bytes! {
    /// Its tables, and its rare tables' box where a build made it.
    MeasuredText { prefix, paragraphs, extents, rare }
}

heap_bytes! {
    PrefixAdvances { sums }
}

heap_bytes! {
    ItemExtents { extents; strut }
}

heap_bytes! {
    ItemEmBoxes { items, boxes }
}

heap_bytes! {
    KeptBoxes { boxes }
}

heap_bytes! {
    RubyColumns { columns, levels, steps, floors, rooms, tab_reaches, trimmed_ems }
}

heap_bytes! {
    GeneratedTexts { texts, glyphs }
}

impl Default for Measured {
    fn default() -> Self {
        Self::new()
    }
}
