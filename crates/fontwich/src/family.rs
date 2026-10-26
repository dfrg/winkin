//! Font families: the [`Family`] handle, and when two family names are the
//! same family.
//!
//! One rule compares names. The layer's name index sorts by it, lookups
//! match by it, and a stylesheet's `font-family` is compared by it. Outside a
//! collection, a family is its name. Inside, a lookup returns a [`Family`]
//! handle.

use alloc::sync::Arc;
use core::cmp::Ordering;

use crate::font::{Attributes, Font, matching};
use crate::layer::{FamilyId, FamilyRecord, Layer, Role};

/// A shared handle to a font family.
///
/// Retains the family's layer, independently of the collection. Font
/// metadata is loaded on first use and shared by all handles to the same
/// record. Cloning a handle does not load fonts.
///
/// # Identity
///
/// Equality and hashing use family-record identity, not the family name.
/// Unchanged families compare equal across layer snapshots; modifying a
/// family creates a new record. Use [`names_match`] to compare names.
///
/// Handles can be used as cache keys. They retain layer snapshots until
/// dropped. Equality and hashing are unaffected by lazy loading, so
/// Clippy's `mutable_key_type` warning may be suppressed for maps keyed by
/// `Family`.
#[derive(Clone, Debug)]
pub struct Family {
    layer: Arc<Layer>,
    id: FamilyId,
}

impl Family {
    /// Returns the family's name.
    pub fn name(&self) -> &str {
        self.record().name()
    }

    /// Returns the family's aliases.
    pub fn aliases(&self) -> impl Iterator<Item = &str> {
        self.record().aliases()
    }

    /// Returns the role of the family's layer.
    pub fn role(&self) -> Role {
        self.layer.role()
    }

    /// Returns the family's fonts.
    ///
    /// Loads metadata on first use. The slice may be empty if no fonts
    /// could be loaded.
    pub fn fonts(&self) -> &[Font] {
        self.layer.fonts(self.id)
    }

    /// Returns `true` if the family's metadata has been loaded.
    pub fn is_loaded(&self) -> bool {
        self.record().is_loaded()
    }

    /// Returns `true` if any font maps `c`.
    ///
    /// Loads family metadata on first use.
    pub fn covers(&self, c: char) -> bool {
        self.fonts().iter().any(|font| font.charset().contains(c))
    }

    /// Returns the index of the best matching font.
    ///
    /// Matches width, style, then weight according to CSS Fonts. Variable
    /// fonts match within their axis ranges. Italic requests target 14°
    /// when only oblique fonts are available. Ranges
    /// containing 14° tie, leaving weight to decide.
    ///
    /// `synthesize_style` permits an upright font for an oblique request
    /// when no italic or oblique font exists. Returns `None` for an empty
    /// family. Does not allocate, but may load metadata on first use.
    pub fn match_index(&self, request: Attributes, synthesize_style: bool) -> Option<usize> {
        // Among tied fonts, CSS tries a document's last declared first, and
        // elsewhere the first.
        self.matching_indices(request, synthesize_style).next()
    }

    /// Returns the best matching font.
    ///
    /// Uses [`match_index`](Self::match_index). See [`Font::synthesis`] for
    /// the variations and synthetic styling needed for the request.
    pub fn match_font(&self, request: Attributes, synthesize_style: bool) -> Option<&Font> {
        self.match_index(request, synthesize_style)
            .map(|index| &self.fonts()[index])
    }

    /// Returns the indices of all best-matching fonts.
    ///
    /// Uses the same order as [`matching`](Self::matching). Indices refer
    /// to [`fonts`](Self::fonts), allowing candidates to be stored without
    /// cloning a [`Font`].
    ///
    /// Matching does not allocate, but may load family metadata on first
    /// use.
    pub fn matching_indices(
        &self,
        request: Attributes,
        synthesize_style: bool,
    ) -> impl Iterator<Item = usize> {
        let reverse = self.layer.role() == Role::Document;
        matching(self.fonts(), request, synthesize_style, reverse)
    }

    /// Returns all fonts tied for the best match.
    ///
    /// Uses [`match_index`](Self::match_index) to rank fonts. Ties are
    /// returned in reverse declaration order for document layers and
    /// insertion order otherwise. This supports faces that differ only in
    /// `unicode-range`.
    ///
    /// Matching does not allocate, but may load family metadata on first
    /// use.
    pub fn matching(
        &self,
        request: Attributes,
        synthesize_style: bool,
    ) -> impl Iterator<Item = &Font> {
        let fonts = self.fonts();
        self.matching_indices(request, synthesize_style)
            .map(move |at| &fonts[at])
    }

    pub(crate) fn new(layer: &Arc<Layer>, id: FamilyId) -> Self {
        Self {
            layer: layer.clone(),
            id,
        }
    }

    fn record(&self) -> &Arc<FamilyRecord> {
        self.layer.record(self.id)
    }

    /// Whether family `id` of `layer` is this family's record.
    pub(crate) fn is_record(&self, layer: &Layer, id: FamilyId) -> bool {
        Arc::ptr_eq(self.record(), layer.record(id))
    }

    pub(crate) fn covers_u32(&self, c: u32) -> bool {
        char::from_u32(c).is_some_and(|c| self.covers(c))
    }
}

impl PartialEq for Family {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(self.record(), other.record())
    }
}

impl Eq for Family {}

impl core::hash::Hash for Family {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        // The record's address, which equality compares. A handle holds its
        // layer and so its record, so no other record can reuse the address
        // while the handle lives.
        core::ptr::hash(Arc::as_ptr(self.record()), state);
    }
}

/// Returns `true` if family names match.
///
/// Compares Unicode lowercase characters without normalizing whitespace,
/// punctuation, or Unicode composition. Does not allocate.
///
/// This differs from CSS Default Caseless Matching, which uses full case
/// folding. For example, `ß` does not match `ss`, and `ς` does not match
/// `σ`.
pub fn names_match(a: &str, b: &str) -> bool {
    compare_names(a, b) == Ordering::Equal
}

/// Orders names by their lowercased characters, as the name index sorts them.
///
/// Names that [`names_match`] compare equal, so a binary search finds them
/// without lowercasing into a buffer.
pub(crate) fn compare_names(a: &str, b: &str) -> Ordering {
    // Nearly every family name is ASCII. An ASCII byte lowercases to one
    // ASCII byte, so ASCII prefixes compare byte by byte. At the first
    // non-ASCII byte, both names sit at a character boundary, and the rest
    // compares by full lowercasing. The Kelvin sign needs this: it lowercases
    // to an ASCII `k`.
    let (x, y) = (a.as_bytes(), b.as_bytes());
    let mut at = 0;
    while let (Some(&p), Some(&q)) = (x.get(at), y.get(at)) {
        if !p.is_ascii() || !q.is_ascii() {
            break;
        }
        match p.to_ascii_lowercase().cmp(&q.to_ascii_lowercase()) {
            Ordering::Equal => at += 1,
            other => return other,
        }
    }
    folded(&a[at..]).cmp(folded(&b[at..]))
}

/// Returns `name` lowercased, as family names are compared.
fn folded(name: &str) -> impl Iterator<Item = char> + '_ {
    name.chars().flat_map(char::to_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_does_not_matter() {
        assert!(names_match("Noto Sans", "noto sans"));
        assert!(names_match("Roboto", "ROBOTO"));
        assert!(names_match("ÉCOLE", "école"));
        assert!(names_match("ΑΛΦΑ", "αλφα"));
    }

    #[test]
    fn where_lowercasing_and_css_folding_part() {
        // Pinned so that moving to full case folding is a decision someone
        // makes, not a side effect: CSS would call each of these a match.
        assert!(!names_match("Straße", "STRASSE"));
        assert!(!names_match("σ", "ς"));
    }

    #[test]
    fn nothing_else_is_folded() {
        // Whitespace and spelling are compared as written. A stylesheet that
        // spells a family differently gets a different family.
        assert!(!names_match("Noto Sans", "NotoSans"));
        assert!(!names_match("Noto Sans", "Noto Sans "));
        assert!(!names_match("Roboto", "Roboto Flex"));
    }

    #[test]
    fn the_ascii_shortcut_orders_exactly_as_lowercasing_does() {
        // Every pair, both ways, against the plain definition. The awkward
        // ones are characters outside ASCII that lowercase into it (the
        // Kelvin sign, dotted capital I) and names that part at the point
        // one of them stops being ASCII.
        let names = [
            "",
            "a",
            "A",
            "ab",
            "Ab",
            "aB",
            "b",
            "Noto Sans",
            "NOTO SANS",
            "Noto Sans CJK",
            "noto sans cjk jp",
            "\u{212A}",
            "k",
            "K",
            "\u{212A}elvin",
            "kelvin",
            "\u{130}",
            "i",
            "i\u{307}",
            "Istanbul",
            "\u{130}stanbul",
            "École",
            "ecole",
            "ÉCOLE",
            "Zapf",
            "zapf ",
            "Z",
            "\u{FF21}",
            "ＭＳ ゴシック",
            "MS Gothic",
            "Ω",
            "ω",
            "~",
        ];
        for a in names {
            for b in names {
                let plain = folded(a).cmp(folded(b));
                assert_eq!(compare_names(a, b), plain, "{a:?} against {b:?}");
            }
        }
    }

    #[test]
    fn the_order_agrees_with_matching() {
        // A binary search over names sorted by `compare_names` only finds a
        // match if matching names compare equal and nothing else does.
        assert_eq!(compare_names("Noto Sans", "NOTO SANS"), Ordering::Equal);
        assert_eq!(compare_names("Arial", "roboto"), Ordering::Less);
        assert_eq!(compare_names("roboto", "Arial"), Ordering::Greater);
        assert_eq!(compare_names("Roboto", "Roboto Flex"), Ordering::Less);
    }

    /// A static font of `weight`, italic or upright.
    fn font(weight: u16, italic: bool) -> Font {
        use crate::test_fonts::{font_with_tables, os2};
        let selection = if italic { 0x0001 } else { 0 };
        Font::from_data(
            font_with_tables(&[(*b"OS/2", os2(weight, 5, selection))]),
            0,
        )
    }

    /// Regular, bold, italic, a second regular, a second bold: two pairs
    /// that tie.
    fn five_fonts() -> alloc::vec::Vec<Font> {
        [
            (400, false),
            (700, false),
            (400, true),
            (400, false),
            (700, false),
        ]
        .into_iter()
        .map(|(weight, italic)| font(weight, italic))
        .collect()
    }

    #[derive(Debug)]
    struct Five;

    impl crate::LoadFamily for Five {
        fn load(&self, _: &str) -> alloc::vec::Vec<Font> {
            five_fonts()
        }
    }

    #[test]
    fn matching_indices_are_the_fonts_matching_gives() {
        use crate::{FaceDescriptors, LayerBuilder};
        use alloc::string::String;
        use alloc::vec::Vec;
        use parlance::{FontStyle, FontWeight, FontWidth};

        let system = Arc::new(Layer::from_names(
            Role::System,
            [(String::from("Five"), Vec::<String>::new())],
            Arc::new(Five),
        ));
        let system = Family::new(&system, FamilyId::FIRST);
        let mut document = LayerBuilder::new(Role::Document);
        for font in five_fonts() {
            document.add_face_font("Five", FaceDescriptors::default(), font);
        }
        let document = document.family("Five").expect("declared");

        let requests = [
            Attributes::default(),
            Attributes {
                weight: FontWeight::BOLD,
                ..Attributes::default()
            },
            Attributes {
                style: FontStyle::Italic,
                weight: FontWeight::new(900.0),
                width: FontWidth::CONDENSED,
            },
            Attributes {
                style: FontStyle::Oblique(Some(20.0)),
                ..Attributes::default()
            },
        ];
        for family in [&system, &document] {
            let fonts = family.fonts();
            for request in requests {
                for synthesize in [false, true] {
                    let by_index: Vec<*const Font> = family
                        .matching_indices(request, synthesize)
                        .map(|at| &fonts[at] as *const Font)
                        .collect();
                    let by_font: Vec<*const Font> = family
                        .matching(request, synthesize)
                        .map(|font| font as *const Font)
                        .collect();
                    assert!(!by_index.is_empty());
                    assert_eq!(by_index, by_font, "{:?} {request:?}", family.role());
                    assert_eq!(
                        family.match_index(request, synthesize),
                        family.matching_indices(request, synthesize).next()
                    );
                }
            }
        }
        // Ties come in CSS's order: as listed for the system, and the last
        // declared first for a document.
        let regular = |family: &Family| -> Vec<usize> {
            family
                .matching_indices(Attributes::default(), true)
                .collect()
        };
        assert_eq!(regular(&system), [0, 3]);
        assert_eq!(regular(&document), [3, 0]);
    }
}
