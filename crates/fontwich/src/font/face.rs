//! `@font-face`: a face's declared descriptors, which CSS matches on in
//! place of what the font says about itself.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::RangeInclusive;

use crate::sort;

use parlance::{FontFeature, FontStyle, FontVariation, FontWeight, FontWidth};

use super::matching::clamp;
use super::{Attributes, Builder, Charset};

/// Descriptors for an `@font-face` rule.
///
/// Optional matching descriptors default to the font's values or variation
/// ranges when `None`. Declared values override font metadata during
/// matching and constrain synthesized axis settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FaceDescriptors {
    /// The `font-weight` value or range.
    pub weight: Option<(FontWeight, FontWeight)>,
    /// The `font-width` (`font-stretch`) value or range.
    pub width: Option<(FontWidth, FontWidth)>,
    /// The `font-style` descriptor.
    pub style: Option<FaceStyle>,
    /// The character ranges allowed by `unicode-range`.
    ///
    /// An empty vector allows all characters. Coverage is intersected with
    /// the font's character map. Pending faces are requested only for
    /// characters in these ranges.
    pub unicode_range: Vec<RangeInclusive<u32>>,

    // What follows is carried for the caller, who shapes and measures:
    // nothing here reads it.
    /// Feature settings applied before the element's settings.
    pub feature_settings: Vec<FontFeature>,
    /// Variation settings applied after synthesized axis values.
    ///
    /// The element's own variation settings are applied after these
    /// settings.
    pub variation_settings: Vec<FontVariation>,
    /// The `size-adjust` ratio, where `1.0` means `100%`.
    pub size_adjust: Option<f32>,
    /// The ascent override as a fraction of the em.
    pub ascent_override: Option<f32>,
    /// The descent override as a fraction of the em.
    pub descent_override: Option<f32>,
    /// The line-gap override as a fraction of the em.
    pub line_gap_override: Option<f32>,
}

/// A CSS `font-style` descriptor.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FaceStyle {
    /// `normal`.
    Normal,
    /// `italic`.
    Italic,
    /// An oblique angle range in clockwise degrees.
    ///
    /// `oblique 20deg` is `(20.0, 20.0)`; bare `oblique` is `(14.0, 14.0)`.
    Oblique(f32, f32),
}

/// An identifier for a declared face.
///
/// Valid within the layer that assigned it. Used to load or remove the
/// face.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct FaceId(pub(crate) u32);

/// What a font knows of the rule that declared it.
#[derive(Debug)]
pub(crate) struct Face {
    pub(crate) id: FaceId,
    pub(crate) descriptors: FaceDescriptors,
    /// `unicode_range` as a charset, where there is one.
    pub(crate) range: Option<Charset>,
}

impl Face {
    pub(crate) fn new(id: FaceId, mut descriptors: FaceDescriptors) -> Arc<Self> {
        // A value that is not a number declares nothing: taken as `auto`,
        // rather than left to make a comparison panic.
        let finite = |a: f32, b: f32| a.is_finite() && b.is_finite();
        descriptors.weight = descriptors
            .weight
            .filter(|(lo, hi)| finite(lo.value(), hi.value()));
        descriptors.width = descriptors
            .width
            .filter(|(lo, hi)| finite(lo.ratio(), hi.ratio()));
        descriptors.style = descriptors.style.filter(|style| match style {
            FaceStyle::Oblique(lo, hi) => finite(*lo, *hi),
            _ => true,
        });
        let range = (!descriptors.unicode_range.is_empty()).then(|| {
            let mut builder = Builder::default();
            // Sorted first: the builder is fastest, and folds pages correctly,
            // either way, but a rule may list its ranges in any order.
            let mut ranges: Vec<(u32, u32)> = descriptors
                .unicode_range
                .iter()
                .map(|range| (*range.start(), (*range.end()).min(0x10FFFF)))
                .filter(|(start, end)| start <= end)
                .collect();
            sort::by(&mut ranges, Ord::cmp);
            for (start, end) in ranges {
                builder.insert(start, end);
            }
            builder.finish()
        });
        Arc::new(Self {
            id,
            descriptors,
            range,
        })
    }

    /// Whether the face's range includes `c`.
    pub(super) fn serves(&self, c: u32) -> bool {
        self.range
            .as_ref()
            .is_none_or(|range| range.contains_u32(c))
    }

    /// `attributes`, with what the rule declares in their place: a declared
    /// range holds the font's own value where it can.
    pub(crate) fn attributes(&self, attributes: Attributes) -> Attributes {
        let d = &self.descriptors;
        Attributes {
            weight: d.weight.map_or(attributes.weight, |(lo, hi)| {
                FontWeight::new(clamp(
                    attributes.weight.value(),
                    lo.value(),
                    hi.value().max(lo.value()),
                ))
            }),
            width: d.width.map_or(attributes.width, |(lo, hi)| {
                FontWidth::from_ratio(clamp(
                    attributes.width.ratio(),
                    lo.ratio(),
                    hi.ratio().max(lo.ratio()),
                ))
            }),
            style: match d.style {
                None => attributes.style,
                Some(FaceStyle::Normal) => FontStyle::Normal,
                Some(FaceStyle::Italic) => FontStyle::Italic,
                Some(FaceStyle::Oblique(lo, _)) => FontStyle::Oblique(Some(lo)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_fonts::{cmap, font_with_tables, format4, fvar, os2};
    use crate::{Collection, Font, FontBytes, Layer, LayerBuilder, LoadFamily, Role};
    use alloc::string::String;
    use alloc::vec;

    /// A font mapping a–z and а–я, regular, upright, with `axes`.
    fn font(axes: &[([u8; 4], f32, f32, f32)]) -> FontBytes {
        let mut tables = vec![
            (*b"OS/2", os2(400, 5, 0)),
            (
                *b"cmap",
                cmap(&[(3, 1, format4(&[(0x61, 0x7A), (0x430, 0x44F)]))]),
            ),
        ];
        if !axes.is_empty() {
            tables.push((*b"fvar", fvar(axes)));
        }
        FontBytes::new(font_with_tables(&tables))
    }

    fn weight(value: f32) -> Option<(FontWeight, FontWeight)> {
        Some((FontWeight::new(value), FontWeight::new(value)))
    }

    fn request(weight: f32, style: FontStyle) -> Attributes {
        Attributes {
            weight: FontWeight::new(weight),
            style,
            ..Attributes::default()
        }
    }

    #[test]
    fn a_face_matches_as_declared() {
        let mut layer = LayerBuilder::new(Role::Document);
        let data = font(&[]);
        for value in [400.0, 700.0] {
            layer
                .add_face(
                    "Brand",
                    FaceDescriptors {
                        weight: weight(value),
                        ..FaceDescriptors::default()
                    },
                    Some((data.clone(), 0)),
                )
                .unwrap();
        }
        let family = layer.family("Brand").unwrap();
        let bold = family
            .match_font(request(700.0, FontStyle::Normal), true)
            .unwrap();
        assert_eq!(bold.weight(), FontWeight::new(700.0));
        // Declared bold is bold: nothing to fake, though the file is regular.
        assert!(!bold.synthesis(request(700.0, FontStyle::Normal)).embolden());
        let regular = family
            .match_font(request(400.0, FontStyle::Normal), true)
            .unwrap();
        assert!(
            regular
                .synthesis(request(700.0, FontStyle::Normal))
                .embolden()
        );
    }

    #[test]
    fn a_pending_face_matches_maps_nothing_and_wants_its_range() {
        let mut layer = LayerBuilder::new(Role::Document);
        let id = layer
            .add_face(
                "Brand",
                FaceDescriptors {
                    unicode_range: core::iter::once(0x0430..=0x044F).collect(),
                    ..FaceDescriptors::default()
                },
                None,
            )
            .unwrap();
        let before = layer.generation();
        let family = layer.family("Brand").unwrap();
        let face = family.match_font(Attributes::default(), true).unwrap();
        assert!(face.is_pending());
        assert!(face.charset().is_empty());
        assert!(face.wants('б'));
        assert!(!face.wants('a'));
        assert_eq!(face.face_id(), Some(id));

        // Not a font: the face stays as it was.
        assert_eq!(
            layer.load_face(id, FontBytes::new(vec![0u8; 16]), 0),
            Err(crate::AddError::NotAFont)
        );
        layer.load_face(id, font(&[]), 0).unwrap();
        assert!(layer.generation() > before);
        let family = layer.family("Brand").unwrap();
        let face = family.match_font(Attributes::default(), true).unwrap();
        assert!(!face.is_pending());
        assert!(!face.wants('б'));
        // What the font maps, within the range.
        assert!(face.charset().contains('б'));
        assert!(!face.charset().contains('a'));
    }

    #[test]
    fn faces_split_by_range_are_tried_last_declared_first() {
        let mut layer = LayerBuilder::new(Role::Document);
        let data = font(&[]);
        let latin = FaceDescriptors {
            unicode_range: core::iter::once(0x0000..=0x00FF).collect(),
            ..FaceDescriptors::default()
        };
        let cyrillic = FaceDescriptors {
            unicode_range: core::iter::once(0x0400..=0x04FF).collect(),
            ..FaceDescriptors::default()
        };
        let first = layer
            .add_face("Brand", latin, Some((data.clone(), 0)))
            .unwrap();
        let second = layer.add_face("Brand", cyrillic, Some((data, 0))).unwrap();
        let family = layer.family("Brand").unwrap();
        let faces: Vec<_> = family
            .matching(Attributes::default(), true)
            .map(|font| font.face_id())
            .collect();
        assert_eq!(faces, [Some(second), Some(first)]);
        let drawing = |c: char| {
            family
                .matching(Attributes::default(), true)
                .find(|font| font.charset().contains(c))
                .and_then(Font::face_id)
        };
        assert_eq!(drawing('a'), Some(first));
        assert_eq!(drawing('б'), Some(second));
    }

    #[derive(Debug)]
    struct Nothing;

    impl LoadFamily for Nothing {
        fn load(&self, _: &str) -> Vec<Font> {
            Vec::new()
        }
    }

    #[test]
    fn a_face_shadows_its_name_until_its_last_rule_goes() {
        let system = Layer::from_names(
            Role::System,
            [(String::from("Brand"), Vec::new())],
            Arc::new(Nothing),
        );
        let mut document = LayerBuilder::new(Role::Document);
        let a = document
            .add_face("brand", FaceDescriptors::default(), None)
            .unwrap();
        let b = document
            .add_face("BRAND", FaceDescriptors::default(), None)
            .unwrap();
        let role = |document: &LayerBuilder| {
            Collection::new()
                .with_layer(Arc::new(system.clone()))
                .with_layer(document.snapshot())
                .family("Brand")
                .unwrap()
                .role()
        };
        assert_eq!(role(&document), Role::Document);
        assert!(document.remove_face(a));
        assert_eq!(role(&document), Role::Document);
        assert!(document.remove_face(b));
        assert!(!document.remove_face(b));
        assert_eq!(role(&document), Role::System);
    }

    #[test]
    fn a_variable_face_goes_as_far_as_declared() {
        let mut layer = LayerBuilder::new(Role::Document);
        layer
            .add_face(
                "Brand",
                FaceDescriptors {
                    weight: Some((FontWeight::new(300.0), FontWeight::new(500.0))),
                    style: Some(FaceStyle::Normal),
                    ..FaceDescriptors::default()
                },
                Some((
                    font(&[(*b"wght", 100.0, 400.0, 900.0), (*b"ital", 0.0, 0.0, 1.0)]),
                    0,
                )),
            )
            .unwrap();
        let family = layer.family("Brand").unwrap();
        let face = family.match_font(Attributes::default(), true).unwrap();
        let synthesis = face.synthesis(request(700.0, FontStyle::Italic));
        let settings: Vec<_> = synthesis
            .variation_settings()
            .iter()
            .map(|v| (v.tag.to_bytes(), v.value))
            .collect();
        // Bold stops at the declared 500 and is faked past it; italic is
        // declared away, so leaned rather than set on the axis.
        assert_eq!(settings, [(*b"wght", 500.0)]);
        assert!(synthesis.embolden());
        assert_eq!(synthesis.skew(), Some(14.0));
    }

    #[test]
    fn descriptors_that_are_not_numbers_declare_nothing() {
        let mut layer = LayerBuilder::new(Role::Document);
        layer
            .add_face(
                "Brand",
                FaceDescriptors {
                    weight: Some((FontWeight::new(f32::NAN), FontWeight::new(700.0))),
                    style: Some(FaceStyle::Oblique(f32::NAN, 10.0)),
                    ..FaceDescriptors::default()
                },
                Some((font(&[]), 0)),
            )
            .unwrap();
        let family = layer.family("Brand").unwrap();
        let face = family
            .match_font(request(900.0, FontStyle::Italic), true)
            .unwrap();
        assert_eq!(face.weight(), FontWeight::new(400.0));
        assert!(face.descriptors().unwrap().weight.is_none());
    }
}
