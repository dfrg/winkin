//! The shaping stage: each cluster's glyphs and the runs they are shaped in.
//!
//! In: [`ShapeInput`], the content, the analysis and the fonts. Out:
//! [`Shaped`], and the [`Advances`] handed to measurement.
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
//! [`Shaped`] is frozen after preparation. [`Advances`] moves the advance
//! buffers to measurement, which turns them into prefix sums in place.

mod context;
mod provider;
mod shaper;
mod sink;
#[cfg(test)]
mod tests;
mod trim;
mod vertical;
mod walk;

use alloc::vec::Vec;
use core::ops::Range;

use crate::config::PunctuationTrim;
use crate::data::{
    Id, RankedBitTable, Run, RunCursor, Runs, SortedTable, Table, define_flags, define_id,
    heap_bytes,
};
use crate::stages::analysis::{Analysis, ClusterId, ScriptRunId};
use crate::stages::content::{Content, ShapingFactsId};
use crate::stages::fonts::{Fonts, UsedFontId};
use crate::style::FirstLine;
use crate::style::FirstLineState;
use crate::style::FirstLineVariant;
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::work;

pub(crate) use context::{ShapeContext, ShapeSession};
pub(crate) use shaper::{NeighbourFonts, ShapingKey, ShapingSource, shape_generated, shape_range};
pub(super) use sink::GlyphSink;
pub(crate) use trim::{ShapingEdges, maybe_closing_mark, maybe_opening_mark};
use walk::shape_text;

/// What shaping reads besides the context: the content, the analysis and the
/// fonts.
///
/// The used fonts hold everything their runs are shaped with.
pub(crate) struct ShapeInput<'a> {
    pub(crate) content: &'a Content,
    pub(crate) analysis: &'a Analysis,
    pub(crate) fonts: &'a Fonts,
    /// Where `text-spacing-trim` trims punctuation: the context's config.
    pub(crate) punctuation_trim: PunctuationTrim,
}

/// Shapes the text of `input` into `out`, each cluster's advance among
/// them, using the caches in `cx`.
///
/// Where font selection gave the first paragraph runs of its own under
/// `::first-line`, it shapes that paragraph again in those styles and fonts.
/// The result goes into `out`'s first-line text and the first line's
/// advances.
///
/// Returns how many glyphs were left out because a sidecar was full.
///
/// This call alone clears and fills `out`. It sets `out`'s flags once,
/// from both shapings: an unsafe break in either lets the breaker reshape.
/// Measurement copies the advances next.
pub(crate) fn shape_runs(
    input: &ShapeInput<'_>,
    cx: &mut ShapeSession<'_, '_>,
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
    // The first line's variant, where it sets text differently. Otherwise
    // the first line is set as the text is, and its advances stay empty.
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
    out.flags = flags;
    dropped
}

define_id! {
    /// Names a shaping run in a layout's table of [`ShapedRun`]s.
    pub(crate) struct ShapedRunId(u32);
}

define_id! {
    /// Names a glyph in a table of [`SidecarGlyph`]s: the shaped text's, or
    /// a line's own for its reshaped edges.
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
/// The word records what shaping decided, never a measurement. The
/// cluster's advance belongs to measurement, which takes it from the buffer
/// shaping hands over ([`Advances`]).
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
    /// The cluster's glyphs are an earlier cluster's: it is the inside of a
    /// ligature, or of anything else the shaper merged across clusters.
    const CONTINUATION: u32 = 1 << 28;
    const PAYLOAD: u32 = (1 << 28) - 1;
    /// The payload of a cluster with no glyphs of its own.
    const NONE: u32 = Self::PAYLOAD;
    /// The largest glyph id, or sidecar start, a word holds.
    const MAX_PAYLOAD: u32 = Self::PAYLOAD - 1;

    /// A cluster with no glyphs: one not shaped, or in a run no font draws.
    pub(super) const EMPTY: Self = Self(Self::NONE);

    /// A compact cluster: one glyph, drawn with no offset, or `None` for an
    /// id a word cannot hold.
    fn from_glyph(glyph: u32) -> Option<Self> {
        (glyph <= Self::MAX_PAYLOAD).then_some(Self(glyph))
    }

    /// An expanded cluster: glyphs from `start` in the sidecar, or `None`
    /// for a start a word cannot hold.
    fn from_sidecar(start: SidecarGlyphId) -> Option<Self> {
        let start = u32::try_from(start.get()).ok()?;
        (start <= Self::MAX_PAYLOAD).then_some(Self(Self::EXPANDED | start))
    }

    /// The same, unsafe to break before where `to_break`.
    fn with_unsafe(self, to_break: bool) -> Self {
        if to_break {
            Self(self.0 | Self::UNSAFE_TO_BREAK)
        } else {
            self
        }
    }

    /// The same, marked as a continuation.
    fn continuing(self) -> Self {
        Self(self.0 | Self::CONTINUATION)
    }

    /// The same bits, its glyphs from `start` in the sidecar: a copied
    /// expanded cluster's word pointing where its glyphs were copied to, or
    /// `None` for a start a word cannot hold.
    fn with_sidecar_start(self, start: SidecarGlyphId) -> Option<Self> {
        let start = Self::from_sidecar(start)?;
        Some(Self((self.0 & !Self::PAYLOAD) | (start.0 & Self::PAYLOAD)))
    }

    /// Whether breaking before the cluster changes the shaping.
    pub(super) fn is_unsafe_to_break(self) -> bool {
        self.0 & Self::UNSAFE_TO_BREAK != 0
    }

    /// Whether the cluster's glyphs are an earlier cluster's.
    pub(crate) fn is_continuation(self) -> bool {
        self.0 & Self::CONTINUATION != 0
    }

    /// Whether the cluster's glyphs are in the sidecar.
    fn is_expanded(self) -> bool {
        self.0 & Self::EXPANDED != 0
    }

    /// Returns the one glyph of a compact cluster, drawn at its pen with no
    /// offset, or `None` for any other.
    ///
    /// A paint walk asks this first of every cluster, since nearly every
    /// cluster is compact.
    #[inline]
    pub(super) fn single(self) -> Option<u32> {
        let payload = self.0 & Self::PAYLOAD;
        (payload != Self::NONE && !self.is_expanded()).then_some(payload)
    }

    /// The cluster's own glyphs, `sidecar` being the table the word was
    /// written with. Inlined: a paint walk asks it of every cluster.
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
        // The cluster's last glyph says so; a sidecar cut short ends it.
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

    /// The glyph id.
    pub(crate) fn id(&self) -> u32 {
        self.id & !Self::LAST
    }

    /// Whether it is its cluster's last glyph.
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

/// How a combined unit fits its one em.
///
/// It says how much the unit is narrowed beyond the form it was shaped in,
/// and where across its line it is centred.
#[derive(Copy, Clone, Debug)]
pub(crate) struct CombineFit {
    /// The horizontal scale that fits it, 16.16: one where it fits.
    scale: TextUnit,
    /// How far over the line's baseline its text is centred: the middle of
    /// its primary font's ascent and descent (`FontLineMetrics::
    /// text_middle`).
    pub(crate) middle: LayoutUnit,
}

impl CombineFit {
    /// A unit that fits as it is, centred on the baseline.
    const NATURAL: Self = Self {
        scale: TextUnit::from_raw(1 << 16),
        middle: LayoutUnit::ZERO,
    };

    /// How much its glyphs are narrowed across: one where they fit as
    /// shaped.
    pub(crate) fn scale(&self) -> f32 {
        self.scale.to_px()
    }
}

define_flags! {
    /// Facts about the shaped text that let a later stage skip work at once.
    pub(crate) struct ShapedFlags(u8) {
        /// Some cluster is unsafe to break before. Without it, no line edge
        /// is ever reshaped.
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
/// Words are in cluster order, right-to-left runs included. A reader walking
/// a text item beside a line's reshaped edges takes the item's words as a
/// slice ([`words`](Self::words)). It decodes each word against the sidecar
/// it was written with: this table's ([`sidecar`](Self::sidecar)) or the
/// line's own.
pub(crate) struct GlyphStore {
    /// One word per cluster, in cluster order.
    words: Table<ClusterId, GlyphWord>,
    /// The glyphs of expanded clusters, a cluster's together, in cluster
    /// order, which the words are decoded against ([`GlyphWord::glyphs`]).
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

    /// `cluster`'s word, or [`GlyphWord::EMPTY`] past the last.
    pub(crate) fn word(&self, cluster: ClusterId) -> GlyphWord {
        self.words.get(cluster).copied().unwrap_or(GlyphWord::EMPTY)
    }

    /// `cluster`'s own glyphs: none past the last.
    pub(crate) fn glyphs(&self, cluster: ClusterId) -> ClusterGlyphs<'_> {
        self.word(cluster).glyphs(&self.sidecar)
    }

    /// The words of `clusters`, in cluster order: none where the range is
    /// not the table's.
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

    /// How many clusters it has a word for.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.words.len()
    }

    /// Whether it has no word: nothing was shaped.
    fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Every cluster's word with its id, in cluster order, for tests to
    /// look at.
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = (ClusterId, &GlyphWord)> {
        self.words.iter()
    }
}

/// The shaping runs in text order: which run a cluster is in, what each is
/// shaped with, and which clusters it holds.
///
/// The runs tile the clusters the glyph table has words for. Each ends where
/// the next starts, and the last ends where the shaped clusters do. A reader
/// asking about one cluster finds its run by counting start bits
/// ([`containing`](Self::containing)). A reader walking forward uses a cursor
/// ([`step`](Self::step)).
pub(crate) struct ShapedRuns {
    runs: Runs<ShapedRunId, ShapedRun>,
    /// Where the last run ends: the end of the shaped clusters. Each push
    /// moves it to the new run's end.
    end: ClusterId,
    /// A bit for each cluster a run starts at, with a count before each word.
    ///
    /// The run holding a cluster is the number of starts at or before it,
    /// less one, found without a search. A paint walk asks this of every
    /// text run for its font, and line layout of every ruby annotation
    /// cluster. It is fixed before any line is broken, so a relayout writes
    /// nothing here.
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

    /// Makes room for the runs of `clusters` clusters, `runs` of them at
    /// least: a start bit a cluster, and a record a run.
    fn reserve(&mut self, clusters: usize, runs: usize) {
        self.starts.reserve(clusters);
        self.runs.reserve(runs);
    }

    /// Appends `run`, which holds the clusters from its start to `end`, and
    /// returns its id, or `None` past what a run id names, which no more
    /// runs than clusters reach. Its clusters are the runs' either way.
    fn push(&mut self, run: ShapedRun, end: ClusterId) -> Option<ShapedRunId> {
        self.end = end;
        let id = self.runs.push(run)?;
        self.starts.set(run.start);
        Some(id)
    }

    /// The run `id` names, or `None` for one the text does not have.
    pub(crate) fn get(&self, id: ShapedRunId) -> Option<&ShapedRun> {
        self.runs.get(id)
    }

    /// How many there are.
    pub(super) fn len(&self) -> usize {
        self.runs.len()
    }

    /// Whether there are none: nothing was shaped.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Every run with its id and its clusters, in text order, for the tests
    /// to check.
    #[cfg(test)]
    fn iter(&self) -> impl Iterator<Item = (ShapedRunId, &ShapedRun, Range<ClusterId>)> {
        self.runs
            .iter()
            .map(|(id, run)| (id, run, self.run_clusters(id)))
    }

    /// The run holding `cluster`: the last that starts at or before it,
    /// numbered by how many do (see `starts`). `None` past the last cluster
    /// shaped.
    #[inline]
    pub(crate) fn containing(&self, cluster: ClusterId) -> Option<ShapedRunId> {
        if cluster >= self.end {
            return None;
        }
        work::seek();
        let held = self.starts.count_through(cluster).checked_sub(1)?;
        Some(ShapedRunId::new(held))
    }

    /// Returns the run holding `cluster`, or `None` past the last cluster
    /// shaped.
    ///
    /// A reader asks this for the font a cluster is shaped in, and a reshape
    /// for the key it is shaped with.
    pub(crate) fn run_containing(&self, cluster: ClusterId) -> Option<&ShapedRun> {
        self.runs.get(self.containing(cluster)?)
    }

    /// Run `id`'s clusters: from its start to the next run's, or the last
    /// run's to where the clusters shaped end. Empty, at that end, for a run
    /// the text does not have.
    #[inline]
    pub(super) fn run_clusters(&self, id: ShapedRunId) -> Range<ClusterId> {
        self.runs.span(id, self.end)
    }
}

/// The cursor a walk over the shaping runs holds ([`Segments`]).
///
/// [`Segments`]: crate::stages::Segments
impl ShapedRuns {
    /// A cursor at run `id`, from a link to it.
    #[inline]
    pub(super) fn cursor(&self, id: ShapedRunId) -> RunCursor<ShapedRunId, ClusterId> {
        self.runs.cursor(id, self.end)
    }

    /// A cursor at the run holding `cluster`, numbered by the start bits at
    /// or before it ([`containing`](Self::containing)), with no search; `None`
    /// past the last cluster shaped.
    #[inline]
    pub(super) fn cursor_containing(
        &self,
        cluster: ClusterId,
    ) -> Option<RunCursor<ShapedRunId, ClusterId>> {
        self.containing(cluster).map(|run| self.cursor(run))
    }

    /// Moves `cursor` to the next run, none being empty: past the last, it
    /// stands where the clusters shaped end, and stays there
    /// ([`Runs::step`]).
    #[inline]
    pub(super) fn step(&self, cursor: &mut RunCursor<ShapedRunId, ClusterId>) {
        self.runs.step(cursor, self.end);
    }
}

/// Text as shaped: its glyph table, its shaping runs and the fits of its
/// combined text, each a table of its own that readers ask.
///
/// [`Shaped`] holds one for the text. Where the first line shapes
/// differently, it holds a second for the first paragraph under
/// `::first-line` styles, with the same shape over fewer clusters.
pub(crate) struct ShapedText {
    /// Its glyph store: each cluster's word, and the sidecar.
    pub(crate) glyphs: GlyphStore,
    /// Its shaping runs, in text order.
    pub(crate) runs: ShapedRuns,
    /// How each `text-combine-upright` unit fits its one em, keyed by the
    /// unit's script run.
    ///
    /// Each unit is one script run. Font fallback may split it into several
    /// shaping runs, which share the fit. Sorted by run, and sparse.
    combined: SortedTable<(ScriptRunId, CombineFit)>,
    /// Each cluster's advance along the line, in 48.16, which measurement
    /// copies and sums.
    pub(crate) advances: Vec<InlineLayoutUnit>,
}

impl ShapedText {
    fn new() -> Self {
        Self {
            glyphs: GlyphStore::new(),
            runs: ShapedRuns::new(),
            combined: SortedTable::new(),
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

impl FirstLineState for ShapedText {
    fn clear(&mut self) {
        ShapedText::clear(self);
    }

    /// Whether nothing was shaped: a first line set as the text is.
    fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

impl Default for ShapedText {
    fn default() -> Self {
        Self::new()
    }
}

/// The shaped text: the shaping stage's output.
pub(crate) struct Shaped {
    /// The text, and the first paragraph under `::first-line` styles where
    /// font selection made first-line fonts.
    ///
    /// The first-line variant covers the same clusters. It is empty where
    /// the first line is set as the text is.
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

    /// Empties it, keeping every allocation.
    ///
    /// A layout also holds this state when its builder is dropped without
    /// finishing.
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.flags = ShapedFlags::NONE;
    }

    /// The first paragraph as shaped under `::first-line` styles, where it
    /// shapes differently from the text.
    #[cfg(test)]
    pub(super) fn first_line(&self) -> Option<&ShapedText> {
        self.text.first_line()
    }

    /// The text as `variant` reads it: the first line's shaping where there
    /// is one, and the text's otherwise, which it is the same as where there
    /// is none.
    #[inline]
    pub(crate) fn text(&self, variant: FirstLineVariant) -> &ShapedText {
        self.text.get(variant)
    }

    /// The bytes the first line's variant takes on the heap, which
    /// [`heap_bytes`](crate::data::HeapBytes::heap_bytes) also counts.
    pub(crate) fn first_line_heap_bytes(&self) -> usize {
        self.text.first_line_heap_bytes()
    }

    /// Returns `into` holding a copy of the text's and the first line's
    /// advances, its own buffers cleared first: one allocation each going
    /// round.
    pub(crate) fn advances_into(&self, into: Advances) -> Advances {
        let (mut text, mut first_line) = into.into_parts();
        text.clear();
        text.extend_from_slice(&self.text.get(FirstLineVariant::Standard).advances);
        first_line.clear();
        if let Some(first) = self.text.first_line() {
            first_line.extend_from_slice(&first.advances);
        }
        Advances::new(text, first_line)
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

impl Default for Shaped {
    fn default() -> Self {
        Self::new()
    }
}

/// Each cluster's advance along the line, as shaping hands it to
/// measurement.
///
/// There is one buffer for the text, and one for the first paragraph under
/// `::first-line` styles where font selection gave it runs of its own. The
/// second is empty otherwise.
///
/// Advances are in 48.16, the type of the prefix sums measurement turns
/// them into. So a cluster's advance is exactly its glyphs', however wide.
///
/// Each [`ShapedText`] keeps its own, so that the text can be measured again
/// with other box sizes. Measurement copies them into the buffers its last
/// prefix sums were kept in (`Measured::recycle`) and sums them there in
/// place.
pub(crate) struct Advances {
    text: Vec<InlineLayoutUnit>,
    first_line: Vec<InlineLayoutUnit>,
}

impl Advances {
    /// The pair around the buffers `text` and `first_line`, which shaping
    /// clears before it fills them.
    pub(crate) fn new(text: Vec<InlineLayoutUnit>, first_line: Vec<InlineLayoutUnit>) -> Self {
        Self { text, first_line }
    }

    /// Returns the text's and the first line's buffers, for measurement to
    /// turn into prefix sums in place.
    pub(crate) fn into_parts(self) -> (Vec<InlineLayoutUnit>, Vec<InlineLayoutUnit>) {
        (self.text, self.first_line)
    }
}
