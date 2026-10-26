//! A font: where its bytes are, and what it is — its attributes, its axes,
//! the characters it maps.

use alloc::boxed::Box;
use alloc::sync::Arc;

use parlance::{FontStyle, FontWeight, FontWidth};
use read_fonts::FontRef;

mod attributes;
mod bytes;
mod charset;
mod face;
#[cfg(feature = "std")]
mod load;
mod matching;
mod names;
mod sequences;
#[cfg(feature = "std")]
mod sfnt;

#[cfg(fontwich_fontconfig)]
pub(crate) use attributes::instance_from_file;
#[cfg(all(feature = "system", windows))]
pub(crate) use attributes::width_from_class;
pub use attributes::{Attributes, Axis};
pub use bytes::FontBytes;
use charset::Builder;
pub use charset::{Charset, CharsetPage};
pub(crate) use face::Face;
pub use face::{FaceDescriptors, FaceId, FaceStyle};
#[cfg(feature = "std")]
pub(crate) use load::FontFile;
pub use matching::Synthesis;
pub(crate) use matching::matching;
#[cfg(all(target_vendor = "apple", feature = "system"))]
pub(crate) use names::read_family_names;
pub(crate) use names::{family_names, local_names, name_table};
#[cfg(feature = "std")]
pub(crate) use names::{postscript_name, standalone_name_table};
use sequences::{Selector, Sequences};
#[cfg(feature = "std")]
pub(crate) use sfnt::{FileFont, TABLE_LIMIT, read_u32};

/// The tables a font draws color glyphs from.
const COLOR_TABLES: [&[u8; 4]; 4] = [b"COLR", b"CBDT", b"sbix", b"SVG "];

/// The source of a font's bytes.
#[derive(Clone, Debug)]
pub enum Source {
    /// Shared font bytes in memory.
    Data(FontBytes),
    /// A font file loaded on demand.
    ///
    /// Paths preserve non-UTF-8 file names on supported platforms.
    #[cfg(feature = "std")]
    Path(Arc<std::path::Path>),
    /// A declared face with no font data.
    ///
    /// Participates in matching but maps no characters. See [`Font::wants`].
    Pending,
}

/// A font and its metadata.
///
/// Identifies one font within a file or buffer. A font collection contains
/// multiple fonts distinguished by [`index`](Self::index). Variable-font
/// metadata describes the default instance and its axes, unless the font
/// represents a pinned named instance.
///
/// Attributes, axes, and character coverage are read at construction. Fonts
/// with identical coverage in a layer share a [`Charset`]. Font bytes are
/// loaded separately by [`load`](Self::load).
#[derive(Clone, Debug)]
pub struct Font {
    pub(crate) source: Source,
    index: u32,
    attributes: Attributes,
    /// Boxed rather than a `Vec`: most fonts have none, and a font is
    /// eight bytes smaller for it.
    axes: Box<[Axis]>,
    pub(crate) charset: Arc<Charset>,
    /// Where its variation sequences are, and what has been read of them,
    /// where it has any: see
    /// [`maps_variation_sequence`](Self::maps_variation_sequence). Shared by
    /// every copy of the font, so an answer read once is kept for all.
    sequences: Option<Arc<Sequences>>,
    /// Whether it has color glyphs: see [`is_color`](Self::is_color).
    color: bool,
    /// The `@font-face` rule it was declared by, if any.
    pub(crate) face: Option<Arc<Face>>,
    /// For a font on disk, the file it loads through, with the id its bytes
    /// will have: its layer's, shared by every font there from the file, or
    /// its own for a font made alone.
    #[cfg(feature = "std")]
    pub(crate) file: Option<Arc<load::FontFile>>,
}

/// An identifier for font data.
///
/// Combines the byte-source ID with the font's collection index. Equal keys
/// identify the same font data. [`Font::key`] returns the key without
/// loading bytes.
///
/// Face descriptors are not part of the key. Caches that depend on
/// variation settings or size adjustments must include them separately.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct FontKey {
    /// The byte-source ID. See [`FontBytes::id`].
    pub source: u64,
    /// The font index within the source. See [`Font::index`].
    pub index: u32,
}

impl Font {
    /// Creates a font from a byte buffer.
    ///
    /// `index` identifies the font within a collection. Invalid data or an
    /// invalid index produces default attributes and empty character
    /// coverage.
    pub fn from_data(data: impl Into<FontBytes>, index: u32) -> Self {
        let data = data.into();
        let ((attributes, axes), charset, sequences, color) =
            FontRef::from_index(data.data(), index)
                .map(|font| {
                    let color = COLOR_TABLES
                        .iter()
                        .any(|tag| font.table_data(read_fonts::types::Tag::new(tag)).is_some());
                    (
                        attributes::from_font(&font),
                        Charset::from_font(&font),
                        Sequences::from_font(&font),
                        color,
                    )
                })
                .unwrap_or_default();
        Self {
            source: Source::Data(data),
            index,
            attributes,
            axes: axes.into_boxed_slice(),
            charset: Arc::new(charset),
            sequences: sequences.map(Arc::new),
            color,
            face: None,
            #[cfg(feature = "std")]
            file: None,
        }
    }

    /// Reads font metadata from a file.
    ///
    /// Returns `None` if the file is unreadable, is not a font, or has no
    /// font at `index`. Reads metadata tables only; [`load`](Self::load)
    /// reads the full file.
    #[cfg(feature = "std")]
    pub fn from_path(path: impl Into<Arc<std::path::Path>>, index: u32) -> Option<Self> {
        let path = path.into();
        let mut font = sfnt::FileFont::open(&path, index)?;
        Some(Self::from_file(path, index, &mut font))
    }

    /// Returns the source of the font's bytes.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Returns the font's index within its source.
    ///
    /// Zero for a source containing a single font.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Returns the font's data key, if available.
    ///
    /// Returns `None` for a pending face. Does not load bytes or acquire a
    /// lock. The key matches the ID returned by [`load`](Self::load) and
    /// remains stable across reloads.
    ///
    /// Fonts from the same file in a layer share a source ID. Independently
    /// created layers or fonts may assign different IDs to the same path.
    pub fn key(&self) -> Option<FontKey> {
        let source = match &self.source {
            Source::Data(bytes) => bytes.id(),
            #[cfg(feature = "std")]
            Source::Path(_) => self.file.as_ref()?.id(),
            Source::Pending => return None,
        };
        Some(FontKey {
            source,
            index: self.index,
        })
    }

    /// Returns the font's width, style, and weight.
    ///
    /// For an unpinned variable font, these describe its default instance.
    pub fn attributes(&self) -> Attributes {
        self.attributes
    }

    /// Returns the font's width.
    pub fn width(&self) -> FontWidth {
        self.attributes.width
    }

    /// Returns the font's style.
    pub fn style(&self) -> FontStyle {
        self.attributes.style
    }

    /// Returns the font's weight.
    pub fn weight(&self) -> FontWeight {
        self.attributes.weight
    }

    /// Returns the font's variation axes.
    ///
    /// The slice is empty for a static font.
    pub fn axes(&self) -> &[Axis] {
        &self.axes
    }

    /// Returns the variation axis with the given tag, if present.
    pub fn axis(&self, tag: &[u8; 4]) -> Option<&Axis> {
        self.axes.iter().find(|axis| axis.tag.to_bytes() == *tag)
    }

    /// Returns `true` if the font has a weight axis.
    pub fn has_weight_axis(&self) -> bool {
        self.has_axis(b"wght")
    }

    /// Returns `true` if the font has a width axis.
    pub fn has_width_axis(&self) -> bool {
        self.has_axis(b"wdth")
    }

    /// Returns `true` if the font has a slant axis.
    pub fn has_slant_axis(&self) -> bool {
        self.has_axis(b"slnt")
    }

    /// Returns `true` if the font has an italic axis.
    pub fn has_italic_axis(&self) -> bool {
        self.has_axis(b"ital")
    }

    /// Returns `true` if the font has color glyph tables.
    ///
    /// Recognizes `COLR`, `CBDT`, `sbix`, and `SVG` tables. This checks
    /// table presence, not whether a particular glyph has color data.
    pub fn is_color(&self) -> bool {
        self.color
    }

    /// Returns the face descriptors, if present.
    pub fn descriptors(&self) -> Option<&FaceDescriptors> {
        self.face.as_ref().map(|face| &face.descriptors)
    }

    /// Returns the face ID, if present.
    pub fn face_id(&self) -> Option<FaceId> {
        self.face.as_ref().map(|face| face.id)
    }

    /// Returns `true` if the face has no font data.
    ///
    /// Pending faces participate in matching but map no characters. Use
    /// [`LayerBuilder::load_face`](crate::LayerBuilder::load_face) to
    /// supply data.
    pub fn is_pending(&self) -> bool {
        matches!(self.source, Source::Pending)
    }

    /// Returns `true` if the face's range includes `c`.
    ///
    /// Always returns `true` for fonts not declared by an `@font-face`
    /// rule. CSS uses U+0020 to select the first available font.
    pub fn serves(&self, c: char) -> bool {
        self.face
            .as_ref()
            .is_none_or(|face| face.serves(u32::from(c)))
    }

    /// Returns `true` if a pending face's range includes `c`.
    ///
    /// After matching a face, use this to decide whether to request its
    /// data. It does not check whether the eventual font maps the
    /// character.
    pub fn wants(&self, c: char) -> bool {
        self.is_pending()
            && self
                .face
                .as_ref()
                .is_some_and(|face| face.serves(u32::from(c)))
    }

    /// Returns the font's character coverage.
    ///
    /// Empty if the font has no Unicode character map. Face coverage is
    /// limited by its `unicode-range` descriptor.
    pub fn charset(&self) -> &Charset {
        &self.charset
    }

    /// Returns `true` if the font has a variation-sequence table.
    ///
    /// Checks for a format 14 character-map subtable without reading its
    /// contents. See
    /// [`maps_variation_sequence`](Self::maps_variation_sequence).
    pub fn has_variation_sequences(&self) -> bool {
        self.sequences.is_some()
    }

    /// Returns `true` if the font maps a variation sequence.
    ///
    /// Checks the pair `base`, `selector` against the format 14
    /// character-map subtable. Both default and non-default sequences
    /// require `base` to be in the font's charset, including any face
    /// `unicode-range` restriction.
    ///
    /// Returns `false` for invalid selectors, absent sequences, or
    /// unreadable tables. Supports VS1 through VS256.
    ///
    /// The first lookup allocates a shared cache of 256 pairs and, for a
    /// file-backed font, reads the subtable. Subsequent lookups use this
    /// cache; entries may be replaced by other pairs.
    pub fn maps_variation_sequence(&self, base: char, selector: char) -> bool {
        let Some(sequences) = &self.sequences else {
            return false;
        };
        let Some(selector) = Selector::new(selector) else {
            return false;
        };
        self.charset.contains(base) && sequences.lists(&self.source, base, selector)
    }

    /// Returns adjustments for the requested attributes.
    ///
    /// Includes variation settings and synthetic bold or slant. Use with
    /// the same request passed to
    /// [`Family::match_font`](crate::Family::match_font).
    pub fn synthesis(&self, request: Attributes) -> Synthesis {
        matching::synthesis(self, request)
    }

    /// Loads the font's bytes.
    ///
    /// Returns the supplied buffer for an in-memory font. Reads the file,
    /// or maps it with `mmap`, for a file-backed font. Returns `None` for a
    /// pending face or an unreadable file.
    ///
    /// Fonts from the same file in a layer share bytes while any caller
    /// retains them. The byte ID remains stable across reloads. A
    /// collection file is returned in full; use [`index`](Self::index) to
    /// select its font.
    ///
    /// Concurrent loads of one file in a layer read it once. Other threads
    /// loading that file block until its bytes are available; loads of other
    /// files do not wait.
    pub fn load(&self) -> Option<FontBytes> {
        match &self.source {
            Source::Data(bytes) => Some(bytes.clone()),
            #[cfg(feature = "std")]
            Source::Path(path) => self.file.as_ref()?.load(path),
            Source::Pending => None,
        }
    }

    /// A face declared with no data yet.
    pub(crate) fn pending(face: Arc<Face>) -> Self {
        Self {
            source: Source::Pending,
            index: 0,
            attributes: face.attributes(Attributes::default()),
            axes: Box::default(),
            charset: Arc::default(),
            sequences: None,
            color: false,
            face: Some(face),
            #[cfg(feature = "std")]
            file: None,
        }
    }

    /// This font as the face `face` declares it: its descriptors in place of
    /// its own attributes, and mapping only what it maps within the face's
    /// `unicode-range`.
    pub(crate) fn with_face(mut self, face: Arc<Face>) -> Self {
        self.attributes = face.attributes(self.attributes);
        if let Some(range) = &face.range {
            self.charset = Arc::new(self.charset.intersection(range));
        }
        self.face = Some(face);
        self
    }

    /// The font `font`, opened from `path` at `index`: what
    /// [`from_path`](Self::from_path) reads, for a caller that has the file
    /// open for more.
    #[cfg(feature = "std")]
    pub(crate) fn from_file(
        path: Arc<std::path::Path>,
        index: u32,
        font: &mut sfnt::FileFont,
    ) -> Self {
        let (attributes, axes) = attributes::from_file(font);
        let (charset, sequences) = charset::from_file(font);
        let color = COLOR_TABLES.iter().any(|tag| font.locate(tag).is_some());
        Self {
            source: Source::Path(path),
            index,
            attributes,
            axes: axes.into_boxed_slice(),
            charset: Arc::new(charset),
            sequences: sequences.map(Arc::new),
            color,
            face: None,
            // A file of its own until a layer takes it in, so that alone it
            // still shares its bytes between loads and keeps their id.
            file: Some(Arc::new(load::FontFile::new())),
        }
    }

    /// Takes a weight and width the platform knows better than the font's own
    /// tables: see the DirectWrite layer.
    #[cfg(all(feature = "system", windows))]
    pub(crate) fn correct(&mut self, weight: FontWeight, width: FontWidth) {
        self.attributes.weight = weight;
        self.attributes.width = width;
    }

    /// Holds it at `coordinates`: the named instance the platform matched by
    /// name, which is one point of the font rather than the whole of it.
    ///
    /// Each axis named narrows to that point, so everything downstream —
    /// which requests it matches, what
    /// [`synthesis`](Self::synthesis) sets — follows from its axes as
    /// before. Their defaults stay the file's, since it is the file's default
    /// a shaper draws when nothing is applied, and so the point still has to
    /// be asked for. Its weight, width and style become the point's.
    ///
    /// Nothing is remembered anywhere: the platform is asked when a font is
    /// looked up by name, and the answer goes here.
    #[cfg(feature = "std")]
    // Every system layer's `local()`: a build with no system layer at all
    // looks nothing up by name and so pins nothing.
    // Android's layer is the files it scanned, with no loader to look a font
    // up by name, so it pins nothing either.
    #[cfg_attr(
        any(
            not(all(any(windows, unix), feature = "system")),
            target_os = "android"
        ),
        allow(dead_code)
    )]
    pub(crate) fn pin(&mut self, coordinates: &[([u8; 4], f32)]) {
        for (tag, value) in coordinates {
            let Some(axis) = self
                .axes
                .iter_mut()
                .find(|axis| axis.tag.to_bytes() == *tag)
            else {
                continue;
            };
            if axis.min == axis.max {
                continue;
            }
            let value = matching::clamp(*value, axis.min, axis.max);
            // A GX axis states its own units, so its value is not a weight
            // or a percentage to read off: `local("Skia Bold")` would come
            // back at weight 1.9 and `local("Skia Condensed")` at 0.6% of
            // normal. The font is still held at the point, and matching
            // converts — see `attributes::css_scale`.
            let css = attributes::states_css(axis);
            axis.min = value;
            axis.max = value;
            match tag {
                b"wght" if css => self.attributes.weight = FontWeight::new(value),
                b"wdth" if css => self.attributes.width = FontWidth::from_percentage(value),
                // As `attributes::from_tables` reads them: `ital` is a flag,
                // and `slnt` counts the other way from CSS.
                b"ital" if value >= 0.5 => self.attributes.style = FontStyle::Italic,
                b"slnt" if value != 0.0 => {
                    self.attributes.style = FontStyle::Oblique(Some(-value));
                }
                _ => {}
            }
        }
    }

    /// Whether `coordinates` hold one of its variable axes away from its
    /// default: a named instance of it, where the platform matched a name,
    /// rather than the font itself.
    ///
    /// A platform gives a static font coordinates too — DirectWrite its
    /// weight and slope, Core Text an empty dictionary or, for PingFang's
    /// Medium, a stray one — so it is the font's own `fvar` that decides
    /// which of them vary at all.
    #[cfg(all(feature = "system", any(windows, target_vendor = "apple")))]
    pub(crate) fn is_instance(&self, coordinates: &[([u8; 4], f32)]) -> bool {
        coordinates.iter().any(|(tag, value)| {
            self.axis(tag)
                .is_some_and(|axis| axis.min < axis.max && *value != axis.default)
        })
    }

    fn has_axis(&self, tag: &[u8; 4]) -> bool {
        self.axis(tag).is_some()
    }

    /// Whether its cache of variation sequences has been made, for tests to
    /// see that a font never asked, or without them, has none.
    #[cfg(test)]
    pub(crate) fn has_sequence_cache(&self) -> bool {
        self.sequences
            .as_ref()
            .is_some_and(|sequences| sequences.is_cached())
    }

    /// What it holds on the heap beyond itself, with anything `seen` has
    /// already counted left out.
    pub(crate) fn heap(&self, seen: &mut crate::heap::Seen) -> usize {
        use crate::heap::ARC;
        use core::mem::size_of;
        let source = match &self.source {
            Source::Data(bytes) => bytes.heap(seen),
            #[cfg(feature = "std")]
            Source::Path(path) => seen.once(Arc::as_ptr(path), || ARC + path.as_os_str().len()),
            Source::Pending => 0,
        };
        let face = self.face.as_ref().map_or(0, |face| {
            seen.once(Arc::as_ptr(face), || {
                ARC + size_of::<Face>()
                    + face.descriptors.unicode_range.capacity()
                        * size_of::<core::ops::RangeInclusive<u32>>()
                    + face.range.as_ref().map_or(0, Charset::heap)
                    + face.descriptors.feature_settings.capacity()
                        * size_of::<parlance::FontFeature>()
                    + face.descriptors.variation_settings.capacity()
                        * size_of::<parlance::FontVariation>()
            })
        });
        let charset = seen.once(Arc::as_ptr(&self.charset), || {
            ARC + size_of::<Charset>() + self.charset.heap()
        });
        let sequences = self.sequences.as_ref().map_or(0, |sequences| {
            seen.once(Arc::as_ptr(sequences), || {
                ARC + size_of::<Sequences>() + sequences.heap()
            })
        });
        // The cell, not the bytes: those are held weakly, by whoever loaded
        // them.
        #[cfg(feature = "std")]
        let file = self.file.as_ref().map_or(0, |file| {
            seen.once(Arc::as_ptr(file), || ARC + size_of::<load::FontFile>())
        });
        #[cfg(not(feature = "std"))]
        let file = 0;
        self.axes.len() * size_of::<Axis>() + source + charset + sequences + file + face
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::font_with_tables;

    #[test]
    fn a_font_with_color_glyph_tables_is_a_color_font() {
        for tag in COLOR_TABLES {
            let font = Font::from_data(font_with_tables(&[(*tag, alloc::vec![0; 8])]), 0);
            assert!(font.is_color(), "{:?}", core::str::from_utf8(tag));
        }
        let plain = Font::from_data(font_with_tables(&[(*b"CPAL", alloc::vec![0; 8])]), 0);
        assert!(!plain.is_color());
    }

    #[test]
    fn a_font_from_bytes_is_keyed_by_their_id_and_a_pending_face_by_nothing() {
        use crate::test_fonts::{font_collection, font_named};
        use crate::{FaceDescriptors, LayerBuilder, Role};

        let bytes = FontBytes::new(font_named("Alone"));
        let font = Font::from_data(bytes.clone(), 0);
        let key = FontKey {
            source: bytes.id(),
            index: 0,
        };
        assert_eq!(font.key(), Some(key));
        assert_eq!(font.clone().key(), Some(key));

        // Two families from one `.ttc`: one source, told apart by index,
        // and the same in a later snapshot, where the one family is copied.
        let mut layer = LayerBuilder::new(Role::Application);
        layer
            .add_data(font_collection(&[font_named("One"), font_named("Two")]))
            .expect("adds");
        let keys = |layer: &LayerBuilder| -> alloc::vec::Vec<FontKey> {
            ["One", "Two"]
                .iter()
                .flat_map(|name| layer.family(name).expect("added").fonts()[0].key())
                .collect()
        };
        let before = keys(&layer);
        assert_eq!(before[0].source, before[1].source);
        assert_eq!((before[0].index, before[1].index), (0, 1));
        let _held = layer.snapshot();
        layer.add_data(font_named("One")).expect("adds");
        assert_eq!(keys(&layer), before);
        for name in ["One", "Two"] {
            for font in layer.family(name).expect("added").fonts() {
                let key = font.key().expect("bytes");
                assert_eq!(Some(key.source), font.load().map(|bytes| bytes.id()));
            }
        }

        let mut document = LayerBuilder::new(Role::Document);
        document
            .add_face("Brand", FaceDescriptors::default(), None)
            .expect("declared");
        let family = document.family("Brand").expect("declared");
        let pending = &family.fonts()[0];
        assert!(pending.key().is_none() && pending.load().is_none());
    }

    #[cfg(feature = "std")]
    #[test]
    fn a_font_on_disk_says_so_too() {
        let file = crate::test_fonts::Temporary::new(
            "color.ttf",
            &font_with_tables(&[(*b"COLR", alloc::vec![0; 8])]),
        );
        let font = Font::from_path(std::sync::Arc::<std::path::Path>::from(file.path()), 0)
            .expect("a font");
        assert!(font.is_color());
    }
}
