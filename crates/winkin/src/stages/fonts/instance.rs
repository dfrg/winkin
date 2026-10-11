//! Font instances, kept in the context across layouts.
//!
//! An instance holds everything apart from size that makes two runs shape
//! differently: a font ([`FontKey`]) at a point in its design space, with its
//! synthesis, its OpenType features and its `@font-face` metric descriptors.
//! It holds its record ([`UsedInstance`]) and the font's unscaled metrics at
//! its coordinates, both made when the instance is. A used font shares the
//! record and scales the metrics to its size.
//!
//! **A face** is a font held whole at its default coordinates. Every
//! instance of the font is cut from its [`Face`]
//! (`Font::instance_builder`), so they share one reading of its tables,
//! harfrust's included. The feature offers and coverages read the face too.
//! A face counts the instances cut from it, and goes at the first trim
//! after the last one does, which releases its bytes. Where the context
//! shares fonts ([`SharedFonts`]), a face it lacks is cloned from the shared
//! set, or made there.
//!
//! **An instance is found without reading a byte.** Its key is what was
//! asked of the font: [`fontwich::Font::key`], the merged variation settings
//! in user space, the synthesis, the resolved features and the metric
//! overrides. The coordinates are normalized once, when the instance is
//! made. Two keys that normalize alike cost an extra cache entry, never a
//! wrong answer. No layout holds an [`InstanceId`].
//!
//! **Variations** merge by tag, the later winning, in Chrome's order (Blink's
//! `FontCustomPlatformData`, measured against Chrome 153):
//! 1. the axes font matching sets for weight, width and style ([`Synthesis`]);
//! 2. the `@font-face` descriptor's `font-variation-settings`;
//! 3. the element's `font-variation-settings`;
//! 4. `opsz` at the used size under `font-optical-sizing: auto`, where
//!    neither setting names `opsz`.
//!
//! A platform font may take the matching axes alone, as Chrome on Windows and
//! Linux leaves its variations to the platform
//! ([`PlatformFontVariations`]).

use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::{Hash, Hasher};
use core::mem;

use fontwich::{Attributes, Collection, Family, FontBytes, FontKey, Role, Synthesis};
use parlance::{FontFeature, FontVariation};
use read_fonts::model::{Font, Variation};
use read_fonts::types::Tag;

use super::UsedInstance;
use super::features;
use super::metrics::UnscaledMetrics;
use super::shared::{SharedFonts, face_font};
use crate::config::PlatformFontVariations;
use crate::data::{FxHasher, HeapBytes, LruCache, define_id};
use crate::stages::content::FontRequest;
use crate::style::{FontOpticalSizing, FontVariantCaps, FontVariantPosition};
use crate::style::{Same, style_struct};

define_id! {
    /// Names a font instance in the context's table of instances.
    ///
    /// Instances live across layouts, so two layouts setting text in one
    /// font share its bytes and metrics. Only font selection holds an id,
    /// in its scratch, for one build. A used font shares the instance's
    /// record of everything it is shaped and drawn with. No layout holds an
    /// id, so the table may drop entries between builds.
    pub(super) struct InstanceId(u32);
}

define_id! {
    /// Names a face in the context's table of faces.
    ///
    /// A face is found by a font's key whenever an instance of the font is
    /// made or its tables are read. Only the instances cut from it hold the
    /// id.
    struct FaceId(u32);
}

/// A font held whole at its default coordinates, which every instance of it
/// is cut from (see the module documentation).
pub(super) struct Face {
    /// fontwich's key for the font, which the face is found by.
    pub(super) key: FontKey,
    /// The font's family, by which a new collection says whether it still
    /// has the font.
    family: Family,
    /// The font's bytes, on which each instance's record takes a count.
    pub(super) bytes: FontBytes,
    /// The font held whole at its default coordinates.
    pub(super) font: Font,
    /// How many instances are cut from it.
    ///
    /// A face no instance is cut from goes at the next trim.
    instances: u32,
}

style_struct! {
    /// A font's `@font-face` metric descriptors, as ratios of the em.
    ///
    /// They are `size-adjust` and the ascent, descent and line gap
    /// overrides. fontwich carries them without applying them. A used font
    /// applies them, as Chrome does. `size-adjust` multiplies the size a
    /// font is used at, unless `font-size-adjust` sets it. Each override
    /// replaces its metric at that size.
    pub(super) struct FaceOverrides {
        /// `size-adjust`: the used size is multiplied by it.
        size_adjust: Option<f32>,
        /// `ascent-override`.
        ascent: Option<f32>,
        /// `descent-override`.
        descent: Option<f32>,
        /// `line-gap-override`.
        line_gap: Option<f32>,
    }
}

impl FaceOverrides {
    /// No descriptors: the font's own metrics at the used size.
    pub(super) const NONE: Self = Self {
        size_adjust: None,
        ascent: None,
        descent: None,
        line_gap: None,
    };

    /// Reads `font`'s descriptors, dropping invalid ones.
    ///
    /// A ratio that is not a finite number is dropped. So is a `size-adjust`
    /// that is not above zero, or a metric below zero.
    pub(super) fn new(font: &fontwich::Font) -> Self {
        let Some(descriptors) = font.descriptors() else {
            return Self::NONE;
        };
        let metric = |value: Option<f32>| value.filter(|v| v.is_finite() && *v >= 0.0);
        Self {
            size_adjust: descriptors
                .size_adjust
                .filter(|v| v.is_finite() && *v > 0.0),
            ascent: metric(descriptors.ascent_override),
            descent: metric(descriptors.descent_override),
            line_gap: metric(descriptors.line_gap_override),
        }
    }

    /// Returns `size-adjust`, where the face gives one.
    pub(super) fn size_adjust(&self) -> Option<f32> {
        self.size_adjust
    }

    /// Returns the ascent, descent and line gap overrides, as ratios of the
    /// em.
    pub(super) fn line(&self) -> (Option<f32>, Option<f32>, Option<f32>) {
        (self.ascent, self.descent, self.line_gap)
    }
}

/// A font instance: a font at a point in its design space, with its
/// synthesis and features.
#[derive(Clone, Debug)]
pub(crate) struct Instance {
    /// The face it is cut from, which counts it.
    face: FaceId,
    /// The merged variation settings asked for, in user space and tag order.
    ///
    /// They are part of the key.
    request: Box<[FontVariation]>,
    /// Its font's `@font-face` metric descriptors.
    overrides: FaceOverrides,
    /// The font's metrics at the coordinates.
    unscaled: UnscaledMetrics,
    /// Its record, which every used font set in it shares.
    ///
    /// It holds the font, its bytes, its coordinates, its synthesis and its
    /// features. The last three are part of the key.
    used: Arc<UsedInstance>,
}

impl Instance {
    /// Returns its record, which a used font set in it shares.
    pub(crate) fn used(&self) -> &Arc<UsedInstance> {
        &self.used
    }

    /// Returns its font's `@font-face` metric descriptors.
    pub(super) fn overrides(&self) -> FaceOverrides {
        self.overrides
    }

    /// Returns the font's metrics at the coordinates.
    pub(super) fn unscaled(&self) -> &UnscaledMetrics {
        &self.unscaled
    }

    /// Whether this is the instance `key` names.
    fn is(&self, key: &Key<'_>) -> bool {
        let used = &*self.used;
        used.key() == key.font
            && used.embolden == key.embolden
            && used.skew.map(f32::to_bits) == key.skew
            && *used.features == *key.features
            && self.overrides.same(&key.overrides)
            && same_settings(&self.request, key.request)
    }
}

/// Whether two lists of variation settings hold the same bits.
fn same_settings(a: &[FontVariation], b: &[FontVariation]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| a.same(b))
}

/// What names an instance, before it is stored.
struct Key<'a> {
    font: FontKey,
    request: &'a [FontVariation],
    embolden: bool,
    skew: Option<u32>,
    features: &'a [FontFeature],
    overrides: FaceOverrides,
}

impl Key<'_> {
    fn hash(&self) -> u64 {
        let mut fx = FxHasher::new();
        Hash::hash(&self.font, &mut fx);
        fx.write_usize(self.request.len());
        for setting in self.request {
            setting.feed(&mut fx);
        }
        fx.write_u8(u8::from(self.embolden));
        self.skew.feed(&mut fx);
        fx.write_usize(self.features.len());
        for feature in self.features {
            feature.feed(&mut fx);
        }
        self.overrides.feed(&mut fx);
        fx.finish()
    }
}

/// What a font request asks of one font, for the instance it is set in.
pub(super) struct InstanceRequest<'a> {
    /// The font request.
    ///
    /// Its weight, width, slope, synthesis, optical sizing,
    /// `font-variant-alternates` and the features its text chooses are read.
    pub(super) font_request: &'a FontRequest,
    /// Its `font-variation-settings`.
    pub(super) variations: &'a [FontVariation],
    /// Its `font-feature-settings`.
    pub(super) settings: &'a [FontFeature],
    /// Whether its text is set upright or mixed in a vertical line.
    ///
    /// Such text shapes with the vertical forms of the spacing features, as
    /// Blink's `FontDescription::IsVerticalAnyUpright` chooses them for the
    /// whole style. A run of it on its side shapes with them too, where they
    /// do nothing.
    pub(super) upright: bool,
    /// The capitals feature the font sets its text with (see `CapsPlan`).
    pub(super) caps: FontVariantCaps,
    /// The position whose feature the font sets its text with.
    ///
    /// It is the style's `font-variant-position`, or `normal` where the
    /// position is synthesized. Synthesized text draws from the glyphs the
    /// feature would have replaced (CSS Fonts 4, section 6.5).
    pub(super) position: FontVariantPosition,
    /// The size the font is used at, in pixels, which `opsz` follows under
    /// `font-optical-sizing: auto`.
    pub(super) size: f32,
    /// The font's `@font-face` metric descriptors, which its size was
    /// worked out with.
    pub(super) overrides: FaceOverrides,
    /// Whether this is a platform font whose variations are left to font
    /// matching, so only the axes matching sets apply.
    pub(super) matching_only: bool,
}

/// The context's font instances, and the faces they are cut from.
///
/// Each instance is a font as a style asks for it, with its bytes, its
/// coordinates and its unscaled metrics. There is one face per font.
///
/// This is a cache, trimmed between builds to the instances used last,
/// since no layout holds an [`InstanceId`]. A change of collection keeps
/// the instances of the fonts the new collection still has, since a font's
/// key is its bytes' id, which fontwich keeps stable. A face is made only
/// for a font that font selection reads, and goes at the first trim no
/// instance is cut from it.
pub(crate) struct Instances {
    table: LruCache<InstanceId, Instance>,
    /// The faces the instances are cut from, one a font.
    ///
    /// They are dropped by count, not by use, so their capacity is never
    /// reached.
    faces: LruCache<FaceId, Face>,
    /// Whether a face was made or lost an instance since the last trim, so
    /// that a face no instance is cut from may be left.
    faces_changed: bool,
    /// Working memory for one request's merged variation settings.
    request: Vec<FontVariation>,
    /// Working memory for one request's merged features.
    features: Vec<FontFeature>,
    /// The fonts the context shares with others, which a face it lacks is
    /// taken from.
    shared: Option<SharedFonts>,
}

impl Instances {
    /// Makes an empty cache that keeps `capacity` instances and takes the
    /// faces it lacks from `shared`, allocating nothing.
    pub(super) const fn new(capacity: usize, shared: Option<SharedFonts>) -> Self {
        Self {
            table: LruCache::new(capacity),
            faces: LruCache::new(usize::MAX),
            faces_changed: false,
            request: Vec::new(),
            features: Vec::new(),
            shared,
        }
    }

    /// Drops every instance and face, keeping `capacity` instances from now
    /// on and the shared fonts.
    pub(super) fn clear(&mut self, capacity: usize) {
        *self = Self::new(capacity, self.shared.take());
    }

    /// Sets how many instances a trim keeps.
    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.table.set_capacity(capacity);
    }

    /// Drops the instances used longest ago down to the capacity, and every
    /// face no instance is cut from.
    ///
    /// Font selection calls it between builds, when no [`InstanceId`] is
    /// held.
    pub(super) fn trim(&mut self) {
        let Self {
            table,
            faces,
            faces_changed,
            ..
        } = self;
        table.trim(|_, instance| {
            if let Some(face) = faces.get_mut(instance.face) {
                face.instances = face.instances.saturating_sub(1);
            }
            *faces_changed = true;
        });
        if mem::take(faces_changed) {
            faces.retain(|_, face| face.instances > 0, |_, _| {});
        }
    }

    /// Drops the faces of the fonts `collection` lacks, with every instance
    /// cut from them, so their bytes are released.
    pub(super) fn retain_collection(&mut self, collection: &Collection) {
        let Self { table, faces, .. } = self;
        faces.retain(|_, face| collection.contains(&face.family), |_, _| {});
        table.retain(|_, instance| faces.get(instance.face).is_some(), |_, _| {});
    }

    /// Whether the face of the font `key` names is held.
    pub(super) fn has_face(&self, key: FontKey) -> bool {
        self.faces
            .peek(face_hash(key), |face| face.key == key)
            .is_some()
    }

    /// Returns the bytes of the fonts the faces hold, which the collection
    /// shares.
    pub(super) fn face_bytes(&self) -> usize {
        self.faces
            .iter()
            .map(|(_, face)| face.bytes.data().len())
            .sum()
    }

    /// Returns the instance `id` names, or `None` for an id this context did
    /// not hand out.
    pub(super) fn get(&self, id: InstanceId) -> Option<&Instance> {
        self.table.get(id)
    }

    /// Returns the first instance of the font `key` names, for tests.
    ///
    /// A layout names no instance, but holds its fonts' keys.
    #[cfg(test)]
    pub(crate) fn first_instance(&self, key: FontKey) -> Option<&Instance> {
        self.all().find(|instance| instance.used.key() == key)
    }

    /// Every instance, for tests to look at.
    #[cfg(test)]
    pub(super) fn all(&self) -> impl Iterator<Item = &Instance> {
        self.table.iter().map(|(_, instance)| instance)
    }

    /// Every face, for tests to see one made per font.
    #[cfg(test)]
    pub(super) fn faces(&self) -> impl Iterator<Item = &Face> {
        self.faces.iter().map(|(_, face)| face)
    }

    /// Finds or makes the face of `font`, of `family`.
    ///
    /// Returns `None` for a font whose bytes are not here or are not a font
    /// read-fonts reads, or when every face id is taken.
    pub(super) fn face(&mut self, family: &Family, font: &fontwich::Font) -> Option<&Face> {
        self.faces_changed = true;
        let id = find_or_make_face(&mut self.faces, self.shared.as_ref(), family, font)?;
        self.faces.get(id)
    }

    /// Merges into `self.request` the variation settings `request` makes of
    /// `font`.
    ///
    /// `synthesis` is the font's synthesis for the style. The settings merge
    /// in Chrome's order (see the module documentation). Each tag appears
    /// once, at the value set last, in tag order. A value that is not a
    /// finite number sets nothing.
    fn merge_variations(
        &mut self,
        font: &fontwich::Font,
        request: &InstanceRequest<'_>,
        synthesis: &Synthesis,
    ) {
        let merged = &mut self.request;
        merged.clear();
        merged.extend_from_slice(synthesis.variation_settings());
        if !request.matching_only {
            let descriptor = font
                .descriptors()
                .map_or(&[][..], |d| d.variation_settings.as_slice());
            let opsz = parlance::Tag::new(b"opsz");
            let explicit = descriptor
                .iter()
                .chain(request.variations)
                .any(|setting| setting.tag == opsz);
            merged.extend_from_slice(descriptor);
            merged.extend_from_slice(request.variations);
            if !explicit
                && request.font_request.font.optical_sizing == FontOpticalSizing::Auto
                && font.axis(b"opsz").is_some()
            {
                merged.push(FontVariation::new(opsz, request.size));
            }
        }
        merged.retain(|setting| setting.value.is_finite());
        features::settle(merged, |setting| setting.tag);
    }

    /// Finds or makes the instance `font` is set in as `request` asks.
    ///
    /// Finding one reads nothing. The key is fontwich's font key and what
    /// the font's record and the style give, with the variations and
    /// features merged in working memory.
    ///
    /// Making one cuts the font from its face at the normalized settings,
    /// so it shares what every instance of the font has read. The face is
    /// made the first time, which loads the bytes. It also reads the
    /// unscaled metrics, once per context.
    ///
    /// Returns `None` where the font's bytes are not here (pending, or a
    /// file that cannot be read) or are not a font read-fonts reads. Also
    /// returns `None` when the instances' or the faces' table is full.
    pub(super) fn find_or_make(
        &mut self,
        family: &Family,
        font: &fontwich::Font,
        request: &InstanceRequest<'_>,
    ) -> Option<InstanceId> {
        let style = &request.font_request.font;
        let attributes = Attributes {
            width: style.width,
            style: style.style,
            weight: style.weight,
        };
        let key = font.key()?;
        let synthesis = font.synthesis(attributes);
        self.merge_variations(font, request, &synthesis);
        features::merge_features(
            font,
            request.font_request,
            request.settings,
            request.upright,
            request.caps,
            request.position,
            &mut self.features,
        );
        let Self {
            table,
            faces,
            faces_changed,
            request: merged,
            features,
            shared,
        } = self;
        let key = Key {
            font: key,
            request: merged,
            embolden: synthesis.embolden() && style.synthesis.weight,
            skew: synthesis
                .skew()
                .filter(|_| style.synthesis.style)
                .map(f32::to_bits),
            features,
            overrides: request.overrides,
        };
        let hash = key.hash();
        if let Some(id) = table.find(hash, |instance| instance.is(&key)) {
            return Some(id);
        }
        let face_id = find_or_make_face(faces, shared.as_ref(), family, font)?;
        *faces_changed = true;
        let face = faces.get_mut(face_id)?;
        // Cut from the face, so it shares what every instance of the font
        // has read of it. Each setting is clamped to its axis and mapped by
        // `fvar` and then `avar`. A tag the font has no axis for is dropped.
        // The default instance holds no coordinates: it is the face's font.
        let held =
            face.font
                .instance_builder()
                .variations(key.request.iter().map(|setting| {
                    Variation::new(Tag::new(&setting.tag.to_bytes()), setting.value)
                }))
                .build();
        let unscaled = UnscaledMetrics::new(&held);
        let used = UsedInstance {
            bytes: face.bytes.clone(),
            index: font.index(),
            coords: held
                .normalized_coords()
                .iter()
                .map(|coord| coord.to_bits())
                .collect(),
            font: held,
            features: Box::from(key.features),
            embolden: key.embolden,
            skew: key.skew.map(f32::from_bits),
        };
        let made = table.insert(
            hash,
            Instance {
                face: face_id,
                request: Box::from(key.request),
                overrides: key.overrides,
                unscaled,
                used: Arc::new(used),
            },
        )?;
        face.instances = face.instances.saturating_add(1);
        Some(made)
    }

    /// How many instances there are.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.table.len()
    }

    /// How many faces there are.
    #[cfg(test)]
    pub(crate) fn face_count(&self) -> usize {
        self.faces.len()
    }
}

/// Returns the hash the face of the font `key` names is found by.
fn face_hash(key: FontKey) -> u64 {
    let mut fx = FxHasher::new();
    Hash::hash(&key, &mut fx);
    fx.finish()
}

/// Finds the face of `font`, of `family`, in `faces` by its key, or makes
/// it.
///
/// Making it takes the bytes and the font held at its default coordinates
/// from `shared` where given, and otherwise loads and reads them. Returns
/// `None` for a font whose bytes are not here or are not a font read-fonts
/// reads, or when every face id is taken.
fn find_or_make_face(
    faces: &mut LruCache<FaceId, Face>,
    shared: Option<&SharedFonts>,
    family: &Family,
    font: &fontwich::Font,
) -> Option<FaceId> {
    let key = font.key()?;
    let hash = face_hash(key);
    if let Some(id) = faces.find(hash, |face| face.key == key) {
        return Some(id);
    }
    let (bytes, held) = face_font(shared, family, font, key, hash)?;
    faces.insert(
        hash,
        Face {
            key,
            family: family.clone(),
            bytes,
            font: held,
            instances: 0,
        },
    )
}

impl HeapBytes for Instances {
    /// Its tables, its working memory, and each instance's settings and
    /// record with the record's coordinates and features.
    ///
    /// The fonts' bytes do not count, since the collection shares them;
    /// [`face_bytes`](Self::face_bytes) counts them apart. Nor does what
    /// read-fonts keeps inside a font, which it does not report.
    fn heap_bytes(&self) -> usize {
        let Self {
            table,
            faces,
            faces_changed: _,
            request,
            features,
            shared: _,
        } = self;
        let own: usize = table
            .iter()
            .map(|(_, instance)| {
                let used = &*instance.used;
                size_of_val::<[FontVariation]>(&instance.request)
                    + size_of::<UsedInstance>()
                    + size_of_val::<[i16]>(&used.coords)
                    + size_of_val::<[FontFeature]>(&used.features)
            })
            .sum();
        let working = request.heap_bytes() + features.heap_bytes();
        table.heap_bytes() + faces.heap_bytes() + working + own
    }
}

/// Whether `family`'s variations are left to font matching.
///
/// True for a platform font when the config's choice `platform` follows
/// Chrome.
pub(super) fn variations_left_to_matching(
    family: &Family,
    platform: PlatformFontVariations,
) -> bool {
    family.role() == Role::System && platform == PlatformFontVariations::MatchingOnly
}
