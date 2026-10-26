//! Fallback requests and canonical keys.
//!
//! [`FallbackRequest`] describes text, a generic family, a Standard font, or
//! emoji. [`Collection::key`](crate::Collection::key) reduces the request to
//! a [`FallbackKey`] using the properties read by the collection's backends.
//!
//! Equal keys have equal backend answers. Unknown script tags reduce to
//! Common; unsupported language tags are omitted. Keys use finite sets of
//! scripts, language tags, and locale buckets.

use core::fmt;
use core::num::NonZeroU16;

use parlance::{GenericFamily, Language, Script};

use crate::fallback::emoji::Presentation;
use crate::fallback::generic;
use crate::fallback::language as lang;
use crate::fallback::unicode::{self, SCRIPT_COUNT};
use crate::script as sc;

pub use crate::fallback::language::parse_language;

/// A request for fallback families.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum FallbackRequest {
    /// Text fallback for a script, language, and generic class.
    Text {
        /// The resolved script of the text.
        ///
        /// Accepts Unicode tags and combined CLDR tags such as `Jpan` and `Hans`.
        script: Script,
        /// The content language, if any.
        language: Option<Language>,
        /// The generic class of the first generic in the family list.
        generic: GenericClass,
    },
    /// A generic family resolved for a language.
    Generic(GenericFamily, Option<Language>),
    /// The default font for a language.
    ///
    /// Tried after the family list and before system fallback.
    Standard(Option<Language>),
    /// Emoji fallback for a presentation.
    Emoji(Presentation),
}

/// The generic class used for text fallback.
///
/// Backends retain this distinction only when it affects their results.
/// Conversion from [`GenericFamily`] folds the corresponding `ui-*`
/// aliases.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum GenericClass {
    /// No serif or monospace preference.
    #[default]
    Plain,
    /// `serif` or `ui-serif`.
    Serif,
    /// `monospace` or `ui-monospace`.
    Monospace,
}

impl From<GenericFamily> for GenericClass {
    fn from(generic: GenericFamily) -> Self {
        match generic::normalize(generic) {
            GenericFamily::Serif => Self::Serif,
            GenericFamily::Monospace => Self::Monospace,
            _ => Self::Plain,
        }
    }
}

/// A regional form of Han typography.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Han {
    /// Simplified Chinese.
    Hans,
    /// Traditional Chinese.
    Hant,
    /// Traditional Chinese for Hong Kong and Macau.
    HantHK,
    /// Japanese.
    Jpan,
    /// Korean.
    Kore,
}

impl Han {
    /// Returns the script tag the tradition stands for: `Hans`, `Hant`,
    /// `Jpan` or `Kore`. Hong Kong's is `Hant`.
    pub(crate) const fn script(self) -> Script {
        match self {
            Self::Hans => sc::HANS,
            Self::Hant | Self::HantHK => sc::HANT,
            Self::Jpan => sc::JPAN,
            Self::Kore => sc::KORE,
        }
    }
}

/// A set of locales that share default fonts for the generic families.
///
/// Locales without a dedicated default use [`Common`](Self::Common).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum GenericBucket {
    /// Locales without a dedicated default.
    Common,
    /// Simplified Chinese.
    Hans,
    /// Traditional Chinese, Hong Kong's included.
    Hant,
    /// Japanese.
    Jpan,
    /// Korean.
    Kore,
    /// Languages written in Devanagari.
    Deva,
    /// Languages written in Arabic.
    Arab,
    /// Languages written in Cyrillic.
    Cyrl,
    /// Greek.
    Grek,
}

/// What the backends answering a key read of a request.
///
/// The facts decide how far a request reduces. A collection's facts combine
/// those of every layer fallback reads, so a reduction applies only where no
/// backend would answer differently.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) struct BackendFacts {
    /// Some backend answers the serif class differently from the plain one.
    pub(crate) reads_serif: bool,
    /// Some backend answers monospace Arabic and Hebrew with a font of their
    /// own.
    pub(crate) reads_monospace: bool,
    /// Every backend orders its lists by language, so a script's key carries
    /// the language's Han tradition.
    pub(crate) per_language: bool,
    /// Some backend sorts by the language itself, as fontconfig does, so a
    /// script's key carries the language.
    pub(crate) reads_language: bool,
}

/// A canonical fallback key.
///
/// Created by [`Collection::key`](crate::Collection::key). Retains only
/// properties that affect the backend results of the collection. Scripts and
/// languages are drawn from finite sets, bounding the cache key space.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FallbackKey(pub(super) KeyKind);

/// What a [`FallbackKey`] holds, by kind of request.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum KeyKind {
    /// A non-Han script, Common, `Zsym` or `Zmth`. Carries the language's
    /// Han tradition where a backend orders its lists by it, and the
    /// language where a backend sorts by it.
    Script(ScriptId, GenericClass, Option<Han>, Option<LanguageId>),
    /// A Han tradition.
    Han(Han, GenericClass),
    /// A generic in a locale bucket.
    Generic(GenericFamily, GenericBucket),
    /// The Standard font in a locale bucket.
    Standard(GenericBucket),
    /// Emoji in a presentation, [`Emoji`](Presentation::Emoji) or
    /// [`Text`](Presentation::Text).
    Emoji(Presentation),
}

impl FallbackKey {
    /// Returns the script for a text key.
    ///
    /// Han keys return `Hani`. Generic, Standard, and emoji keys return
    /// `None`.
    pub fn script(&self) -> Option<Script> {
        match self.0 {
            KeyKind::Script(id, ..) => Some(id.tag()),
            KeyKind::Han(..) => Some(sc::HANI),
            KeyKind::Generic(..) | KeyKind::Standard(_) | KeyKind::Emoji(_) => None,
        }
    }

    /// Returns the fallback language tag, if retained.
    ///
    /// Uses fontconfig spelling, such as `fa` or `zh-hk`. Only text keys
    /// whose backends sort by language retain a tag.
    pub fn language(&self) -> Option<&'static str> {
        match self.0 {
            KeyKind::Script(.., language) => language.map(LanguageId::tag),
            _ => None,
        }
    }

    /// Returns the key's Han tradition, if any.
    ///
    /// For a Han key, this is the text's tradition. Other text keys may
    /// retain the language's tradition when backends use it to order
    /// fallback.
    pub fn han(&self) -> Option<Han> {
        match self.0 {
            KeyKind::Script(_, _, han, _) => han,
            KeyKind::Han(han, _) => Some(han),
            KeyKind::Generic(..) | KeyKind::Standard(_) | KeyKind::Emoji(_) => None,
        }
    }

    /// Returns the text key's generic class.
    ///
    /// Other key kinds return [`GenericClass::Plain`].
    pub fn class(&self) -> GenericClass {
        match self.0 {
            KeyKind::Script(_, class, ..) | KeyKind::Han(_, class) => class,
            KeyKind::Generic(..) | KeyKind::Standard(_) | KeyKind::Emoji(_) => GenericClass::Plain,
        }
    }

    /// Returns the generic family and its locale bucket.
    ///
    /// Returns `None` for other key kinds. The family has its `ui-*`
    /// aliases normalized.
    pub fn generic(&self) -> Option<(GenericFamily, GenericBucket)> {
        match self.0 {
            KeyKind::Generic(family, bucket) => Some((family, bucket)),
            _ => None,
        }
    }

    /// Returns the Standard font's locale bucket.
    ///
    /// Returns `None` for other key kinds.
    pub fn standard(&self) -> Option<GenericBucket> {
        match self.0 {
            KeyKind::Standard(bucket) => Some(bucket),
            _ => None,
        }
    }

    /// Returns the emoji key's presentation.
    ///
    /// Returns [`Presentation::Text`] or [`Presentation::Emoji`], without
    /// selector forcing. Returns `None` for other key kinds.
    pub fn presentation(&self) -> Option<Presentation> {
        match self.0 {
            KeyKind::Emoji(presentation) => Some(presentation),
            _ => None,
        }
    }

    /// Returns the key that answers `request`, given what the backends read.
    pub(crate) fn new(request: &FallbackRequest, facts: BackendFacts) -> Self {
        Self(match *request {
            FallbackRequest::Text {
                script,
                language,
                generic,
            } => text(script, language, generic, facts),
            FallbackRequest::Generic(family, language) => match generic::normalize(family) {
                // The emoji generic names the emoji fonts, which is the emoji
                // key's answer.
                GenericFamily::Emoji => KeyKind::Emoji(Presentation::Emoji),
                family => KeyKind::Generic(family, bucket(language)),
            },
            FallbackRequest::Standard(language) => KeyKind::Standard(bucket(language)),
            FallbackRequest::Emoji(presentation) => KeyKind::Emoji(presentation.base()),
        })
    }

    /// Whether this is the Common key.
    ///
    /// Every missed character asks the Common key after its own. Its answer
    /// is the symbol and wide-coverage fonts.
    pub(crate) fn is_common(&self) -> bool {
        matches!(self.0, KeyKind::Script(ScriptId::COMMON, ..))
    }

    /// Returns the generic key for `family` in a run of `script` under
    /// `language`.
    ///
    /// The bucket is the language's. With no language, it is the run's own
    /// script's.
    pub(crate) fn from_generic(
        family: GenericFamily,
        script: Script,
        language: Option<Language>,
    ) -> Self {
        if language.is_some() {
            return Self::new(
                &FallbackRequest::Generic(family, language),
                BackendFacts::default(),
            );
        }
        let bucket = match tradition(titlecase(script).unwrap_or(Script::COMMON), None) {
            Some(Han::Hans) => GenericBucket::Hans,
            Some(Han::Hant | Han::HantHK) => GenericBucket::Hant,
            Some(Han::Jpan) => GenericBucket::Jpan,
            Some(Han::Kore) => GenericBucket::Kore,
            None => match titlecase(script) {
                Some(sc::HANI) => GenericBucket::Hans,
                Some(sc::DEVA) => GenericBucket::Deva,
                Some(sc::ARAB) => GenericBucket::Arab,
                Some(sc::CYRL) => GenericBucket::Cyrl,
                Some(sc::GREK) => GenericBucket::Grek,
                _ => GenericBucket::Common,
            },
        };
        Self(match generic::normalize(family) {
            GenericFamily::Emoji => KeyKind::Emoji(Presentation::Emoji),
            family => KeyKind::Generic(family, bucket),
        })
    }

    /// Whether the key's answer is one family: a generic's or the Standard
    /// font.
    pub(super) fn is_one_family(&self) -> bool {
        matches!(self.0, KeyKind::Generic(..) | KeyKind::Standard(_))
    }
}

/// Indexes the scripts a [`FallbackKey`] can name: Unicode's scripts with
/// characters of their own, and the pseudo-scripts fallback answers.
///
/// A key holds a byte, not a tag, and its scripts are a closed set. A tag
/// outside the set has no id.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub(super) struct ScriptId(u8);

/// The pseudo-scripts, which take the first ids: Common, symbols, math and
/// emoji.
const PSEUDO_SCRIPTS: [[u8; 4]; 4] = [*b"Zyyy", *b"Zsym", *b"Zmth", *b"Zsye"];

impl ScriptId {
    /// Common, which every missed character asks after its own key.
    pub(crate) const COMMON: Self = Self(0);

    /// How many ids there are.
    pub(super) const COUNT: usize = PSEUDO_SCRIPTS.len() + SCRIPT_COUNT;

    /// The id of `script`, in any case, if it is one of the set.
    ///
    /// Inherited, Unknown, the combined CLDR tags (`Hans`, `Hant`, `Jpan`,
    /// `Kore`, `Hrkt`) and every tag Unicode does not assign have none.
    pub(crate) fn new(script: Script) -> Option<Self> {
        let tag = titlecase(script)?.to_bytes();
        let at = match PSEUDO_SCRIPTS.iter().position(|&pseudo| pseudo == tag) {
            Some(at) => at,
            None => PSEUDO_SCRIPTS.len() + unicode::script_tags().binary_search(&tag).ok()?,
        };
        u8::try_from(at).ok().map(Self)
    }

    /// The script's tag, titlecased.
    pub(crate) fn tag(self) -> Script {
        let at = usize::from(self.0);
        let tag = match at.checked_sub(PSEUDO_SCRIPTS.len()) {
            None => PSEUDO_SCRIPTS.get(at),
            Some(at) => unicode::script_tags().get(at),
        };
        tag.map_or(Script::COMMON, |&tag| Script::from_bytes(tag))
    }

    /// Every id, in order.
    #[cfg(test)]
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        (0..Self::COUNT).filter_map(|at| u8::try_from(at).ok().map(Self))
    }
}

impl fmt::Debug for ScriptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = self.tag().to_bytes();
        let tag = core::str::from_utf8(&tag).unwrap_or("Zyyy");
        write!(f, "ScriptId({tag})")
    }
}

// Every id fits a byte.
const _: () = assert!(ScriptId::COUNT <= 256);

/// Indexes the languages fontconfig has an orthography for, which it sorts
/// fonts by.
///
/// The id is one more than the place, so a key with no language costs
/// nothing extra. The languages are a closed set: a language fontconfig
/// does not know has no id, and sorts as the default does.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub(super) struct LanguageId(NonZeroU16);

impl LanguageId {
    /// How many ids there are.
    #[cfg(test)]
    pub(super) const COUNT: usize = LANGUAGE_COUNT;

    /// Returns the id fontconfig would sort `language` by, if it has one.
    ///
    /// Tries the tag with its region (`pa-pk`, `zh-hk`), with its script
    /// (`und-zsye`), then alone (`fa`). Chinese with no region goes by its
    /// Han tradition (`zh-tw` for `zh-Hant`).
    pub(crate) fn new(language: &Language) -> Option<Self> {
        let primary = language.language();
        let with = |subtag: Option<&str>| subtag.and_then(|subtag| Self::find(primary, subtag));
        if let Some(id) = with(language.region()).or_else(|| with(language.script())) {
            return Some(id);
        }
        if primary.eq_ignore_ascii_case("zh") {
            let region = match tradition(Script::COMMON, Some(*language))? {
                Han::Hans => "cn",
                Han::Hant => "tw",
                Han::HantHK => "hk",
                Han::Jpan | Han::Kore => return None,
            };
            return Self::find(primary, region);
        }
        Self::find(primary, "")
    }

    /// The id of `primary`, followed by `subtag` where there is one, in any
    /// case.
    fn find(primary: &str, subtag: &str) -> Option<Self> {
        // The longest tag fontconfig has is seven bytes; a language and a
        // region can be at most eight and three, joined by a hyphen.
        let mut buffer = [0u8; 16];
        let length = primary.len() + usize::from(!subtag.is_empty()) + subtag.len();
        let tag = buffer.get_mut(..length)?;
        let (head, tail) = tag.split_at_mut(primary.len());
        head.copy_from_slice(primary.as_bytes());
        if let Some((hyphen, rest)) = tail.split_first_mut() {
            *hyphen = b'-';
            rest.copy_from_slice(subtag.as_bytes());
        }
        tag.make_ascii_lowercase();
        let at = FC_LANGUAGES
            .binary_search_by(|known| known.as_bytes().cmp(tag))
            .ok()?;
        u16::try_from(at + 1)
            .ok()
            .and_then(NonZeroU16::new)
            .map(Self)
    }

    /// The language's tag, as fontconfig spells it.
    pub(crate) fn tag(self) -> &'static str {
        let at = usize::from(self.0.get()) - 1;
        FC_LANGUAGES.get(at).copied().unwrap_or("und")
    }

    /// Every id, in order.
    #[cfg(test)]
    pub(crate) fn all() -> impl Iterator<Item = Self> {
        (1..=LANGUAGE_COUNT)
            .filter_map(|at| u16::try_from(at).ok().and_then(NonZeroU16::new).map(Self))
    }
}

impl fmt::Debug for LanguageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LanguageId({})", self.tag())
    }
}

// Every id fits sixteen bits.
const _: () = assert!(LANGUAGE_COUNT < u16::MAX as usize);

include!("language_table.rs");

/// Returns a run's key: its Han tradition, or its script and the
/// language's.
fn text(
    script: Script,
    language: Option<Language>,
    generic: GenericClass,
    facts: BackendFacts,
) -> KeyKind {
    let script = titlecase(script).unwrap_or(Script::COMMON);
    if script == sc::ZSYE {
        return KeyKind::Emoji(Presentation::Emoji);
    }
    let han = match script {
        // An unresolved `Hani` still has to render. It resolves as
        // Simplified, Blink's final default.
        sc::HANI => Some(tradition(script, language).unwrap_or(Han::Hans)),
        _ if cjk_script(script) => tradition(script, language),
        _ => None,
    };
    let class = |monospace: bool| match generic {
        GenericClass::Serif if facts.reads_serif => GenericClass::Serif,
        GenericClass::Monospace if monospace && facts.reads_monospace => GenericClass::Monospace,
        _ => GenericClass::Plain,
    };
    if let Some(han) = han {
        return KeyKind::Han(han, class(false));
    }
    // A backend that orders its lists by language orders them by its Han
    // tradition: Skia's locale tags, the locale-wide `FcFontSort`.
    let locale = if facts.per_language {
        tradition(Script::COMMON, language)
    } else {
        None
    };
    // fontconfig sorts by the language itself: one `FcFontSort` per
    // language it has an orthography for.
    let language = match language {
        Some(language) if facts.reads_language => LanguageId::new(&language),
        _ => None,
    };
    // Inherited, Unknown and every tag outside the set have no fonts of
    // their own, so they take the Common key.
    let id = ScriptId::new(script).unwrap_or(ScriptId::COMMON);
    let monospace = matches!(script, sc::ARAB | sc::HEBR);
    KeyKind::Script(id, class(monospace), locale, language)
}

/// `script` titlecased, or `None` if it is not four ASCII letters.
fn titlecase(script: Script) -> Option<Script> {
    let bytes = script.to_bytes();
    if !bytes.iter().all(u8::is_ascii_alphabetic) {
        return None;
    }
    Some(Script::from_bytes([
        bytes[0].to_ascii_uppercase(),
        bytes[1].to_ascii_lowercase(),
        bytes[2].to_ascii_lowercase(),
        bytes[3].to_ascii_lowercase(),
    ]))
}

/// Whether `script` is written in one Han tradition whatever the language.
fn cjk_script(script: Script) -> bool {
    matches!(
        script,
        sc::HANS
            | sc::HANT
            | sc::BOPO
            | sc::JPAN
            | sc::HIRA
            | sc::KANA
            | sc::HRKT
            | sc::KORE
            | sc::HANG
    )
}

/// Returns the Han tradition text in `script` is set in under `language`,
/// if any.
///
/// The script decides first, then the tag's script, region and language. A
/// Hong Kong or Macau region makes Traditional text Hong Kong's.
pub(crate) fn tradition(script: Script, language: Option<Language>) -> Option<Han> {
    let han = lang::resolve_han(script, language)?;
    // Bopomofo is Taiwan's, whatever the region.
    if han != Han::Hant || script == sc::BOPO {
        return Some(han);
    }
    let hong_kong = language
        .as_ref()
        .and_then(Language::region)
        .is_some_and(|region| {
            region.eq_ignore_ascii_case("HK") || region.eq_ignore_ascii_case("MO")
        });
    Some(if hong_kong { Han::HantHK } else { Han::Hant })
}

/// Returns the bucket a generic resolves in for `language`: its script's,
/// where the generic-family settings have a default for that script.
fn bucket(language: Option<Language>) -> GenericBucket {
    let Some(language) = language else {
        return GenericBucket::Common;
    };
    // Hong Kong is already Traditional here: the generic-family settings
    // have no bucket of its own for it.
    match lang::locale_script(language) {
        sc::HANS => GenericBucket::Hans,
        sc::HANT => GenericBucket::Hant,
        sc::JPAN => GenericBucket::Jpan,
        sc::KORE => GenericBucket::Kore,
        sc::DEVA => GenericBucket::Deva,
        sc::ARAB => GenericBucket::Arab,
        sc::CYRL => GenericBucket::Cyrl,
        sc::GREK => GenericBucket::Grek,
        _ => GenericBucket::Common,
    }
}
