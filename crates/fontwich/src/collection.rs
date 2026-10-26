//! The stack of layers, and the fallback asked of it.
//!
//! A [`Collection`] is a stack of [`Layer`]s. Lookup walks the stack from the
//! top, so a document's fonts shadow an application's, which shadow the
//! system's, as CSS does with `@font-face` names. Each layer's name set is
//! complete, so a miss is a real miss.
//!
//! Layers are immutable once published and shared behind an `Arc`. Every
//! read takes `&self`, and a collection is cheap to clone and send to another
//! thread. A layer changes through its [`LayerBuilder`](crate::LayerBuilder),
//! which copies on write and never disturbs a snapshot someone else holds.
//!
//! [`Collection::local`] finds an installed font by the full or PostScript
//! name a `src: local(...)` gives.
//!
//! Fallback: [`Collection::key`] reduces a request to the key its layers'
//! backends answer by. [`Collection::fallback`] returns a key's installed
//! families. [`Collection::char_fallback`] returns the families a missed
//! character walks.

use alloc::sync::Arc;
use alloc::vec::Vec;

use parlance::{FontFamilyName, Language};

use crate::Family;
use crate::fallback::Presentation;
use crate::fallback::{self, FallbackCache, FallbackFor};
use crate::fallback::{BackendFacts, FallbackKey, FallbackRequest};
use crate::font::Font;
use crate::layer::{Layer, Role};

/// A stack of font layers.
///
/// Name lookup searches from the top. Fallback uses system layers, or
/// application layers if no system layer exists, and excludes document
/// layers.
///
/// Cloning shares layers and cached fallback lists. A collection retains
/// its snapshots when the builders that created them change.
#[derive(Clone, Debug, Default)]
pub struct Collection {
    /// Bottom first. The top is the most specific, usually a document.
    layers: Vec<Arc<Layer>>,
    /// Fallback's answers, by key. Shared by clones whose fallback reads the
    /// same layers, and replaced when a layer it reads is pushed.
    fallback: Arc<FallbackCache>,
    /// The language a request with none is answered in.
    default_language: Option<Language>,
}

impl Collection {
    /// Creates an empty collection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a collection of system fonts.
    ///
    /// Contains a single [`Layer::system`] layer.
    #[cfg(all(any(windows, unix), feature = "system"))]
    pub fn system() -> Self {
        Self::new().with_layer(Arc::new(Layer::system()))
    }

    /// Adds a layer to the top and returns the collection.
    pub fn with_layer(mut self, layer: Arc<Layer>) -> Self {
        self.push(layer);
        self
    }

    /// Sets the default fallback language and returns the collection.
    ///
    /// Used when a request does not specify a language.
    #[must_use]
    pub fn with_default_language(mut self, language: Option<Language>) -> Self {
        self.set_default_language(language);
        self
    }

    /// Adds a layer to the top of the collection.
    pub fn push(&mut self, layer: Arc<Layer>) {
        self.layers.push(layer);
        // A layer fallback reads changes every answer; a document's changes
        // none, and neither does an application layer under a system one.
        let pushed = self.layers.last().map(Arc::as_ptr);
        if self
            .fallback_layers()
            .any(|reads| Some(Arc::as_ptr(reads)) == pushed)
        {
            self.fallback = Arc::default();
        }
    }

    /// Sets the default fallback language.
    ///
    /// Used when a request does not specify a language.
    pub fn set_default_language(&mut self, language: Option<Language>) {
        self.default_language = language;
    }

    /// Returns the layers in order, from bottom to top.
    pub fn layers(&self) -> impl DoubleEndedIterator<Item = &Layer> {
        self.layers.iter().map(|layer| &**layer)
    }

    /// Returns the family matching `name`, if present.
    ///
    /// Searches layers from top to bottom. Document families shadow installed
    /// families with the same name and match only their declared names.
    /// Use [`fallback_family`](Self::fallback_family) for fallback lookup.
    ///
    /// Installed families have aliases: `Times`/`Times New Roman`,
    /// `Courier`/`Courier New`, and `Helvetica`/`Arial` substitute for each
    /// other when absent. On Windows, `Times`, `Courier`, `MS Serif` and
    /// `MS Sans Serif` prefer TrueType substitutes over bitmap fonts;
    /// missing `Courier New` does not fall back to bitmap `Courier`.
    ///
    /// Does not allocate or load font metadata.
    pub fn family(&self, name: &str) -> Option<Family> {
        let windows = cfg!(windows);
        let installed = replacement(name, windows).unwrap_or(name);
        self.layers
            .iter()
            .rev()
            .find_map(|layer| {
                let name = if layer.role() == Role::Document {
                    name
                } else {
                    installed
                };
                Some(Family::new(layer, layer.find(name)?))
            })
            .or_else(|| {
                let alternate = alternate(installed, windows)?;
                self.layers
                    .iter()
                    .rev()
                    .filter(|layer| layer.role() != Role::Document)
                    .find_map(|layer| Some(Family::new(layer, layer.find(alternate)?)))
            })
    }

    /// Returns `true` if `family` belongs to this collection.
    ///
    /// Compares by identity, as `==` on [`Family`] does. Returns `true` even
    /// if a higher layer shadows the family. Does not allocate or load font
    /// metadata.
    pub fn contains(&self, family: &Family) -> bool {
        let name = family.name();
        self.layers.iter().any(|layer| {
            layer
                .find(name)
                .is_some_and(|id| family.is_record(layer, id))
        })
    }

    /// Finds a font by its full or PostScript name.
    ///
    /// Searches non-document layers from top to bottom. Use the result with
    /// [`LayerBuilder::add_face_font`](crate::LayerBuilder::add_face_font) to implement `src: local(...)`.
    pub fn local(&self, name: &str) -> Option<Font> {
        self.layers
            .iter()
            .rev()
            .filter(|layer| layer.role() != Role::Document)
            .find_map(|layer| layer.local(name))
    }

    /// Resolves a family name or generic.
    ///
    /// Named families use the topmost matching layer. Generics use the
    /// request's language, or the run's script if no language is specified.
    pub fn resolve(&self, name: &FontFamilyName<'_>, request: &FallbackRequest) -> Option<Family> {
        let generic = match name {
            FontFamilyName::Named(name) => return self.family(name),
            FontFamilyName::Generic(generic) => *generic,
        };
        let key = match *request {
            FallbackRequest::Text {
                script, language, ..
            } => FallbackKey::from_generic(generic, script, language.or(self.default_language)),
            FallbackRequest::Generic(_, language) | FallbackRequest::Standard(language) => {
                self.key(&FallbackRequest::Generic(generic, language))
            }
            FallbackRequest::Emoji(_) => self.key(&FallbackRequest::Generic(generic, None)),
        };
        self.fallback(&key).first().cloned()
    }

    /// Returns the canonical key for a fallback request.
    ///
    /// Uses the default language if the request has none. Retains only the
    /// request properties used by the fallback backends. Does not allocate.
    pub fn key(&self, request: &FallbackRequest) -> FallbackKey {
        let request = match *request {
            FallbackRequest::Text {
                script,
                language,
                generic,
            } => FallbackRequest::Text {
                script,
                language: language.or(self.default_language),
                generic,
            },
            FallbackRequest::Generic(family, language) => {
                FallbackRequest::Generic(family, language.or(self.default_language))
            }
            FallbackRequest::Standard(language) => {
                FallbackRequest::Standard(language.or(self.default_language))
            }
            emoji @ FallbackRequest::Emoji(_) => emoji,
        };
        FallbackKey::new(&request, self.facts())
    }

    /// Returns the installed families for a fallback key.
    ///
    /// Families are ordered by preference, without duplicates. Each layer's
    /// override precedes its backend. Generic and Standard keys return at
    /// most one family.
    ///
    /// Results are cached on first use. Resolving a key does not load font
    /// metadata, except on Android, where script coverage is checked
    /// against the fonts.
    pub fn fallback(&self, key: &FallbackKey) -> Arc<[Family]> {
        let stamp = fallback::stamp(self.fallback_layers());
        if let Some(families) = self.fallback.get(stamp, key) {
            return families;
        }
        // No lock held: a backend may ask the platform.
        let families = fallback::answer(self, key);
        self.fallback.insert(stamp, *key, families)
    }

    /// Returns a lazy fallback iterator for a character.
    ///
    /// Tries the character's own key, the Common key, the platform's
    /// character fallback, and then the remaining families in
    /// fallback-layer order. Each family is returned once.
    ///
    /// A control character (`Cc`) stops after the platform character
    /// fallback, since the fonts that map one draw it blank.
    ///
    /// `presentation` selects text or emoji fallback. `request` supplies
    /// the language and generic class. Fallback lists are resolved when
    /// reached; font metadata is loaded only when requested from a family.
    pub fn char_fallback(
        &self,
        c: char,
        presentation: Presentation,
        request: &FallbackRequest,
    ) -> impl Iterator<Item = Family> {
        FallbackFor::new(self, c, presentation, request)
    }

    /// Returns a family from the fallback layers.
    ///
    /// Searches system layers, or application layers if no system layer
    /// exists. Document layers are excluded. Returns `None` if none of
    /// these layers contains the name.
    pub fn fallback_family(&self, name: &str) -> Option<Family> {
        self.fallback_layers()
            .find_map(|layer| Some(Family::new(layer, layer.find(name)?)))
    }

    /// Returns the estimated heap usage in bytes.
    ///
    /// Includes layers and cached fallback lists. Shared allocations are
    /// counted once within this collection, but are counted separately by
    /// other collections that share them. See [`Layer::heap_usage`].
    pub fn heap_usage(&self) -> usize {
        use crate::heap::{ARC, Seen};
        use core::mem::size_of;
        let mut seen = Seen::default();
        let mut total = self.layers.capacity() * size_of::<Arc<Layer>>();
        for layer in &self.layers {
            total += seen.once(Arc::as_ptr(layer), || ARC + size_of::<Layer>());
            total += layer.heap(&mut seen);
        }
        total += seen.once(Arc::as_ptr(&self.fallback), || {
            ARC + size_of::<FallbackCache>() + self.fallback.heap()
        });
        total
    }

    /// The language a request with none is answered in.
    pub(crate) fn default_language(&self) -> Option<Language> {
        self.default_language
    }

    /// What the fallback layers' backends read: what any one reads, and
    /// per language only where every one is.
    pub(crate) fn facts(&self) -> BackendFacts {
        let mut facts = BackendFacts::default();
        let mut per_language = None;
        for backend in self.fallback_layers().filter_map(|layer| layer.backend()) {
            let own = backend.facts();
            facts.reads_serif |= own.reads_serif;
            facts.reads_monospace |= own.reads_monospace;
            facts.reads_language |= own.reads_language;
            per_language = Some(per_language.unwrap_or(true) && own.per_language);
        }
        facts.per_language = per_language.unwrap_or(false);
        facts
    }

    /// The layers fallback may read, top first.
    pub(crate) fn fallback_layers(&self) -> impl Iterator<Item = &Arc<Layer>> {
        let from = if self.layers().any(|layer| layer.role() == Role::System) {
            Role::System
        } else {
            Role::Application
        };
        self.layers
            .iter()
            .rev()
            .filter(move |layer| layer.role() == from)
    }
}

/// The installed family looked up in place of `name` on Windows, where the
/// families it names are bitmap fonts, as in Blink's
/// `AdjustFamilyNameToAvoidUnsupportedFonts`.
fn replacement(name: &str, windows: bool) -> Option<&'static str> {
    const REPLACEMENTS: [(&str, &str); 4] = [
        ("Courier", "Courier New"),
        ("MS Sans Serif", "Microsoft Sans Serif"),
        ("MS Serif", "Times New Roman"),
        ("Times", "Times New Roman"),
    ];
    if !windows {
        return None;
    }
    REPLACEMENTS
        .iter()
        .find(|(from, _)| from.eq_ignore_ascii_case(name))
        .map(|&(_, to)| to)
}

/// The installed family tried when `name` is missing, as in Blink's
/// `AlternateFamilyName`.
///
/// Windows has no `Courier New` to `Courier` alias, since its `Courier` is a
/// bitmap font.
fn alternate(name: &str, windows: bool) -> Option<&'static str> {
    const ALTERNATES: [(&str, &str); 6] = [
        ("Courier", "Courier New"),
        ("Courier New", "Courier"),
        ("Times", "Times New Roman"),
        ("Times New Roman", "Times"),
        ("Arial", "Helvetica"),
        ("Helvetica", "Arial"),
    ];
    ALTERNATES
        .iter()
        .find(|(from, _)| from.eq_ignore_ascii_case(name))
        .map(|&(_, to)| to)
        .filter(|&to| !(windows && to == "Courier"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::test_fonts::font_named;
    use crate::{Charset, LayerBuilder, LoadFamily};
    use alloc::string::String;
    use alloc::vec;

    /// A font called `family` mapping `ranges`.
    fn font_covering(family: &str, ranges: &[(u16, u16)]) -> Vec<u8> {
        use crate::test_fonts::{EN_US, WINDOWS, cmap, font_with_tables, format4, name};
        use read_fonts::tables::name::NameId;
        font_with_tables(&[
            (*b"cmap", cmap(&[(3, 1, format4(ranges))])),
            (
                *b"name",
                name(&[(WINDOWS, EN_US, NameId::FAMILY_NAME, family)]),
            ),
        ])
    }

    #[test]
    fn a_collection_contains_the_families_of_its_layers() {
        // A family is the collection's while any layer holds its record, an
        // unchanged family across snapshots and a shadowed one included.
        let mut lower = LayerBuilder::new(Role::Application);
        lower.add_data(font_named("Shared Name")).expect("adds");
        lower.add_data(font_named("Kept")).expect("adds");
        let mut upper = LayerBuilder::new(Role::Document);
        upper.add_data(font_named("Shared Name")).expect("adds");
        let both = Collection::new()
            .with_layer(lower.snapshot())
            .with_layer(upper.snapshot());
        let shadowed = Collection::new()
            .with_layer(lower.snapshot())
            .family("Shared Name")
            .expect("in the lower layer");
        let kept = both.family("Kept").expect("kept");
        let shadowing = both.family("Shared Name").expect("the upper one");
        assert!(both.contains(&shadowed) && both.contains(&kept));
        assert!(both.contains(&shadowing));

        // A later snapshot of the lower layer keeps its records.
        lower.add_data(font_named("Added")).expect("adds");
        let later = Collection::new().with_layer(lower.snapshot());
        assert!(later.contains(&kept));
        assert!(!later.contains(&shadowing), "the upper layer is gone");
        assert!(!Collection::new().contains(&kept));
    }

    #[test]
    fn a_control_character_does_not_walk_every_family() {
        // Fonts that map C0 and C1 controls draw them blank; the walk past
        // the keys and the platform stops for them, so `.notdef` shows.
        let mut fonts = LayerBuilder::new(Role::Application);
        fonts
            .add_data(font_covering(
                "Blank Controls",
                &[(0x0001, 0x001F), (0x007F, 0x009F), (0x0800, 0x0800)],
            ))
            .expect("adds");
        let collection = Collection::new().with_layer(fonts.snapshot());
        let request = FallbackRequest::Text {
            script: parlance::Script::COMMON,
            language: None,
            generic: crate::GenericClass::Plain,
        };
        let walked = |c: char| {
            collection
                .char_fallback(c, Presentation::Text, &request)
                .count()
        };
        for c in ['\u{2}', '\u{1D}', '\u{7F}', '\u{85}'] {
            assert_eq!(walked(c), 0, "U+{:04X}", u32::from(c));
        }
        assert_eq!(walked('\u{0800}'), 1);
    }

    #[test]
    fn a_miss_asks_windows_system_fallback_before_the_walk() {
        // Samaritan, which no key of the Windows backend names a font for:
        // DirectWrite's answer, recorded, comes before the walk reaches the
        // family listed first.
        let samaritan = &[(0x0800, 0x082D)];
        let mut fonts = LayerBuilder::new(Role::Application);
        for family in ["Listed First", "Sans Serif Collection"] {
            fonts
                .add_data(font_covering(family, samaritan))
                .expect("adds");
        }
        fonts.set_fallback(Backend::Windows(crate::backend::Windows::new()));
        let collection = Collection::new().with_layer(fonts.snapshot());
        let request = FallbackRequest::Text {
            script: parlance::Script::from_bytes(*b"Latn"),
            language: None,
            generic: crate::GenericClass::Plain,
        };
        let walked: Vec<String> = collection
            .char_fallback('\u{0800}', Presentation::Text, &request)
            .map(|family| String::from(family.name()))
            .collect();
        assert_eq!(walked, ["Sans Serif Collection", "Listed First"]);
    }

    #[test]
    fn multiple_windows_layers_share_one_platform_answer() {
        let mut lower = LayerBuilder::new(Role::Application);
        lower.add_data(font_named("Lower Family")).expect("adds");
        lower.set_fallback(Backend::Windows(crate::backend::Windows::new()));
        let mut upper = LayerBuilder::new(Role::Application);
        for name in ["Listed First", "Sans Serif Collection"] {
            upper.add_data(font_named(name)).expect("adds");
        }
        upper.set_fallback(Backend::Windows(crate::backend::Windows::new()));
        let collection = Collection::new()
            .with_layer(lower.snapshot())
            .with_layer(upper.snapshot());
        let request = FallbackRequest::Text {
            script: parlance::Script::from_bytes(*b"Latn"),
            language: None,
            generic: crate::GenericClass::Plain,
        };
        let walked: Vec<String> = collection
            .char_fallback('\u{0800}', Presentation::Text, &request)
            .map(|family| String::from(family.name()))
            .collect();
        assert_eq!(
            walked,
            ["Sans Serif Collection", "Listed First", "Lower Family"]
        );
    }

    #[test]
    fn heap_usage_counts_what_is_shared_once() {
        let latin = &[(0x20, 0x7E), (0xA0, 0xFF)];
        let mut fonts = LayerBuilder::new(Role::Application);
        let empty = fonts.layer().heap_usage();
        let a = font_covering("A", latin);
        let a_len = a.len();
        fonts.add_data(a).expect("adds");
        let one = fonts.layer().heap_usage();
        assert!(one > empty + a_len, "the bytes, and more: {empty} -> {one}");

        // The same characters under another name: its own bytes and record,
        // but the charset is the first font's, and not counted again.
        let b = font_covering("B", latin);
        let b_len = b.len();
        fonts.add_data(b).expect("adds");
        let two = fonts.layer().heap_usage();
        assert!(two - one < one - empty, "{empty} -> {one} -> {two}");
        assert!(two - one > b_len);

        // A collection counts its layers, and a layer twice over once.
        let layer = fonts.snapshot();
        let collection = Collection::new()
            .with_layer(layer.clone())
            .with_layer(layer);
        let held = collection.heap_usage();
        assert!(held > two && held < 2 * two, "{two} -> {held}");
    }

    #[test]
    fn fonts_mapping_the_same_characters_share_one_charset() {
        let mut fonts = LayerBuilder::new(Role::Application);
        let latin = &[(0x20, 0x7E), (0xA0, 0xFF)];
        let a = fonts.add_data(font_covering("A", latin)).expect("adds");
        let b = fonts.add_data(font_covering("B", latin)).expect("adds");
        let c = fonts
            .add_data(font_covering("C", &[(0x20, 0x7E)]))
            .expect("adds");
        let charset = |family: &Family| family.fonts()[0].charset() as *const Charset;
        assert_eq!(charset(&a[0]), charset(&b[0]), "identical, so one copy");
        assert_ne!(charset(&a[0]), charset(&c[0]), "different, so two");
        assert!(a[0].covers('ÿ') && !c[0].covers('ÿ'));
    }

    fn layer(role: Role, families: &[&str]) -> Arc<Layer> {
        let mut fonts = LayerBuilder::new(role);
        for family in families {
            fonts.add_data(font_named(family)).expect("adds");
        }
        fonts.snapshot()
    }

    #[test]
    fn a_family_is_found_by_name_in_any_case() {
        let collection = Collection::new().with_layer(layer(Role::Application, &["Noto Sans"]));
        for name in ["Noto Sans", "noto sans", "NOTO SANS"] {
            let family = collection.family(name).expect(name);
            assert_eq!(family.name(), "Noto Sans");
        }
    }

    #[test]
    fn a_miss_is_a_miss() {
        let collection = Collection::new().with_layer(layer(Role::System, &["Roboto"]));
        assert!(collection.family("Helvetica").is_none());
        assert!(collection.family("Robot").is_none());
        assert!(collection.family("Roboto Flex").is_none());
    }

    #[test]
    fn many_families_are_all_found_and_nothing_else_is() {
        // A binary search over the name index: every name in, in any case,
        // and the names between them out.
        let names = [
            "Arial",
            "Courier New",
            "Helvetica",
            "Noto Sans",
            "Roboto",
            "Times",
        ];
        let collection = Collection::new().with_layer(layer(Role::System, &names));
        for name in names {
            assert_eq!(collection.family(name).expect(name).name(), name);
            assert!(
                collection.family(&name.to_uppercase()).is_some(),
                "{name} in capitals"
            );
        }
        for name in ["", "A", "Bodoni", "Noto", "Zapfino"] {
            assert!(collection.family(name).is_none(), "{name:?} should miss");
        }
    }

    #[test]
    fn a_missing_family_finds_chrome_alias() {
        let collection = Collection::new().with_layer(layer(
            Role::System,
            &["Times New Roman", "Courier New", "Arial"],
        ));
        let found = |name: &str| {
            collection
                .family(name)
                .map(|family| String::from(family.name()))
        };
        assert_eq!(found("Times").as_deref(), Some("Times New Roman"));
        assert_eq!(found("COURIER").as_deref(), Some("Courier New"));
        assert_eq!(found("helvetica").as_deref(), Some("Arial"));
        assert_eq!(found("Helvetica Neue"), None);

        // The other way about.
        let collection = Collection::new().with_layer(layer(Role::System, &["Times", "Helvetica"]));
        let found = |name: &str| {
            collection
                .family(name)
                .map(|family| String::from(family.name()))
        };
        assert_eq!(found("Times New Roman").as_deref(), Some("Times"));
        assert_eq!(found("Arial").as_deref(), Some("Helvetica"));
    }

    #[test]
    fn a_document_family_takes_no_alias() {
        // `@font-face` names match as written; an alias reaches only
        // installed families.
        let collection = Collection::new()
            .with_layer(layer(Role::System, &["Roboto"]))
            .with_layer(layer(Role::Document, &["Arial", "Times"]));
        assert!(collection.family("Helvetica").is_none());
        let times = collection.family("Times").expect("the page's Times");
        assert_eq!(times.role(), Role::Document);
    }

    #[test]
    fn chrome_aliases_follow_the_platform() {
        // Windows looks up its truetype stand-ins for bitmap families first.
        assert_eq!(replacement("times", true), Some("Times New Roman"));
        assert_eq!(
            replacement("MS Sans Serif", true),
            Some("Microsoft Sans Serif")
        );
        assert_eq!(replacement("MS Serif", true), Some("Times New Roman"));
        assert_eq!(replacement("Courier", true), Some("Courier New"));
        assert_eq!(replacement("Times", false), None);
        // A missing Courier New tries Courier everywhere but Windows.
        assert_eq!(alternate("Courier New", false), Some("Courier"));
        assert_eq!(alternate("Courier New", true), None);
        assert_eq!(alternate("Courier", true), Some("Courier New"));
        for windows in [false, true] {
            assert_eq!(alternate("Times New Roman", windows), Some("Times"));
            assert_eq!(alternate("ARIAL", windows), Some("Helvetica"));
            assert_eq!(alternate("Verdana", windows), None);
        }
    }

    #[test]
    fn a_document_font_shadows_the_system_for_lookup() {
        let collection = Collection::new()
            .with_layer(layer(Role::System, &["Roboto"]))
            .with_layer(layer(Role::Document, &["Roboto"]));
        let found = collection.family("Roboto").expect("present");
        assert_eq!(found.role(), Role::Document);
    }

    #[test]
    fn fallback_does_not_see_document_fonts() {
        // The same stack as above. Fallback naming Roboto must get the
        // system's, whatever the page calls Roboto.
        let collection = Collection::new()
            .with_layer(layer(Role::System, &["Roboto"]))
            .with_layer(layer(Role::Document, &["Roboto", "Brand Serif"]));
        let roboto = collection
            .fallback_family("Roboto")
            .expect("the system has it");
        assert_eq!(roboto.role(), Role::System);
        // A family only a document has is not available to fallback at all.
        assert!(collection.fallback_family("Brand Serif").is_none());
    }

    #[test]
    fn fallback_skips_application_fonts_when_there_is_a_system() {
        let collection = Collection::new()
            .with_layer(layer(Role::System, &["Roboto"]))
            .with_layer(layer(Role::Application, &["Noto Sans"]));
        assert!(collection.fallback_family("Noto Sans").is_none());
    }

    #[test]
    fn fallback_reads_application_fonts_when_there_is_no_system() {
        // wasm, or a program shipping every font it uses.
        let collection = Collection::new()
            .with_layer(layer(Role::Application, &["Noto Sans"]))
            .with_layer(layer(Role::Document, &["Noto Sans"]));
        let found = collection
            .fallback_family("Noto Sans")
            .expect("the application has it");
        assert_eq!(found.role(), Role::Application);
    }

    /// Lists names up front and hands over fonts only when asked, counting
    /// how often it is asked — the shape of DirectWrite and fontconfig.
    #[derive(Debug, Default)]
    struct Counted {
        loads: core::sync::atomic::AtomicUsize,
    }

    impl LoadFamily for Counted {
        fn load(&self, name: &str) -> Vec<Font> {
            self.loads
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            vec![Font::from_data(font_named(name), 0)]
        }
    }

    impl Counted {
        fn loads(&self) -> usize {
            self.loads.load(core::sync::atomic::Ordering::Relaxed)
        }
    }

    fn named(names: &[(&str, &[&str])]) -> (Arc<Layer>, Arc<Counted>) {
        let loader = Arc::new(Counted::default());
        let layer = Arc::new(Layer::from_names(
            Role::System,
            names.iter().map(|(name, aliases)| {
                (
                    String::from(*name),
                    aliases.iter().map(|a| String::from(*a)),
                )
            }),
            loader.clone(),
        ));
        (layer, loader)
    }

    #[test]
    fn a_lookup_loads_nothing() {
        // Completeness needs the names and nothing else. Finding a family must
        // not load it, or listing a thousand families would parse them all.
        let (layer, loader) = named(&[("Roboto", &[]), ("Noto Sans", &[])]);
        let collection = Collection::new().with_layer(layer.clone());
        let roboto = collection.family("Roboto").expect("present");
        let _ = (roboto.name(), roboto.role(), roboto.aliases().count());
        assert!(!roboto.is_loaded());
        assert_eq!(loader.loads(), 0);
        assert_eq!(layer.loaded(), 0);
    }

    #[test]
    fn a_family_loads_once_and_every_handle_sees_it() {
        let (layer, loader) = named(&[("Roboto", &[]), ("Noto Sans", &[])]);
        let collection = Collection::new().with_layer(layer.clone());
        let first = collection.family("Roboto").expect("present");
        let second = collection.family("ROBOTO").expect("present");

        assert_eq!(first.fonts().len(), 1);
        assert_eq!(loader.loads(), 1);
        // A second handle, and a clone of the first, find it loaded.
        assert!(second.is_loaded());
        assert_eq!(first.clone().fonts().len(), 1);
        assert_eq!(loader.loads(), 1, "loaded again");
        // The family nobody asked about is still untouched.
        assert_eq!(layer.loaded(), 1);
    }

    #[cfg(feature = "std")]
    #[test]
    fn threads_loading_families_of_one_cold_layer_load_each_once() {
        const THREADS: usize = 8;
        let names: Vec<String> = (0..16).map(|n| alloc::format!("Family {n}")).collect();
        let listed: Vec<(&str, &[&str])> = names.iter().map(|name| (&**name, &[][..])).collect();
        let (layer, loader) = named(&listed);
        let collection = Collection::new().with_layer(layer.clone());
        let barrier = std::sync::Barrier::new(THREADS);
        // Each thread walks every family from its own starting point, so
        // threads meet on some families and miss each other on others.
        let walk = |start: usize| -> Vec<usize> {
            barrier.wait();
            let mut seen = vec![0; names.len()];
            for step in 0..names.len() {
                let at = (start * 2 + step) % names.len();
                let family = collection.family(&names[at]).expect("listed");
                let fonts = family.fonts();
                assert_eq!(fonts.len(), 1);
                let bytes = fonts[0].load().expect("loads");
                assert_eq!(bytes.data(), font_named(&names[at]));
                seen[at] = fonts.as_ptr() as usize;
            }
            seen
        };
        let walks: Vec<Vec<usize>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..THREADS)
                .map(|start| scope.spawn(move || walk(start)))
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("no thread panics"))
                .collect()
        });
        // One load a family, and every thread was handed its one table.
        assert_eq!(loader.loads(), names.len());
        assert_eq!(layer.loaded(), names.len());
        assert!(walks.iter().all(|seen| *seen == walks[0]));
    }

    #[test]
    fn an_alias_finds_the_family() {
        // DirectWrite lists MS Gothic under its Japanese name too.
        let (layer, _) = named(&[("MS Gothic", &["ＭＳ ゴシック"])]);
        let collection = Collection::new().with_layer(layer);
        let by_alias = collection.family("ＭＳ ゴシック").expect("by alias");
        let by_name = collection.family("ms gothic").expect("by name");
        assert_eq!(by_alias, by_name);
        assert_eq!(by_alias.name(), "MS Gothic");
        assert_eq!(by_alias.aliases().collect::<Vec<_>>(), ["ＭＳ ゴシック"]);
    }

    #[test]
    fn a_name_listed_twice_keeps_the_first_family() {
        // Two families, or a family and another's alias, claiming one name:
        // the first listed keeps it, and the second is not a second answer.
        let (layer, _) = named(&[
            ("Gothic", &[]),
            ("GOTHIC", &[]),
            ("Other", &["gothic", "Other Alias"]),
        ]);
        assert_eq!(layer.len(), 2, "the second Gothic should be dropped");
        let collection = Collection::new().with_layer(layer);
        assert_eq!(
            collection.family("gothic").expect("present").name(),
            "Gothic"
        );
        assert_eq!(
            collection.family("other alias").expect("present").name(),
            "Other"
        );
    }

    #[test]
    fn handles_are_equal_while_their_family_is_untouched() {
        let mut fonts = LayerBuilder::new(Role::Application);
        fonts.add_data(font_named("Roboto")).expect("adds");
        let before = fonts.family("Roboto").expect("present");
        assert_eq!(before, fonts.family("ROBOTO").expect("present"));

        // Adding another family changes the layer, not Roboto: a shaping
        // cache keyed by the handle keeps its entries.
        fonts.add_data(font_named("Noto Sans")).expect("adds");
        assert_eq!(before, fonts.family("Roboto").expect("present"));

        // Adding a font to Roboto changes Roboto, so it is a new family.
        fonts.add_data(font_named("Roboto")).expect("adds");
        assert_ne!(before, fonts.family("Roboto").expect("present"));
    }

    #[test]
    fn different_families_are_different_handles() {
        let collection =
            Collection::new().with_layer(layer(Role::System, &["Roboto", "Noto Sans"]));
        assert_ne!(
            collection.family("Roboto").expect("present"),
            collection.family("Noto Sans").expect("present")
        );
    }

    #[test]
    fn a_handle_outlives_the_collection_it_came_from() {
        let (layer, _) = named(&[("Roboto", &[])]);
        let family = Collection::new()
            .with_layer(layer)
            .family("Roboto")
            .expect("present");
        // The collection is gone; the layer the handle needs is not.
        assert_eq!(family.fonts().len(), 1);
    }

    #[test]
    fn collections_and_handles_can_cross_threads() {
        fn send_sync<T: Send + Sync>() {}
        send_sync::<Collection>();
        send_sync::<Family>();
        send_sync::<Arc<Layer>>();
    }

    #[test]
    fn an_alias_is_another_name_for_a_family_and_not_a_family() {
        let mut builder = LayerBuilder::new(Role::Application);
        builder
            .add_data(crate::test_fonts::font_named("Roboto"))
            .expect("adds");
        builder
            .add_data(crate::test_fonts::font_named("Noto Serif"))
            .expect("adds");

        assert!(builder.add_alias("arial", "Roboto"));
        assert!(builder.add_alias("georgia", "Noto Serif"));
        // Nothing to point at, and nothing that can be pointed at twice.
        assert!(!builder.add_alias("helvetica", "No Such Family"));
        assert!(!builder.add_alias("arial", "Noto Serif"));
        // A family's own name is taken too.
        assert!(!builder.add_alias("Roboto", "Noto Serif"));

        let layer = builder.snapshot();
        let collection = Collection::new().with_layer(layer.clone());
        let fonts = |name: &str| {
            collection
                .family(name)
                .map(|f| f.fonts().len())
                .unwrap_or(0)
        };
        assert_eq!(fonts("arial"), fonts("Roboto"));
        assert_eq!(fonts("Arial"), fonts("Roboto"));
        assert_eq!(fonts("georgia"), fonts("Noto Serif"));
        assert_eq!(fonts("verdana"), 0);
        // Two families, four names.
        assert_eq!(layer.len(), 2);
        assert_eq!(layer.names().count(), 2);
    }

    #[test]
    fn local_finds_a_font_by_its_full_or_postscript_name_in_a_layer_of_bytes() {
        // A layer built from bytes or files has no loader to ask, so it reads
        // its own fonts' names. Android's system layer is one of these, and
        // so is anything on wasm.
        use crate::test_fonts::{EN_US, WINDOWS, cmap, font_with_tables, format4, name, os2};
        use read_fonts::tables::name::NameId;

        let font = |family: &str, full: &str, postscript: &str, weight: u16| {
            font_with_tables(&[
                (*b"OS/2", os2(weight, 5, 0)),
                (*b"cmap", cmap(&[(3, 1, format4(&[(0x41, 0x5A)]))])),
                (
                    *b"name",
                    name(&[
                        (WINDOWS, EN_US, NameId::FAMILY_NAME, family),
                        (WINDOWS, EN_US, NameId::FULL_NAME, full),
                        (WINDOWS, EN_US, NameId::POSTSCRIPT_NAME, postscript),
                    ]),
                ),
            ])
        };
        let mut builder = LayerBuilder::new(Role::System);
        builder
            .add_data(font(
                "Brand Sans",
                "Brand Sans Regular",
                "BrandSans-Regular",
                400,
            ))
            .expect("adds");
        builder
            .add_data(font(
                "Brand Sans",
                "Brand Sans Semibold",
                "BrandSans-Semibold",
                600,
            ))
            .expect("adds");
        let layer = builder.snapshot();

        let weight = |name: &str| layer.local(name).map(|font| font.weight().value());
        assert_eq!(weight("Brand Sans Semibold"), Some(600.0));
        assert_eq!(weight("BrandSans-Semibold"), Some(600.0));
        assert_eq!(weight("Brand Sans Regular"), Some(400.0));
        // The name is matched as a family name is, so case need not agree.
        assert_eq!(weight("brandsans-semibold"), Some(600.0));
        // A family name is not a `local()` name.
        assert_eq!(weight("Brand Sans"), None);
        assert_eq!(weight("No Such Font"), None);

        // Fonts added after the index was built are found too.
        let mut builder = LayerBuilder::new(Role::System);
        builder
            .add_data(font(
                "Brand Sans",
                "Brand Sans Regular",
                "BrandSans-Regular",
                400,
            ))
            .expect("adds");
        assert!(builder.layer().local("Brand Sans Black").is_none());
        builder
            .add_data(font(
                "Brand Sans",
                "Brand Sans Black",
                "BrandSans-Black",
                900,
            ))
            .expect("adds");
        assert_eq!(
            builder
                .layer()
                .local("Brand Sans Black")
                .map(|font| font.weight().value()),
            Some(900.0)
        );
    }

    #[cfg(feature = "std")]
    #[test]
    fn threads_asking_local_of_one_cold_layer_of_files_all_find_their_fonts() {
        use crate::test_fonts::{EN_US, Temporary, WINDOWS, font_with_names};
        use read_fonts::tables::name::NameId;

        let files: Vec<Temporary> = (0..8)
            .map(|n| {
                let full = alloc::format!("Local Sans {n}");
                let bytes = font_with_names(&[
                    (WINDOWS, EN_US, NameId::FAMILY_NAME, "Local Sans"),
                    (WINDOWS, EN_US, NameId::FULL_NAME, &full),
                ]);
                Temporary::new(&alloc::format!("local-{n}.ttf"), &bytes)
            })
            .collect();
        let mut builder = LayerBuilder::new(Role::Application);
        for file in &files {
            builder.add_path(file.path());
        }
        let layer = builder.snapshot();
        let barrier = std::sync::Barrier::new(files.len());
        let found: Vec<Option<std::path::PathBuf>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..files.len())
                .map(|n| {
                    let (layer, barrier) = (&layer, &barrier);
                    scope.spawn(move || {
                        barrier.wait();
                        let font = layer.local(&alloc::format!("Local Sans {n}"))?;
                        match font.source() {
                            crate::Source::Path(path) => Some(path.to_path_buf()),
                            _ => None,
                        }
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().expect("no thread panics"))
                .collect()
        });
        for (file, found) in files.iter().zip(&found) {
            assert_eq!(found.as_deref(), Some(file.path()));
        }
    }
}
