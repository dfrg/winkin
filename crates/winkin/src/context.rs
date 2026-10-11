//! The context every layout is built and broken with, and what it keeps on the heap.
//!
//! It holds the fonts, the config, the caches that outlive one layout, and
//! every stage's scratch except the content's, which the layout owns. The
//! stages borrow one cache and scratch at a time ([`Context::analyzing`] and
//! the rest).
//!
//! Every cache keeps at most its capacity ([`CacheLimits`]).
//! [`Context::start_build`] and [`Context::start_break`] trim the caches to
//! it before every build and break, when no id into a cache is held: a
//! layout holds none, and a call holds its ids only while it runs. They also
//! release scratch far bigger than the last calls needed.

use fontwich::Collection;

use crate::config::Config;
use crate::data::HeapBytes;
use crate::stages::analysis::{AnalysisContext, AnalysisScratch};
use crate::stages::fonts::{FontContext, FontScratch, SharedFonts};
use crate::stages::fragments::PlaceScratch;
use crate::stages::lines::{BreakScratch, LinePlacements};
use crate::stages::measure::MeasureScratch;
use crate::stages::shape::ShapeContext;

/// Fonts, configuration, caches and working memory for layout.
///
/// Use one context per thread; layout operations require mutable access.
/// Layouts retain their font data and can be read without a context, including
/// on another thread. Relayout uses a context for configuration, shaping
/// caches and working memory. Equivalent settings produce equivalent results.
///
/// Caches hold at most the number of entries [`CacheLimits`] sets, and drop
/// the entries used longest ago at the start of each build and line
/// breaking. Working memory far larger than recent calls needed is released
/// at the same points. [`clear_caches`](Self::clear_caches) releases every
/// cache and all working memory at once. Existing layouts remain valid
/// either way. Font bytes remain shared until the last reference is
/// dropped. Subsequent builds and line breaking repopulate the caches.
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
/// let mut builder = layout.builder(NodeKey(0), &block, BuildOptions::default());
/// builder.text(NodeKey(1), "Hello, world");
/// builder.finish(&mut cx);
///
/// // Every cache dropped, the fonts and the config kept.
/// cx.clear_caches();
/// assert_eq!(cx.heap_bytes().total(), 0);
///
/// // The layout built before breaks as it would have.
/// layout.break_lines(&mut cx, Area::new(100.0), &mut NoExclusions);
/// assert_eq!(layout.lines().len(), 2);
/// ```
pub struct Context {
    /// The engine's choices that are not CSS.
    config: Config,
    /// How many entries each cache keeps.
    limits: CacheLimits,
    /// The fonts, and font selection's caches: family lists, fallback lists
    /// and font instances.
    fonts: FontContext,
    /// Font selection's working memory.
    font_scratch: FontScratch,
    /// The analysis cache: the segmenters analysis runs.
    analysis: AnalysisContext,
    /// The analysis's working memory.
    analysis_scratch: AnalysisScratch,
    /// The shaping caches, which the breaker's reshapes share.
    ///
    /// They hold the shape plans, each font's `text-spacing-trim` answers,
    /// the fonts set down the line, and the one buffer every shaping call
    /// fills.
    shaping: ShapeContext,
    /// Measurement's cursors for walking intrinsic ruby pieces.
    measure_scratch: MeasureScratch,
    /// The breaker's working memory, one part per phase.
    ///
    /// It holds a line whose height is worked out box by box, the boxes open
    /// across a line's start, and a line's annotation levels.
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

    /// Creates a context as [`new`](Self::new) does, which parses each font
    /// through `shared`.
    ///
    /// Contexts created with clones of one [`SharedFonts`] parse a font once
    /// between them, so a new context lays out its first text sooner. Layout
    /// results are the same as with [`new`](Self::new).
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
    /// Existing layouts retain their fonts until rebuilt. Cached family
    /// lists are discarded unless `fonts` holds the same layer snapshots.
    /// Cached data of fonts that `fonts` no longer contains is discarded,
    /// which releases their bytes once no layout uses them. Cached data of
    /// the remaining fonts is retained for reuse.
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
    /// Existing layouts retain build-time settings until rebuilt, including
    /// word-spacing rules. Settings applied during line layout, such as tab
    /// justification, take effect at the next
    /// [`break_lines`](crate::Layout::break_lines) call.
    ///
    /// Configuration changes do not invalidate caches. Font line metrics
    /// are derived from unscaled metrics during each build.
    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// Returns the layout configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Sets how many entries each cache keeps.
    ///
    /// Caches above a new limit drop their least recently used entries
    /// immediately. Existing layouts remain valid.
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
    /// Keeps the collection, the configuration and the cache limits.
    /// Existing layouts remain valid. Later builds and line breaking fill
    /// the caches again and give the same results.
    pub fn clear_caches(&mut self) {
        self.fonts.clear(&self.limits);
        self.shaping = ShapeContext::new(&self.limits);
        self.release_scratch();
        self.scratch_sizes = ScratchSizes::new();
    }

    /// Returns estimated heap usage by cache, in bytes.
    ///
    /// Includes table capacities, cached values and retained working memory.
    /// Font bytes shared with the collection are reported separately in
    /// [`ContextHeap::font_data`]. Excludes the collection, [`SharedFonts`],
    /// which reports its own usage, and allocations within read-fonts,
    /// harfrust and fontwich values that do not report their size: parsed
    /// fonts, shape plans, the shaping buffer, and family handles.
    pub fn heap_bytes(&self) -> ContextHeap {
        let Self {
            config: _,
            limits: _,
            fonts,
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
            scratch: analysis_scratch.heap_bytes()
                + font_scratch.heap_bytes()
                + shaping_scratch
                + measure_scratch.heap_bytes()
                + break_scratch.heap_bytes()
                + place_scratch.heap_bytes(),
            font_data: fonts.face_bytes(),
        }
    }

    /// Readies the context for a build of content `size` long.
    ///
    /// Trims every cache to its limit, as no call holds an id into one now.
    /// Releases the scratch where the calls before it grew it far past what
    /// the last calls need. `size` is the content's text bytes and items,
    /// which every scratch grows with.
    pub(crate) fn start_build(&mut self, size: usize) {
        self.fonts.trim();
        self.start_break(size);
    }

    /// Readies the context for a break of content `size` long.
    ///
    /// As [`start_build`](Self::start_build), but leaves the font caches,
    /// which only a build fills.
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

    /// Returns what font selection runs with: its caches and its scratch.
    pub(crate) fn selecting_fonts(&mut self) -> (&mut FontContext, &mut FontScratch) {
        (&mut self.fonts, &mut self.font_scratch)
    }

    /// Returns measurement's shaping caches and reusable level cursors.
    pub(crate) fn measuring(&mut self) -> (&mut ShapeContext, &mut MeasureScratch) {
        (&mut self.shaping, &mut self.measure_scratch)
    }

    /// Returns the shaping caches, also used for generated text and tests.
    pub(crate) fn shaping(&mut self) -> &mut ShapeContext {
        &mut self.shaping
    }

    /// Returns what the breaker runs with: the shaping caches its reshapes share, and its scratch.
    pub(crate) fn breaking(&mut self) -> (&mut ShapeContext, &mut BreakScratch) {
        (&mut self.shaping, &mut self.break_scratch)
    }

    /// Returns the transient line facts breaking made, and line layout's scratch.
    pub(crate) fn placing(&mut self) -> (&mut LinePlacements, &mut PlaceScratch) {
        (&mut self.break_scratch.placements, &mut self.place_scratch)
    }

    /// Returns font selection's caches, for tests to inspect.
    #[cfg(test)]
    pub(crate) fn font_context(&self) -> &FontContext {
        &self.fonts
    }
}

/// How many scratch sizes [`ScratchSizes`] remembers.
const SCRATCH_CALLS: usize = 8;

/// How many times the largest of the last calls the scratch may have grown
/// for before it is released.
const SCRATCH_SLACK: usize = 4;

/// The call size below which the scratch is never released, so a context
/// of small calls never gives back and takes again.
///
/// It is a size, not bytes, since the shaping buffer, which grows with the
/// longest run, does not report its bytes.
const SCRATCH_FLOOR: usize = 16 * 1024;

/// The sizes of the last calls, and the largest the scratch grew for.
///
/// The scratch keeps its capacity, so one huge paragraph would keep it huge.
/// Once the last [`SCRATCH_CALLS`] calls are each under a
/// [`SCRATCH_SLACK`]th of what it grew for, it is released. Steady calls of
/// one size never release it, so a warm relayout allocates nothing.
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

    /// Notes a call of `size`, and returns whether the scratch is far
    /// bigger than the last calls need.
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
/// The defaults suffice for content with a few dozen distinct styles.
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
    pub family_lists: usize,
    /// Fallback lists: the fonts to try for one family list, script,
    /// language, weight, width and style. Default 128.
    pub fallback_lists: usize,
    /// Font instances: a font at one set of variation coordinates, with
    /// its features, synthesis and `@font-face` metric overrides. Default 64.
    ///
    /// Each distinct `font-variation-settings`, and each font size of a font
    /// with an `opsz` axis under `font-optical-sizing: auto`, makes one. The
    /// same limit applies to per-font shaping data: `text-spacing-trim`
    /// data, vertical metrics at each size, and the features a font supports
    /// and covers.
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
/// memory retained for later layouts. Caches are released when the context
/// is dropped or by [`Context::clear_caches`].
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
    /// Empty between calls. Capacity is retained until a series of much
    /// smaller calls releases it.
    pub scratch: usize,
    /// The bytes of the font files the font caches hold.
    ///
    /// The collection and the layouts set in these fonts share the bytes, so
    /// they stay in memory while any of them holds the font. Not included in
    /// [`total`](Self::total).
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
