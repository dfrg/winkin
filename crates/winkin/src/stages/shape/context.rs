//! The shaping caches and scratch that outlive a layout.
//!
//! Everything here is a cache, keyed by content: a font and its
//! coordinates, a script, a direction, a language, a list of features.
//! Nothing is keyed by a width, a first-line cut or a layout's ids. A call
//! shapes in the layout's own used-font record ([`UsedInstance`]), which
//! holds the whole font. Lookups use that record's [`FontKey`] and
//! coordinates, and a miss rebuilds the entry from it. So any entry may be
//! dropped between calls, and a reshape never finds another font's data.
//! Each cache keeps at most its capacity: [`ShapeContext::trim`] drops the
//! entries used longest ago between calls. An id lives for one shaping
//! call, so none is held then.
//!
//! The caches:
//! - **languages**: harfrust's [`Language`] per [`PlanLanguage`], since
//!   building a tag longer than eight bytes allocates.
//! - **plans**: a [`ShapePlan`] per font, feature variations, script,
//!   direction, language and features. Without one, harfrust compiles a plan
//!   on every call, which dominates shaping a short run.
//! - **trim fonts**: what a font says to `text-spacing-trim` ([`TrimFont`]),
//!   per instance, language and direction, read once from its tables and ten
//!   probe characters.
//! - **vertical metrics** ([`VerticalFont`], [`VerticalGlyphs`]): each
//!   glyph's advance and origin down the line, read once per glyph (see
//!   `vertical`).
//!
//! The scratch, which keeps its capacity:
//! - **call buffers** ([`CallBuffers`]): the converted features and one
//!   harfrust [`Buffer`].
//! - **trim marks** ([`TrimMarks`]): a call's marks, passed to harfrust as
//!   `halt` over each mark's range. A plan compiled with one such range
//!   serves a call with any number, since harfrust sets ranged masks per
//!   call.
//!
//! Warm, a call finds each entry by hash and allocates nothing. A plan's
//! hash names its language by the language table's id, and its match
//! compares the language itself, so a language dropped and its id reused
//! never finds another language's plan. A table whose ids are all taken
//! (four billion live entries) starts again rather than panicking.

use alloc::vec::Vec;
use core::hash::{Hash, Hasher};
use core::ops::{Deref, DerefMut};
use core::{mem, str};

use fontwich::FontKey;
use harfrust::{
    Buffer, BufferFlags, ClusterLevel, Direction, Feature, FontFuncs, Language, Script,
    ShapeOptions, ShapePlan, ShapePlanKey, ShaperFont,
};
use parlance::FontFeature;
use read_fonts::TableProvider;
use read_fonts::model::Font;
use read_fonts::types::{GlyphId, Tag};

use super::provider::ProvidedFuncs;
use super::sink::SharedGlyphs;
use super::trim::{TRIM_PROBES, TrimFont, TrimMark, TrimMarks, TrimProbe};
use super::vertical::{
    VerticalFallback, VerticalFont, VerticalFontId, VerticalFuncs, VerticalGlyphs, VerticalKey,
};
use crate::FontInstance;
use crate::context::CacheLimits;
use crate::data::{FxHasher, HeapBytes, LruCache, define_id, hash_one, heap_bytes};
use crate::font::FontMetricsProvider;
use crate::stages::fonts::{CaseMap, UsedInstance};
use crate::style::FontLanguageOverride;
use crate::unit;

define_id! {
    /// Names a language in the context's table of [`ShapeLanguage`]s.
    struct ShapeLanguageId(u32);
}

define_id! {
    /// Names a plan in the context's table of [`Plan`]s.
    struct PlanId(u32);
}

define_id! {
    /// Names an entry in the context's table of [`TrimEntry`]s.
    struct TrimEntryId(u32);
}

/// What one font instance says to `text-spacing-trim` in one language and
/// direction.
struct TrimEntry {
    /// The font, by its key, and held at the instance's coordinates, which
    /// are compared.
    key: FontKey,
    font: Font,
    language: PlanLanguage,
    horizontal: bool,
    trim: TrimFont,
}

/// Whether `font`, held at an instance's coordinates and keyed `key`, is the
/// instance `used`: the same font at the same coordinates.
pub(super) fn is_instance(key: FontKey, font: &Font, used: &UsedInstance) -> bool {
    key == used.key() && font.normalized_coords() == used.font.normalized_coords()
}

/// Feeds `used`'s font and coordinates to `fx`, as a table keyed by
/// instance hashes it.
fn hash_instance(fx: &mut FxHasher, used: &UsedInstance) {
    used.key().hash(fx);
    used.coords.hash(fx);
}

/// What a run's plan is made for: the language its fonts were chosen in, or
/// the OpenType language system its style's `font-language-override` names.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum PlanLanguage {
    /// The language the run's fonts were chosen in.
    ///
    /// It is the content language, or the build's default where the style
    /// names none, or [`parlance::Language::UND`] where neither does.
    Text(parlance::Language),
    /// A language system tag, passed as HarfBuzz's private `x-hbot` subtag
    /// in its hexadecimal form, which names the system itself, case and
    /// all, rather than a language.
    System(parlance::Tag),
}

impl PlanLanguage {
    /// Returns what text with fonts chosen in `language` and styled with
    /// `language_override` is planned for.
    ///
    /// That is the language system the override names, or else `language`.
    pub(super) fn new(
        language: parlance::Language,
        language_override: FontLanguageOverride,
    ) -> Self {
        match language_override {
            FontLanguageOverride::System(tag) => Self::System(tag),
            FontLanguageOverride::Normal => Self::Text(language),
        }
    }

    /// The script a font's [`TRIM_PROBES`] are shaped in for text planned
    /// for it: kana for Japanese, hangul for Korean, and Han for any other
    /// language or a language system.
    fn probe_script(self) -> Option<Script> {
        let tag = match self {
            Self::Text(language) => match language.language() {
                "ja" => b"Kana",
                "ko" => b"Hang",
                _ => b"Hani",
            },
            Self::System(_) => b"Hani",
        };
        Script::from_iso15924_tag(harfrust::Tag::new(tag))
    }

    /// harfrust's language for it, or `None` for none: undetermined, or a
    /// tag harfrust cannot hold.
    fn to_shaper(self) -> Option<Language> {
        match self {
            Self::Text(language) if language == parlance::Language::UND => None,
            Self::Text(language) => Language::new(language.as_str()),
            Self::System(tag) => {
                // The hexadecimal form keeps the tag's case: harfrust
                // uppercases a tag spelled out, which turns `trk ` into
                // Turkish.
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let mut named = *b"x-hbot-00000000";
                for (digits, byte) in named[7..]
                    .as_chunks_mut::<2>()
                    .0
                    .iter_mut()
                    .zip(tag.to_bytes())
                {
                    digits[0] = HEX[usize::from(byte >> 4)];
                    digits[1] = HEX[usize::from(byte & 0xF)];
                }
                Language::new(named)
            }
        }
    }
}

/// A language as harfrust plans for it.
struct ShapeLanguage {
    language: PlanLanguage,
    shaper: Language,
}

/// A compiled plan, and the font it was compiled for, by its key, which
/// harfrust's own key does not name.
struct Plan {
    font: FontKey,
    plan: ShapePlan,
}

/// The shaping caches, which outlive every layout, and a call's scratch.
///
/// The scratch serves the two steps of a call: finding the marks
/// `text-spacing-trim` trims, and calling harfrust.
pub(crate) struct ShapeContext {
    languages: LruCache<ShapeLanguageId, ShapeLanguage>,
    plans: LruCache<PlanId, Plan>,
    /// What each font says to `text-spacing-trim`, by instance, language
    /// and direction.
    trims: LruCache<TrimEntryId, TrimEntry>,
    /// Each instance set down the line at a size, by instance, size and
    /// fallback, and its glyphs' metrics down the line.
    ///
    /// The glyphs of a font dropped go with it.
    verticals: LruCache<VerticalFontId, VerticalFont>,
    vertical_glyphs: VerticalGlyphs,
    /// What a call to harfrust fills.
    buffers: CallBuffers,
    /// A call's marks that give their blanks back, and where they make
    /// breaking unsafe.
    ///
    /// The call that finds them takes them and gives them back
    /// ([`take_trim_marks`](Self::take_trim_marks)).
    trim_marks: TrimMarks,
    /// Where a paragraph's one-glyph clusters find the sidecar entry already
    /// holding their glyph.
    ///
    /// The shaping pass takes it and gives it back
    /// ([`take_shared_glyphs`](Self::take_shared_glyphs)).
    shared_glyphs: SharedGlyphs,
}

/// One build or break pass over the shaping caches, with a borrowed metric
/// source and the fonts the pass shapes in.
///
/// The context retains neither the provider, its cache, nor the fonts.
pub(crate) struct ShapeSession<'c, 'p> {
    context: &'c mut ShapeContext,
    pass: PassFonts<'p>,
}

impl<'c, 'p> ShapeSession<'c, 'p> {
    pub(crate) fn new(
        context: &'c mut ShapeContext,
        provider: Option<&'p dyn FontMetricsProvider>,
    ) -> Self {
        let pass = PassFonts {
            provider,
            held: Default::default(),
            next: 0,
        };
        Self { context, pass }
    }

    pub(super) fn shape_call<R>(
        &mut self,
        font: &'p UsedInstance,
        text: &CallText<'_>,
        settings: &CallSettings,
        wanted: Features<'_>,
        read: impl FnOnce(&Buffer) -> R,
    ) -> Option<R> {
        self.context
            .shape_call(font, text, settings, wanted, Some(&mut self.pass), read)
    }

    pub(crate) fn provider(&self) -> Option<&'p dyn FontMetricsProvider> {
        self.pass.provider
    }
}

impl Deref for ShapeSession<'_, '_> {
    type Target = ShapeContext;

    fn deref(&self) -> &Self::Target {
        self.context
    }
}

impl DerefMut for ShapeSession<'_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.context
    }
}

/// How many prepared fonts a pass keeps.
const SHAPERS: usize = 4;

/// What a pass shapes with beside the context: its metric provider, and
/// the fonts it last shaped in, prepared for harfrust.
///
/// Preparing a font reads its layout tables and character map, and its
/// glyph metrics on first use. A pass shapes run after run in the same few
/// fonts, so a few prepared fonts spare nearly every preparation.
struct PassFonts<'f> {
    provider: Option<&'f dyn FontMetricsProvider>,
    /// The prepared fonts, each with its key and the scale it was last set
    /// to. A miss replaces the one prepared longest ago, at `next`.
    held: [Option<(FontKey, i32, ShaperFont<'f, 'static>)>; SHAPERS],
    next: usize,
}

impl<'f> PassFonts<'f> {
    /// Returns `font` prepared for harfrust at `scale`, preparing it where
    /// no entry holds it.
    fn shaper(&mut self, font: &'f UsedInstance, scale: i32) -> &ShaperFont<'f, 'static> {
        let found = self.held.iter().position(|held| {
            held.as_ref()
                .is_some_and(|(key, _, shaper)| is_instance(*key, shaper, font))
        });
        let at = found.unwrap_or_else(|| {
            let at = self.next % SHAPERS;
            self.next = at + 1;
            self.held[at] = None;
            at
        });
        let (_, held, shaper) = self.held[at].get_or_insert_with(|| {
            let mut shaper = ShaperFont::new(&font.font);
            shaper.set_scale(scale);
            (font.key(), scale, shaper)
        });
        if *held != scale {
            *held = scale;
            shaper.set_scale(scale);
        }
        shaper
    }
}

/// What a call to harfrust fills: [`ShapeContext::shape_call`]'s scratch.
///
/// It is kept between calls, and keeps the base features' conversion while
/// their content is unchanged.
struct CallBuffers {
    /// The source of the converted prefix; compared by content, never a layout id.
    base_features: Vec<FontFeature>,
    /// Converted base features, followed by the call's ranged/width overrides.
    features: Vec<Feature>,
    /// The one buffer, filled for every call, shaped in place, and cleared
    /// after it, keeping its capacity.
    buffer: Buffer,
}

impl CallBuffers {
    /// Empty, allocating nothing.
    fn new() -> Self {
        Self {
            base_features: Vec::new(),
            features: Vec::new(),
            buffer: Buffer::new(),
        }
    }

    /// Sets the features a call in `font` shapes with, as `wanted` says,
    /// shaping down the line where `vertical`.
    ///
    /// Returns them, how many of them the plan is compiled for, and the
    /// buffer to fill. The plan is compiled for the features and one `halt`
    /// range at most, and the call shapes with every range. A probe shapes
    /// with no features, and the font's stay converted for the next call.
    fn features(
        &mut self,
        font: &UsedInstance,
        wanted: Features<'_>,
        vertical: bool,
    ) -> (&[Feature], usize, &mut Buffer) {
        let Self {
            base_features,
            features,
            buffer,
        } = self;
        let Features::Font { halt, narrow } = wanted else {
            return (&[], 0, buffer);
        };
        if *base_features.as_slice() != *font.features {
            base_features.clear();
            base_features.extend_from_slice(&font.features);
            features.clear();
            features.extend(font.features.iter().map(to_shaper));
        } else {
            // Drop only the preceding call's ranged and width overrides.
            features.truncate(base_features.len());
        }
        // After the style's, so that it holds whatever they say, as Blink's
        // compressed font holds its width variant.
        if let Some(tag) = narrow {
            features.push(Feature {
                tag: harfrust::Tag::new(tag),
                value: 1,
                start: 0,
                end: u32::MAX,
            });
        }
        let planned = features.len() + usize::from(!halt.is_empty());
        // `vhal` down the line, as Blink's `HanKerning` applies it.
        let tag = if vertical { b"vhal" } else { b"halt" };
        features.extend(halt.iter().map(|mark| Feature {
            tag: harfrust::Tag::new(tag),
            value: 1,
            start: mark.start(),
            end: mark.end(),
        }));
        (features, planned, buffer)
    }
}

/// Which features a call shapes with.
#[derive(Copy, Clone, Debug)]
pub(super) enum Features<'a> {
    /// The used font's features, plus `halt` over each mark's bytes (`vhal`
    /// down the line).
    ///
    /// Where combined text is fitted to its em, it also adds a narrower
    /// form: `hwid`, `twid` or `qwid`.
    Font {
        halt: &'a [TrimMark],
        narrow: Option<&'static [u8; 4]>,
    },
    /// Only the shaper's defaults. A font's glyphs are read this way for
    /// its punctuation classes, as Blink's `HarfBuzzShaper::GetGlyphData`
    /// reads them.
    Plain,
}

/// The text one call shapes.
///
/// The shaping function and the trim-font reader build one per call, and
/// [`ShapeContext::shape_call`] reads it. It lives no longer than the call.
pub(super) struct CallText<'a> {
    /// The characters, which start at byte `base` of the text.
    pub(super) chars: &'a str,
    pub(super) base: u32,
    /// Up to five characters before and after, for joining.
    pub(super) before: &'a str,
    pub(super) after: &'a str,
    /// The characters start their paragraph.
    pub(super) starts_paragraph: bool,
}

/// How one call shapes its text, besides the font and its features.
///
/// A run's come from its key (`ShapingKey::settings`); the trim-font reader
/// writes its own for the probes.
#[derive(Copy, Clone, Debug)]
pub(super) struct CallSettings {
    pub(super) direction: Direction,
    pub(super) script: Option<Script>,
    pub(super) language: PlanLanguage,
    /// harfrust's scale: the size in 1/65536 px.
    pub(super) scale: i32,
    /// The point size in CSS pixels, for `trak` tables, as Chrome hands
    /// HarfBuzz the specified size.
    ///
    /// It differs from `scale`'s size where `font-size-adjust`, a face's
    /// `size-adjust` or synthesis sets the glyphs at another size.
    pub(super) point_size: f32,
    /// How the characters are fed: as written, or in another case.
    pub(super) case: CaseMap,
    /// The text is Turkish or Azerbaijani, for how `i` and `I` case.
    pub(super) turkic: bool,
    /// What the font's vertical metrics fall back to where it has none, for
    /// a call shaping down the line (see `vertical`). `None` for a call
    /// shaping across.
    pub(super) vertical: Option<VerticalFallback>,
}

impl ShapeContext {
    /// Returns empty caches, each keeping what `limits` says, allocating
    /// nothing.
    ///
    /// The languages keep as many as the plans, since a plan names one. The
    /// trim answers and the fonts set down the line keep as many as the
    /// font instances, since each is read per instance.
    pub(crate) fn new(limits: &CacheLimits) -> Self {
        Self {
            languages: LruCache::new(limits.shape_plans),
            plans: LruCache::new(limits.shape_plans),
            trims: LruCache::new(limits.font_instances),
            verticals: LruCache::new(limits.font_instances),
            vertical_glyphs: VerticalGlyphs::new(),
            buffers: CallBuffers::new(),
            trim_marks: TrimMarks::default(),
            shared_glyphs: SharedGlyphs::new(),
        }
    }

    /// Sets how many entries each cache keeps from the next trim on.
    pub(crate) fn set_limits(&mut self, limits: &CacheLimits) {
        self.languages.set_capacity(limits.shape_plans);
        self.plans.set_capacity(limits.shape_plans);
        self.trims.set_capacity(limits.font_instances);
        self.verticals.set_capacity(limits.font_instances);
    }

    /// Drops the entries used longest ago from each cache down to its
    /// capacity, and the glyphs of each font set down the line it drops.
    ///
    /// The context calls it between calls, when no id is held.
    pub(crate) fn trim(&mut self) {
        self.languages.trim(|_, _| {});
        self.plans.trim(|_, _| {});
        self.trims.trim(|_, _| {});
        let mut dropped = false;
        self.verticals.trim(|_, _| dropped = true);
        if dropped {
            self.drop_orphan_glyphs();
        }
    }

    /// Drops the entries of each font `keep` refuses by its key: they hold
    /// the font's bytes.
    pub(crate) fn retain_fonts(&mut self, keep: impl Fn(FontKey) -> bool) {
        self.plans.retain(|_, plan| keep(plan.font), |_, _| {});
        self.trims.retain(|_, entry| keep(entry.key), |_, _| {});
        let mut dropped = false;
        self.verticals
            .retain(|_, font| keep(font.key()), |_, _| dropped = true);
        if dropped {
            self.drop_orphan_glyphs();
        }
    }

    /// Drops the glyphs of the fonts set down the line that are gone.
    fn drop_orphan_glyphs(&mut self) {
        let verticals = &self.verticals;
        self.vertical_glyphs
            .retain(|font| verticals.get(font).is_some());
    }

    /// Replaces the scratch with empty scratch, releasing its capacity.
    pub(crate) fn release_scratch(&mut self) {
        self.buffers = CallBuffers::new();
        self.trim_marks = TrimMarks::default();
        self.shared_glyphs = SharedGlyphs::new();
    }

    /// Takes the emptied scratch a call finds `text-spacing-trim` marks
    /// into.
    ///
    /// The call holds it while it shapes with the context, then hands it
    /// back ([`restore_trim_marks`](Self::restore_trim_marks)) so its
    /// capacity is kept.
    pub(super) fn take_trim_marks(&mut self) -> TrimMarks {
        let mut marks = mem::take(&mut self.trim_marks);
        marks.clear();
        marks
    }

    /// Takes back the scratch [`take_trim_marks`](Self::take_trim_marks)
    /// handed out, for the next call.
    pub(super) fn restore_trim_marks(&mut self, marks: TrimMarks) {
        self.trim_marks = marks;
    }

    /// Takes the emptied scratch a sink finds shared sidecar entries in.
    ///
    /// The shaping of a text holds it in its sink while it shapes with the
    /// context, then hands it back
    /// ([`restore_shared_glyphs`](Self::restore_shared_glyphs)) so its
    /// capacity is kept.
    pub(super) fn take_shared_glyphs(&mut self) -> SharedGlyphs {
        let mut shared = mem::replace(&mut self.shared_glyphs, SharedGlyphs::new());
        shared.clear();
        shared
    }

    /// Takes back the scratch
    /// [`take_shared_glyphs`](Self::take_shared_glyphs) handed out.
    pub(super) fn restore_shared_glyphs(&mut self, shared: SharedGlyphs) {
        self.shared_glyphs = shared;
    }

    /// The bytes the scratch takes on the heap.
    ///
    /// [`heap_bytes`](HeapBytes::heap_bytes) counts these too; the rest is
    /// the caches.
    pub(crate) fn scratch_heap_bytes(&self) -> usize {
        self.buffers.heap_bytes() + self.trim_marks.heap_bytes() + self.shared_glyphs.heap_bytes()
    }

    /// Drops every instance set down the line, and its glyphs.
    fn clear_verticals(&mut self) {
        self.verticals.clear();
        self.vertical_glyphs.clear();
    }

    /// How many plans are compiled.
    #[cfg(test)]
    pub(crate) fn plan_count(&self) -> usize {
        self.plans.len()
    }

    /// How many languages, plans, trim answers, fonts set down the line and
    /// their glyphs are held, in that order.
    #[cfg(test)]
    pub(crate) fn counts(&self) -> [usize; 5] {
        [
            self.languages.len(),
            self.plans.len(),
            self.trims.len(),
            self.verticals.len(),
            self.vertical_glyphs.len(),
        ]
    }

    /// How many instances are set down the line at a size, and how many of
    /// their glyphs' metrics down the line are read.
    #[cfg(test)]
    pub(super) fn vertical_counts(&self) -> (usize, usize) {
        (self.verticals.len(), self.vertical_glyphs.len())
    }

    /// Shapes `text` in `font`, a layout's used font, as `call` says,
    /// and hands the shaped buffer to `read`.
    ///
    /// It uses the glyph metrics of `pass`'s provider where there is one,
    /// and otherwise the font as `pass` holds it prepared. Without `pass`, it
    /// prepares the font for the call. Returns `None` where harfrust refuses
    /// the call, which never happens for a buffer filled this way.
    fn shape_call<'f, R>(
        &mut self,
        font: &'f UsedInstance,
        text: &CallText<'_>,
        call: &CallSettings,
        wanted: Features<'_>,
        pass: Option<&mut PassFonts<'f>>,
        read: impl FnOnce(&Buffer) -> R,
    ) -> Option<R> {
        // Down the line, use the font's vertical metrics at the call's size,
        // cached per glyph. Where its em is zero, use harfrust's own.
        let vertical = call.vertical.and_then(|fallback| {
            self.vertical_font(VerticalKey::new(font.key(), call.scale, fallback), font)
        });
        let Self {
            languages,
            plans,
            verticals,
            vertical_glyphs,
            buffers,
            ..
        } = self;
        let shapes_down = matches!(
            call.direction,
            Direction::TopToBottom | Direction::BottomToTop
        );
        let (features, planned, buffer) = buffers.features(font, wanted, shapes_down);
        let language = intern_language(languages, plans, call.language)
            .and_then(|id| Some((id, &languages.get(id)?.shaper)));
        let plan = cached_plan(
            plans,
            &PlanRequest {
                font: font.key(),
                instance: &font.font,
                direction: call.direction,
                script: call.script,
                language,
                features: features.get(..planned).unwrap_or_default(),
            },
        );
        fill(buffer, text, call);
        // Trim probes shape at the font's em rather than a used CSS size;
        // they classify the font's design-space punctuation, not hinted ink.
        let provided = (pass.as_ref())
            .and_then(|pass| pass.provider)
            .filter(|_| matches!(wanted, Features::Font { .. }))
            .map(|provider| (provider, provider_instance(font, call.scale)));
        let options = ShapeOptions::new()
            .plan(plan)
            .point_size(Some(call.point_size))
            .features(features);
        let shaped = match vertical.and_then(|id| Some((id, verticals.get(id)?))) {
            Some(down) => shape_down(
                font,
                call.scale,
                down,
                vertical_glyphs,
                provided,
                buffer,
                options,
            ),
            None => match (provided, pass) {
                (None, Some(pass)) => {
                    harfrust::shape(pass.shaper(font, call.scale), buffer, options).is_ok()
                }
                (provided, _) => {
                    let funcs = provided
                        .map(|(provider, request)| ProvidedFuncs::new(provider, request, None));
                    let mut shaper = ShaperFont::new(&font.font);
                    shaper.set_scale(call.scale);
                    shaper.set_font_funcs(funcs.as_ref().map(|funcs| funcs as &dyn FontFuncs));
                    harfrust::shape(&shaper, buffer, options).is_ok()
                }
            },
        };
        let answer = shaped.then(|| read(buffer));
        buffer.clear();
        answer
    }

    /// Returns the vertical font `key` names, `font` at its size, building
    /// it the first time.
    ///
    /// Returns `None` where its em is zero, and harfrust's own metrics are
    /// used. A table whose ids are all taken drops every vertical font and
    /// its glyphs and starts again.
    fn vertical_font(&mut self, key: VerticalKey, font: &UsedInstance) -> Option<VerticalFontId> {
        let hash = || {
            let mut fx = FxHasher::new();
            key.hash(&mut fx);
            hash_instance(&mut fx, font);
            fx.finish()
        };
        let hash = match self.verticals.find_recent(hash, |held| held.is(&key, font)) {
            Ok(id) => return Some(id),
            Err(hash) => hash,
        };
        let made = VerticalFont::new(&font.font, key)?;
        if let Some(id) = self.verticals.insert(hash, made.clone()) {
            return Some(id);
        }
        self.clear_verticals();
        self.verticals.insert(hash, made)
    }
}

impl ShapeContext {
    /// Returns what `font` says to `text-spacing-trim` in `language`, set
    /// `horizontal`ly or not.
    ///
    /// It is cached per instance, or read once from the font's feature lists
    /// and from [`TRIM_PROBES`] shaped in it. Where the bytes are not a font,
    /// it is the default: no `halt`, no `chws`.
    ///
    /// Upright text in a vertical line reads `vhal` and `vchw`, and shapes
    /// its probes down the line to read their heights, as Blink's
    /// `HanKerning::FontData` reads them `!is_horizontal`.
    pub(super) fn trim_font(
        &mut self,
        font: &UsedInstance,
        language: PlanLanguage,
        horizontal: bool,
    ) -> TrimFont {
        // Read the font only where the cache lacks this language and
        // direction. This runs once per call, so once per run.
        let hash = || {
            let mut fx = FxHasher::new();
            hash_instance(&mut fx, font);
            language.hash(&mut fx);
            fx.write_u8(u8::from(horizontal));
            fx.finish()
        };
        let found = self.trims.find_recent(hash, |entry| {
            is_instance(entry.key, &entry.font, font)
                && entry.language == language
                && entry.horizontal == horizontal
        });
        let hash = match found {
            Ok(id) => {
                return self
                    .trims
                    .get(id)
                    .map_or_else(TrimFont::default, |entry| entry.trim);
            }
            Err(hash) => hash,
        };
        let read = self.read_trim_font(font, language, horizontal);
        let entry = TrimEntry {
            key: font.key(),
            font: font.font.clone(),
            language,
            horizontal,
            trim: read,
        };
        if self.trims.insert(hash, entry).is_none() {
            self.trims.clear();
        }
        read
    }

    /// Reads what `font` says to `text-spacing-trim` in `language`, set
    /// `horizontal`ly or upright in a vertical line.
    ///
    /// Down the line, the probes are shaped down it, and their ink is read
    /// from the top of each glyph's advance, as Blink's `CharTypeFromBounds`
    /// reads bounds `!is_horizontal`.
    ///
    /// This reads in font units with harfrust's own vertical metrics. Those
    /// match Blink's except that Blink rounds a bounds' top out to a whole
    /// pixel at a size (see `vertical`). The reading is kept for any size,
    /// so it leaves that rounding out.
    fn read_trim_font(
        &mut self,
        font: &UsedInstance,
        language: PlanLanguage,
        horizontal: bool,
    ) -> TrimFont {
        let tables = font.font.tables();
        // Whether either layout table names a feature, under any script,
        // as Blink's `OpenTypeFeatures` collects them.
        let lists = [
            tables
                .gsub()
                .ok()
                .and_then(|table| table.feature_list().ok()),
            tables
                .gpos()
                .ok()
                .and_then(|table| table.feature_list().ok()),
        ];
        let has = |tag: &[u8; 4]| {
            let tag = Tag::new(tag);
            lists.iter().flatten().any(|list| {
                list.feature_records()
                    .iter()
                    .any(|r| r.feature_tag() == tag)
            })
        };
        let (halt, chws) = if horizontal {
            (has(b"halt"), has(b"chws"))
        } else {
            (has(b"vhal"), has(b"vchw"))
        };
        let upem = match font.font.units_per_em() {
            0 => 1000,
            upem => i32::from(upem),
        };
        // The probes, as the font shapes them in the text's language, each
        // its own cluster, measured in font units.
        let mut text = [0u8; 30];
        let mut len = 0;
        for ch in TRIM_PROBES {
            len += ch.encode_utf8(&mut text[len..]).len();
        }
        let chars = str::from_utf8(&text[..len]).unwrap_or_default();
        let text = CallText {
            chars,
            base: 0,
            before: "",
            after: "",
            starts_paragraph: true,
        };
        let settings = CallSettings {
            direction: if horizontal {
                Direction::LeftToRight
            } else {
                Direction::TopToBottom
            },
            script: language.probe_script(),
            language,
            scale: upem,
            point_size: 0.0,
            case: CaseMap::Keep,
            turkic: false,
            vertical: None,
        };
        let shaped = self.shape_call(font, &text, &settings, Features::Plain, None, |output| {
            let infos = output.glyph_infos();
            let positions = output.glyph_positions();
            if infos.len() != TRIM_PROBES.len() || positions.len() != TRIM_PROBES.len() {
                return None;
            }
            let mut glyphs = [(0u32, 0i32, 0i32); 10];
            let mut label = 0;
            for (at, (info, position)) in infos.iter().zip(positions).enumerate() {
                if info.cluster != label {
                    return None;
                }
                // Along the line: across, or down, where the shaper's y
                // grows up.
                glyphs[at] = if horizontal {
                    (info.glyph_id, position.x_advance, position.x_offset)
                } else {
                    (info.glyph_id, -position.y_advance, -position.y_offset)
                };
                label += u32::try_from(TRIM_PROBES[at].len_utf8()).unwrap_or(0);
            }
            Some(glyphs)
        });
        let probes = shaped.flatten().map(|glyphs| {
            // The ink's bounds at the font's coordinates, each moved by its
            // glyph's offset; a glyph with no outline has an empty box at
            // its origin, as Skia gives one.
            let metrics = font.font.glyph_metrics();
            let mut probes = [TrimProbe::default(); 10];
            for (probe, &(glyph, advance, offset)) in probes.iter_mut().zip(&glyphs) {
                // The ink's extent along the line: across, or down from the
                // pen, the top first.
                let bounds = metrics.extents(GlyphId::new(glyph));
                let (left, right) = bounds.map_or((0.0, 0.0), |ink| {
                    if horizontal {
                        (ink.x_bearing, ink.x_bearing + ink.width)
                    } else {
                        (-ink.y_bearing, ink.height - ink.y_bearing)
                    }
                });
                let offset = unit::whole_to_f32(offset);
                *probe = TrimProbe {
                    drawn: glyph != 0,
                    advance: unit::whole_to_f32(advance),
                    left: left + offset,
                    right: right + offset,
                };
            }
            probes
        });
        TrimFont::from_probes(halt, chws, probes.as_ref())
    }
}

heap_bytes! {
    /// Its caches and a call's scratch. What read-fonts and harfrust keep
    /// inside a font, a language, a plan and the buffer is their own, and
    /// counted at their size alone: neither says how much.
    ShapeContext {
        languages, plans, trims, verticals, vertical_glyphs, buffers, trim_marks, shared_glyphs
    }
}

heap_bytes! {
    /// The source and converted feature vectors; the buffer's own heap is harfrust's.
    CallBuffers { base_features, features; buffer }
}

/// Returns `font` at `scale`, harfrust's size in 1/65536 px, as a host's
/// metrics provider is asked for it.
fn provider_instance(font: &UsedInstance, scale: i32) -> FontInstance<'_> {
    FontInstance {
        bytes: &font.bytes,
        index: font.index,
        coords: &font.coords,
        size: unit::TextUnit::from_raw(scale).to_px(),
        embolden: font.embolden,
        skew: font.skew,
    }
}

/// Converts `feature` to harfrust's global form.
///
/// It builds the struct directly, since `Feature::new` stores an exclusive
/// end one short.
fn to_shaper(feature: &FontFeature) -> Feature {
    Feature {
        tag: harfrust::Tag::new(&feature.tag.to_bytes()),
        value: u32::from(feature.value),
        start: 0,
        end: u32::MAX,
    }
}

/// Interns `language` as harfrust plans for it, the first time, and returns
/// its id, or `None` for no language: undetermined, or not one harfrust can
/// hold.
///
/// A table whose ids are all taken starts again, and the plans with it,
/// since their hashes name languages by id.
fn intern_language(
    languages: &mut LruCache<ShapeLanguageId, ShapeLanguage>,
    plans: &mut LruCache<PlanId, Plan>,
    language: PlanLanguage,
) -> Option<ShapeLanguageId> {
    if language == PlanLanguage::Text(parlance::Language::UND) {
        return None;
    }
    let hash = match languages.find_recent(|| hash_one(&language), |held| held.language == language)
    {
        Ok(id) => return Some(id),
        Err(hash) => hash,
    };
    let shaper = language.to_shaper()?;
    let entry = ShapeLanguage {
        language,
        shaper: shaper.clone(),
    };
    match languages.insert(hash, entry) {
        Some(id) => Some(id),
        None => {
            languages.clear();
            plans.clear();
            languages.insert(hash, ShapeLanguage { language, shaper })
        }
    }
}

/// What a plan is asked for.
struct PlanRequest<'a> {
    font: FontKey,
    /// The font at the instance's coordinates, which select its feature
    /// variations.
    instance: &'a Font,
    direction: Direction,
    script: Option<Script>,
    language: Option<(ShapeLanguageId, &'a Language)>,
    features: &'a [Feature],
}

impl PlanRequest<'_> {
    fn compile(&self) -> Plan {
        Plan {
            font: self.font,
            plan: ShapePlan::new(
                self.instance,
                self.direction,
                self.script,
                self.language.map(|(_, language)| language),
                self.features,
            ),
        }
    }
}

/// Returns the plan for `request`, compiling it the first time.
///
/// The key is the font, the instance's feature variations, the script, the
/// direction, the language and the features in order, each by content.
/// harfrust's own [`ShapePlanKey`] compares all but the font, including the
/// feature variations, which only it can read. A plan depends on an
/// instance's coordinates only through those variations, so instances of
/// one font that select the same variations share a plan.
///
/// The hash covers everything but the variations. A table whose ids are all
/// taken starts again.
fn cached_plan<'p>(
    plans: &'p mut LruCache<PlanId, Plan>,
    request: &PlanRequest<'_>,
) -> Option<&'p ShapePlan> {
    let hash = || {
        let mut fx = FxHasher::new();
        request.font.hash(&mut fx);
        fx.write_usize(request.features.len());
        for feature in request.features {
            feature.tag.hash(&mut fx);
            fx.write_u32(feature.value);
        }
        request.script.hash(&mut fx);
        request.direction.hash(&mut fx);
        request.language.map(|(id, _)| id).hash(&mut fx);
        fx.finish()
    };
    let key = ShapePlanKey::new(request.instance, request.script, request.direction)
        .language(request.language.map(|(_, language)| language))
        .features(request.features);
    let found = plans.find_recent(hash, |held| {
        held.font == request.font && key.matches(&held.plan)
    });
    let id = match found {
        Ok(id) => id,
        Err(hash) => match plans.insert(hash, request.compile()) {
            Some(id) => id,
            None => {
                plans.clear();
                plans.insert(hash, request.compile())?
            }
        },
    };
    Some(&plans.get(id)?.plan)
}

/// Fills `buffer` with `text`, as `call` sets it.
///
/// It sets every property clearing reset, the context either side, and the
/// characters. Each character is labelled with its byte in the text, so
/// glyphs come back naming the clusters they draw.
fn fill(buffer: &mut Buffer, text: &CallText<'_>, call: &CallSettings) {
    buffer.set_direction(call.direction);
    buffer.set_script(call.script);
    // Marks are not merged into their bases, so a cluster's glyphs are
    // found by where its characters are; a grapheme's inside is marked
    // unsafe instead.
    buffer.set_cluster_level(ClusterLevel::MonotoneCharacters);
    // Nothing asks for harfrust's unsafe-to-concatenate flags. A reshape
    // window starts and ends where breaking is safe, as Chrome's do.
    // Producing the flags makes the shaper track glyphs for lookups it then
    // rejects.
    let mut flags = BufferFlags::empty();
    if text.starts_paragraph {
        flags |= BufferFlags::BEGINNING_OF_TEXT;
    }
    buffer.set_flags(flags);
    buffer.set_pre_context(text.before);
    if call.case == CaseMap::Keep {
        // One count and one reservation for the whole text: pushing a
        // character at a time grows harfrust's buffer by one record a push.
        // Its clusters count from the text's start, so `base` is added after.
        buffer.push_str(text.chars);
        for info in buffer.glyph_infos_mut() {
            info.cluster = text.base.saturating_add(info.cluster);
        }
    } else {
        let upper = call.case == CaseMap::Upper;
        for (at, ch) in text.chars.char_indices() {
            // Within the text, which is under 2^30 bytes, so this never
            // saturates.
            let at = u32::try_from(at).unwrap_or(u32::MAX);
            let label = text.base.saturating_add(at);
            // Each character a case mapping makes is labelled with the byte
            // of the one it came from, as Blink's
            // `CaseMappingHarfBuzzBufferFiller` adds them at one cluster: an
            // uppercased `ß` is two glyphs in one cluster, and the text keeps
            // the author's case.
            match ch {
                'i' if upper && call.turkic => buffer.push(0x130, label),
                _ if upper => ch
                    .to_uppercase()
                    .for_each(|up| buffer.push(u32::from(up), label)),
                'I' if call.turkic => buffer.push(0x131, label),
                '\u{130}' if call.turkic => buffer.push(u32::from('i'), label),
                _ => ch
                    .to_lowercase()
                    .for_each(|low| buffer.push(u32::from(low), label)),
            }
        }
    }
    // After the characters, as HarfBuzz sets the context after the text.
    buffer.set_post_context(text.after);
}

/// Shapes `buffer` down the line with `options`, in `font` at `scale`, and
/// returns whether it shaped.
///
/// It uses the font's vertical metrics as Blink reads them (see `vertical`),
/// set as `vertical` says. Each glyph's metrics come from `glyphs`, or are
/// read at the instance's coordinates where it lacks them. `provided`
/// overrides them where the host gives metrics.
///
/// Out of line, so a call shaping across, which is nearly every call, keeps
/// a frame without the vertical metrics' tables.
#[allow(clippy::too_many_arguments)]
#[inline(never)]
fn shape_down(
    font: &UsedInstance,
    scale: i32,
    vertical: (VerticalFontId, &VerticalFont),
    glyphs: &mut VerticalGlyphs,
    provided: Option<(&dyn FontMetricsProvider, FontInstance<'_>)>,
    buffer: &mut Buffer,
    options: ShapeOptions<'_>,
) -> bool {
    let (id, down) = vertical;
    let funcs = VerticalFuncs::new(id, down, &font.font, glyphs);
    let provided =
        provided.map(|(provider, request)| ProvidedFuncs::new(provider, request, Some(&funcs)));
    let font_funcs: &dyn FontFuncs = match provided.as_ref() {
        Some(provided) => provided,
        None => &funcs,
    };
    let mut shaper = ShaperFont::new(&font.font);
    shaper.set_scale(scale);
    shaper.set_font_funcs(Some(font_funcs));
    harfrust::shape(&shaper, buffer, options).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{TestAxis, TestFont, allocator::count_allocations};
    use alloc::boxed::Box;
    use alloc::vec;
    use fontwich::FontBytes;
    use harfrust::Script;
    use read_fonts::model::Blob;
    use read_fonts::types::F2Dot14;

    /// The record of the font `bytes` hold at `coords`, shaping with
    /// `features`.
    fn record(bytes: FontBytes, coords: &[i16], features: &[FontFeature]) -> UsedInstance {
        let (shared, _) = bytes.clone().into_raw_parts();
        let font = Font::new(Blob::Shared(shared), 0)
            .expect("a font")
            .instance_builder()
            .normalized_coords(coords.iter().map(|&bits| F2Dot14::from_bits(bits)))
            .build();
        UsedInstance {
            bytes,
            index: 0,
            font,
            coords: Box::from(coords),
            features: Box::from(features),
            embolden: false,
            skew: None,
        }
    }

    /// Reading a font for `text-spacing-trim` again, as after the table
    /// starts again, reads its probes' bounds at the instance's coordinates
    /// without allocating.
    #[test]
    fn a_trim_miss_allocates_nothing() {
        let mut font = TestFont::cjk("Variable trim", true);
        font.axes = vec![TestAxis {
            tag: *b"wght",
            min: 100.0,
            default: 400.0,
            max: 900.0,
            delta: 0,
        }];
        let font = record(FontBytes::from(font.build()), &[8192], &[]);
        let mut cx = ShapeContext::new(&CacheLimits::default());
        let language = PlanLanguage::Text(parlance::Language::UND);
        assert!(cx.trim_font(&font, language, true).has_halt());
        // Invalidate the result only: the next call must actually read the probes again.
        cx.trims.clear();
        let allocations = count_allocations(|| {
            assert!(cx.trim_font(&font, language, true).has_halt());
        });
        assert_eq!(allocations, 0, "a trim miss allocates nothing");
    }

    #[test]
    fn cached_features_do_not_leak_into_probes_or_other_styles() {
        let mut font = TestFont::new("Feature cache", &[(0x20, 0x7e)]);
        font.ligatures = vec![vec!['f', 'i']];
        let bytes = FontBytes::from(font.build());
        let disabled = [FontFeature::new(parlance::Tag::new(b"liga"), 0)];
        let mut font = record(bytes.clone(), &[], &disabled);
        let text = CallText {
            chars: "fi",
            base: 0,
            before: "",
            after: "",
            starts_paragraph: true,
        };
        let call = CallSettings {
            direction: Direction::LeftToRight,
            script: Some(Script::LATIN),
            language: PlanLanguage::Text(parlance::Language::UND),
            scale: 1000,
            point_size: 0.0,
            case: CaseMap::Keep,
            turkic: false,
            vertical: None,
        };
        let mut cx = ShapeContext::new(&CacheLimits::default());
        let wanted = Features::Font {
            halt: &[],
            narrow: None,
        };
        let count = |output: &Buffer| output.glyph_infos().len();
        assert_eq!(
            cx.shape_call(&font, &text, &call, wanted, None, count),
            Some(2)
        );
        assert_eq!(
            cx.shape_call(&font, &text, &call, Features::Plain, None, count),
            Some(1)
        );
        assert_eq!(
            cx.shape_call(&font, &text, &call, wanted, None, count),
            Some(2)
        );
        font.features = Box::default();
        assert_eq!(
            cx.shape_call(&font, &text, &call, wanted, None, count),
            Some(1)
        );
        font.features = Box::from(disabled);
        assert_eq!(
            cx.shape_call(&font, &text, &call, wanted, None, count),
            Some(2)
        );
    }
}
