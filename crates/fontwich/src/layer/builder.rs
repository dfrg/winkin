//! The builder for layers of fonts the caller supplies.
//!
//! A [`LayerBuilder`] adds fonts from bytes, files and directories, and
//! declares a document's `@font-face` rules. It copies on write, so a
//! snapshot someone else holds never changes.
//!
//! A document's `@font-face` rules are a [`Role::Document`] layer of faces.
//! [`LayerBuilder::add_face`] declares a face with its descriptors, with or
//! without its data. [`LayerBuilder::load_face`] supplies the data when the
//! download lands. [`LayerBuilder::add_face_font`] declares a face over an
//! installed font, for `src: local(...)`.
//!
//! [`LayerBuilder::add_path`] adds the fonts in a file or under a directory.
//! To read them on another thread, use [`Scanned`](crate::Scanned), then
//! [`LayerBuilder::add_scanned`].

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

use read_fonts::FileRef;

use crate::Family;
use crate::backend::Backend;
use crate::fallback::FallbackOverride;
use crate::font::{FaceDescriptors, FaceId, Font, FontBytes, family_names, name_table};
use crate::layer::{FamilyId, Layer, Role};

/// An error encountered while adding a font.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddError {
    /// The bytes are not a font or a font collection.
    NotAFont,
    /// No font has a readable family name.
    Unnamed,
    /// No font exists at the given index.
    NoSuchIndex,
    /// No face exists with the given ID in this layer.
    NoSuchFace,
}

impl core::fmt::Display for AddError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NotAFont => "not a font or font collection",
            Self::Unnamed => "no font in it has a readable family name",
            Self::NoSuchIndex => "no font at that index",
            Self::NoSuchFace => "no such face in this layer",
        })
    }
}

/// Returns how many fonts `data` holds: one, or a collection's count.
fn font_count(data: &FontBytes) -> Result<u32, AddError> {
    Ok(
        match FileRef::new(data.data()).map_err(|_| AddError::NotAFont)? {
            FileRef::Font(_) => 1,
            FileRef::Collection(collection) => collection.len(),
        },
    )
}

/// Returns the font at `index` in `data`.
fn font(data: FontBytes, index: u32) -> Result<Font, AddError> {
    if index >= font_count(&data)? {
        return Err(AddError::NoSuchIndex);
    }
    Ok(Font::from_data(data, index))
}

/// A mutable font layer.
///
/// Use [`snapshot`](Self::snapshot) to share the current layer. Later
/// changes use copy-on-write and do not affect existing snapshots.
#[derive(Clone, Debug)]
pub struct LayerBuilder {
    layer: Arc<Layer>,
}

impl LayerBuilder {
    /// Creates an empty layer with the given role.
    ///
    /// Use [`Role::System`] for platform fonts supplied by the caller,
    /// including on targets without a native font API.
    pub fn new(role: Role) -> Self {
        Self {
            layer: Arc::new(Layer::new(role)),
        }
    }

    /// Adds fonts from a buffer.
    ///
    /// Accepts a single font or a font collection. Returns the affected
    /// families in font order, without duplicates. Members without a
    /// readable family name are skipped. Returned handles retain a snapshot
    /// of the updated layer.
    ///
    /// # Errors
    ///
    /// Returns [`AddError::NotAFont`] for invalid data or
    /// [`AddError::Unnamed`] if no member has a readable family name. On
    /// error, the layer is unchanged.
    pub fn add_data(&mut self, data: impl Into<FontBytes>) -> Result<Vec<Family>, AddError> {
        let data: FontBytes = data.into();
        // Reads every name before touching the layer, so a failure cannot
        // leave half a file added.
        let named: Vec<_> = (0..font_count(&data)?)
            .filter_map(|index| {
                let name = name_table(data.data(), index)?;
                Some((family_names(&name)?, index))
            })
            .collect();
        if named.is_empty() {
            return Err(AddError::Unnamed);
        }

        Ok(self.add_named(
            named.into_iter().map(|((name, legacy), index)| {
                (name, legacy, Font::from_data(data.clone(), index))
            }),
        ))
    }

    /// Scans a path and adds its fonts.
    ///
    /// Accepts a font file or a directory. Returns affected families in
    /// scan order, without duplicates. Unreadable files are skipped.
    ///
    /// To scan on another thread, use
    /// [`Scanned::path`](crate::Scanned::path) followed by
    /// [`add_scanned`](Self::add_scanned).
    #[cfg(feature = "std")]
    pub fn add_path(&mut self, path: impl AsRef<std::path::Path>) -> Vec<Family> {
        self.add_scanned(crate::Scanned::path(path.as_ref()))
    }

    /// Adds the fonts from a completed scan.
    ///
    /// Returns affected families in scan order, without duplicates. Font
    /// metadata has already been read by [`Scanned`](crate::Scanned).
    #[cfg(feature = "std")]
    pub fn add_scanned(&mut self, scanned: crate::Scanned) -> Vec<Family> {
        if scanned.is_empty() {
            return Vec::new();
        }
        self.add_named(scanned.fonts)
    }

    /// Adds an alias for a family.
    ///
    /// Returns `false` without changing the layer if `to` is absent or
    /// `alias` is already in use. An alias resolves to the same family and
    /// does not add a family to the layer.
    pub fn add_alias(&mut self, alias: &str, to: &str) -> bool {
        let layer = Arc::make_mut(&mut self.layer);
        let added = layer.add_alias(alias, to);
        if added {
            layer.changed();
        }
        added
    }

    /// Declares an `@font-face` rule.
    ///
    /// `family` is the rule's family name. `data` contains the font bytes
    /// and collection index, or is `None` while the font is pending.
    ///
    /// Matching uses the descriptors even before data is available. A
    /// pending face maps no characters; [`Font::wants`] identifies
    /// characters that require a download. Use
    /// [`load_face`](Self::load_face) to supply the data.
    ///
    /// The declared family shadows families with the same name in lower
    /// layers.
    ///
    /// # Errors
    ///
    /// Returns [`AddError::NotAFont`] for invalid data or
    /// [`AddError::NoSuchIndex`] for an invalid collection index. On error,
    /// the layer is unchanged.
    pub fn add_face(
        &mut self,
        family: &str,
        descriptors: FaceDescriptors,
        data: Option<(FontBytes, u32)>,
    ) -> Result<FaceId, AddError> {
        let font = match data {
            Some((data, index)) => Some(font(data, index)?),
            None => None,
        };
        let layer = Arc::make_mut(&mut self.layer);
        layer.changed();
        Ok(layer.insert_face(String::from(family), descriptors, font).0)
    }

    /// Declares a face using an existing font.
    ///
    /// Use this with [`Collection::local`](crate::Collection::local) to implement `src: local(...)`.
    pub fn add_face_font(
        &mut self,
        family: &str,
        descriptors: FaceDescriptors,
        font: Font,
    ) -> FaceId {
        let layer = Arc::make_mut(&mut self.layer);
        layer.changed();
        layer
            .insert_face(String::from(family), descriptors, Some(font))
            .0
    }

    /// Sets or replaces a face's font data.
    ///
    /// Preserves the face's descriptors.
    ///
    /// # Errors
    ///
    /// Returns [`AddError::NotAFont`] for invalid data,
    /// [`AddError::NoSuchIndex`] for an invalid collection index, or
    /// [`AddError::NoSuchFace`] for an unknown ID. On error, the face is
    /// unchanged.
    pub fn load_face(&mut self, id: FaceId, data: FontBytes, index: u32) -> Result<(), AddError> {
        let font = font(data, index)?;
        let layer = Arc::make_mut(&mut self.layer);
        if !layer.replace_face(id, font) {
            return Err(AddError::NoSuchFace);
        }
        layer.changed();
        Ok(())
    }

    /// Removes a declared face.
    ///
    /// Returns `false` if `id` is unknown. Removes the family if this was
    /// its last face, allowing lookup to find the same name in lower
    /// layers.
    pub fn remove_face(&mut self, id: FaceId) -> bool {
        let layer = Arc::make_mut(&mut self.layer);
        let removed = layer.remove_face(id);
        if removed {
            layer.changed();
        }
        removed
    }

    /// Sets the layer's fallback backend.
    ///
    /// Fallback uses system layers, or application layers if no system
    /// layer exists. It never uses document layers.
    pub fn set_fallback(&mut self, backend: Backend) {
        Arc::make_mut(&mut self.layer).set_fallback(backend);
    }

    /// Sets a fallback override for the layer.
    ///
    /// The override is consulted before the layer's backend, if any. It can
    /// provide a fallback order for fonts supplied by the caller.
    pub fn set_fallback_override(&mut self, fallback: impl FallbackOverride + 'static) {
        Arc::make_mut(&mut self.layer).set_fallback_override(Arc::new(fallback));
    }

    /// Returns a shared snapshot of the layer.
    ///
    /// Later changes to the builder do not affect this snapshot.
    pub fn snapshot(&self) -> Arc<Layer> {
        self.layer.clone()
    }

    /// Returns the current layer.
    pub fn layer(&self) -> &Layer {
        &self.layer
    }

    /// Returns the family matching `name`, if present.
    pub fn family(&self, name: &str) -> Option<Family> {
        Some(Family::new(&self.layer, self.layer.find(name)?))
    }

    /// Returns the layer's generation. See [`Layer::generation`].
    pub fn generation(&self) -> u64 {
        self.layer.generation()
    }

    /// Adds each font to its family, and to a secondary family under its
    /// legacy name if it has one.
    ///
    /// Returns the primary families the fonts landed in, in order, without
    /// repeats.
    fn add_named(
        &mut self,
        fonts: impl IntoIterator<Item = (String, Option<String>, Font)>,
    ) -> Vec<Family> {
        let layer = Arc::make_mut(&mut self.layer);
        layer.changed();
        let mut added: Vec<FamilyId> = Vec::new();
        for (name, legacy, font) in fonts {
            if let Some(legacy) = legacy {
                layer.insert_secondary(legacy, font.clone());
            }
            let family = layer.insert(name, font);
            if !added.contains(&family) {
                added.push(family);
            }
        }
        added
            .into_iter()
            .map(|family| Family::new(&self.layer, family))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Source;
    use crate::test_fonts::{font_collection, font_named};
    use alloc::vec;

    #[test]
    fn every_font_in_a_collection_file_is_added() {
        let file = font_collection(&[
            font_named("Noto Sans CJK JP"),
            font_named("Noto Sans CJK KR"),
        ]);
        let mut fonts = LayerBuilder::new(Role::Application);
        let added = fonts.add_data(file).expect("adds");
        assert_eq!(added.len(), 2);
        assert_eq!(added[0].name(), "Noto Sans CJK JP");

        let jp = fonts.family("Noto Sans CJK JP").expect("JP");
        let kr = fonts.family("Noto Sans CJK KR").expect("KR");
        assert_eq!(jp.fonts()[0].index(), 0);
        assert_eq!(kr.fonts()[0].index(), 1);
        // A match with a catch-all rather than `let ... else`: without `std`
        // there is no `Path` variant, and the pattern would be irrefutable.
        let data = |font: &Font| match font.source() {
            Source::Data(data) => data.clone(),
            #[allow(unreachable_patterns)]
            _ => panic!("fonts added from bytes should be in memory"),
        };
        let (jp, kr) = (data(&jp.fonts()[0]), data(&kr.fonts()[0]));
        assert!(
            Arc::ptr_eq(jp.arc(), kr.arc()),
            "both fonts should share the one file's bytes"
        );
        assert_eq!(jp.id(), kr.id(), "and so the one id");
    }

    #[test]
    fn two_files_of_one_family_share_it() {
        // A regular and a bold, shipped as separate files, are one family.
        // The second spells it in a different case, which is still the same
        // family.
        let mut fonts = LayerBuilder::new(Role::Application);
        let a = fonts.add_data(font_named("Roboto")).expect("adds");
        let b = fonts.add_data(font_named("ROBOTO")).expect("adds");
        assert_eq!(a[0].name(), b[0].name());
        assert_eq!(fonts.layer().len(), 1);
        assert_eq!(fonts.family("Roboto").expect("present").fonts().len(), 2);
    }

    #[test]
    fn data_that_is_not_a_font_adds_nothing() {
        let mut fonts = LayerBuilder::new(Role::Application);
        assert_eq!(fonts.add_data(vec![0u8; 64]), Err(AddError::NotAFont));
        assert_eq!(fonts.add_data(vec![]), Err(AddError::NotAFont));
        assert!(fonts.layer().is_empty());
    }

    #[test]
    fn a_font_with_no_family_name_adds_nothing() {
        use crate::test_fonts::{EN_US, WINDOWS, font_with_names};
        use read_fonts::tables::name::NameId;
        let unnamed = font_with_names(&[(WINDOWS, EN_US, NameId::FULL_NAME, "Roboto Bold")]);
        let mut fonts = LayerBuilder::new(Role::Application);
        assert_eq!(fonts.add_data(unnamed), Err(AddError::Unnamed));
        assert!(fonts.layer().is_empty());
    }

    #[test]
    fn a_snapshot_does_not_see_later_additions() {
        let mut fonts = LayerBuilder::new(Role::Application);
        fonts.add_data(font_named("Roboto")).expect("adds");
        let before = fonts.snapshot();

        fonts.add_data(font_named("Noto Sans")).expect("adds");
        assert_eq!(
            before.len(),
            1,
            "an earlier snapshot changed under its holder"
        );
        assert_eq!(fonts.snapshot().len(), 2);
        assert!(fonts.family("Noto Sans").is_some());
    }
}
