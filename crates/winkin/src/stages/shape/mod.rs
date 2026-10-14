//! The shaping stage: each cluster's glyphs and the runs they are shaped in.
//!
//! In: [`ShapeInput`], the content, the analysis and the fonts. Out:
//! [`Shaped`], with each cluster's advance for measurement.
//! Start at: [`shape_runs`], then `walk::shape_text`.
//!
//! - `walk` finds the shaping runs and shapes each one.
//! - `shaper` is the one shaping call, used here and for line-edge reshaping.
//! - `sink` encodes glyphs into cluster words and sidecar entries.
//! - `context` holds the caches that outlive a layout.
//! - `provider` bridges metrics the host supplies.
//! - `trim` applies `text-spacing-trim`.
//! - `vertical` holds the vertical metrics for upright text.
//!
//! [`Shaped`] is frozen after preparation. Measurement sums its advances into
//! prefix sums, and the build then drops them where nothing measures again.

mod context;
mod provider;
mod shaper;
mod sink;
#[cfg(test)]
mod tests;
mod trim;
mod vertical;
mod walk;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ops::Range;

use crate::config::PunctuationTrim;
use crate::data::{
    HeapBytes, Id, RankedBitTable, Run, RunCursor, Runs, SortedTable, Table, define_flags,
    define_id, heap_bytes,
};
use crate::stages::analysis::{Analysis, ClusterId, ScriptRunId};
use crate::stages::content::{Content, ShapingFactsId};
use crate::stages::fonts::{Fonts, UsedFontId};
use crate::style::FirstLine;
use crate::style::FirstLineState;
use crate::style::FirstLineVariant;
use crate::unit::{LayoutUnit, TextUnit};
use crate::work;

pub(crate) use context::{ShapeContext, ShapeSession};
pub(crate) use shaper::{NeighbourFonts, ShapingKey, ShapingSource, shape_generated, shape_range};
pub(super) use sink::GlyphSink;
pub(crate) use trim::{ShapingEdges, maybe_closing_mark, maybe_opening_mark};
use walk::shape_text;

/// What shaping reads besides the context.
///
/// The used fonts hold everything their runs are shaped with.
pub(crate) struct ShapeInput<'a> {
    pub(crate) content: &'a Content,
    pub(crate) analysis: &'a Analysis,
    pub(crate) fonts: &'a Fonts,
    /// How `text-spacing-trim` trims punctuation, from the context's config.
    pub(crate) punctuation_trim: PunctuationTrim,
}

/// Shapes the text of `input` into `out`, with each cluster's advance.
///
/// Where font selection made `::first-line` runs, it also shapes the first
/// paragraph in those styles and fonts into `out`'s first-line variant.
///
/// Clears `out` first. Sets `out`'s flags once from both shapings: an unsafe
/// break in either lets the breaker reshape. Measurement sums the advances
/// next.
///
/// Returns how many glyphs were left out because a sidecar was full.
pub(crate) fn shape_runs<'f>(
    input: &ShapeInput<'f>,
    cx: &mut ShapeSession<'_, 'f>,
    out: &mut Shaped,
) -> usize {
    out.clear();
    let clusters = &input.analysis.clusters;
    if clusters.is_empty() {
        return 0;
    }
    let (mut dropped, mut flags) = shape_text(
        input,
        FirstLineVariant::Standard,
        clusters.end_id(),
        cx,
        out.text.text_mut(),
    );
    // The first-line variant, only where its fonts differ. Otherwise it stays
    // empty and the first line reads the text's shaping.
    if input.fonts.has_first_line_runs()
        && let Some(reach) = input.analysis.first_line_reach(input.content)
    {
        let (first_dropped, first_flags) = shape_text(
            input,
            FirstLineVariant::FirstLine,
            reach.end,
            cx,
            out.text.first_line_mut(),
        );
        dropped = dropped.saturating_add(first_dropped);
        flags.insert(first_flags);
    }
    if input.punctuation_trim.halves_advances() {
        flags.insert(ShapedFlags::HALVES_PUNCTUATION);
    }
    out.flags = flags;
    dropped
}

define_id! {
    /// Names a shaping run in a layout's table of [`ShapedRun`]s.
    pub(crate) struct ShapedRunId(u32);
}

define_id! {
    /// Names a glyph in a table of [`SidecarGlyph`]s: the shaped text's, or
    /// the one for lines' reshaped edges.
    pub(crate) struct SidecarGlyphId(u32);
}

/// One cluster's entry in a glyph table, 4 bytes.
///
/// | Bits | Field |
/// |---|---|
/// | 31 | [`EXPANDED`](Self::EXPANDED): the payload is where the cluster's glyphs start in the sidecar |
/// | 30 | [`UNSAFE_TO_BREAK`](Self::UNSAFE_TO_BREAK) |
/// | 29 | unused |
/// | 28 | [`CONTINUATION`](Self::CONTINUATION) |
/// | 0-27 | the glyph id, the sidecar start, or all ones for no glyphs |
///
/// The word holds no advance. Advances are in [`ShapedText::advances`].
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct GlyphWord(u32);

impl GlyphWord {
    const EXPANDED: u32 = 1 << 31;
    /// Breaking before the cluster changes how the text either side shapes.
    ///
    /// A line that starts or ends here is reshaped there, and a reshape
    /// window starts and ends only where the bit is clear. A run's first
    /// cluster is shaped as a start, so it carries the bit only where a mark
    /// is kerned against the run before it.
    const UNSAFE_TO_BREAK: u32 = 1 << 30;
    /// The cluster's glyphs belong to an earlier cluster, as inside a
    /// ligature or any other merge across clusters.
    const CONTINUATION: u32 = 1 << 28;
    const PAYLOAD: u32 = (1 << 28) - 1;
    /// The payload of a cluster with no glyphs of its own.
    const NONE: u32 = Self::PAYLOAD;
    /// The largest glyph id, or sidecar start, a word holds.
    const MAX_PAYLOAD: u32 = Self::PAYLOAD - 1;

    /// A cluster with no glyphs: one not shaped, or in a run no font draws.
    pub(super) const EMPTY: Self = Self(Self::NONE);

    /// Returns a compact word for one glyph drawn with no offset, or `None`
    /// for an id above [`MAX_PAYLOAD`](Self::MAX_PAYLOAD).
    fn from_glyph(glyph: u32) -> Option<Self> {
        (glyph <= Self::MAX_PAYLOAD).then_some(Self(glyph))
    }

    /// Returns an expanded word for glyphs from `start` in the sidecar, or
    /// `None` for a start above [`MAX_PAYLOAD`](Self::MAX_PAYLOAD).
    fn from_sidecar(start: SidecarGlyphId) -> Option<Self> {
        let start = u32::try_from(start.get()).ok()?;
        (start <= Self::MAX_PAYLOAD).then_some(Self(Self::EXPANDED | start))
    }

    /// Returns the word, marked unsafe to break before where `to_break`.
    fn with_unsafe(self, to_break: bool) -> Self {
        if to_break {
            Self(self.0 | Self::UNSAFE_TO_BREAK)
        } else {
            self
        }
    }

    fn continuing(self) -> Self {
        Self(self.0 | Self::CONTINUATION)
    }

    /// Returns the word with its flags, pointing at glyphs from `start` in
    /// the sidecar, or `None` for a start above
    /// [`MAX_PAYLOAD`](Self::MAX_PAYLOAD).
    ///
    /// A copied expanded cluster points where its glyphs were copied to.
    fn with_sidecar_start(self, start: SidecarGlyphId) -> Option<Self> {
        let start = Self::from_sidecar(start)?;
        Some(Self((self.0 & !Self::PAYLOAD) | (start.0 & Self::PAYLOAD)))
    }

    /// Returns whether breaking before the cluster changes the shaping.
    pub(super) fn is_unsafe_to_break(self) -> bool {
        self.0 & Self::UNSAFE_TO_BREAK != 0
    }

    /// Returns whether the cluster's glyphs belong to an earlier cluster.
    pub(crate) fn is_continuation(self) -> bool {
        self.0 & Self::CONTINUATION != 0
    }

    fn is_expanded(self) -> bool {
        self.0 & Self::EXPANDED != 0
    }

    /// Returns the one glyph of a compact cluster, drawn at its pen with no
    /// offset, or `None` for any other.
    ///
    /// A paint walk tries this first, since nearly every cluster is compact.
    #[inline]
    pub(super) fn single(self) -> Option<u32> {
        let payload = self.0 & Self::PAYLOAD;
        (payload != Self::NONE && !self.is_expanded()).then_some(payload)
    }

    /// Returns the cluster's glyphs, decoded against `sidecar`, the table
    /// the word was written with.
    ///
    /// Inlined, since a paint walk calls it for every cluster.
    #[inline]
    pub(super) fn glyphs(self, sidecar: &Table<SidecarGlyphId, SidecarGlyph>) -> ClusterGlyphs<'_> {
        let payload = self.0 & Self::PAYLOAD;
        if payload == Self::NONE {
            return ClusterGlyphs::None;
        }
        if !self.is_expanded() {
            return ClusterGlyphs::One(payload);
        }
        // An expanded word's payload is where its glyphs start.
        let Some(start) = usize::try_from(payload)
            .ok()
            .and_then(SidecarGlyphId::try_new)
        else {
            return ClusterGlyphs::None;
        };
        let rest = sidecar
            .get_slice(start..sidecar.next_id())
            .unwrap_or_default();
        // The cluster's last glyph carries the last bit. A sidecar cut short
        // ends the cluster too.
        let len = rest
            .iter()
            .position(SidecarGlyph::is_last)
            .map_or(rest.len(), |last| last + 1);
        ClusterGlyphs::Many(rest.get(..len).unwrap_or_default())
    }
}

/// A glyph of an expanded cluster, 16 bytes, every length in 16.16.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct SidecarGlyph {
    /// The glyph id, with the top bit set on the cluster's last glyph.
    id: u32,
    /// How far the glyph is drawn from its pen position, along the line and
    /// over it.
    ///
    /// For text shaped across the line these are harfrust's right and up.
    /// For text shaped down the line they are its down and right.
    pub(crate) x_offset: TextUnit,
    pub(crate) y_offset: TextUnit,
    /// How far the pen moves along the line after it.
    pub(crate) advance: TextUnit,
}

impl SidecarGlyph {
    const LAST: u32 = 1 << 31;

    /// Returns the glyph id.
    pub(crate) fn id(&self) -> u32 {
        self.id & !Self::LAST
    }

    fn is_last(&self) -> bool {
        self.id & Self::LAST != 0
    }
}

/// A cluster's glyphs, as its [`GlyphWord`] and the sidecar give them.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum ClusterGlyphs<'a> {
    /// None of its own: not shaped, a continuation, or nothing to draw with.
    None,
    /// One glyph with no offset, whose advance is the cluster's.
    One(u32),
    /// Its glyphs, in the order they are drawn: visual order within the
    /// cluster, which for a right-to-left run is the reverse of the text's.
    Many(&'a [SidecarGlyph]),
}

/// A shaping run, 12 bytes.
///
/// Runs tile the clusters, each ending where the next starts. Clusters that
/// are not shaped form a run of their own, whose words are
/// [`GlyphWord::EMPTY`].
///
/// A reshape reaches everything it needs from here ([`ShapingKey::new`]):
/// - the script, language, level and orientation from its script run;
/// - the font, its coordinates, its features and the size from the used
///   font;
/// - the computed size, the language system and `text-spacing-trim` from
///   its shaping facts.
///
/// The used font also says how the text is fed: uppercased for synthesized
/// small capitals, lowercased for unicase. So a run carries no flags of its
/// own.
///
/// The start is also a start bit in [`ShapedRuns`], which finds the run
/// holding a cluster. It is kept here too, so a forward walk finds each
/// run's end at the next one's start. Reading it from the bits instead makes
/// line layout's relayout of many-run text 3 to 5% slower.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ShapedRun {
    /// Its first cluster.
    start: ClusterId,
    /// The script run it lies in, whose script, language, level and
    /// orientation it is shaped in. A shaping run never crosses one.
    pub(crate) script_run: ScriptRunId,
    /// The used font it is shaped in.
    pub(crate) font: UsedFontId,
    /// The shaping facts of every text item in it: its spacing, its trim and
    /// its font request.
    ///
    /// A run ends where these change. The id fits in two bytes of padding;
    /// widening it to `u32` grows the run to 16 bytes.
    pub(super) shaping: ShapingFactsId,
}

impl Run for ShapedRun {
    type Position = ClusterId;

    #[inline]
    fn start(&self) -> ClusterId {
        self.start
    }
}

/// Which of a font's narrower forms combined text is shaped with, as Blink's
/// `FontWidthVariant` names them.
///
/// Combined text shaped in its own widths has no form, and its key names
/// none.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
enum CombineWidth {
    /// `hwid`, half widths.
    Half,
    /// `twid`, third widths.
    Third,
    /// `qwid`, quarter widths.
    Quarter,
}

impl CombineWidth {
    /// The narrower forms, in the order Blink tries them
    /// (`InlineNode::AdjustFontForTextCombineUprightAll`).
    const NARROWER: [Self; 3] = [Self::Half, Self::Third, Self::Quarter];

    /// The OpenType feature that sets the form.
    fn feature(self) -> &'static [u8; 4] {
        match self {
            Self::Half => b"hwid",
            Self::Third => b"twid",
            Self::Quarter => b"qwid",
        }
    }
}

/// The fit of a combined unit to its one em: how much it is narrowed beyond
/// its shaped form, and where across the line it is centred.
#[derive(Copy, Clone, Debug)]
pub(crate) struct CombineFit {
    /// The horizontal scale, 16.16; one where the unit fits as shaped.
    scale: TextUnit,
    /// How far over the line's baseline its text is centred: the middle of
    /// its primary font's ascent and descent (`FontLineMetrics::text_middle`).
    pub(crate) middle: LayoutUnit,
}

impl CombineFit {
    /// A unit that fits as it is, centred on the baseline.
    const NATURAL: Self = Self {
        scale: TextUnit::from_raw(1 << 16),
        middle: LayoutUnit::ZERO,
    };

    /// Returns the horizontal scale of its glyphs: one where they fit as
    /// shaped.
    pub(crate) fn scale(&self) -> f32 {
        self.scale.to_px()
    }
}

define_flags! {
    /// Facts about the shaped text that let a later stage skip work at once.
    pub(crate) struct ShapedFlags(u8) {
        /// Some cluster is unsafe to break before. Without it, no line edge
        /// is reshaped.
        pub(super) const HAS_UNSAFE = 1 << 0;
        /// Punctuation in a font with no `halt` is trimmed by halving its
        /// advance, as the build's config says (`PunctuationTrim::Always`).
        ///
        /// A line edge the breaker reshapes is trimmed the same way, even if
        /// the config changes after the build.
        pub(crate) const HALVES_PUNCTUATION = 1 << 1;
    }
}

/// The glyph table: one [`GlyphWord`] per cluster, and the sidecar its
/// expanded words point into.
///
/// Words are in cluster order, right-to-left runs included. A reader takes a
/// text item's words as a slice ([`words`](Self::words)) and decodes each
/// against the sidecar it was written with: this one, or the reshaped edges'.
pub(crate) struct GlyphStore {
    /// One word per cluster, in cluster order.
    words: Table<ClusterId, GlyphWord>,
    /// The glyphs of expanded clusters, each cluster's together, in cluster
    /// order ([`GlyphWord::glyphs`]).
    pub(super) sidecar: Table<SidecarGlyphId, SidecarGlyph>,
}

impl GlyphStore {
    const fn new() -> Self {
        Self {
            words: Table::new(),
            sidecar: Table::new(),
        }
    }

    fn clear(&mut self) {
        self.words.clear();
        self.sidecar.clear();
    }

    /// Returns `cluster`'s word, or [`GlyphWord::EMPTY`] past the last
    /// cluster.
    pub(crate) fn word(&self, cluster: ClusterId) -> GlyphWord {
        self.words.get(cluster).copied().unwrap_or(GlyphWord::EMPTY)
    }

    /// Returns `cluster`'s glyphs, or none past the last cluster.
    pub(crate) fn glyphs(&self, cluster: ClusterId) -> ClusterGlyphs<'_> {
        self.word(cluster).glyphs(&self.sidecar)
    }

    /// Returns the words of `clusters`, or none where the range is out of
    /// bounds.
    pub(super) fn words(&self, clusters: Range<ClusterId>) -> &[GlyphWord] {
        self.words.get_slice(clusters).unwrap_or_default()
    }

    /// Returns the first cluster from `from` before `limit` that is safe to
    /// break before, or `limit`.
    ///
    /// A reshape window after a line's start may end there.
    pub(super) fn next_safe_to_break(&self, from: ClusterId, limit: ClusterId) -> ClusterId {
        let mut at = from;
        while at < limit {
            if !self.word(at).is_unsafe_to_break() {
                return at;
            }
            at = ClusterId::new(at.get() + 1);
        }
        limit
    }

    /// Returns the last cluster before `below`, and no earlier than `floor`,
    /// that is safe to break before, or `floor`.
    ///
    /// A reshape window before a line's end may start there.
    pub(super) fn prev_safe_to_break(&self, below: ClusterId, floor: ClusterId) -> ClusterId {
        let mut at = below;
        while at > floor {
            at = ClusterId::new(at.get() - 1);
            if !self.word(at).is_unsafe_to_break() {
                return at;
            }
        }
        floor
    }

    /// Returns how many clusters have a word.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.words.len()
    }

    /// Returns whether nothing was shaped.
    fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Returns every cluster's word with its id, in cluster order.
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = (ClusterId, &GlyphWord)> {
        self.words.iter()
    }
}

/// The shaping runs in text order.
///
/// The runs tile the clusters the glyph table has words for. Each ends where
/// the next starts, and the last ends where the shaped clusters do. A seek
/// finds a cluster's run by counting start bits
/// ([`containing`](Self::containing)); a forward walk steps a cursor
/// ([`step`](Self::step)).
pub(crate) struct ShapedRuns {
    runs: Runs<ShapedRunId, ShapedRun>,
    /// The end of the shaped clusters, where the last run ends.
    end: ClusterId,
    /// A bit for each cluster a run starts at, with a count before each word.
    ///
    /// The run holding a cluster is the number of starts at or before it,
    /// less one, found without a search. Walks that start at an arbitrary
    /// cluster seek here once. It is fixed before any line is broken.
    starts: RankedBitTable<ClusterId>,
}

impl ShapedRuns {
    fn new() -> Self {
        Self {
            runs: Runs::new(),
            end: ClusterId::new(0),
            starts: RankedBitTable::new(),
        }
    }

    fn clear(&mut self) {
        self.runs.clear();
        self.end = ClusterId::new(0);
        self.starts.clear();
    }

    /// Reserves a start bit for each of `clusters` clusters and a record for
    /// each of `runs` runs.
    fn reserve(&mut self, clusters: usize, runs: usize) {
        self.starts.reserve(clusters);
        self.runs.reserve(runs);
    }

    /// Appends `run`, which ends at `end`, and returns its id.
    ///
    /// Returns `None` once run ids run out, which takes more runs than
    /// clusters. The shaped clusters end at `end` either way.
    fn push(&mut self, run: ShapedRun, end: ClusterId) -> Option<ShapedRunId> {
        self.end = end;
        let id = self.runs.push(run)?;
        self.starts.set(run.start);
        Some(id)
    }

    /// Returns run `id`, or `None` past the last run.
    pub(crate) fn get(&self, id: ShapedRunId) -> Option<&ShapedRun> {
        self.runs.get(id)
    }

    /// Returns how many runs there are.
    pub(super) fn len(&self) -> usize {
        self.runs.len()
    }

    /// Returns whether there are no runs: nothing was shaped.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Returns every run with its id and its clusters, in text order.
    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = (ShapedRunId, &ShapedRun, Range<ClusterId>)> {
        self.runs
            .iter()
            .map(|(id, run)| (id, run, self.run_clusters(id)))
    }

    /// Returns the run holding `cluster`, found by counting start bits, or
    /// `None` past the last shaped cluster.
    #[inline]
    pub(crate) fn containing(&self, cluster: ClusterId) -> Option<ShapedRunId> {
        if cluster >= self.end {
            return None;
        }
        work::seek();
        let held = self.starts.count_through(cluster).checked_sub(1)?;
        Some(ShapedRunId::new(held))
    }

    /// Returns the run holding `cluster`, or `None` past the last shaped
    /// cluster.
    ///
    /// A reshape reads a cluster's shaping key from it.
    pub(crate) fn run_containing(&self, cluster: ClusterId) -> Option<&ShapedRun> {
        self.runs.get(self.containing(cluster)?)
    }

    /// Returns run `id`'s clusters, up to the next run's start or the end of
    /// the shaped clusters. Past the last run, the range is empty at that
    /// end.
    #[inline]
    pub(super) fn run_clusters(&self, id: ShapedRunId) -> Range<ClusterId> {
        self.runs.span(id, self.end)
    }
}

/// Cursors for walks over the shaping runs ([`Segments`]).
///
/// [`Segments`]: crate::stages::Segments
impl ShapedRuns {
    /// Returns a cursor at run `id`.
    #[inline]
    pub(super) fn cursor(&self, id: ShapedRunId) -> RunCursor<ShapedRunId, ClusterId> {
        self.runs.cursor(id, self.end)
    }

    /// Returns a cursor at the run holding `cluster`, found without a search
    /// ([`containing`](Self::containing)), or `None` past the last shaped
    /// cluster.
    #[inline]
    pub(super) fn cursor_containing(
        &self,
        cluster: ClusterId,
    ) -> Option<RunCursor<ShapedRunId, ClusterId>> {
        self.containing(cluster).map(|run| self.cursor(run))
    }

    /// Moves `cursor` to the next run ([`Runs::step`]).
    ///
    /// No run is empty. Past the last run, the cursor stays at the end of the
    /// shaped clusters.
    #[inline]
    pub(super) fn step(&self, cursor: &mut RunCursor<ShapedRunId, ClusterId>) {
        self.runs.step(cursor, self.end);
    }
}

/// Text as shaped: its glyph table, shaping runs, combined-text fits and
/// advances.
///
/// [`Shaped`] holds one for the text. Where the first line shapes
/// differently, it holds a second for the first paragraph under
/// `::first-line` styles, over the same cluster ids.
pub(crate) struct ShapedText {
    /// Each cluster's word, and the sidecar.
    pub(crate) glyphs: GlyphStore,
    /// The shaping runs, in text order.
    pub(crate) runs: ShapedRuns,
    /// How each `text-combine-upright` unit fits its one em, keyed by the
    /// unit's script run.
    ///
    /// Each unit is one script run. Font fallback may split it into several
    /// shaping runs, which share the fit.
    combined: CombineFits,
    /// Each cluster's advance along the line, 16.16, saturating.
    /// Measurement sums them into 48.16 prefix sums.
    ///
    /// Empty after the build where measuring again with new box geometry
    /// changes nothing ([`Shaped::settle_advances`]).
    pub(crate) advances: Vec<TextUnit>,
}

impl ShapedText {
    fn new() -> Self {
        Self {
            glyphs: GlyphStore::new(),
            runs: ShapedRuns::new(),
            combined: CombineFits(None),
            advances: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.glyphs.clear();
        self.runs.clear();
        self.combined.clear();
        self.advances.clear();
    }

    /// Returns how the combined unit in script run `run` fits its em.
    ///
    /// A run that is not a unit fits as it is, centred on the baseline.
    pub(crate) fn combine_fit(&self, run: ScriptRunId) -> CombineFit {
        self.combined
            .get(run)
            .map_or(CombineFit::NATURAL, |&(_, fit)| fit)
    }
}

/// How each `text-combine-upright` unit fits its one em, sorted by the
/// unit's script run.
///
/// Boxed, and allocated only for text with a unit.
struct CombineFits(Option<Box<SortedTable<(ScriptRunId, CombineFit)>>>);

impl CombineFits {
    /// Returns the fit of the unit in script run `run`, or `None` where the
    /// run is not a unit.
    fn get(&self, run: ScriptRunId) -> Option<&(ScriptRunId, CombineFit)> {
        self.0.as_deref()?.get(run)
    }

    /// Appends the fit of the unit in script run `run`, which follows every
    /// unit already added.
    fn push(&mut self, run: ScriptRunId, fit: CombineFit) {
        self.0.get_or_insert_with(Box::default).push((run, fit));
    }

    /// Removes every fit, keeping the allocation.
    fn clear(&mut self) {
        if let Some(fits) = &mut self.0 {
            fits.clear();
        }
    }

    /// Returns whether no unit has a fit.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.0.as_deref().is_none_or(SortedTable::is_empty)
    }

    /// Returns every unit's fit, in run order.
    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = &(ScriptRunId, CombineFit)> {
        self.0.as_deref().into_iter().flat_map(SortedTable::iter)
    }
}

impl HeapBytes for CombineFits {
    fn heap_bytes(&self) -> usize {
        self.0.heap_bytes()
    }
}

impl FirstLineState for ShapedText {
    fn clear(&mut self) {
        ShapedText::clear(self);
    }

    /// Returns whether nothing was shaped: the first line is set as the
    /// text is.
    fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

impl Default for ShapedText {
    fn default() -> Self {
        Self::new()
    }
}

/// The shaping stage's output.
pub(crate) struct Shaped {
    /// The text, and the first paragraph under `::first-line` styles where
    /// font selection made first-line fonts.
    ///
    /// The first-line variant covers the first paragraph's clusters, under
    /// the same ids. It is empty where the first line is set as the text is.
    text: FirstLine<ShapedText>,
    /// Facts about the shaped text.
    pub(crate) flags: ShapedFlags,
}

impl Shaped {
    /// Returns an empty `Shaped`, allocating nothing.
    pub(crate) fn new() -> Self {
        Self {
            text: FirstLine::new(ShapedText::new()),
            flags: ShapedFlags::NONE,
        }
    }

    /// Clears the shaped text, keeping every allocation.
    ///
    /// A layout whose builder is dropped unfinished holds this state.
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.flags = ShapedFlags::NONE;
    }

    /// Returns the first paragraph as shaped under `::first-line` styles,
    /// where it shapes differently from the text.
    #[cfg(test)]
    pub(super) fn first_line(&self) -> Option<&ShapedText> {
        self.text.first_line()
    }

    /// Returns the shaping `variant` reads: the first line's where it
    /// shapes differently, and the text's otherwise.
    #[inline]
    pub(crate) fn text(&self, variant: FirstLineVariant) -> &ShapedText {
        self.text.get(variant)
    }

    /// Keeps the advances where `keep`, and otherwise drops them and gives
    /// the context back its room.
    ///
    /// The build calls it after measurement, with `keep` true where the
    /// content can be measured again.
    pub(crate) fn settle_advances(&mut self, cx: &mut ShapeContext, keep: bool) {
        let rooms = cx.advance_rooms();
        let text = self.text.text_mut();
        rooms.settle(FirstLineVariant::Standard, &mut text.advances, keep);
        if let Some(first_line) = self.text.kept_first_line_mut() {
            rooms.settle(FirstLineVariant::FirstLine, &mut first_line.advances, keep);
        }
    }

    /// Returns the heap bytes of the first-line variant, which
    /// [`heap_bytes`](crate::data::HeapBytes::heap_bytes) also counts.
    pub(crate) fn first_line_heap_bytes(&self) -> usize {
        self.text.first_line_heap_bytes()
    }
}

heap_bytes! {
    Shaped { text; flags }
}

heap_bytes! {
    ShapedText { glyphs, runs, combined, advances }
}

heap_bytes! {
    GlyphStore { words, sidecar }
}

heap_bytes! {
    ShapedRuns { runs, starts; end }
}
