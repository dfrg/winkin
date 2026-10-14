//! Bridges a host metric provider to harfrust's font callbacks.

use harfrust::{Advances, FontFuncs, GlyphExtents, GlyphId, ShaperFont};

use super::vertical::VerticalFuncs;
use crate::FontInstance;
use crate::font::{FontMetricsProvider, GlyphAdvanceBatch, GlyphBounds};

/// harfrust font callbacks that ask a host's metric provider first.
///
/// Where the provider has no answer, a call down the line takes vertical
/// advances and origins from the font's vertical metrics (see `vertical`).
/// Everything else falls back to harfrust's own reading of the font.
pub(super) struct ProvidedFuncs<'a> {
    provider: &'a dyn FontMetricsProvider,
    font: FontInstance<'a>,
    vertical: Option<&'a VerticalFuncs<'a>>,
}

impl<'a> ProvidedFuncs<'a> {
    pub(super) fn new(
        provider: &'a dyn FontMetricsProvider,
        font: FontInstance<'a>,
        vertical: Option<&'a VerticalFuncs<'a>>,
    ) -> Self {
        Self {
            provider,
            font,
            vertical,
        }
    }
}

impl FontFuncs for ProvidedFuncs<'_> {
    fn glyph_h_advance(&self, font: &ShaperFont, glyph: GlyphId) -> i32 {
        self.provider
            .h_advance(self.font, glyph.to_u32())
            .unwrap_or_else(|| font.default_glyph_h_advance(glyph))
    }

    fn glyph_h_advances(&self, font: &ShaperFont, advances: Advances<'_>) {
        let result = self
            .provider
            .h_advance_batched(self.font, GlyphAdvanceBatch::new(advances));
        if let Err(batch) = result {
            font.default_glyph_h_advances(batch.into_inner());
        }
    }

    fn glyph_v_advance(&self, font: &ShaperFont, glyph: GlyphId) -> i32 {
        // The provider's advance is positive down the line; harfrust's y
        // grows up.
        self.provider
            .v_advance(self.font, glyph.to_u32())
            .map(i32::saturating_neg)
            .unwrap_or_else(|| match self.vertical {
                Some(vertical) => vertical.glyph_v_advance(font, glyph),
                None => font.default_glyph_v_advance(glyph),
            })
    }

    fn glyph_v_origin(&self, font: &ShaperFont, glyph: GlyphId) -> (i32, i32) {
        self.provider
            .v_origin(self.font, glyph.to_u32())
            .unwrap_or_else(|| match self.vertical {
                Some(vertical) => vertical.glyph_v_origin(font, glyph),
                None => font.default_glyph_v_origin(glyph),
            })
    }

    fn glyph_extents(&self, font: &ShaperFont, glyph: GlyphId) -> Option<GlyphExtents> {
        // `Some(None)` is a glyph with no ink.
        match self.provider.glyph_extents(self.font, glyph.to_u32()) {
            Some(bounds) => bounds.map(GlyphBounds::to_harfrust),
            None => font.default_glyph_extents(glyph),
        }
    }
}

impl GlyphBounds {
    /// Converts the bounds to harfrust's extents: the top-left corner, and a
    /// negative height for ink that extends down.
    fn to_harfrust(self) -> GlyphExtents {
        GlyphExtents {
            x_bearing: self.x_min,
            y_bearing: self.y_max,
            width: self.x_max.saturating_sub(self.x_min),
            height: self.y_min.saturating_sub(self.y_max),
        }
    }
}
