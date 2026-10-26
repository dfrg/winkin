//! A font request resolved to its used size and primary font.

use super::UsedFontId;
use super::instance::FaceOverrides;
use crate::data::Id;
use crate::style::sanitized_font_size;

/// What a font request resolves to: its used size and its primary font.
///
/// Font selection resolves each request once, before choosing any font.
/// Later stages read it by [`FontRequestId`]. 12 bytes.
///
/// [`FontRequestId`]: crate::stages::content::FontRequestId
#[derive(Copy, Clone, Debug)]
pub(crate) struct FontResolution {
    /// The size in pixels its fonts are used at before any face's
    /// `size-adjust`: the computed size, or that size as `font-size-adjust`
    /// or an initial letter's lines adjust it against the primary font.
    pub(super) base: f32,
    /// Whether `base` was adjusted, so no face's `size-adjust` applies
    /// (Blink's `FontDescription::SizeAdjustedFontDescription`).
    adjusted: bool,
    /// The computed size in pixels, before any adjustment. Synthesized
    /// small capitals are drawn at 0.7 of it.
    pub(crate) computed: f32,
    /// Whether its text is set upright or mixed in a vertical line. It then
    /// shapes with the vertical forms of its spacing features.
    pub(super) upright: bool,
    /// Its primary font.
    pub(crate) primary: UsedFontId,
}

impl FontResolution {
    /// Returns a resolution at the computed size `computed`. Its primary
    /// font is a placeholder until resolved.
    pub(super) fn new(computed: f32, upright: bool) -> Self {
        Self {
            base: computed,
            adjusted: false,
            computed,
            upright,
            primary: UsedFontId::new(0),
        }
    }

    /// Sets the used size to `size`, from `font-size-adjust` or an initial
    /// letter. No face's `size-adjust` applies after this.
    pub(super) fn adjust_to(&mut self, size: f32) {
        self.base = size;
        self.adjusted = true;
    }

    /// Returns the size in pixels this request uses a font at, given its
    /// `@font-face` descriptors `overrides`.
    pub(super) fn font_px(&self, overrides: FaceOverrides) -> f32 {
        match overrides.size_adjust() {
            Some(ratio) if !self.adjusted => sanitized_font_size(self.base * ratio),
            _ => self.base,
        }
    }
}
