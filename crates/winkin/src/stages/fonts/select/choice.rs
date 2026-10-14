//! Makes a candidate's choice: the used fonts it sets a span's text in,
//! kept per request, script and candidate for one build.

use fontwich::{Charset, Family, Font};
use hashbrown::HashTable;

use super::{Input, decides_position};
use crate::data::{hash_one, heap_bytes};
use crate::stages::content::{FontRequest, FontRequestId};
use crate::stages::fonts::context::{CacheChange, FontCaches};
use crate::stages::fonts::features::{self, CapsPlan, CaseSide, OfferedFeatures};
use crate::stages::fonts::instance::variations_left_to_matching;
use crate::stages::fonts::instance::{FaceOverrides, InstanceRequest};
use crate::stages::fonts::lists::{CandidateId, CandidateListId};
use crate::stages::fonts::positions::FeatureCoverages;
use crate::stages::fonts::used::{UsedFontInterner, used_size};
use crate::stages::fonts::{CaseMap, FontResolution, UsedFontId, UsedSynthesis};
use crate::style::{FontVariantCaps, FontVariantPosition};
use crate::unit::{self, LayoutUnit};

/// The choices made in the current build.
pub(super) struct Choices {
    table: HashTable<(u64, ChoiceKey, Choice)>,
}

impl Choices {
    /// Returns an empty table, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            table: HashTable::new(),
        }
    }

    /// Clears the choices, keeping the capacity.
    pub(super) fn clear(&mut self) {
        self.table.clear();
    }

    /// Returns the choice made for `key` in this build, or makes it with
    /// `choose` and keeps it.
    ///
    /// Where `choose` fails because the candidate's bytes are gone, returns
    /// the change that marks the candidate unusable. The walk and generated
    /// text both make choices here.
    pub(super) fn find_or_choose(
        &mut self,
        key: ChoiceKey,
        choose: impl FnOnce() -> Option<Choice>,
    ) -> Result<Choice, CacheChange> {
        let hash = hash_one(&key);
        if let Some(&(.., choice)) = self
            .table
            .find(hash, |&(h, held, _)| h == hash && held == key)
        {
            return Ok(choice);
        }
        let choice = choose().ok_or(CacheChange::Unusable(key.list, key.candidate))?;
        self.table
            .insert_unique(hash, (hash, key, choice), |&(h, _, _)| h);
        Ok(choice)
    }
}

/// The key of a [`Choice`]: a font request, a script, and a candidate of a
/// list.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct ChoiceKey {
    request: FontRequestId,
    pub(super) script: [u8; 4],
    pub(super) list: CandidateListId,
    pub(super) candidate: CandidateId,
}

heap_bytes! {
    Choices { table }
}

/// What a font request's text under a script asks of a font, whichever
/// list the font comes from. [`choose`] reads it.
#[derive(Copy, Clone)]
pub(super) struct ResolvedRequest<'a> {
    /// The request, whose variations and features key each instance it
    /// chooses, and its id.
    pub(super) request: &'a FontRequest,
    pub(super) request_id: FontRequestId,
    /// The request's resolution: its sizes, whether it is set upright, and
    /// its primary font.
    ///
    /// The primary font draws what no font covers, and stands in where the
    /// table of used fonts is full.
    pub(super) resolution: FontResolution,
    /// The OpenType script under which the fonts' capitals and position
    /// features are looked up.
    pub(super) script: [u8; 4],
}

impl ResolvedRequest<'_> {
    /// Returns the key of this request's choice in `candidate` of `list`.
    pub(super) fn key(&self, list: CandidateListId, candidate: CandidateId) -> ChoiceKey {
        ChoiceKey {
            request: self.request_id,
            script: self.script,
            list,
            candidate,
        }
    }
}

/// The used fonts a candidate sets a span's text in.
///
/// That is one font, or one for each side of a change of case where its
/// capitals are synthesized. Where a position run decides, a synthesized
/// pair comes besides.
#[derive(Copy, Clone, Debug)]
pub(super) struct Choice {
    /// The font for text that does not change when uppercased.
    pub(super) same: UsedFontId,
    /// The font for text that does, equal to `same` where the two are set
    /// alike.
    pub(super) upper: UsedFontId,
    /// How the candidate sets text under `font-variant-position`.
    pub(super) position: PositionPlan,
}

/// How a candidate sets the text of a style whose `font-variant-position`
/// may be synthesized.
#[derive(Copy, Clone, Debug)]
pub(super) enum PositionPlan {
    /// Nothing to decide: `same` and `upper` set the text, with the feature
    /// the style asks for, if any, as Chrome sets it.
    Settled,
    /// A font that lacks the feature under the script: `same` and `upper`
    /// are synthesized, and so is every run with text in the font.
    Lacking,
    /// A font that has the feature: `same` and `upper` shape with it for a
    /// run it covers whole, and these synthesized fonts set any other run.
    Covering {
        /// `sups` or `subs`.
        feature: [u8; 4],
        same: UsedFontId,
        upper: UsedFontId,
    },
}

/// Returns the charset of the position feature of `choice`, made for `key`
/// in `font`, where a position run decides on it.
///
/// The charset is read once per font, script and feature, the first time a
/// run asks. Returns the change that reads it where it is not read yet.
/// `None` where nothing decides, or where the font has no key.
pub(super) fn feature_coverage<'c>(
    coverages: &'c FeatureCoverages,
    choice: Choice,
    font: &Font,
    key: ChoiceKey,
) -> Result<Option<&'c Charset>, CacheChange> {
    let PositionPlan::Covering { feature, .. } = choice.position else {
        return Ok(None);
    };
    let Some(font) = font.key() else {
        return Ok(None);
    };
    match coverages.find(font, key.script, feature) {
        Some(chars) => Ok(Some(chars)),
        None => Err(CacheChange::Coverage {
            list: key.list,
            candidate: key.candidate,
            script: key.script,
            feature,
        }),
    }
}

/// Returns the used fonts that `font`, a font of `family`, sets the text
/// `resolved` describes in.
///
/// `None` where the font's bytes are gone. Makes the instances in `fonts`
/// and the used fonts in `used` the first time a build asks. The font's
/// offered features under the script decide how it sets capitals
/// ([`CapsPlan`]) and whether it has the position feature. The style and the
/// `@font-face` descriptors decide its size and instance.
///
/// Where a position run decides and the font has the feature, both versions
/// are made, since only the run's end says which one is used. The
/// synthesized one uses an instance without the feature, whose glyphs are
/// the ones the feature would replace (CSS Fonts 4, section 6.5). Firefox's
/// `gfxFontStyle::AdjustForSubSuperscript` clears the position the same way.
pub(super) fn choose(
    input: &Input<'_>,
    resolved: &ResolvedRequest<'_>,
    font: &Font,
    family: &Family,
    fonts: &mut FontCaches<'_>,
    used: &mut UsedFontInterner<'_>,
) -> Option<Choice> {
    let request = resolved.request;
    let caps = request.font.variant_caps;
    let position = request.font.variant_position;
    let stage = input.stage;
    let decides = decides_position(request, stage.position_synthesis);
    let offered = if caps != FontVariantCaps::Normal || decides {
        fonts
            .offers
            .find_or_read(family, font, resolved.script, fonts.instances)
    } else {
        OfferedFeatures::NONE
    };
    let plan = CapsPlan::new(caps, offered, request.font.synthesis.small_caps);
    let feature = features::position_tag(position);
    let has_feature = offered.has_position(position);
    let overrides = FaceOverrides::new(font);
    let px = resolved.resolution.font_px(overrides);
    let size = used_size(px);
    let matching_only = variations_left_to_matching(family, stage.platform_font_variations);
    let lists = stage.lists;
    let variations = lists.variation_settings().get(request.font.variations);
    let settings = lists.feature_settings().get(request.font.features);
    // The used font for `side`: with the position's feature, or synthesized
    // without it.
    let mut used_for = |side: CaseSide, synthesized: bool| -> Option<UsedFontId> {
        let instance_request = InstanceRequest {
            font_request: request,
            variations,
            settings,
            upright: resolved.resolution.upright,
            caps: side.caps(),
            position: if synthesized {
                FontVariantPosition::Normal
            } else {
                position
            },
            size: px,
            overrides,
            matching_only,
        };
        let instance = fonts
            .instances
            .find_or_make(family, font, &instance_request)?;
        let unscaled = fonts.instances.get(instance)?.unscaled();
        let moved = if synthesized {
            unscaled.synthesized_position(position, px)
        } else {
            None
        };
        let synthesis = side_synthesis(side, moved, resolved.resolution.computed, px);
        let instances = &*fonts.instances;
        Some(used.intern_or(
            Some(instance),
            size,
            synthesis,
            instances,
            resolved.resolution.primary,
        ))
    };
    let mut pair = |synthesized: bool| -> Option<(UsedFontId, UsedFontId)> {
        let same = used_for(plan.same(), synthesized)?;
        let upper = if plan.is_uniform() {
            same
        } else {
            used_for(plan.upper(), synthesized)?
        };
        Some((same, upper))
    };
    let ((same, upper), position) = match feature {
        Some(feature) if decides && has_feature => {
            let with = pair(false)?;
            let (same, upper) = pair(true)?;
            (
                with,
                PositionPlan::Covering {
                    feature,
                    same,
                    upper,
                },
            )
        }
        Some(_) if decides => (pair(true)?, PositionPlan::Lacking),
        _ => (pair(false)?, PositionPlan::Settled),
    };
    Some(Choice {
        same,
        upper,
        position,
    })
}

/// Returns the synthesis for a used font that sets the text on `side` of a
/// change of case.
///
/// `position` is a synthesized position's glyph size in pixels and raise,
/// if any. `computed` is the style's computed size and `px` the font's used
/// size, both in pixels.
///
/// Synthesized small capitals are uppercase at 0.7 of the computed size,
/// rounded to a whole pixel, whatever `font-size-adjust` did. This matches
/// Blink's `SimpleFontData::SmallCapsFontData`
/// (`kSmallCapsFontSizeMultiplier`, `lroundf`). With a synthesized position
/// too, they are 0.7 of the position's glyph size, unrounded.
fn side_synthesis(
    side: CaseSide,
    position: Option<(f32, LayoutUnit)>,
    computed: f32,
    px: f32,
) -> UsedSynthesis {
    let case = side.case_map();
    if case == CaseMap::Keep && position.is_none() {
        return UsedSynthesis::None;
    }
    let (glyph_px, raise) = position.unwrap_or((px, LayoutUnit::ZERO));
    let glyph_px = match (side.is_synthetic(), position) {
        (true, None) => unit::round_to_whole(computed * SMALL_CAPS),
        (true, Some(_)) => glyph_px * SMALL_CAPS,
        (false, _) => glyph_px,
    };
    UsedSynthesis::Synthesized {
        case,
        glyph_size: used_size(glyph_px),
        raise,
    }
}

/// The scale of synthesized small capitals to the size: Blink's
/// `kSmallCapsFontSizeMultiplier`.
const SMALL_CAPS: f32 = 0.7;
