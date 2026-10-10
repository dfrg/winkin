//! The walk over the clusters that finds the shaping runs and shapes each one.
//!
//! `shape_text` shapes one variant's clusters. A walk finds where each
//! shaping run ends, and each run goes through the one shaping call,
//! [`shape_range`]. Combined units are fitted to their ems as the walk
//! reaches each one's end.

use core::ops::Range;

use super::sink::SinkMark;
use super::{
    CombineFit, CombineWidth, GlyphSink, NeighbourFonts, ShapeInput, ShapeSession, ShapedFlags,
    ShapedRun, ShapedRunId, ShapedRuns, ShapedText, ShapingEdges, ShapingKey, ShapingSource,
    maybe_opening_mark, shape_range,
};
use crate::data::{Id, IdRange, RunCursor, SortedTable, TextOffset, make_room};
use crate::stages::analysis::{
    Analysis, ClusterAttrs, ClusterClass, ClusterId, ParagraphFlags, ParagraphId, RunOrientation,
    ScriptRunId,
};
use crate::stages::content::{ItemFlags, ItemId, ItemKind, ShapingFactsId};
use crate::stages::fonts::{FontRunId, FontRuns, UsedFontId};
use crate::style::FirstLineVariant;
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::work;

/// Shapes the clusters before `end` in `variant`'s styles, fonts and text
/// into `out` and `advances`.
///
/// Returns how many glyphs were left out, and the flags for the shaping.
pub(super) fn shape_text(
    input: &ShapeInput<'_>,
    variant: FirstLineVariant,
    end: ClusterId,
    cx: &mut ShapeSession<'_, '_>,
    out: &mut ShapedText,
) -> (usize, ShapedFlags) {
    let ShapedText {
        glyphs,
        runs,
        combined,
        advances,
    } = out;
    let analysis = input.analysis;
    // Reserve a word per cluster, and at least one shaping run per script
    // run the walk will pass. Reserve an advance per cluster plus the one
    // entry measurement appends for its prefix sums; without that entry the
    // buffer would double.
    let script_runs = analysis
        .runs
        .iter()
        .take_while(|(_, run)| run.start < end)
        .count();
    glyphs.words.reserve(end.get());
    runs.reserve(end.get(), script_runs);
    make_room(advances, end.get() + 1);
    // A cluster of one expanded glyph points at the entry already holding
    // it. Upright text draws a few hundred glyphs thousands of times.
    let mut shared = cx.take_shared_glyphs();
    let mut sink =
        GlyphSink::new(&mut glyphs.words, &mut glyphs.sidecar, advances).with_shared(&mut shared);
    // One source for the walk, so the runs read the text in order.
    let source = ShapingSource::new(input.content, analysis, variant);
    let paragraphs = &analysis.paragraphs;
    let mut paragraph = paragraphs.cursor(ParagraphId::new(0));
    // Combined units are looked for only where analysis made some.
    let mut units = analysis
        .flags
        .contains(ParagraphFlags::HAS_COMBINED)
        .then(|| CombinedUnits::new(input, &source));
    let mut walk = Walk::new(input, variant, end);
    // The font of the run before, which the text before a run is in.
    let mut before = None;
    loop {
        // Open the run where the walk is, find its end, record it, and shape
        // it in its paragraph, beside the fonts of the runs either side.
        let run = walk.open();
        let annotation = walk.annotation;
        paragraphs.step_to(&mut paragraph, run.start);
        let run_end = walk.run_end(paragraph.end(), paragraphs.flags(paragraph.id()));
        let after = (run_end < end).then(|| walk.font());
        let neighbours = NeighbourFonts::new(input.fonts, before, after);
        before = Some(run.font);
        if let Some(units) = &mut units {
            units.before(cx, runs, &mut sink, combined, &run, paragraph.id());
        }
        // No more runs than clusters, which a run id can always name.
        let pushed = runs.push(run, run_end);
        debug_assert!(pushed.is_some(), "no more runs than clusters");
        shape_run(
            input,
            &source,
            cx,
            &mut sink,
            &run,
            run.start..run_end,
            paragraph_text(analysis, paragraph.id()),
            annotation,
            neighbours,
            None,
        );
        if run_end >= end {
            break;
        }
    }
    if let Some(units) = &mut units {
        units.finish(cx, runs, &mut sink, combined);
    }
    let (any_unsafe, dropped) = (sink.any_unsafe(), sink.dropped());
    cx.restore_shared_glyphs(shared);
    let mut flags = ShapedFlags::NONE;
    if any_unsafe {
        flags.insert(ShapedFlags::HAS_UNSAFE);
    }
    if input.punctuation_trim.halves_advances() {
        flags.insert(ShapedFlags::HALVES_PUNCTUATION);
    }
    (dropped, flags)
}

/// The walk over the clusters that finds where shaping runs end.
///
/// It holds a forward cursor for each run the current cluster lies in: the
/// script run, the font run and the item. Each cursor steps where its run
/// ends, so no run facts are derived from a cluster id. Shaping makes the
/// runs the readers' walk (`Segments`) holds, so it keeps a walk of its own.
struct Walk<'a> {
    input: &'a ShapeInput<'a>,
    /// Whose styles and fonts the text is shaped in.
    variant: FirstLineVariant,
    /// The font runs of that variant.
    font_runs: &'a FontRuns,
    /// The cluster the walk is at.
    at: ClusterId,
    /// Where it ends, as the variant's runs do.
    end: ClusterId,
    script: RunCursor<ScriptRunId, ClusterId>,
    font: RunCursor<FontRunId, ClusterId>,
    item: RunCursor<ItemId, ClusterId>,
    /// The first of the three cursors' ends: where the walk steps one next.
    next: ClusterId,
    /// The shaping facts of the item holding the cluster the walk is at,
    /// which a run starting there links.
    shaping: ShapingFactsId,
    /// Those of the last text item with text, whose text a shaping run may
    /// continue.
    text: Option<ShapingFactsId>,
    /// That item is a ruby annotation's text. A shaping run never holds
    /// both annotation and base text, since an annotation's edges end runs.
    annotation: bool,
}

impl<'a> Walk<'a> {
    /// Starts a walk over the clusters before `end` in `variant`, at the
    /// first cluster, having read the item holding it.
    fn new(input: &'a ShapeInput<'a>, variant: FirstLineVariant, end: ClusterId) -> Self {
        let analysis = input.analysis;
        let font_runs = input.fonts.runs(variant);
        let mut walk = Self {
            input,
            variant,
            font_runs,
            at: ClusterId::new(0),
            end,
            script: analysis.runs.cursor(ScriptRunId::new(0), end),
            font: font_runs.cursor(FontRunId::new(0), end),
            item: analysis.item_clusters.cursor(ItemId::new(0)),
            next: ClusterId::new(0),
            shaping: ShapingFactsId::new(0),
            text: None,
            annotation: false,
        };
        walk.items_to(ClusterId::new(0));
        walk.next = walk.next_end();
        walk
    }

    /// Returns the run starting where the walk is: its script run, its font
    /// run's used font, and the shaping facts of the item holding its start.
    fn open(&self) -> ShapedRun {
        ShapedRun {
            start: self.at,
            script_run: self.script.id(),
            font: self.font(),
            shaping: self.shaping,
        }
    }

    /// Returns the used font of the font run holding the cluster the walk is
    /// at.
    fn font(&self) -> UsedFontId {
        self.font_runs
            .get(self.font.id())
            .map_or(UsedFontId::new(0), |run| run.font)
    }

    /// Finds the run's end by the carried script, font and item boundaries
    /// where analysis found no internal shaping stops. The paragraph's final
    /// undrawn separator still gets its own run. Exceptional paragraphs use
    /// the cluster scan, including explicit stops and shaped-class changes.
    // Keep both discovery loops out of the shaping loop: inlining them slows
    // preparation in the article benchmarks.
    #[inline(never)]
    fn run_end(&mut self, paragraph_end: ClusterId, flags: ParagraphFlags) -> ClusterId {
        if work::fast_paths() && !flags.contains(ParagraphFlags::HAS_SHAPING_STOPS) {
            let end = paragraph_end.min(self.end);
            let last = ClusterId::new(end.get().saturating_sub(1));
            let plain_end = if self
                .input
                .analysis
                .clusters
                .attrs(last)
                .is_some_and(|attrs| attrs.class() == ClusterClass::Separator)
            {
                last
            } else {
                end
            };
            while self.at < plain_end {
                let next = self.next.min(plain_end);
                if next >= self.end {
                    self.at = self.end;
                    return self.end;
                }
                let ends = next >= self.next && self.step_to(next);
                self.at = next;
                if ends || next >= plain_end {
                    return next;
                }
            }
        }
        self.scan_run_end()
    }

    /// The general path: a comparison per cluster for shaping stops and
    /// shaped-class transitions, stepping cursors where their runs end.
    fn scan_run_end(&mut self) -> ClusterId {
        let clusters = &self.input.analysis.clusters;
        let shaped = |attrs: Option<ClusterAttrs>| attrs.is_some_and(|a| a.class().is_shaped());
        let mut before = clusters.attrs(self.at);
        let mut cluster = self.at;
        loop {
            cluster = ClusterId::new(cluster.get() + 1);
            if cluster >= self.end {
                self.at = self.end;
                return self.end;
            }
            let here = clusters.attrs(cluster);
            let mut ends = before.is_some_and(|attrs| attrs.has(ClusterAttrs::SHAPE_BREAK_AFTER))
                || shaped(before) != shaped(here);
            if cluster >= self.next {
                ends |= self.step_to(cluster);
            }
            if ends {
                self.at = cluster;
                return cluster;
            }
            before = here;
        }
    }

    /// Steps each cursor whose run ends at `cluster` to the run holding it,
    /// and returns whether a shaping run ends there.
    ///
    /// A shaping run always ends where the script run or the font run does.
    /// It also ends where the item is text that does not shape like the
    /// text item before it.
    fn step_to(&mut self, cluster: ClusterId) -> bool {
        let mut ends = false;
        if cluster >= self.script.end() {
            let script_runs = &self.input.analysis.runs;
            script_runs.step_to(&mut self.script, cluster, self.end);
            ends = true;
        }
        if cluster >= self.font.end() {
            self.font_runs.step_to(&mut self.font, cluster, self.end);
            ends = true;
        }
        if cluster >= self.item.end() && self.items_to(cluster) {
            ends = true;
        }
        self.next = self.next_end();
        ends
    }

    /// Where the first of the runs the walk holds ends.
    fn next_end(&self) -> ClusterId {
        self.script.end().min(self.font.end()).min(self.item.end())
    }

    /// Steps the item cursor to the item holding `cluster`, which starts
    /// there, and reads it.
    ///
    /// It skips the items ending at or before `cluster`, empty ones
    /// included. Returns whether the item is text that does not shape like
    /// the text item before it.
    fn items_to(&mut self, cluster: ClusterId) -> bool {
        let content = self.input.content;
        let item_clusters = &self.input.analysis.item_clusters;
        while self.item.end() <= cluster && content.items.get(self.item.id()).is_some() {
            item_clusters.step(&mut self.item);
        }
        let Some(item) = content.items.get(self.item.id()) else {
            return false;
        };
        let shaping = content
            .facts
            .text(content.nodes.text_facts(item.node, self.variant))
            .shaping;
        self.shaping = shaping;
        if item.kind != ItemKind::Text {
            return false;
        }
        let differs = self.text.is_some_and(|last| last != shaping);
        self.text = Some(shaping);
        self.annotation = item.flags.contains(ItemFlags::ANNOTATION);
        differs
    }
}

/// Shapes `run`, whose clusters are `range`, in its paragraph's text
/// `paragraph`, into `sink`.
///
/// Where the run is not shaped, or its font has nothing to draw with, it
/// writes its clusters with no glyphs. `annotation` marks a ruby
/// annotation's text. `neighbours` names the fonts of the runs either side.
/// `narrow` names the form combined text is shaped in, if any.
#[allow(clippy::too_many_arguments)]
fn shape_run(
    input: &ShapeInput<'_>,
    source: &ShapingSource<'_>,
    cx: &mut ShapeSession<'_, '_>,
    sink: &mut GlyphSink<'_, ClusterId, InlineLayoutUnit>,
    run: &ShapedRun,
    range: Range<ClusterId>,
    paragraph: Range<TextOffset>,
    annotation: bool,
    neighbours: NeighbourFonts<'_>,
    narrow: Option<CombineWidth>,
) {
    let ShapeInput {
        content,
        analysis,
        fonts,
        punctuation_trim,
    } = *input;
    let clusters = &analysis.clusters;
    let len = range.end.get() - range.start.get();
    let shaped = clusters
        .class(range.start)
        .is_some_and(ClusterClass::is_shaped);
    if !shaped {
        sink.empty(len);
        return;
    }
    let halving = punctuation_trim.halves_advances();
    let mut key = ShapingKey::new(run, content, analysis, fonts, halving);
    if let Some(width) = narrow {
        key = key.with_combine_width(width);
    }
    // A ruby annotation's text keeps its advances with its glyphs, since
    // the prefix sums do not hold them. So does combined text, whose glyphs
    // are set across its em by them.
    sink.keep_advances(annotation || key.is_combined());
    // Combined text's glyphs are moved into its em once written, each its
    // own.
    sink.share(!key.is_combined());
    // A run that starts its paragraph starts a line. Under `trim-start` an
    // opening mark there gives its blank back, as Blink's `ShapeText` asks
    // for at every paragraph's first text item.
    let starts = clusters.start(range.start) == paragraph.start;
    let opens = starts
        && key.trim().trims_paragraph_start()
        && source
            .text()
            .slice(paragraph.clone())
            .chars()
            .next()
            .is_some_and(maybe_opening_mark);
    let edges = ShapingEdges {
        line_start: starts,
        trim_start: opens,
        trim_end: false,
    };
    shape_range(cx, source, paragraph, range, &key, edges, neighbours, sink);
}

/// A combined unit being shaped.
///
/// It records the unit's script run, its first shaping run, that run's
/// shaping facts and paragraph, and where the tables stood before it. That
/// lets the unit be shaped again in a narrower form.
#[derive(Copy, Clone, Debug)]
struct OpenUnit {
    script_run: ScriptRunId,
    first: ShapedRunId,
    shaping: ShapingFactsId,
    paragraph: ParagraphId,
    mark: SinkMark,
}

/// The combined units of a text, fitted to their ems as the walk reaches
/// each one's end.
///
/// This is shaping's part of `text-combine-upright`. Only text where
/// analysis found combined units pays for it.
struct CombinedUnits<'a> {
    input: &'a ShapeInput<'a>,
    /// The walk's text, which a unit is shaped again from.
    source: &'a ShapingSource<'a>,
    /// The unit being shaped, until its last shaping run is.
    open: Option<OpenUnit>,
}

impl<'a> CombinedUnits<'a> {
    fn new(input: &'a ShapeInput<'a>, source: &'a ShapingSource<'a>) -> Self {
        Self {
            input,
            source,
            open: None,
        }
    }

    /// Updates the open unit before `run`, in `paragraph`, is recorded.
    ///
    /// It fits the open unit where the run is past it, and opens a unit
    /// where the run starts one.
    fn before(
        &mut self,
        cx: &mut ShapeSession<'_, '_>,
        runs: &ShapedRuns,
        sink: &mut GlyphSink<'_, ClusterId, InlineLayoutUnit>,
        fits: &mut SortedTable<(ScriptRunId, CombineFit)>,
        run: &ShapedRun,
        paragraph: ParagraphId,
    ) {
        if self
            .open
            .is_some_and(|open| open.script_run != run.script_run)
        {
            self.finish(cx, runs, sink, fits);
        }
        let combined = self
            .input
            .analysis
            .runs
            .get(run.script_run)
            .is_some_and(|run| run.orientation == RunOrientation::Combined);
        if combined && self.open.is_none() {
            self.open = Some(OpenUnit {
                script_run: run.script_run,
                first: ShapedRunId::new(runs.len()),
                shaping: run.shaping,
                paragraph,
                mark: sink.mark(),
            });
        }
    }

    /// Fits the open unit to its em and records its fit in `fits`.
    ///
    /// `runs` holds every shaping run of the unit, and `sink` has shaped
    /// them. This follows Blink's
    /// `InlineNode::AdjustFontForTextCombineUprightAll`:
    /// - The desired width is the em plus a tenth
    ///   (`LayoutTextCombine::DesiredWidth`, the margin EPUB asks for).
    ///   Under a decoration Blink uses the em alone, but decorations are the
    ///   host's to draw and unknown here.
    /// - Text wider than that, set horizontally, is shaped again in each
    ///   narrower form (half, third, quarter widths) until one fits. If none
    ///   does, it keeps its own widths and is narrowed by the ratio.
    /// - The em is the first run's computed font size on the grid
    ///   (`ComputedFontSizeAsFixed`).
    /// - The baseline sits the primary font's ascent down, less half its
    ///   internal leading (`LayoutTextCombine::AdjustTextTopForPaint`).
    ///
    /// Across the line, the unit is centred between the primary font's
    /// text-over and text-under baselines, as CSS Writing Modes 3, section
    /// 9.1.2 centres its square. Blink sets its `LayoutTextCombine` at the
    /// text's top, as wide as the font is tall (`AdjustStyleForTextCombine`),
    /// and centres the text in it. So on a centred line of odd height, with
    /// the odd pixel over, the unit sits half a pixel over its baseline.
    fn finish(
        &mut self,
        cx: &mut ShapeSession<'_, '_>,
        runs: &ShapedRuns,
        sink: &mut GlyphSink<'_, ClusterId, InlineLayoutUnit>,
        fits: &mut SortedTable<(ScriptRunId, CombineFit)>,
    ) {
        let Some(open) = self.open.take() else {
            return;
        };
        let input = self.input;
        let facts = &input.content.facts;
        let request = facts.shaping(open.shaping).font;
        let size = facts.request(request).font.computed_size();
        let desired = size * 1.1;
        let natural = InlineLayoutUnit::from_raw(sink.width_since(open.mark)).to_px();
        let mut fit = CombineFit::NATURAL;
        if natural > desired {
            let narrower = CombineWidth::NARROWER.into_iter().any(|width| {
                sink.rewind(open.mark);
                self.shape_unit(cx, runs, sink, open, Some(width));
                InlineLayoutUnit::from_raw(sink.width_since(open.mark)).to_px() <= desired
            });
            if !narrower {
                sink.rewind(open.mark);
                self.shape_unit(cx, runs, sink, open, None);
                fit.scale = TextUnit::from_px(desired / natural);
            }
        }
        let fonts = input.fonts;
        let primary = fonts
            .resolution(request)
            .and_then(|resolution| fonts.used.get(resolution.primary));
        let baseline = primary.map_or_else(
            || LayoutUnit::from_px_truncated(0.8 * size),
            |used| {
                let metrics = &used.metrics;
                let ascent = metrics.alphabetic_ascent().to_px();
                let height = ascent + metrics.alphabetic_descent().to_px();
                let leading = height - used.size.to_px();
                LayoutUnit::from_px_truncated(ascent - leading / 2.0)
            },
        );
        fit.middle = primary.map_or(LayoutUnit::ZERO, |used| used.metrics.text_middle());
        let em = LayoutUnit::from_px(size);
        sink.set_combined(open.mark, em, baseline, fit);
        fits.push((open.script_run, fit));
    }

    /// Shapes every shaping run of the unit `open` again, in `narrow`'s
    /// form.
    fn shape_unit(
        &self,
        cx: &mut ShapeSession<'_, '_>,
        runs: &ShapedRuns,
        sink: &mut GlyphSink<'_, ClusterId, InlineLayoutUnit>,
        open: OpenUnit,
        narrow: Option<CombineWidth>,
    ) {
        let paragraph = paragraph_text(self.input.analysis, open.paragraph);
        for id in (open.first..ShapedRunId::new(runs.len())).ids() {
            work::step();
            let Some(run) = runs.get(id) else {
                break;
            };
            let range = runs.run_clusters(id);
            shape_run(
                self.input,
                self.source,
                cx,
                sink,
                run,
                range,
                paragraph.clone(),
                false,
                NeighbourFonts::default(),
                narrow,
            );
        }
    }
}

/// Returns the text of `paragraph`, from its first cluster's start to its
/// last cluster's end, its separator included.
pub(super) fn paragraph_text(analysis: &Analysis, paragraph: ParagraphId) -> Range<TextOffset> {
    let clusters = &analysis.clusters;
    let range = analysis.paragraphs.clusters(paragraph);
    clusters.start(range.start)..clusters.start(range.end)
}
