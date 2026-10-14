//! Text analysis: clusters, break opportunities, paragraphs and runs.
//!
//! In: [`AnalysisInput`], the content. Out: [`Analysis`].
//! Start at: [`analyze`], then `ClusterWriter::visit_item` in `walk`.
//!
//! - `walk` walks the items and the text once and writes every table.
//! - `classify` classifies characters for the walk.
//! - `breaks` holds the line segmenters' keys and the line stream.
//! - `graphemes` finds the grapheme boundaries clusters start at.
//! - `ascii` and `pairs` answer most Latin-1 breaks without ICU.
//! - `scripts` resolves each cluster's script and splits the runs.
//! - `bidi` builds each paragraph's bidi input and resolves its levels.
//! - `context` caches the segmenters in the [`Context`](crate::Context).
//! - `clusters`, `items`, `paragraphs` and `runs` own the stored tables.
//!
//! Every per-character table is indexed by [`ClusterId`]. An analyzed
//! content has at least one paragraph. Later stages only read [`Analysis`].

mod ascii;
mod bidi;
mod breaks;
mod classify;
mod clusters;
mod context;
mod graphemes;
mod items;
mod pairs;
mod paragraphs;
mod runs;
mod scripts;
#[cfg(test)]
mod tests;
mod walk;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::ops::Range;

use crate::config::SmallKana;
use crate::data::{define_id, heap_bytes};
use crate::stages::content::{Content, ContentFlags, ItemId, NodeId};

pub(super) use bidi::ControlLevel;
pub(crate) use clusters::{ClusterAttrs, ClusterClass, ClusterEnd, Clusters};
pub(crate) use context::AnalysisContext;
pub(super) use items::ItemClusters;

pub(crate) use paragraphs::{Paragraph, ParagraphFlags, Paragraphs};
pub use runs::{BidiLevel, RunOrientation};
pub(crate) use runs::{ScriptRun, ScriptRuns};
use walk::ClusterWriter;

/// The input to analysis: the content and one config choice.
pub(crate) struct AnalysisInput<'a> {
    pub(crate) content: &'a Content,
    /// Whether a small kana may start a line under `line-break: normal`
    /// (`Config::small_kana`).
    pub(crate) small_kana: SmallKana,
}

/// The working memory of analysis, kept on the context for its capacity.
///
/// Only `hyphen_nodes` is read after a call; the rest holds nothing between
/// calls.
pub(crate) struct AnalysisScratch {
    /// The runs found so far, in text order, held until their scripts and
    /// levels are known. A run of Common characters takes the script that
    /// follows it, and levels are known when the paragraph ends.
    pending_runs: Vec<scripts::PendingRun>,
    /// A paragraph's bidi input and the resolver's working memory.
    bidi: bidi::BidiInput,
    /// A ruby column's base and the text after the column, which decide a
    /// break after the column.
    column_text: String,
    /// See [`AnalysisScratch::hyphen_nodes`].
    hyphen_nodes: Vec<(NodeId, ClusterId)>,
}

impl AnalysisScratch {
    /// Returns empty scratch, allocating nothing.
    pub(crate) fn new() -> Self {
        Self {
            pending_runs: Vec::new(),
            bidi: bidi::BidiInput::new(),
            column_text: String::new(),
            hyphen_nodes: Vec::new(),
        }
    }

    /// Returns the nodes whose text holds a breakable soft hyphen, each with
    /// its first such cluster, in text order, from the last analysis.
    ///
    /// A node repeats only where another node's text comes between. Font
    /// selection reads this to choose the hyphen's font.
    pub(crate) fn hyphen_nodes(&self) -> &[(NodeId, ClusterId)] {
        &self.hyphen_nodes
    }

    /// Records `cluster` as a breakable soft hyphen in `node`, unless `node`
    /// is the last node recorded.
    fn push_hyphen_node(&mut self, node: NodeId, cluster: ClusterId) {
        let nodes = &mut self.hyphen_nodes;
        if nodes.last().is_none_or(|&(last, _)| last != node) {
            nodes.push((node, cluster));
        }
    }
}

heap_bytes! {
    AnalysisScratch { pending_runs, bidi, column_text, hyphen_nodes }
}

impl Default for AnalysisScratch {
    fn default() -> Self {
        Self::new()
    }
}

/// The open paragraph, which `walk` fills and `bidi` resolves when it ends.
#[derive(Default)]
struct ParagraphState {
    /// Whether a paragraph has begun and not ended.
    open: bool,
    start: ClusterId,
    /// The item holding the first cluster, where the bidi input starts.
    first_item: ItemId,
    flags: ParagraphFlags,
    /// Whether a style synthesizes a bidi control here: a box with
    /// `unicode-bidi` or an annotation opens or closes inside the paragraph
    /// or is open across its start, or the block overrides bidi.
    controls: bool,
    /// The bidi classes of its characters, one bit per class.
    classes: u32,
    east_asian: bool,
}

impl ParagraphState {
    /// Returns whether a cluster may leave the paragraph's level: the
    /// paragraph has a synthesized control or a right-to-left class.
    fn may_differ(&self) -> bool {
        self.controls || self.classes & classify::LEVEL_CLASSES != 0
    }
}

/// Analyzes the content of `input` into `out` in one pass over the items
/// and the text.
///
/// Clears and fills `out`; nothing else writes it.
pub(crate) fn analyze(
    input: &AnalysisInput<'_>,
    cx: &AnalysisContext,
    scratch: &mut AnalysisScratch,
    out: &mut Analysis,
) {
    let content = input.content;
    out.clear();
    scratch.pending_runs.clear();
    scratch.hyphen_nodes.clear();
    scratch.bidi.begin();
    // A text has at most one cluster per character, and Latin and CJK text
    // has exactly one.
    out.clusters.reserve(content.text.as_str().chars().count());
    out.item_clusters.reserve(content.items.len());
    let mut writer = ClusterWriter::new(input, cx, scratch, out);
    for (id, item) in content.items.iter() {
        writer.visit_item(id, item);
    }
    writer.finish();
}

define_id! {
    /// Indexes the clusters, the finest unit of analysis.
    ///
    /// A position between clusters is the id of the cluster after it. The end
    /// of the text is the cluster count.
    pub(crate) struct ClusterId(u32);
}

define_id! {
    /// Indexes the paragraphs, in text order.
    pub(crate) struct ParagraphId(u32);
}

define_id! {
    /// Indexes the script runs, in text order.
    pub(crate) struct ScriptRunId(u32);
}

/// The analysis of a content: its clusters, items, paragraphs and runs.
pub(crate) struct Analysis {
    pub(crate) clusters: Clusters,
    /// The clusters of each item.
    pub(crate) item_clusters: ItemClusters,
    /// The paragraphs, in text order. There is at least one, except in a
    /// cleared analysis.
    pub(crate) paragraphs: Paragraphs,
    /// The script runs, in text order.
    pub(crate) runs: ScriptRuns,
    /// The rarely filled tables, boxed on first use so that an analysis
    /// without them costs one pointer.
    extras: Option<Box<AnalysisExtras>>,
    /// The union of every paragraph's flags, so a stage can skip work for
    /// the whole content.
    pub(crate) flags: ParagraphFlags,
}

impl Analysis {
    /// Returns an empty analysis, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            clusters: Clusters::new(),
            item_clusters: ItemClusters::new(),
            paragraphs: Paragraphs::new(),
            runs: ScriptRuns::new(),
            extras: None,
            flags: ParagraphFlags::NONE,
        }
    }

    /// Empties the analysis, keeping every allocation.
    ///
    /// A cleared analysis has no paragraph. A layout holds one when its
    /// builder was dropped without finishing.
    pub(crate) fn clear(&mut self) {
        self.clusters.clear();
        self.item_clusters.clear();
        self.paragraphs.clear();
        self.runs.clear();
        if let Some(extras) = &mut self.extras {
            extras.control_levels.clear();
        }
        self.flags = ParagraphFlags::NONE;
    }

    /// Returns the levels of the bidi controls that line layout reorders
    /// with, in item order. Empty when no paragraph has any.
    pub(super) fn control_levels(&self) -> &[ControlLevel] {
        self.extras
            .as_deref()
            .map_or(&[], |extras| extras.control_levels.as_slice())
    }

    /// Replaces the control levels with `levels`, boxing the extras on first
    /// use.
    fn set_control_levels(&mut self, levels: &[ControlLevel]) {
        if levels.is_empty() && self.extras.is_none() {
            return;
        }
        let kept = &mut self.extras.get_or_insert_default().control_levels;
        kept.clear();
        kept.extend_from_slice(levels);
    }

    /// Returns the clusters the block's first line can reach, if `content`
    /// restyles anything under `::first-line`.
    ///
    /// `content` is the analyzed content. The range is the first paragraph,
    /// since no line crosses a forced break. Returns `None` if nothing is
    /// restyled or there is no text.
    ///
    /// Font selection, shaping and measurement run their first-line variants
    /// over this range. Blink reshapes the whole text instead
    /// (`ShapeTextForFirstLineIfNeeded`), which gives the first line nothing
    /// more.
    pub(crate) fn first_line_reach(&self, content: &Content) -> Option<Range<ClusterId>> {
        if !content.flags.contains(ContentFlags::FIRST_LINE_RESTYLE) {
            return None;
        }
        let first = self.paragraphs.first_clusters()?;
        (!first.is_empty()).then_some(first)
    }
}

heap_bytes! {
    Analysis { clusters, item_clusters, paragraphs, runs, extras; flags }
}

/// The tables of an analysis that few texts fill, boxed together.
#[derive(Default)]
struct AnalysisExtras {
    /// The levels of the bidi controls that line layout reorders with, in
    /// item order. These are a box's controls whose level is below the
    /// clusters on both sides, and every absolutely positioned box's anchor.
    /// Only paragraphs with right-to-left text and mixed levels keep any.
    control_levels: Vec<ControlLevel>,
}

heap_bytes! {
    AnalysisExtras { control_levels }
}

impl Default for Analysis {
    fn default() -> Self {
        Self::new()
    }
}
