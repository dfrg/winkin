//! The context layouts are built and broken with, and its heap accounting.
//!
//! It holds the fonts, the config, the caches that outlive one layout, and
//! every stage's scratch. Each stage borrows its cache and scratch through
//! an accessor ([`Context::analyzing`] and the rest).
//!
//! [`Context::start_build`] and [`Context::start_break`] trim each cache to
//! its [`CacheLimits`] entry. No id into a cache is held then: a layout
//! reads none after its build, and a call holds its ids only while it runs.
//! [`Context::finish_build`] and [`Context::start_break`] release scratch
//! far bigger than the last calls needed.

use alloc::boxed::Box;
#[cfg(test)]
use alloc::vec::Vec;
use core::mem;

use fontwich::Collection;

use crate::config::Config;
use crate::data::HeapBytes;
use crate::segment::{Hyphenator, PhraseSegmenter};
use crate::stages::analysis::{AnalysisContext, AnalysisScratch, ClusterId};
use crate::stages::content::{ContentScratch, Lists, NodeId};
#[cfg(test)]
use crate::stages::fonts::GeneratedFonts;
use crate::stages::fonts::{FontContext, FontScratch, SharedFonts};
use crate::stages::fragments::PlaceScratch;
use crate::stages::lines::{BreakScratch, LinePlacements};
use crate::stages::measure::{GeneratedInput, MeasureScratch};
use crate::stages::shape::ShapeContext;
#[cfg(test)]
use crate::unit::InlineLayoutUnit;

/// Fonts, configuration, caches and working memory for layout.
///
/// Building and breaking lines take the context mutably, so each thread
/// uses its own. Layouts keep their font data and are read without a
/// context, on any thread. Contexts with equal settings give equal results.
///
/// Each cache holds at most the entries [`CacheLimits`] sets, and drops the
/// least recently used at the start of each build and line breaking.
/// Working memory far larger than recent calls needed is released at the
/// same points. [`clear_caches`](Self::clear_caches) releases every cache
/// and all working memory at once. Existing layouts stay valid either way,
/// and font bytes stay shared until the last reference is dropped.
///
/// ```
/// use winkin::{
///     Area, BuildOptions, ComputedBlockStyle, ComputedStyle, Context, Layout, NoExclusions,
///     NodeKey,
/// };
/// # use fontwich::{Collection, LayerBuilder, Role};
/// # let mut fonts = LayerBuilder::new(Role::Application);
/// # fonts.add_data(include_bytes!("../../../support/testing/fonts/Ahem.ttf").as_slice()).unwrap();
/// # let fonts = Collection::new().with_layer(fonts.snapshot());
///
/// let mut cx = Context::new(fonts);
/// let mut layout = Layout::new();
/// let style = ComputedStyle::initial();
/// let block = ComputedBlockStyle::new(&style);
/// let mut builder = layout.builder(&mut cx, NodeKey(0), &block, BuildOptions::default());
/// builder.text(NodeKey(1), "Hello, world");
/// builder.finish();
///
/// // Drop every cache; keep the fonts and the config.
/// cx.clear_caches();
/// assert_eq!(cx.heap_bytes().total(), 0);
///
/// // The layout built before still breaks the same way.
/// layout.break_lines(&mut cx, Area::new(100.0), &mut NoExclusions);
/// assert_eq!(layout.lines().len(), 2);
/// ```
pub struct Context {
    /// The layout choices that CSS does not make.
    config: Config,
    /// How many entries each cache keeps.
    limits: CacheLimits,
    /// The fonts, and font selection's caches: family lists, fallback lists
    /// and font instances.
    fonts: FontContext,
    /// The content writer's working memory, which a builder takes while it
    /// writes, and the build's lists for font selection.
    content_scratch: ContentScratch,
    /// Font selection's working memory.
    font_scratch: FontScratch,
    /// The segmenters analysis runs, and the host's hyphenator and phrase
    /// segmenter.
    analysis: AnalysisContext,
    /// Analysis's working memory.
    analysis_scratch: AnalysisScratch,
    /// The shaping caches, shared with the breaker's reshapes.
    ///
    /// They hold the shape plans, each font's `text-spacing-trim` data, the
    /// vertical fonts, and the buffer every shaping call fills.
    shaping: ShapeContext,
    /// Measurement's cursors for walking intrinsic ruby pieces.
    measure_scratch: MeasureScratch,
    /// The breaker's working memory, one part per phase.
    break_scratch: BreakScratch,
    /// Line layout's working memory: a line's pieces, its order and its boxes' fragment items.
    place_scratch: PlaceScratch,
    /// The sizes of the last calls, against which the scratch is released.
    scratch_sizes: ScratchSizes,
}

impl Context {
    /// Creates a context with `fonts`, [`Config::platform`] settings and
    /// the default [`CacheLimits`].
    pub fn new(fonts: Collection) -> Self {
        Self::with_fonts(fonts, None)
    }

    /// Creates a context as [`new`](Self::new) does, parsing each font
    /// through `shared`.
    ///
    /// Contexts created with clones of one [`SharedFonts`] parse each font
    /// once between them, so a new context lays out its first text sooner.
    /// Results are the same as with [`new`](Self::new).
    pub fn with_shared_fonts(fonts: Collection, shared: SharedFonts) -> Self {
        Self::with_fonts(fonts, Some(shared))
    }

    /// Creates a context with `fonts`, parsing each through `shared` where
    /// given.
    fn with_fonts(fonts: Collection, shared: Option<SharedFonts>) -> Self {
        let limits = CacheLimits::default();
        Self {
            config: Config::platform(),
            fonts: FontContext::new(fonts, &limits, shared),
            content_scratch: ContentScratch::new(),
            font_scratch: FontScratch::new(),
            analysis: AnalysisContext::new(),
            analysis_scratch: AnalysisScratch::new(),
            shaping: ShapeContext::new(&limits),
            measure_scratch: MeasureScratch::default(),
            break_scratch: BreakScratch::new(),
            place_scratch: PlaceScratch::new(),
            scratch_sizes: ScratchSizes::new(),
            limits,
        }
    }

    /// Replaces the font collection, for example after loading a web font.
    ///
    /// Existing layouts keep their fonts until rebuilt. Cached family lists
    /// are discarded unless `fonts` holds the same layer snapshots. Cached
    /// data of fonts missing from `fonts` is discarded, which releases their
    /// bytes once no layout uses them. Cached data of the other fonts is
    /// kept.
    pub fn set_collection(&mut self, fonts: Collection) {
        if self.fonts.set_collection(fonts) {
            let held = &self.fonts;
            self.shaping.retain_fonts(|key| held.has_face(key));
        }
    }

    /// Returns the font collection.
    pub fn collection(&self) -> &Collection {
        self.fonts.collection()
    }

    /// Sets the layout configuration.
    ///
    /// Existing layouts keep the settings read at build time, such as the
    /// word-spacing rule, until rebuilt. Settings read in line breaking,
    /// such as tab justification, apply at the next
    /// [`break_lines`](crate::Layout::break_lines) call. Each field's
    /// documentation says which applies.
    ///
    /// The caches stay valid: each build derives font line metrics from
    /// unscaled metrics.
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Returns the layout configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Sets what finds the hyphenation opportunities of `hyphens: auto`
    /// text, or removes it with `None`.
    ///
    /// Without a hyphenator, `hyphens: auto` breaks only at soft hyphens, as
    /// `manual` does. Existing layouts keep the opportunities found when
    /// they were built until rebuilt.
    pub fn set_hyphenator(&mut self, hyphenator: Option<Box<dyn Hyphenator>>) {
        self.analysis.hyphenator = hyphenator;
    }

    /// Sets what finds the phrases of `word-break: auto-phrase` text, or
    /// removes it with `None`.
    ///
    /// Without a phrase segmenter, `auto-phrase` breaks as `normal` does.
    /// Existing layouts keep the opportunities found when they were built
    /// until rebuilt.
    pub fn set_phrase_segmenter(&mut self, segmenter: Option<Box<dyn PhraseSegmenter>>) {
        self.analysis.phrases = segmenter;
    }

    /// Sets how many entries each cache keeps.
    ///
    /// A cache above its new limit drops its least recently used entries
    /// at once. Existing layouts stay valid.
    pub fn set_cache_limits(&mut self, limits: CacheLimits) {
        self.limits = limits;
        self.fonts.set_limits(&limits);
        self.shaping.set_limits(&limits);
        self.trim_caches();
    }

    /// Returns how many entries each cache keeps.
    pub fn cache_limits(&self) -> &CacheLimits {
        &self.limits
    }

    /// Drops every cached font, plan and list, and releases all working
    /// memory.
    ///
    /// Keeps the collection, the configuration, the cache limits, and any
    /// hyphenator and phrase segmenter.
    /// Existing layouts stay valid. Later builds and line breaking fill the
    /// caches again and give the same results.
    pub fn clear_caches(&mut self) {
        self.fonts.clear(&self.limits);
        self.shaping = ShapeContext::new(&self.limits);
        self.release_scratch();
        self.scratch_sizes = ScratchSizes::new();
    }

    /// Returns estimated heap usage by cache, in bytes.
    ///
    /// Counts table capacities, cached values and retained working memory.
    /// Font bytes shared with the collection are in
    /// [`ContextHeap::font_data`]. Excludes the collection, [`SharedFonts`],
    /// which reports its own usage, and read-fonts, harfrust and fontwich
    /// values that do not report their size: parsed fonts, shape plans, the
    /// shaping buffer and family handles.
    pub fn heap_bytes(&self) -> ContextHeap {
        let Self {
            config: _,
            limits: _,
            fonts,
            content_scratch,
            font_scratch,
            analysis: _,
            analysis_scratch,
            shaping,
            measure_scratch,
            break_scratch,
            place_scratch,
            scratch_sizes: _,
        } = self;
        let shaping_scratch = shaping.scratch_heap_bytes();
        ContextHeap {
            fonts: fonts.heap_bytes(),
            shaping: shaping.heap_bytes().saturating_sub(shaping_scratch),
            scratch: content_scratch.heap_bytes()
                + analysis_scratch.heap_bytes()
                + font_scratch.heap_bytes()
                + shaping_scratch
                + measure_scratch.heap_bytes()
                + break_scratch.heap_bytes()
                + place_scratch.heap_bytes(),
            font_data: fonts.face_bytes(),
        }
    }

    /// Readies the context for a build as its builder starts.
    ///
    /// Trims every cache to its limit, since no call holds an id into one.
    /// The builder then holds the context until it finishes, so nothing
    /// trims the family lists it names.
    pub(crate) fn start_build(&mut self) {
        self.trim_caches();
    }

    /// Notes a finished build of content `size` long, and releases the
    /// scratch where it is far larger than the last calls need.
    ///
    /// `size` counts the content's text bytes and items, which every scratch
    /// grows with.
    pub(crate) fn finish_build(&mut self, size: usize) {
        if self.scratch_sizes.note(size) {
            self.release_scratch();
        }
    }

    /// Readies the context for a break of content `size` long.
    ///
    /// Trims the shaping caches and releases oversized scratch. Leaves the
    /// font caches, which only a build fills.
    pub(crate) fn start_break(&mut self, size: usize) {
        self.shaping.trim();
        if self.scratch_sizes.note(size) {
            self.release_scratch();
        }
    }

    /// Trims every cache to its limit.
    fn trim_caches(&mut self) {
        self.fonts.trim();
        self.shaping.trim();
    }

    /// Replaces every scratch with empty scratch, releasing its capacity.
    fn release_scratch(&mut self) {
        self.content_scratch = ContentScratch::new();
        self.font_scratch = FontScratch::new();
        self.analysis_scratch = AnalysisScratch::new();
        self.shaping.release_scratch();
        self.measure_scratch = MeasureScratch::default();
        self.break_scratch = BreakScratch::new();
        self.place_scratch = PlaceScratch::new();
    }

    /// Returns what analysis runs with: its segmenters and its scratch.
    pub(crate) fn analyzing(&mut self) -> (&AnalysisContext, &mut AnalysisScratch) {
        (&self.analysis, &mut self.analysis_scratch)
    }

    /// Takes the content writer's working memory, leaving empty scratch.
    pub(crate) fn take_content_scratch(&mut self) -> ContentScratch {
        mem::take(&mut self.content_scratch)
    }

    /// Gives back the content writer's working memory, `scratch`.
    pub(crate) fn put_content_scratch(&mut self, scratch: ContentScratch) {
        self.content_scratch = scratch;
    }

    /// Returns what font selection runs with: its caches, its scratch, the
    /// build's lists, and the nodes the analysis found soft hyphens in.
    pub(crate) fn selecting_fonts(&mut self) -> FontSelection<'_> {
        (
            &mut self.fonts,
            &mut self.font_scratch,
            self.content_scratch.lists(),
            self.analysis_scratch.hyphen_nodes(),
        )
    }

    /// Returns what measurement runs with: the shaping caches, its scratch,
    /// and the generated texts the last build chose fonts for.
    pub(crate) fn measuring(
        &mut self,
    ) -> (&mut ShapeContext, &mut MeasureScratch, GeneratedInput<'_>) {
        let generated = GeneratedInput {
            fonts: self.font_scratch.generated(),
            strings: self.content_scratch.lists().hyphen_strings(),
            block_space: self.font_scratch.block_space(),
        };
        (&mut self.shaping, &mut self.measure_scratch, generated)
    }

    /// Returns the shaping caches.
    pub(crate) fn shaping(&mut self) -> &mut ShapeContext {
        &mut self.shaping
    }

    /// Returns what the breaker runs with: the shaping caches and its scratch.
    pub(crate) fn breaking(&mut self) -> (&mut ShapeContext, &mut BreakScratch) {
        (&mut self.shaping, &mut self.break_scratch)
    }

    /// Returns the line placements breaking made, and line layout's scratch.
    pub(crate) fn placing(&mut self) -> (&mut LinePlacements, &mut PlaceScratch) {
        (&mut self.break_scratch.placements, &mut self.place_scratch)
    }

    /// Returns the last build's lists, for tests to inspect.
    #[cfg(test)]
    pub(crate) fn content_lists(&self) -> &Lists {
        self.content_scratch.lists()
    }

    /// Returns the fonts of the last build's generated texts, for tests to
    /// inspect.
    #[cfg(test)]
    pub(crate) fn generated_fonts(&self) -> &GeneratedFonts {
        self.font_scratch.generated()
    }

    /// Returns each cluster's advance as the last build's shaping wrote it,
    /// whether or not the layout keeps them, for tests to inspect.
    #[cfg(test)]
    pub(crate) fn shaped_advances(&self) -> Vec<InlineLayoutUnit> {
        let advances = self.shaping.last_shaped_advances().iter();
        advances
            .map(|&advance| InlineLayoutUnit::from_text(advance))
            .collect()
    }

    /// Returns font selection's caches, for tests to inspect.
    #[cfg(test)]
    pub(crate) fn font_context(&self) -> &FontContext {
        &self.fonts
    }
}

/// What font selection runs with: see [`Context::selecting_fonts`].
type FontSelection<'a> = (
    &'a mut FontContext,
    &'a mut FontScratch,
    &'a Lists,
    &'a [(NodeId, ClusterId)],
);

/// How many scratch sizes [`ScratchSizes`] remembers.
const SCRATCH_CALLS: usize = 8;

/// How many times larger than the largest recent call the scratch may have
/// grown before it is released.
const SCRATCH_SLACK: usize = 4;

/// The call size below which the scratch is never released, so small calls
/// never release and allocate again.
///
/// It is a call size, not bytes, since the shaping buffer does not report
/// its bytes.
const SCRATCH_FLOOR: usize = 16 * 1024;

/// The sizes of the last calls, and the largest the scratch grew for.
///
/// The scratch keeps its capacity, so one huge paragraph would keep it huge.
/// It is released once the last [`SCRATCH_CALLS`] calls are each under a
/// [`SCRATCH_SLACK`]th of what it grew for. Calls of a steady size never
/// release it, so a warm relayout allocates nothing.
struct ScratchSizes {
    recent: [usize; SCRATCH_CALLS],
    /// Where in `recent` the next size goes.
    next: usize,
    /// The largest call since the scratch was last released.
    grown_for: usize,
}

impl ScratchSizes {
    /// Returns sizes of no calls.
    const fn new() -> Self {
        Self {
            recent: [0; SCRATCH_CALLS],
            next: 0,
            grown_for: 0,
        }
    }

    /// Notes a call of `size` and returns whether the scratch is far bigger
    /// than the last calls need.
    fn note(&mut self, size: usize) -> bool {
        if let Some(recent) = self.recent.get_mut(self.next) {
            *recent = size;
        }
        self.next = (self.next + 1) % SCRATCH_CALLS;
        let needed = self.recent.iter().copied().max().unwrap_or(size);
        let oversized =
            self.grown_for > SCRATCH_FLOOR && self.grown_for > needed.saturating_mul(SCRATCH_SLACK);
        self.grown_for = if oversized {
            needed
        } else {
            self.grown_for.max(size)
        };
        oversized
    }
}

/// How many entries each context cache keeps between calls.
///
/// A cache over its limit drops its least recently used entries at the
/// start of the next build or line breaking. A dropped entry is made again
/// when next needed, with the same result. A limit of zero keeps nothing
/// between calls.
///
/// The defaults suit content with a few dozen distinct styles.
///
/// ```
/// use winkin::{CacheLimits, Context};
/// # use fontwich::Collection;
///
/// let mut cx = Context::new(Collection::new());
/// let mut limits = CacheLimits::default();
/// limits.font_instances = 16;
/// cx.set_cache_limits(limits);
/// assert_eq!(cx.cache_limits().font_instances, 16);
/// ```
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct CacheLimits {
    /// Distinct `font-family` lists. Default 64.
    ///
    /// A build keeps every list it names until it finishes. The next build
    /// trims the cache back to this limit as it starts.
    pub family_lists: usize,
    /// Fallback lists: the fonts to try for one family list, script,
    /// language, weight, width and style. Default 128.
    pub fallback_lists: usize,
    /// Font instances: a font at one set of variation coordinates, with
    /// its features, synthesis and `@font-face` metric overrides. Default 64.
    ///
    /// Each distinct `font-variation-settings` makes one, and so does each
    /// size of a font with an `opsz` axis under `font-optical-sizing: auto`.
    /// The same limit applies to per-font shaping data: `text-spacing-trim`
    /// data, vertical metrics at each size, and the features a font
    /// supports and covers.
    pub font_instances: usize,
    /// Shape plans: one per font, script, direction, language and list of
    /// features. Default 128. The same limit applies to the languages the
    /// plans are made for.
    pub shape_plans: usize,
}

impl Default for CacheLimits {
    fn default() -> Self {
        Self {
            family_lists: 64,
            fallback_lists: 128,
            font_instances: 64,
            shape_plans: 128,
        }
    }
}

/// Estimated context heap usage by cache, in bytes.
///
/// Returned by [`Context::heap_bytes`]. Counts table capacities, including
/// memory kept for later layouts. Dropping the context or calling
/// [`Context::clear_caches`] releases the caches.
///
/// Excludes compiled segmenter data and unreported allocations within
/// read-fonts, harfrust and fontwich. Font bytes shared with the collection
/// are reported in [`font_data`](Self::font_data), outside the total.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct ContextHeap {
    /// The font selection caches.
    ///
    /// They hold resolved family lists, fallback lists, font instances with
    /// their shaping features, and what each font's features cover.
    pub fonts: usize,
    /// The shaping caches.
    ///
    /// They hold languages and shape plans, per-font `text-spacing-trim`
    /// data, and vertical font and glyph metrics at each size.
    pub shaping: usize,
    /// The working memory of text preparation, line breaking and line layout.
    ///
    /// Holds nothing between calls, but keeps its capacity until a series of
    /// much smaller calls releases it.
    pub scratch: usize,
    /// The bytes of the font files the font caches hold.
    ///
    /// The collection and the layouts set in these fonts share the bytes,
    /// which stay in memory while any of them holds the font. Not included
    /// in [`total`](Self::total).
    pub font_data: usize,
}

impl ContextHeap {
    /// Returns the total estimated heap usage, in bytes.
    ///
    /// Excludes [`font_data`](Self::font_data), which is shared.
    pub fn total(&self) -> usize {
        let Self {
            fonts,
            shaping,
            scratch,
            font_data: _,
        } = *self;
        fonts + shaping + scratch
    }
}
