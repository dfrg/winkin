//! Font selection: picks a font for every cluster and keeps the font data
//! later stages read.
//!
//! In: [`FontInput`], the content and its analysis. Out: [`Fonts`].
//! Start at: [`select_fonts`], then `select::select_runs`.
//!
//! - `select` chooses a font for each cluster and writes the font runs.
//!   Its `pick` tries a candidate list on a grapheme, `choice` makes a
//!   candidate's used fonts, `kept` keeps a candidate across clusters
//!   without asking the list, and `generated` chooses the
//!   fonts of a hyphen and an ellipsis.
//! - `used` interns the used fonts and works out the size each is used at.
//! - `coverage` says whether a font covers a cluster, and which
//!   presentation a cluster asks for.
//! - `features` works out the OpenType features and synthesized capitals.
//! - `positions` reads what a font's `sups` or `subs` covers.
//! - `context`, `lists` and `instance` hold the caches kept across layouts.
//! - `shared` holds the faces several contexts share.
//! - `metrics` computes a used font's line metrics.
//! - `resolution` holds a font request's used size and primary font.
//!
//! [`Fonts`] names nothing of the context's, so later stages read it without
//! the caches. Font runs tile the clusters, and the first line has its own
//! runs where `::first-line` changes a font.

mod context;
mod coverage;
mod features;
mod instance;
mod lists;
mod metrics;
mod positions;
mod resolution;
mod select;
mod shared;
#[cfg(test)]
mod tests;
mod used;

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use fontwich::{FaceId, Family, FontBytes, FontKey};
use hashbrown::HashTable;
use parlance::{FontFeature, Language};
use read_fonts::model::Font;

use crate::config::{LineMetricsSource, PlatformFontVariations, PositionSynthesis};
use crate::data::{
    Id, Run, Runs, Table, define_id, heap_bytes, index_to_u32, sort_by_key, u32_to_index,
};
use crate::stages::analysis::{Analysis, ClusterId};
use crate::stages::content::{
    Content, FontRequest, FontRequestId, HyphenStringId, HyphenStrings, Lists, NodeId, TextFactsId,
};
use crate::style::FirstLine;
use crate::style::FirstLineState;
use crate::style::FirstLineVariant;
use crate::style::{FontSizeAdjust, FontVariantCaps};
use crate::unit;
use crate::unit::{LayoutUnit, TextUnit};
use crate::work;
use context::FontCaches;
use features::{CapsPlan, OfferedFeatures};
use instance::{
    FaceOverrides, InstanceId, InstanceRequest, Instances, variations_left_to_matching,
};
use metrics::UnscaledMetrics;
use select::{RunScratch, Selecting};

/// The `@font-face` faces still downloading that some cluster wants, each
/// once: no list until a face is wanted. Selection's pickers add to it.
type WantedFaces = Option<Box<Vec<FaceId>>>;

pub(crate) use context::FontContext;
pub(crate) use lists::NamedListId;
// Private to font selection: no layout holds one of the context's
// instances, so nothing after this stage names one.
use instance::Instance;
pub(crate) use metrics::{FontLineMetrics, LineBaseline};
pub(super) use resolution::FontResolution;
pub use shared::SharedFonts;
use used::{InitialLetterGrid, UsedFontInterner, adjusted_size, used_size};

/// What font selection reads: the content, its analysis and some config.
///
/// The config choices travel with the input, so a config change reaches a
/// layout when it is next built.
pub(crate) struct FontInput<'a> {
    pub(crate) content: &'a Content,
    /// The build's feature and variation settings and hyphen strings, which
    /// the content's facts name.
    pub(crate) lists: &'a Lists,
    /// The nodes whose text holds a soft hyphen a line may break after, in
    /// text order, each with the first such cluster, as analysis found them.
    pub(crate) hyphen_nodes: &'a [(NodeId, ClusterId)],
    pub(crate) analysis: &'a Analysis,
    /// The language text whose style names none chooses its fonts in
    /// (`Config::default_language`).
    pub(crate) default_language: Language,
    /// Which of a font's metrics give a line's ascent, descent and line gap
    /// (`Config::line_metrics`).
    pub(crate) line_metrics: LineMetricsSource,
    /// Which variations a platform font takes
    /// (`Config::platform_font_variations`).
    pub(crate) platform_font_variations: PlatformFontVariations,
    /// Whether `font-variant-position` synthesizes what the font lacks
    /// (`Config::position_synthesis`).
    pub(crate) position_synthesis: PositionSynthesis,
}

/// Font selection's working memory.
///
/// It holds nothing between calls and keeps its capacity.
pub(crate) struct FontScratch {
    /// The layout's used fonts by what they are: the instance each was made
    /// from, which only this build keeps, and its size and synthesis.
    used_index: HashTable<(u64, Option<InstanceId>, UsedFontId)>,
    /// The text facts of the text holding a soft hyphen a line may break
    /// after, each once, in order: those whose hyphen a font is chosen for.
    hyphenated: Vec<TextFactsId>,
    /// The generated texts' fonts, which measurement shapes the generated
    /// texts in as the build finishes. Nothing reads them after the build.
    generated: GeneratedFonts,
    /// The advance of U+0020 in the font a space falls back to among the
    /// block's listed fonts, where its primary font maps none: what a
    /// `tab-size` in spaces counts there, as Chrome sizes such a tab.
    /// Measurement reads it as the build finishes; nothing reads it after.
    block_space: Option<TextUnit>,
    /// The run loop's.
    runs: RunScratch,
}

impl FontScratch {
    /// Empty scratch, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            used_index: HashTable::new(),
            hyphenated: Vec::new(),
            generated: GeneratedFonts::new(),
            block_space: None,
            runs: RunScratch::new(),
        }
    }

    /// Forgets everything, keeping the capacity.
    fn clear(&mut self) {
        self.used_index.clear();
        self.hyphenated.clear();
        self.generated.clear();
        self.block_space = None;
        self.runs.clear();
    }

    /// The fonts of the generated texts the last build may need.
    pub(crate) fn generated(&self) -> &GeneratedFonts {
        &self.generated
    }

    /// Returns the advance of the space a `tab-size` in spaces counts in the
    /// last build, where the block's primary font maps no U+0020: that of
    /// the first of the block's listed fonts that maps one, at the block's
    /// used size.
    ///
    /// `None` where the primary maps a space, whose own advance counts, or
    /// where no listed font maps one.
    pub(crate) fn block_space(&self) -> Option<TextUnit> {
        self.block_space
    }
}

heap_bytes! {
    FontScratch { used_index, hyphenated, generated, runs; block_space }
}

/// Selects fonts for the content of `input` into `out`.
///
/// First resolves each font request, then selects the text's runs, then the
/// first line's where needed, then the generated texts' fonts. Uses the
/// context's caches in `cx`, and clears and fills `out` whole.
///
/// Returns how many used fonts were replaced because the table was full.
pub(crate) fn select_fonts(
    input: &FontInput<'_>,
    cx: &mut FontContext,
    scratch: &mut FontScratch,
    out: &mut Fonts,
) -> usize {
    let FontInput {
        content,
        analysis,
        line_metrics,
        ..
    } = *input;
    out.clear();
    scratch.clear();
    // The language a style naming none chooses its fonts in, kept for the
    // shaping stage and the breaker to shape in.
    let default_language = DefaultLanguage::new(input.default_language);
    out.default_language = default_language;
    hyphenated_texts(input, &mut scratch.hyphenated);
    let ellipsis_texts = ellipsis_texts(content);
    let FontScratch {
        used_index,
        hyphenated,
        generated,
        block_space,
        runs: run_scratch,
    } = scratch;
    let Fonts {
        used,
        runs,
        resolutions,
        wanted,
        ..
    } = out;
    let mut selecting = Selecting {
        cx,
        scratch: run_scratch,
        used: UsedFontInterner {
            used,
            index: used_index,
            source: line_metrics,
            baseline: LineBaseline::from_content(content),
            replaced: 0,
        },
        wanted,
    };
    let sel = &mut selecting;
    *block_space = resolve_requests(input, default_language, sel, resolutions);
    let source = content.text(FirstLineVariant::Standard).cursor();
    let text_in = select::Input {
        stage: input,
        variant: FirstLineVariant::Standard,
        default_language,
        resolutions,
        source: &source,
    };
    select::select_runs(
        &text_in,
        ClusterId::new(0)..analysis.clusters.end_id(),
        sel,
        runs.text_mut(),
    );
    if let Some(reach) = analysis
        .first_line_reach(content)
        .filter(|_| content.first_line_reshapes())
    {
        select_first_line(&text_in, reach, sel, runs.first_line_mut());
    }
    if !hyphenated.is_empty() || ellipsis_texts.is_some() {
        select_generated_texts(&text_in, hyphenated, ellipsis_texts, sel, generated);
    }
    selecting.used.replaced
}

/// Returns the block's text facts, standard and first-line, where a line may
/// be cut for an ellipsis: those an ellipsis is set in.
fn ellipsis_texts(content: &Content) -> Option<(TextFactsId, TextFactsId)> {
    let block = NodeId::BLOCK;
    let nodes = &content.nodes;
    (content.block.may_cut_lines() && nodes.kind(block).is_some()).then(|| {
        (
            nodes.text_facts(block, FirstLineVariant::Standard),
            nodes.text_facts(block, FirstLineVariant::FirstLine),
        )
    })
}

/// Resolves each font request, in request order, into `resolutions`: its
/// used size and its primary font.
///
/// Every request is resolved before any run, so a run's fallback when the
/// used fonts fill is always there.
///
/// Returns the advance of the space the block counts a `tab-size` in, where
/// its primary font maps none (see [`FontScratch::block_space`]).
fn resolve_requests(
    input: &FontInput<'_>,
    default_language: DefaultLanguage,
    sel: &mut Selecting<'_>,
    resolutions: &mut Table<FontRequestId, FontResolution>,
) -> Option<TextUnit> {
    let Selecting {
        cx, used: interner, ..
    } = sel;
    let content = input.content;
    let facts = &content.facts;
    // The text an initial letter is sized against: the block's own, as its
    // first line has it. The builder lowers it before any node's, so its
    // request is resolved first.
    let paragraph_text = content
        .nodes
        .text_facts(NodeId::BLOCK, FirstLineVariant::FirstLine);
    let paragraph = facts.text_request(paragraph_text);
    // The grid an initial letter is sized to, made from the paragraph's
    // primary font once it is sized.
    let mut grid: Option<InitialLetterGrid> = None;
    // The block's own text, whose space a `tab-size` in spaces counts.
    let block = facts.text_request(
        content
            .nodes
            .text_facts(NodeId::BLOCK, FirstLineVariant::Standard),
    );
    let mut block_space = None;
    resolutions.reserve(facts.request_count());
    for (id, request) in facts.requests() {
        let language = default_language.fonts_language(content.languages.get(request.language));
        let primary = cx.primary_font(request.font.families, request, language);
        let space_font = if id == block && primary.as_ref().is_some_and(maps_no_space) {
            cx.space_font(request.font.families, request, language)
        } else {
            None
        };
        let mut caches = cx.font_caches();
        let mut resolution = FontResolution::new(request.font.computed_size(), request.upright);
        let instance = primary.and_then(|(family, index)| {
            let font = family.fonts().get(index)?;
            size_primary(
                input,
                request,
                &family,
                font,
                &mut caches,
                grid.as_ref(),
                &mut resolution,
            )
        });
        let (instance, px) = match instance {
            Some((id, px)) => (Some(id), px),
            None => (None, resolution.base),
        };
        if let Some((family, index)) = space_font {
            block_space = space_advance(input, request, &family, index, &resolution, &mut caches);
        }
        // Where even a primary does not fit, the last used font stands in:
        // the table is full, so there is one.
        let last = interner.used.table.last_id().unwrap_or(UsedFontId::new(0));
        resolution.primary = interner.intern_or(
            instance,
            used_size(px),
            UsedSynthesis::None,
            caches.instances,
            last,
        );
        if id == paragraph {
            grid = Some(initial_letter_grid(
                input,
                paragraph_text,
                (instance, resolution.primary, px),
                interner.used,
                caches.instances,
            ));
        }
        resolutions.push_bounded(resolution, "one entry per request, which an id names");
    }
    block_space
}

/// Whether the font `primary` names, as its family and its index there,
/// maps no U+0020.
fn maps_no_space(primary: &(Family, usize)) -> bool {
    let (family, index) = primary;
    family
        .fonts()
        .get(*index)
        .is_some_and(|font| !font.charset().contains(' '))
}

/// Returns the advance of U+0020 in font `index` of `family` at the size
/// `resolution` uses its fonts at: the space a block whose primary font maps
/// none counts a `tab-size` in.
///
/// `None` where the font's instance cannot be made.
#[cold]
fn space_advance(
    input: &FontInput<'_>,
    request: &FontRequest,
    family: &Family,
    index: usize,
    resolution: &FontResolution,
    caches: &mut FontCaches<'_>,
) -> Option<TextUnit> {
    let font = family.fonts().get(index)?;
    let table = input.lists;
    let overrides = FaceOverrides::new(font);
    let px = resolution.font_px(overrides);
    let instance = InstanceRequest {
        font_request: request,
        variations: table.variation_settings().get(request.font.variations),
        settings: table.feature_settings().get(request.font.features),
        upright: request.upright,
        caps: FontVariantCaps::Normal,
        position: request.font.variant_position,
        size: px,
        overrides,
        matching_only: variations_left_to_matching(family, input.platform_font_variations),
    };
    let made = caches.instances.find_or_make(family, font, &instance)?;
    Some(caches.instances.get(made)?.unscaled().space(px))
}

/// Sizes `request`'s primary font `font`, of `family`, into `resolution`,
/// and returns its instance at that size with the size in pixels.
///
/// `grid` is the grid an initial letter is sized to, once the paragraph's
/// primary is sized. `None` where the instance cannot be made.
fn size_primary(
    input: &FontInput<'_>,
    request: &FontRequest,
    family: &Family,
    font: &fontwich::Font,
    caches: &mut FontCaches<'_>,
    grid: Option<&InitialLetterGrid>,
    resolution: &mut FontResolution,
) -> Option<(InstanceId, f32)> {
    let table = input.lists;
    let line_metrics = input.line_metrics;
    let upright = request.upright;
    let computed = request.font.computed_size();
    // The primary's capitals are Latin's, the script it is chosen for; its
    // text is set as a run's would be.
    let offered = if request.font.variant_caps == FontVariantCaps::Normal {
        OfferedFeatures::NONE
    } else {
        let script = features::opentype_script(context::PRIMARY_SCRIPT);
        caches
            .offers
            .find_or_read(family, font, script, caches.instances)
    };
    let plan = CapsPlan::new(
        request.font.variant_caps,
        offered,
        request.font.synthesis.small_caps,
    );
    // The position's feature as the request asks: the primary measures the
    // text and draws what no font covers, and a run the feature does not
    // cover is synthesized in the font it chose.
    let variations = table.variation_settings().get(request.font.variations);
    let matching_only = variations_left_to_matching(family, input.platform_font_variations);
    let overrides = FaceOverrides::new(font);
    let instance_request = |size: f32| InstanceRequest {
        font_request: request,
        variations,
        settings: table.feature_settings().get(request.font.features),
        upright,
        caps: plan.same().caps(),
        position: request.font.variant_position,
        size,
        overrides,
        matching_only,
    };
    let adjusts = matches!(request.font.size_adjust, FontSizeAdjust::Hold { .. });
    let letter_grid = grid.filter(|_| request.initial_letter.is_set());
    // Both sizings measure the primary at the computed size.
    if (adjusts || letter_grid.is_some())
        && let Some(measured) =
            caches
                .instances
                .find_or_make(family, font, &instance_request(computed))
        && let Some(at) = caches.instances.get(measured)
    {
        let unscaled = at.unscaled();
        // `font-size-adjust` sets the size every font of the request is used
        // at, the primary's own among them (Blink's
        // `FontBuilder::UpdateAdjustedSize`): one used size a request, not
        // one a font.
        if adjusts
            && let Some(size) = adjusted_size(request, computed, unscaled, &overrides, line_metrics)
        {
            resolution.adjust_to(size);
        }
        // An initial letter is set at the size that spans the lines it is
        // sized to, whatever its own says (Chrome's `ComputeInitialLetterFont`).
        // Only the block's initial letter's request sets one.
        if let Some(grid) = letter_grid
            && let Some(size) = grid.letter_size(request, computed, unscaled, line_metrics)
        {
            resolution.adjust_to(size);
        }
    }
    let px = resolution.font_px(overrides);
    let made = caches
        .instances
        .find_or_make(family, font, &instance_request(px))?;
    Some((made, px))
}

/// Returns the grid an initial letter is sized to: the paragraph text's line
/// height and its primary font's metrics and size.
///
/// `paragraph` is that primary's instance, used font and size in pixels.
fn initial_letter_grid(
    input: &FontInput<'_>,
    paragraph_text: TextFactsId,
    paragraph: (Option<InstanceId>, UsedFontId, f32),
    used: &UsedFonts,
    instances: &Instances,
) -> InitialLetterGrid {
    let (made, primary, px) = paragraph;
    let normal = used
        .get(primary)
        .map_or(LayoutUnit::ZERO, |used| used.metrics.normal_line_height());
    InitialLetterGrid {
        line_height: input
            .content
            .facts
            .text(paragraph_text)
            .line_height
            .px(normal),
        unscaled: made
            .and_then(|made| instances.get(made))
            .map_or_else(UnscaledMetrics::default, |made| *made.unscaled()),
        px: unit::floor_to_hundredth(px),
    }
}

/// Selects the first line's runs over `reach`, where `::first-line` changes
/// a font or a transform.
///
/// This is the text's loop over the first paragraph, in the facts
/// `::first-line` gives each node and the first line's characters. The
/// first-line requests' primary fonts are already resolved with the rest.
fn select_first_line(
    text_in: &select::Input<'_>,
    reach: Range<ClusterId>,
    sel: &mut Selecting<'_>,
    runs: &mut FontRuns,
) {
    let source = text_in
        .stage
        .content
        .text(FirstLineVariant::FirstLine)
        .cursor();
    let first_line = select::Input {
        variant: FirstLineVariant::FirstLine,
        source: &source,
        ..*text_in
    };
    select::select_runs(&first_line, reach, sel, runs);
}

/// Selects the string and fonts of each generated text into `out`, in facts
/// order.
///
/// A hyphen is set for each text facts in `hyphenated`, the facts of text
/// holding a soft hyphen a line may break after. An ellipsis is set for the
/// block's standard and first-line facts in `ellipsis`, where a line may be
/// cut.
fn select_generated_texts(
    text_in: &select::Input<'_>,
    hyphenated: &[TextFactsId],
    ellipsis: Option<(TextFactsId, TextFactsId)>,
    sel: &mut Selecting<'_>,
    out: &mut GeneratedFonts,
) {
    let mut hyphenated = hyphenated.iter().copied().peekable();
    for id in text_in.stage.content.facts.text_ids() {
        work::step();
        let hyphen = hyphenated.next_if_eq(&id).is_some();
        let cut = ellipsis.is_some_and(|(own, first)| id == own || id == first);
        if hyphen || cut {
            let kinds = [
                hyphen.then_some(Generated::Hyphen),
                cut.then_some(Generated::Ellipsis),
            ];
            select::select_generated(text_in, id, kinds.into_iter().flatten(), sel, out);
        }
    }
}

/// Collects into `out` the text facts of text holding a soft hyphen a line
/// may break after, each once and in order.
///
/// These are the facts a hyphen is set in. The first-line facts are added
/// too where a soft hyphen falls in the first line's reach, so its hyphen
/// takes the first-line style as in Chrome. Measured in Chrome 153, a hyphen
/// on a 40px `::first-line` is 40px wide. The analysis lists the nodes
/// holding one, each with its first, which falls in the reach where any
/// does, since the reach starts at the text's start.
fn hyphenated_texts(input: &FontInput<'_>, out: &mut Vec<TextFactsId>) {
    out.clear();
    let content = input.content;
    let reach = input.analysis.first_line_reach(content);
    let nodes = &content.nodes;
    for &(node, cluster) in input.hyphen_nodes {
        work::step();
        out.push(nodes.text_facts(node, FirstLineVariant::Standard));
        if reach.as_ref().is_some_and(|reach| reach.contains(&cluster)) {
            out.push(nodes.text_facts(node, FirstLineVariant::FirstLine));
        }
    }
    sort_by_key(out, |&text| text);
    out.dedup();
}

/// A normalized OpenType variation coordinate stored as 2.14 fixed-point bits.
///
/// Represents values from -1 to 1. Uses `i16` for interoperability with
/// renderers such as vello, peniko, swash and parley.
pub type NormalizedCoord = i16;

define_id! {
    /// Names a used font in a layout's table of [`UsedFont`]s.
    ///
    /// Sixteen bits. A layout that needs more distinct used fonts gets its
    /// style's primary font for the rest, and reports it.
    pub(crate) struct UsedFontId(u16);
}

define_id! {
    /// Names a run of clusters set in one used font, in a layout's table of
    /// [`FontRun`]s.
    pub(crate) struct FontRunId(u32);
}

define_id! {
    /// Names a run of a generated text set in one used font, in
    /// [`GeneratedFonts`]' table of every generated text's runs.
    pub(crate) struct GeneratedRunId(u32);
}

/// How a used font's text is fed to the shaper: as written, or case-mapped.
///
/// A case-mapped character is labelled with the byte of the one it came
/// from. So the text keeps the author's case, and an uppercased `ß` is two
/// glyphs in one cluster.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum CaseMap {
    /// As written.
    #[default]
    Keep,
    /// Uppercased: synthesized small capitals.
    Upper,
    /// Lowercased: unicase from a font's small capitals, whose capitals are
    /// fed as lowercase to take `smcp`.
    Lower,
}

/// The language that text with no style language picks fonts and shapes in.
///
/// It is [`default_language`](crate::config::Config::default_language) as of
/// the build that chose the fonts. Shaping and the breaker's reshaping read
/// it from here, so they use the fonts' language even if the config changed.
///
/// Chrome uses its default locale for every font and shaping question
/// (`FontDescription::LocaleOrDefault`):
/// - the script generic families resolve for (`GetScript`, read by
///   `FontSelector::FamilyNameFromSettings`);
/// - the locale fallback and Han's variant;
/// - HarfBuzz's language and the case of synthesized small capitals;
/// - the script `HanKerning` probes.
///
/// Line breaking, hyphenation, `text-transform`, `text-autospace` and
/// `text-emphasis-position` do not take it: they read the style's own
/// locale, or none.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct DefaultLanguage(Language);

impl DefaultLanguage {
    /// Returns a default of `language`. `und` means none, which leaves each
    /// run to its own script.
    const fn new(language: Language) -> Self {
        Self(language)
    }

    /// Returns the language text picks fonts and shapes in: `own`, or the
    /// default where `own` is `und`.
    pub(super) fn fonts_language(self, own: Language) -> Language {
        if own == Language::UND { self.0 } else { own }
    }
}

/// How a used font draws differently from its instance at its size.
///
/// It covers synthesized small capitals, unicase from small capitals, and a
/// synthesized superscript or subscript. The line metrics stay those of the
/// used font's size, so a synthesized superscript does not make its line
/// taller. Chrome's smaller small-capitals font does not either.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum UsedSynthesis {
    /// The glyphs are the used font's, at its size, on the baseline.
    #[default]
    None,
    /// The text is fed to the shaper in `case`, and the glyphs are set at
    /// `glyph_size` and moved up by `raise`.
    Synthesized {
        /// How the text is fed.
        case: CaseMap,
        /// The size the glyphs are set at.
        glyph_size: TextUnit,
        /// How far above the baseline they sit: negative below.
        raise: LayoutUnit,
    },
}

impl UsedSynthesis {
    /// The size glyphs are set at, where the used font's size is `size`.
    fn glyph_size(self, size: TextUnit) -> TextUnit {
        match self {
            Self::None => size,
            Self::Synthesized { glyph_size, .. } => glyph_size,
        }
    }

    /// How far above the baseline the glyphs are drawn.
    pub(crate) fn raise(self) -> LayoutUnit {
        match self {
            Self::None => LayoutUnit::ZERO,
            Self::Synthesized { raise, .. } => raise,
        }
    }

    /// How the text is fed to the shaper.
    pub(super) fn case(self) -> CaseMap {
        match self {
            Self::None => CaseMap::Keep,
            Self::Synthesized { case, .. } => case,
        }
    }
}

/// A font as a layout uses it: an instance at a size, with its line metrics.
///
/// A run shapes in one, and a glyph run hands one to a renderer. One
/// instance at two sizes is two used fonts, so it makes two runs.
#[derive(Clone, Debug)]
pub(crate) struct UsedFont {
    /// The instance it is set in, shared with the context and every layout
    /// that uses it.
    ///
    /// `None` where there is no font at all, such as an empty collection or
    /// a font whose bytes are gone. Nothing is drawn then: the clusters keep
    /// their places with no glyphs.
    pub(crate) instance: Option<Arc<UsedInstance>>,
    /// The used size, after `font-size-adjust` and `@font-face`
    /// `size-adjust`. Its line metrics are at this size.
    pub(crate) size: TextUnit,
    /// Small capitals and positions synthesized here.
    pub(crate) synthesis: UsedSynthesis,
    /// The line metrics at `size`, read as the context's [`Config`](crate::config::Config) says.
    pub(crate) metrics: FontLineMetrics,
}

/// A font instance as shaping and drawing read it.
///
/// Font selection writes one whole when it makes the instance, and never
/// again. The context's instance and every used font in every layout share
/// it through an `Arc`.
///
/// It holds everything shaping and drawing need. Nothing here names the
/// context, so the context may drop anything between calls. A layout is read
/// and broken again without it, so one thread can paint a layout while the
/// context builds the next.
pub(crate) struct UsedInstance {
    /// The font's bytes: a count taken on their `Arc`, never a copy of the
    /// file, of which the font is `index`.
    pub(crate) bytes: FontBytes,
    /// Which font in them: its index in a collection file, 0 otherwise.
    pub(crate) index: u32,
    /// The font held whole at the instance's coordinates, sharing with
    /// every other instance of it what the coordinates do not change.
    pub(super) font: Font,
    /// Its normalized coordinates as 2.14 bits, as a renderer takes them:
    /// none at the font's default.
    pub(crate) coords: Box<[NormalizedCoord]>,
    /// The OpenType features it shapes with, in order: the list the
    /// instance was made with, which is its key's.
    pub(super) features: Box<[FontFeature]>,
    /// Whether its glyphs are emboldened, which harfrust does not apply.
    pub(crate) embolden: bool,
    /// The angle its glyphs are skewed by, in CSS's clockwise degrees,
    /// which harfrust does not apply.
    pub(crate) skew: Option<f32>,
}

impl UsedInstance {
    /// fontwich's key for the font: its bytes' id and its index.
    pub(crate) fn key(&self) -> FontKey {
        FontKey {
            source: self.bytes.id(),
            index: self.index,
        }
    }
}

impl fmt::Debug for UsedInstance {
    // Not the font held whole, which says nothing the rest does not.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UsedInstance")
            .field("font", &self.key())
            .field("coords", &self.coords)
            .field("features", &self.features)
            .field("embolden", &self.embolden)
            .field("skew", &self.skew)
            .finish()
    }
}

impl UsedFont {
    /// The size its glyphs are shaped and drawn at: its used size, or the
    /// smaller one synthesized small capitals and positions are set at.
    #[inline]
    pub(crate) fn glyph_size(&self) -> TextUnit {
        self.synthesis.glyph_size(self.size)
    }
}

/// A glyph's ink bounds from its origin, in whole pixels, as Skia bounds a
/// glyph.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct GlyphInk {
    /// Its left edge from the glyph's origin: negative where the ink
    /// reaches back past it.
    pub(super) left: LayoutUnit,
    /// Its right edge from the glyph's origin.
    pub(super) right: LayoutUnit,
    /// How far it reaches over the baseline.
    pub(super) over: LayoutUnit,
    /// How far it reaches under the baseline.
    pub(super) under: LayoutUnit,
}

/// A layout's used fonts, by id.
///
/// Each shares its instance's record, so it is read with the layout alone.
pub(crate) struct UsedFonts {
    table: Table<UsedFontId, UsedFont>,
}

impl UsedFonts {
    /// None, allocating nothing.
    const fn new() -> Self {
        Self {
            table: Table::new(),
        }
    }

    /// Removes every used font, keeping the allocation.
    fn clear(&mut self) {
        self.table.clear();
    }

    /// The used font `id` names, or `None` for one the layout does not have.
    pub(crate) fn get(&self, id: UsedFontId) -> Option<&UsedFont> {
        self.table.get(id)
    }

    /// How many there are.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether there are none. Only cleared data has none, since every
    /// style has a primary font.
    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Every used font with its id, in the order they were made.
    #[cfg(test)]
    fn iter(&self) -> impl DoubleEndedIterator<Item = (UsedFontId, &UsedFont)> + ExactSizeIterator {
        self.table.iter()
    }

    /// Stores a used font in `instance` (or none) at `size`, drawn as
    /// `synthesis` says. `None`, storing nothing, where the table is full.
    ///
    /// Its line metrics are read as `source` says, about the block's
    /// `baseline`. It shares the instance's record through its `Arc`, so a
    /// warm rebuild allocates nothing.
    fn push(
        &mut self,
        instance: Option<&Instance>,
        size: TextUnit,
        synthesis: UsedSynthesis,
        source: LineMetricsSource,
        baseline: LineBaseline,
    ) -> Option<UsedFontId> {
        if self.table.remaining() == 0 {
            return None;
        }
        let size_px = size.to_px();
        let (metrics, drawn) = match instance {
            Some(instance) => {
                let metrics = FontLineMetrics::new(
                    instance.unscaled(),
                    &instance.overrides(),
                    size_px,
                    source,
                    baseline,
                );
                (metrics, Some(instance.used().clone()))
            }
            None => (FontLineMetrics::from_size(size_px, baseline), None),
        };
        self.table.push(UsedFont {
            instance: drawn,
            size,
            synthesis,
            metrics,
        })
    }
}

/// A run of clusters set in one used font. 8 bytes.
///
/// Runs tile the clusters: each ends where the next starts, the last where
/// the clusters do.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct FontRun {
    /// Its first cluster.
    pub(super) start: ClusterId,
    /// The font its clusters are set in. A cluster no font covers is set in
    /// its style's primary font, which draws `.notdef` for it as Chrome
    /// draws the missing-glyph box; where the primary has no instance,
    /// nothing is drawn.
    pub(super) font: UsedFontId,
}

impl Run for FontRun {
    type Position = ClusterId;

    #[inline]
    fn start(&self) -> ClusterId {
        self.start
    }
}

/// The font runs, in text order: the text's, or the first paragraph's
/// under its `::first-line` styles.
///
/// They tile the clusters they were selected for, so a run is found by its
/// first cluster. A lookup of one cluster searches ([`Runs::containing`]). A
/// forward walk uses a cursor ([`Runs::step_to`]) and passes each run once.
pub(super) type FontRuns = Runs<FontRunId, FontRun>;

impl FirstLineState for FontRuns {
    fn clear(&mut self) {
        Runs::clear(self);
    }

    /// Whether there are none: where there are no clusters, and for the
    /// first line where it was not selected again.
    fn is_empty(&self) -> bool {
        Runs::is_empty(self)
    }
}

impl Default for FontRuns {
    fn default() -> Self {
        Self::new()
    }
}

/// The kind of text inserted by layout, returned by [`TextRun::generated`](crate::TextRun::generated).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Generated {
    /// The hyphen drawn where a line breaks at a soft hyphen.
    Hyphen,
    /// The ellipsis drawn where a line is cut.
    Ellipsis,
}

/// The string a generated text draws, chosen by what the style's primary
/// font maps.
///
/// The hyphen is chosen as Blink's `ComputedStyle::HyphenString` does, and
/// the ellipsis as its `LineTruncator::ComputeEllipsisText` does.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum GeneratedString {
    /// U+2010 HYPHEN, for `hyphenate-character: auto` where the primary
    /// font maps it.
    Hyphen,
    /// U+002D HYPHEN-MINUS, for `auto` where it does not.
    HyphenMinus,
    /// The style's own `hyphenate-character`, which may be empty: a line
    /// broken there is still hyphenated, and pays nothing for it.
    HyphenateCharacter(HyphenStringId),
    /// U+2026 HORIZONTAL ELLIPSIS, where the line style's primary font maps
    /// it.
    Ellipsis,
    /// Three full stops, where it does not.
    FullStops,
}

impl GeneratedString {
    /// Its text, the styles' own `hyphenate-character` strings being
    /// `strings`.
    pub(super) fn text(self, strings: &HyphenStrings) -> &str {
        match self {
            Self::Hyphen => "\u{2010}",
            Self::HyphenMinus => "-",
            Self::HyphenateCharacter(id) => strings.get(id),
            Self::Ellipsis => "\u{2026}",
            Self::FullStops => "...",
        }
    }
}

/// A generated text as font selection chose it: its string and its font runs.
///
/// Measurement shapes it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct GeneratedText {
    /// What it says.
    pub(super) string: GeneratedString,
    /// Its runs, in [`GeneratedFonts`]' table of them: one, in the style's
    /// primary font, where that maps all of it.
    runs_start: GeneratedRunId,
    runs_end: GeneratedRunId,
}

impl GeneratedText {
    /// Its runs, in [`GeneratedFonts`]' table of them.
    fn runs(self) -> Range<GeneratedRunId> {
        self.runs_start..self.runs_end
    }
}

/// A run of a generated text in one used font, ending at a byte offset.
///
/// Its fonts are chosen per grapheme, as the text's own are.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct GeneratedRun {
    end: u32,
    /// The used font it is set in.
    pub(super) font: UsedFontId,
}

impl GeneratedRun {
    /// Where it ends, a byte offset into its text.
    pub(super) fn end(self) -> usize {
        u32_to_index(self.end)
    }
}

/// The fonts of the generated texts: each text facts row's hyphen and
/// ellipsis, where it can need them.
///
/// - A hyphen is made for the facts of text holding a soft hyphen a line may
///   break at.
/// - An ellipsis is made for the block's facts and its first line's, where
///   `text-overflow: ellipsis` or `line-clamp` may cut a line. The ellipsis
///   takes the block's style (CSS Overflow 4, "ellipsing details"), as
///   Chrome's `LineTruncator::EllipsisStyle` sets it.
///
/// The string depends on what the primary font maps, which is where Blink
/// looks for U+2010 and U+2026. Its characters take fonts as the text's do,
/// through the families and the fallback, as Blink's shaper does with the
/// style's `Font`. Nearly always the primary maps all of it: one run.
pub(crate) struct GeneratedFonts {
    /// Each text facts row's generated texts, in row order, a row's hyphen
    /// before its ellipsis.
    texts: Vec<(TextFactsId, Generated, GeneratedText)>,
    /// Every text's runs, a text's together and in the text's order.
    runs: Table<GeneratedRunId, GeneratedRun>,
}

impl GeneratedFonts {
    /// None, allocating nothing.
    const fn new() -> Self {
        Self {
            texts: Vec::new(),
            runs: Table::new(),
        }
    }

    /// Removes every style's, keeping the allocation.
    fn clear(&mut self) {
        self.texts.clear();
        self.runs.clear();
    }

    /// Returns the runs of `text`, in order. The last ends at the text's
    /// end.
    pub(super) fn runs(&self, text: GeneratedText) -> &[GeneratedRun] {
        self.runs.get_slice(text.runs()).unwrap_or_default()
    }

    /// Where the next text's runs start.
    fn next_run(&self) -> GeneratedRunId {
        self.runs.next_id()
    }

    /// Adds a run in `font` ending at byte `end` of the text whose runs
    /// start at `start`.
    ///
    /// Extends the previous run instead where it is this text's and in the
    /// same font. `None` where the table is full.
    fn push_run(&mut self, start: GeneratedRunId, end: usize, font: UsedFontId) -> Option<()> {
        let end = index_to_u32(end);
        if self.runs.next_id() > start
            && let Some(last) = self.runs.as_mut_slice().last_mut()
            && last.font == font
        {
            last.end = end;
            return Some(());
        }
        self.runs.push(GeneratedRun { end, font }).map(|_| ())
    }

    /// Adds the generated text of `kind` for `facts`, saying `string`, with
    /// the runs pushed since `start`.
    ///
    /// Callers walk rows in order and push a row's hyphen before its
    /// ellipsis, so the texts stay sorted.
    fn push_text(
        &mut self,
        facts: TextFactsId,
        kind: Generated,
        string: GeneratedString,
        start: GeneratedRunId,
    ) {
        let text = GeneratedText {
            string,
            runs_start: start,
            runs_end: self.runs.next_id(),
        };
        self.texts.push((facts, kind, text));
    }

    /// The generated text of `kind` of text with the text facts `facts`, or
    /// `None` where it has none.
    #[cfg(test)]
    fn text(&self, facts: TextFactsId, kind: Generated) -> Option<GeneratedText> {
        self.iter()
            .find(|&(held, held_kind, _)| held == facts && held_kind == kind)
            .map(|(.., text)| text)
    }

    /// Every generated text with its text facts, in row order, each row's
    /// hyphen before its ellipsis. Measurement shapes and keeps them in this
    /// order.
    pub(super) fn iter(&self) -> impl Iterator<Item = (TextFactsId, Generated, GeneratedText)> {
        self.texts.iter().copied()
    }
}

/// The fonts: what font selection writes and later stages read.
///
/// It holds the used fonts, the font runs, each request's resolution and
/// the faces the content still wants. The generated texts' fonts stay in
/// the font scratch, since only the build's measure reads them.
pub(crate) struct Fonts {
    /// The used fonts.
    pub(crate) used: UsedFonts,
    /// The text's runs, and the first paragraph's under `::first-line`
    /// where it changes a font or a transform. Both use fonts in `used`.
    runs: FirstLine<FontRuns>,
    /// What each font request resolves to: its used size and primary font.
    ///
    /// The primary font is CSS's first available font: the first listed
    /// font whose `unicode-range` holds U+0020 and whose data is loaded,
    /// else the user agent's default for the language. Text is measured in
    /// it, and it draws clusters no font covers. The builder lowers the
    /// first line's requests beside the others.
    resolutions: Table<FontRequestId, FontResolution>,
    /// The `@font-face` faces still downloading that some cluster wants,
    /// each once. The host fetches them, and lays out again when they land.
    ///
    /// The list is made at the first face wanted, then cleared and kept.
    wanted: WantedFaces,
    /// The language text whose style names none had its fonts chosen in,
    /// and is shaped in.
    pub(super) default_language: DefaultLanguage,
}

impl Fonts {
    /// Empty fonts, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            used: UsedFonts::new(),
            runs: FirstLine::new(FontRuns::new()),
            resolutions: Table::new(),
            wanted: None,
            default_language: DefaultLanguage::new(Language::UND),
        }
    }

    /// Empties it, keeping every allocation. A layout whose builder was
    /// dropped unfinished holds this.
    pub(crate) fn clear(&mut self) {
        self.used.clear();
        self.runs.clear();
        self.resolutions.clear();
        if let Some(wanted) = &mut self.wanted {
            wanted.clear();
        }
        self.default_language = DefaultLanguage::new(Language::UND);
    }

    /// Returns the `@font-face` faces still downloading that some cluster
    /// wants, each once.
    pub(crate) fn wanted(&self) -> &[FaceId] {
        self.wanted.as_deref().map_or(&[], Vec::as_slice)
    }

    /// Returns the runs `variant` reads: the first line's where they were
    /// selected, and the text's otherwise.
    #[inline]
    pub(crate) fn runs(&self, variant: FirstLineVariant) -> &FontRuns {
        self.runs.get(variant)
    }

    /// Whether the first paragraph was selected again under its
    /// `::first-line` styles: where it was not, the first line is set in
    /// the text's fonts.
    pub(super) fn has_first_line_runs(&self) -> bool {
        self.runs.has_first_line()
    }

    /// What font request `request` resolves to, or `None` for one the
    /// content does not have.
    pub(crate) fn resolution(&self, request: FontRequestId) -> Option<&FontResolution> {
        self.resolutions.get(request)
    }

    /// Returns the primary font of `request`, which its text is measured in.
    /// `None` for a request the content does not have.
    ///
    /// This is the one lookup for a text's primary font. A reader holding
    /// the text's facts gets the request from
    /// [`Facts::text_request`](crate::stages::content::Facts::text_request).
    pub(crate) fn primary_font(&self, request: FontRequestId) -> Option<&UsedFont> {
        self.used.get(self.resolution(request)?.primary)
    }

    /// Returns the gap `text-autospace` puts at a seam in text for
    /// `request`.
    ///
    /// The gap is an eighth of the primary font's ideographic advance (its
    /// size where it has no 水), rounded to the nearest 16.16. Chrome's
    /// `Font::TextAutoSpaceInlineSize` divides `IdeographicInlineSize` the
    /// same way. The advance is a stored metric, so no font table is read.
    /// Zero where the request has no used font.
    pub(super) fn autospace_gap(&self, request: FontRequestId) -> TextUnit {
        self.primary_font(request)
            .map_or(TextUnit::from_px(0.0), |used| {
                let ic = i64::from(used.metrics.ideographic_advance.raw());
                // To the nearest, halves away from zero, which no advance of a
                // font is below.
                TextUnit::from_raw(i32::try_from((ic + 4) / 8).unwrap_or(0))
            })
    }

    /// Returns the heap bytes of the first line's runs, which
    /// [`heap_bytes`](crate::data::HeapBytes::heap_bytes) also counts. Their
    /// fonts are the shared used fonts.
    pub(crate) fn first_line_heap_bytes(&self) -> usize {
        self.runs.first_line_heap_bytes()
    }
}

heap_bytes! {
    /// Its tables; not the fonts' bytes, each a count taken on the file
    /// the collection holds.
    Fonts { used, runs, resolutions, wanted; default_language }
}

heap_bytes! {
    /// Its table; not the instances' records, each a count taken on the
    /// context's.
    UsedFonts { table }
}

heap_bytes! {
    GeneratedFonts { texts, runs }
}
