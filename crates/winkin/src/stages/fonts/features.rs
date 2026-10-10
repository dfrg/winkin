//! The OpenType features a style asks for, and how a font sets capitals.
//!
//! It also holds [`ScriptOffers`], the context's cache of which synthesized
//! features a font offers under a script.
//!
//! **The order is Chrome's**, read from Blink's source, since the
//! specification's list differs at the edges:
//! 1. the `@font-face` descriptor's features and `font-variant-alternates`
//!    (`FontDescription::ResolveFontFeatures`);
//! 2. the capitals `font-variant-caps` asks for, which the shaper prepends
//!    (`CapsFeatureSettingsScopedOverlay`);
//! 3. `font-kerning`, the ligatures, `font-variant-east-asian` and
//!    `font-variant-numeric`;
//! 4. `font-feature-settings`;
//! 5. `font-variant-position`'s `sups` or `subs`
//!    (`FontFeatureRange::FromFontDescription`).
//!
//! A later setting of a tag wins, as HarfBuzz merges global features. So an
//! instance keeps its list with each tag once, at its last value, in tag
//! order. Any order that comes to the same settings gives one list.
//!
//! **Capitals** follow Blink's `OpenTypeCapsSupport`. A font that has the
//! asked feature draws it. One that lacks it may fall back to another it
//! has: petite capitals to small ones, unicase to small capitals. One with
//! nothing gets synthesized small capitals, where
//! `font-synthesis-small-caps` allows. The letters that change when
//! uppercased are then fed uppercased and drawn smaller, and the text is
//! divided where that changes, as Blink's `SmallCapsIterator` divides it.
//! Whether a font has a feature is read from its `GSUB` under the run's
//! script, once per font and script.

use alloc::vec::Vec;
use core::hash::{Hash, Hasher};

use fontwich::{Family, Font, FontKey};
use parlance::{FontFeature, Tag};
use read_fonts::model::Font as HeldFont;
use read_fonts::tables::gsub::{FeatureList, Gsub};
use read_fonts::tables::layout::FeatureRecord;
use read_fonts::{TableProvider, types};

use super::CaseMap;
use super::instance::Instances;
use crate::data::{FxHasher, LruCache, define_flags, define_id, heap_bytes, stable_sort_by_key};
use crate::stages::content::FontRequest;
use crate::style::{FontKerning, FontVariantCaps, FontVariantPosition, FontVariants};

define_id! {
    /// Names what one font offers under one script in the context's table
    /// of [`ScriptOffer`]s.
    struct ScriptOfferId(u32);
}

define_flags! {
    /// Which synthesized-where-missing features a font has under a script.
    ///
    /// There is one bit each for the features Blink's `OpenTypeCapsSupport`
    /// asks HarfBuzz about, and for `sups` and `subs`.
    pub(super) struct OfferedFeatures(u8) {
        const SMCP = 1 << 0;
        const C2SC = 1 << 1;
        const PCAP = 1 << 2;
        const C2PC = 1 << 3;
        const UNIC = 1 << 4;
        const TITL = 1 << 5;
        const SUPS = 1 << 6;
        const SUBS = 1 << 7;
    }
}

impl OfferedFeatures {
    /// The tags, in bit order.
    const TAGS: [[u8; 4]; 8] = [
        *b"smcp", *b"c2sc", *b"pcap", *b"c2pc", *b"unic", *b"titl", *b"sups", *b"subs",
    ];

    /// Whether the feature `position` asks for, `sups` or `subs`, is offered.
    ///
    /// Always false for `normal`, which asks for none.
    pub(super) const fn has_position(self, position: FontVariantPosition) -> bool {
        match position {
            FontVariantPosition::Normal => false,
            FontVariantPosition::Super => self.contains(Self::SUPS),
            FontVariantPosition::Sub => self.contains(Self::SUBS),
        }
    }

    /// Reads what `font` offers under the OpenType script `script`.
    ///
    /// It reads the language system [`script_features`] picks. A font with
    /// no readable `GSUB` offers nothing. It never panics.
    fn new(font: &HeldFont, script: [u8; 4]) -> Self {
        let Ok(gsub) = font.tables().gsub() else {
            return Self::NONE;
        };
        let mut offered = Self::NONE;
        script_features(&gsub, script, |record, _| {
            let tag = record.feature_tag().to_be_bytes();
            if let Some(bit) = Self::TAGS.iter().position(|&known| known == tag) {
                offered.0 |= 1 << bit;
            }
        });
        offered
    }
}

/// One font's offer under one script: which synthesized-where-missing
/// features it has.
#[derive(Clone, Debug)]
struct ScriptOffer {
    font: FontKey,
    script: [u8; 4],
    offered: OfferedFeatures,
}

/// Which synthesized-where-missing features each font offers under each
/// script.
///
/// Each offer is read from the font's `GSUB` once per font and script. This
/// is a cache. It survives a change of collection, since a font's key is its
/// bytes' id. It is trimmed between builds to the offers used last, since no
/// layout holds an offer's id.
pub(super) struct ScriptOffers {
    offers: LruCache<ScriptOfferId, ScriptOffer>,
}

impl ScriptOffers {
    /// Makes an empty cache that keeps `capacity` offers, allocating
    /// nothing.
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            offers: LruCache::new(capacity),
        }
    }

    /// Sets how many offers a trim keeps.
    pub(super) fn set_capacity(&mut self, capacity: usize) {
        self.offers.set_capacity(capacity);
    }

    /// Drops the offers used longest ago down to the capacity.
    pub(super) fn trim(&mut self) {
        self.offers.trim(|_, _| {});
    }

    /// How many offers are held, for tests to see them bounded.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.offers.len()
    }

    /// Returns what `font`, of `family`, offers under the OpenType script
    /// `script`.
    ///
    /// The first time a context asks, it reads the `GSUB` of the font's face
    /// in `instances`. Returns nothing where the face cannot be had.
    pub(super) fn find_or_read(
        &mut self,
        family: &Family,
        font: &Font,
        script: [u8; 4],
        instances: &mut Instances,
    ) -> OfferedFeatures {
        let Some(key) = font.key() else {
            return OfferedFeatures::NONE;
        };
        let mut fx = FxHasher::new();
        Hash::hash(&key, &mut fx);
        fx.write(&script);
        let hash = fx.finish();
        let found = self
            .offers
            .find(hash, |held| held.font == key && held.script == script);
        if let Some(id) = found {
            return self
                .offers
                .get(id)
                .map_or(OfferedFeatures::NONE, |held| held.offered);
        }
        let Some(face) = instances.face(family, font) else {
            return OfferedFeatures::NONE;
        };
        let offered = OfferedFeatures::new(&face.font, script);
        // Where every id is taken, the answer is not kept, and asked again.
        let _kept = self.offers.insert(
            hash,
            ScriptOffer {
                font: key,
                script,
                offered,
            },
        );
        offered
    }
}

heap_bytes! {
    ScriptOffers { offers }
}

/// Merges into `out` the features an instance of `font` shapes with.
///
/// `settings` is the request's `font-feature-settings`. `upright` says its
/// text is set upright in a vertical line. `caps` is the capitals feature
/// and `position` the position whose feature applies.
///
/// The features merge and settle in Blink's order:
/// `font-variant-alternates` and the descriptor's features the alternates do
/// not set, the capitals, the request's own ([`push_request_features`]),
/// then the position's. Blink appends the position's after every other,
/// `font-feature-settings` included. So it wins over a setting of its tag,
/// as Chrome 153 draws superscripts under `'sups' 0`. The list is part of
/// the instance's key, so equal lists share one instance.
pub(super) fn merge_features(
    font: &Font,
    request: &FontRequest,
    settings: &[FontFeature],
    upright: bool,
    caps: FontVariantCaps,
    position: FontVariantPosition,
    out: &mut Vec<FontFeature>,
) {
    out.clear();
    let hist = Tag::new(b"hist");
    let historical = request
        .font
        .variants
        .contains(FontVariants::HISTORICAL_FORMS);
    if historical {
        out.push(FontFeature::new(hist, 1));
    }
    if let Some(descriptors) = font.descriptors() {
        out.extend(
            descriptors
                .feature_settings
                .iter()
                .filter(|feature| !(historical && feature.tag == hist)),
        );
    }
    out.extend_from_slice(caps_features(caps));
    push_request_features(request, settings, upright, out);
    out.extend(position_tag(position).map(|tag| FontFeature::new(Tag::new(&tag), 1)));
    settle(out, |feature| feature.tag);
}

/// Calls `each` with every feature record of `gsub` that the OpenType
/// script `script` turns on, and the feature list it is in.
///
/// These are the script's default language system's features and its
/// required one. The script falls back as HarfBuzz's
/// `hb_ot_layout_table_select_script` does, to `DFLT`, `dflt` and `latn`.
/// What a font offers and what an offered feature covers (see `positions`)
/// are both read in this one language system, so the two agree. A record an
/// index does not reach, or a table that cannot be read, is passed over. It
/// never panics.
pub(super) fn script_features<'a>(
    gsub: &Gsub<'a>,
    script: [u8; 4],
    mut each: impl FnMut(&FeatureRecord, &FeatureList<'a>),
) {
    let (Ok(scripts), Ok(features)) = (gsub.script_list(), gsub.feature_list()) else {
        return;
    };
    let records = scripts.script_records();
    let chosen = [script, *b"DFLT", *b"dflt", *b"latn"]
        .into_iter()
        .find_map(|wanted| {
            let wanted = types::Tag::new(&wanted);
            records.iter().find(|record| record.script_tag() == wanted)
        });
    let Some(record) = chosen else {
        return;
    };
    let Some(Ok(system)) = record
        .script(scripts.offset_data())
        .ok()
        .and_then(|script| script.default_lang_sys())
    else {
        return;
    };
    let records = features.feature_records();
    let required = system.required_feature_index();
    let indices = system
        .feature_indices()
        .iter()
        .map(|index| index.get())
        .chain((required != 0xFFFF).then_some(required));
    for index in indices {
        if let Some(record) = records.get(usize::from(index)) {
            each(record, &features);
        }
    }
}

/// Returns the OpenType script tag HarfBuzz asks a font's `GSUB` for, for an
/// ISO 15924 script.
///
/// It is the code lowercased, except for the scripts OpenType names
/// otherwise. Common and inherited text asks for the default script.
/// HarfBuzz tries the Indic scripts' second-generation tags first; this does
/// not. Only capitals and positions are read here, and no Indic script has
/// case.
pub(super) fn opentype_script(script: parlance::Script) -> [u8; 4] {
    let code = script.to_bytes();
    match &code {
        b"Zyyy" | b"Zinh" | b"Zzzz" => *b"DFLT",
        b"Hira" | b"Kana" | b"Hrkt" => *b"kana",
        b"Laoo" => *b"lao ",
        b"Yiii" => *b"yi  ",
        b"Nkoo" => *b"nko ",
        b"Vaii" => *b"vai ",
        b"Hans" | b"Hant" | b"Hani" | b"Jpan" => *b"hani",
        b"Kore" => *b"hang",
        _ => code.map(|byte| byte.to_ascii_lowercase()),
    }
}

/// How a font sets a style's capitals, for each kind of letter.
///
/// It is Blink's `OpenTypeCapsSupport`, answered once for a font.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct CapsPlan {
    /// How text that does not change when uppercased is set.
    ///
    /// This covers capitals, digits, punctuation, and the letters of
    /// scripts without case.
    same: CaseSide,
    /// How text that changes when uppercased, the lowercase letters, is set.
    upper: CaseSide,
}

/// How the text on one side of a change of case is set under a
/// [`CapsPlan`].
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) struct CaseSide {
    /// The capitals feature it shapes with (`FontFeatureToUse`).
    caps: FontVariantCaps,
    /// Whether it is drawn as synthesized small capitals: fed uppercased,
    /// drawn smaller.
    synthetic: bool,
    /// Whether it is fed lowercased, to take the font's small capitals.
    ///
    /// Unicase does this when it falls back to small capitals.
    lower: bool,
}

impl CaseSide {
    /// Returns the capitals feature it shapes with.
    pub(super) fn caps(self) -> FontVariantCaps {
        self.caps
    }

    /// Whether it is drawn as synthesized small capitals, smaller than its
    /// font's size.
    pub(super) fn is_synthetic(self) -> bool {
        self.synthetic
    }

    /// Returns how its text is fed to the shaper.
    ///
    /// Text is uppercased for synthesized small capitals, lowercased to take
    /// a font's small capitals, and otherwise kept as written.
    pub(super) fn case_map(self) -> CaseMap {
        if self.synthetic {
            CaseMap::Upper
        } else if self.lower {
            CaseMap::Lower
        } else {
            CaseMap::Keep
        }
    }
}

impl CapsPlan {
    /// Makes a plan that sets every letter as written, shaping with `caps`.
    const fn from_feature(caps: FontVariantCaps) -> Self {
        let side = CaseSide {
            caps,
            synthetic: false,
            lower: false,
        };
        Self {
            same: side,
            upper: side,
        }
    }

    /// Returns how text that does not change when uppercased is set.
    pub(super) fn same(&self) -> CaseSide {
        self.same
    }

    /// Returns how text that changes when uppercased is set.
    pub(super) fn upper(&self) -> CaseSide {
        self.upper
    }

    /// Whether the two kinds are set alike, so text need not be divided.
    pub(super) fn is_uniform(&self) -> bool {
        self.same == self.upper
    }

    /// Plans how a font offering `offered` sets capitals of `caps`.
    ///
    /// It synthesizes where `synthesize` (`font-synthesis-small-caps: auto`)
    /// allows. Blink's `DetermineFontSupport` sorts the font into full
    /// support (the feature is there), a fallback, or none. A fallback takes
    /// petite capitals from small ones, and unicase from small capitals. It
    /// also names what is synthesized: lowercase to small capitals, capitals
    /// to small capitals, or both. The rest follows `FontFeatureToUse`,
    /// `NeedsSyntheticFont` and `NeedsCaseChange`, for each kind of text.
    /// Titling capitals are never synthesized.
    pub(super) fn new(caps: FontVariantCaps, offered: OfferedFeatures, synthesize: bool) -> Self {
        use FontVariantCaps as C;
        use OfferedFeatures as O;
        #[derive(Copy, Clone, PartialEq, Eq)]
        enum Support {
            Full,
            Fallback,
            None,
        }
        #[derive(Copy, Clone, PartialEq, Eq)]
        enum Make {
            Nothing,
            Lower,
            Upper,
            Both,
        }
        let has = |bits: OfferedFeatures| offered.contains(bits);
        let (support, make) = match caps {
            C::Normal => return Self::from_feature(C::Normal),
            C::SmallCaps if has(O::SMCP) => (Support::Full, Make::Nothing),
            C::SmallCaps => (Support::None, Make::Lower),
            C::AllSmallCaps if has(O::SMCP.union(O::C2SC)) => (Support::Full, Make::Nothing),
            C::AllSmallCaps => (Support::None, Make::Both),
            C::PetiteCaps if has(O::PCAP) => (Support::Full, Make::Nothing),
            C::PetiteCaps if has(O::SMCP) => (Support::Fallback, Make::Nothing),
            C::PetiteCaps => (Support::None, Make::Lower),
            C::AllPetiteCaps if has(O::PCAP.union(O::C2PC)) => (Support::Full, Make::Nothing),
            C::AllPetiteCaps if has(O::SMCP.union(O::C2SC)) => (Support::Fallback, Make::Nothing),
            C::AllPetiteCaps => (Support::None, Make::Both),
            C::Unicase if has(O::UNIC) => (Support::Full, Make::Nothing),
            C::Unicase if has(O::SMCP) => (Support::Fallback, Make::Upper),
            C::Unicase => (Support::None, Make::Upper),
            C::TitlingCaps if has(O::TITL) => (Support::Full, Make::Nothing),
            C::TitlingCaps => (Support::None, Make::Nothing),
        };
        if support == Support::Full {
            return Self::from_feature(caps);
        }
        // `FontFeatureToUse`: what a fallback shapes with.
        let to_use = |same_case: bool| match (support, caps) {
            (Support::Fallback, C::AllPetiteCaps) => C::AllSmallCaps,
            (Support::Fallback, C::PetiteCaps) => C::SmallCaps,
            (Support::Fallback, C::Unicase) if same_case => C::SmallCaps,
            _ => C::Normal,
        };
        // Without synthesis nothing is divided. All of it shapes as Blink
        // shapes an undivided run: as text of the same case.
        if !synthesize || caps == C::TitlingCaps {
            return Self::from_feature(to_use(true));
        }
        let none = support == Support::None;
        Self {
            same: CaseSide {
                caps: to_use(true),
                synthetic: none && matches!(make, Make::Upper | Make::Both),
                lower: support == Support::Fallback && matches!(make, Make::Both | Make::Upper),
            },
            upper: CaseSide {
                caps: to_use(false),
                synthetic: none && matches!(make, Make::Lower | Make::Both),
                lower: false,
            },
        }
    }
}

/// Whether small capitals are synthesized for `ch`.
///
/// That holds for a letter that changes when uppercased, Unicode's
/// `Changes_When_Uppercased`, as Blink's `SmallCapsIterator` asks ICU.
pub(super) fn changes_when_uppercased(ch: char) -> bool {
    let mut upper = ch.to_uppercase();
    !(upper.len() == 1 && upper.next() == Some(ch))
}

/// Returns the capitals features `caps` shapes with, in the order Blink
/// prepends them.
fn caps_features(caps: FontVariantCaps) -> &'static [FontFeature] {
    const fn on(tag: &[u8; 4]) -> FontFeature {
        FontFeature::new(Tag::new(tag), 1)
    }
    const SMALL: [FontFeature; 1] = [on(b"smcp")];
    const ALL_SMALL: [FontFeature; 2] = [on(b"c2sc"), on(b"smcp")];
    const PETITE: [FontFeature; 1] = [on(b"pcap")];
    const ALL_PETITE: [FontFeature; 2] = [on(b"c2pc"), on(b"pcap")];
    const UNICASE: [FontFeature; 1] = [on(b"unic")];
    const TITLING: [FontFeature; 1] = [on(b"titl")];
    match caps {
        FontVariantCaps::Normal => &[],
        FontVariantCaps::SmallCaps => &SMALL,
        FontVariantCaps::AllSmallCaps => &ALL_SMALL,
        FontVariantCaps::PetiteCaps => &PETITE,
        FontVariantCaps::AllPetiteCaps => &ALL_PETITE,
        FontVariantCaps::Unicase => &UNICASE,
        FontVariantCaps::TitlingCaps => &TITLING,
    }
}

/// Appends to `out` the features the request asks for itself.
///
/// They come after the `@font-face` descriptor's and the capitals', in
/// Blink's order (`FontFeatureRange::FromFontDescription`): `font-kerning`,
/// the ligatures, `font-variant-east-asian` and `font-variant-numeric`, then
/// `font-feature-settings` (`settings`). Blink appends
/// `font-variant-position`'s feature after these, but it is not added here.
/// It is asked for per font ([`merge_features`]), since a run the font's
/// feature does not cover whole is synthesized without it.
///
/// Under a nonzero `letter-spacing`, the common ligatures and the contextual
/// alternates are off, and the discretionary and historical ones stay off,
/// as Blink has them. A ligature would otherwise take one gap for all the
/// letters it joins.
///
/// Under any `text-spacing-trim` but `space-all`, `chws` is on, or `vchw`
/// where the text is `upright`. It stays off where `font-feature-settings`
/// names it or turns on the font's own `halt` or `palt` (`vhal`, `vpal`).
/// This is Blink's `default_enable_chws`. A font with contextual half-width
/// spacing then collapses the blanks of adjacent punctuation itself.
fn push_request_features(
    request: &FontRequest,
    settings: &[FontFeature],
    upright: bool,
    out: &mut Vec<FontFeature>,
) {
    let font = &request.font;
    let feature = |tag: &[u8; 4], value: u16| FontFeature::new(Tag::new(tag), value);
    // `vkrn` too: a style does not know whether its text is set upright.
    // In sideways text `vkrn` is not planned, so turning it off does
    // nothing.
    if font.kerning == FontKerning::None {
        out.push(feature(b"kern", 0));
        out.push(feature(b"vkrn", 0));
    }
    let variants = font.variants;
    let has = |keyword: FontVariants| variants.contains(keyword);
    let spaced = request.spaced;
    if spaced || has(FontVariants::NO_COMMON_LIGATURES) {
        out.push(feature(b"liga", 0));
        out.push(feature(b"clig", 0));
    }
    if !spaced && has(FontVariants::DISCRETIONARY_LIGATURES) {
        out.push(feature(b"dlig", 1));
    }
    if !spaced && has(FontVariants::HISTORICAL_LIGATURES) {
        out.push(feature(b"hlig", 1));
    }
    if spaced || has(FontVariants::NO_CONTEXTUAL) {
        out.push(feature(b"calt", 0));
    }
    for (keyword, tag) in [
        (FontVariants::JIS78, b"jp78"),
        (FontVariants::JIS83, b"jp83"),
        (FontVariants::JIS90, b"jp90"),
        (FontVariants::JIS04, b"jp04"),
        (FontVariants::SIMPLIFIED, b"smpl"),
        (FontVariants::TRADITIONAL, b"trad"),
        (FontVariants::FULL_WIDTH, b"fwid"),
        (FontVariants::PROPORTIONAL_WIDTH, b"pwid"),
        (FontVariants::RUBY, b"ruby"),
        (FontVariants::LINING_NUMS, b"lnum"),
        (FontVariants::OLDSTYLE_NUMS, b"onum"),
        (FontVariants::PROPORTIONAL_NUMS, b"pnum"),
        (FontVariants::TABULAR_NUMS, b"tnum"),
        (FontVariants::STACKED_FRACTIONS, b"afrc"),
        (FontVariants::DIAGONAL_FRACTIONS, b"frac"),
        (FontVariants::ORDINAL, b"ordn"),
        (FontVariants::SLASHED_ZERO, b"zero"),
    ] {
        if has(keyword) {
            out.push(feature(tag, 1));
        }
    }
    out.extend_from_slice(settings);
    let (chws, halt, palt) = if upright {
        (b"vchw", b"vhal", b"vpal")
    } else {
        (b"chws", b"halt", b"palt")
    };
    let named = |tag: &[u8; 4], on: bool| {
        settings
            .iter()
            .any(|setting| setting.tag == Tag::new(tag) && (!on || setting.value != 0))
    };
    if request.trims_punctuation && !named(chws, false) && !named(halt, true) && !named(palt, true)
    {
        out.push(feature(chws, 1));
    }
}

/// Returns the OpenType feature `font-variant-position` asks for: `sups` or
/// `subs`, or none for `normal`.
pub(super) fn position_tag(position: FontVariantPosition) -> Option<[u8; 4]> {
    match position {
        FontVariantPosition::Normal => None,
        FontVariantPosition::Sub => Some(*b"subs"),
        FontVariantPosition::Super => Some(*b"sups"),
    }
}

/// Keeps each tag of `settings` once, at the value set last, in tag order.
///
/// `tag` reads a setting's tag. HarfBuzz merges global features this way,
/// and font variations merge the same way (see `instance`). So the list
/// shapes as the one given did, and every order that comes to the same
/// settings gives one list.
pub(super) fn settle<T: Copy>(settings: &mut Vec<T>, tag: impl Fn(&T) -> Tag) {
    // Stable, so the settings of one tag keep their order.
    stable_sort_by_key(settings, |setting| tag(setting).to_bytes());
    // Keeps the first setting of each tag, holding the last one's value.
    settings.dedup_by(|later, kept| {
        let same = tag(later) == tag(kept);
        if same {
            *kept = *later;
        }
        same
    });
}
