//! Optional host glyph metrics for selected fonts.
//!
//! Implement [`FontMetricsProvider`] to use metrics from the rendering
//! rasterizer. Individual queries can fall back to font-table metrics.
//! Horizontal advances support batch writes directly into the shaping buffer.
//!
//! Fallback metrics use harfrust scaling and Blink-compatible vertical metrics,
//! as when no provider is supplied.

use harfrust::Advances;

use crate::FontInstance;

/// Glyph bounds relative to the glyph origin, in 16.16 CSS pixels, with y upward.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GlyphBounds {
    /// The left edge.
    pub x_min: i32,
    /// The bottom edge.
    pub y_min: i32,
    /// The right edge.
    pub x_max: i32,
    /// The top edge.
    pub y_max: i32,
}

/// Glyph identifiers and writable horizontal advances from the shaper.
///
/// Advances use signed 16.16 CSS pixels. Writes update the shaping buffer
/// directly. This view and its iterator do not allocate.
/// [`into_raw`](Self::into_raw) supports rasterizer APIs with strided buffers.
pub struct GlyphAdvanceBatch<'a>(Advances<'a>);

/// Strided pointers to glyph identifiers and writable advances.
///
/// Each pointer addresses `len` entries separated by the corresponding
/// byte stride. Both pointers are null when `len` is zero. Pointers are
/// valid only during the provider call and must not be retained.
#[derive(Copy, Clone, Debug)]
pub struct RawGlyphAdvances {
    /// The number of glyphs.
    pub len: usize,
    /// The first glyph ID, read-only.
    pub glyphs: *const u32,
    /// The first advance, written in signed 16.16 CSS pixels.
    pub advances: *mut i32,
    /// The bytes from one glyph ID to the next.
    pub glyph_stride: isize,
    /// The bytes from one advance to the next.
    pub advance_stride: isize,
}

impl<'a> GlyphAdvanceBatch<'a> {
    pub(crate) fn new(batch: Advances<'a>) -> Self {
        Self(batch)
    }

    /// Returns the number of glyphs.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns `true` if the batch contains no glyphs.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns glyph identifiers and writable advances in order.
    pub fn advances(self) -> impl Iterator<Item = (u32, &'a mut i32)> {
        self.0
            .into_iter()
            .map(|(glyph, advance)| (glyph.to_u32(), advance))
    }

    /// Returns strided pointers for a rasterizer batch API.
    pub fn into_raw(self) -> RawGlyphAdvances {
        let raw = self.0.into_raw();
        RawGlyphAdvances {
            len: raw.len,
            glyphs: raw.gids,
            advances: raw.advances,
            glyph_stride: raw.gid_stride,
            advance_stride: raw.advance_stride,
        }
    }

    /// The shaper's batch, for its own measurement where the host gives
    /// none.
    pub(crate) fn into_inner(self) -> Advances<'a> {
        self.0
    }
}

/// Host-provided glyph metrics for a selected font.
///
/// The host owns any outline or glyph-image cache. Returning `None` retains
/// the font-table result. For `glyph_extents`, outer `None` declines the query;
/// `Some(None)` specifies no ink. Advance batches are all-or-nothing:
/// `Ok(())` requires every entry to be filled; `Err(batch)` requests fallback.
///
/// Results must remain stable for the lifetime of a layout, including edge
/// reshaping. Batch and single horizontal advances must agree. Font selection,
/// face descriptors, line metrics and baseline calculations remain in fontwich
/// and winkin.
///
/// Methods take `&self`. Cache implementations may use interior mutability,
/// such as `RefCell` or a lock. Calls may be reentrant. Release cache borrows
/// before calling back into winkin or the shaper to avoid borrow panics.
pub trait FontMetricsProvider {
    /// Fills every horizontal advance in 16.16 CSS pixels, or returns the
    /// untouched batch for the default table-based measurement.
    fn h_advance_batched<'a>(
        &self,
        _font: FontInstance<'_>,
        batch: GlyphAdvanceBatch<'a>,
    ) -> Result<(), GlyphAdvanceBatch<'a>> {
        Err(batch)
    }

    /// Returns a horizontal advance in signed 16.16 CSS pixels.
    ///
    /// May be called independently of batch queries.
    fn h_advance(&self, _font: FontInstance<'_>, _glyph: u32) -> Option<i32> {
        None
    }

    /// Returns a vertical advance in signed 16.16 CSS pixels, positive down the line.
    fn v_advance(&self, _font: FontInstance<'_>, _glyph: u32) -> Option<i32> {
        None
    }

    /// Returns the vertical origin relative to the horizontal origin.
    ///
    /// Uses signed 16.16 CSS pixels, with x rightward and y upward.
    /// Returning `None` uses `VORG` or derives the origin from `vmtx`
    /// and font-table ink bounds, matching Blink. Fallback does not use
    /// [`glyph_extents`](Self::glyph_extents). Rasterizers that shift ink
    /// should also override this method.
    fn v_origin(&self, _font: FontInstance<'_>, _glyph: u32) -> Option<(i32, i32)> {
        None
    }

    /// Returns glyph ink bounds, or `None` to use font-table bounds.
    ///
    /// `Some(None)` specifies a glyph with no ink.
    fn glyph_extents(&self, _font: FontInstance<'_>, _glyph: u32) -> Option<Option<GlyphBounds>> {
        None
    }
}

/// The default unhinted font-table metrics and vertical fallbacks.
///
/// Using this provider is equivalent to supplying no provider.
#[derive(Copy, Clone, Debug, Default)]
pub struct DefaultFontMetrics;

impl FontMetricsProvider for DefaultFontMetrics {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use core::cell::Cell;

    use crate::layout::{Glyph, Item};
    use crate::stages::lines::{Area, NoExclusions};
    use crate::style::{TextOrientation, WordBreak, WritingMode};
    use crate::tests::{Fixture, LATIN, StageCheck, TestFallback, ahem, latin, sized};
    use crate::{BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey};

    struct FixedAdvances {
        batches: Cell<usize>,
    }

    impl FontMetricsProvider for FixedAdvances {
        fn h_advance_batched<'a>(
            &self,
            font: FontInstance<'_>,
            batch: GlyphAdvanceBatch<'a>,
        ) -> Result<(), GlyphAdvanceBatch<'a>> {
            assert!(font.size > 0.0);
            assert!(!font.bytes.data().is_empty());
            self.batches.set(self.batches.get() + 1);
            for (_, advance) in batch.advances() {
                *advance = 20 << 16;
            }
            Ok(())
        }

        fn h_advance(&self, _font: FontInstance<'_>, _glyph: u32) -> Option<i32> {
            Some(20 << 16)
        }
    }

    fn glyphs(fixture: &mut Fixture, provider: Option<&dyn FontMetricsProvider>) -> Vec<Glyph> {
        let mut layout = Layout::new();
        let style = ahem(16.0);
        if let Some(provider) = provider {
            let mut builder = layout.builder(
                &mut fixture.cx,
                NodeKey(0),
                &ComputedBlockStyle::new(&style),
                BuildOptions::default(),
            );
            builder.text(NodeKey(1), "AAA");
            builder.finish_with_metrics(provider);
            layout.break_lines_with_metrics(
                &mut fixture.cx,
                Area::new(100.0),
                &mut NoExclusions,
                provider,
            );
        } else {
            fixture.text(&mut layout, &style, "AAA");
            fixture.lay_out(&mut layout, 100.0);
        }
        layout
            .lines()
            .flat_map(|line| line.items())
            .filter_map(|item| match item {
                Item::Text(run) => Some(run),
                _ => None,
            })
            .flat_map(|run| run.glyphs())
            .collect()
    }

    fn vertical_glyphs(
        fixture: &mut Fixture,
        provider: Option<&dyn FontMetricsProvider>,
    ) -> Vec<Glyph> {
        let mut layout = Layout::new();
        let mut style = ahem(16.0);
        style.orientation.text_orientation = TextOrientation::Upright;
        let block = ComputedBlockStyle {
            writing_mode: WritingMode::VerticalRl,
            ..ComputedBlockStyle::new(&style)
        };
        if let Some(provider) = provider {
            let mut builder =
                layout.builder(&mut fixture.cx, NodeKey(0), &block, BuildOptions::default());
            builder.text(NodeKey(1), "AAA");
            builder.finish_with_metrics(provider);
            layout.break_lines_with_metrics(
                &mut fixture.cx,
                Area::new(100.0),
                &mut NoExclusions,
                provider,
            );
        } else {
            fixture.block_text(&mut layout, &block, "AAA");
            fixture.lay_out(&mut layout, 100.0);
        }
        layout
            .lines()
            .flat_map(|line| line.items())
            .filter_map(|item| match item {
                Item::Text(run) => Some(run),
                _ => None,
            })
            .flat_map(|run| run.glyphs())
            .collect()
    }

    #[test]
    fn default_provider_preserves_builtin_advances() {
        let mut fixture = Fixture::new(&[], TestFallback::default(), StageCheck::Placed(|_| {}));
        let builtin = glyphs(&mut fixture, None);
        let vertical = vertical_glyphs(&mut fixture, None);
        assert_eq!(glyphs(&mut fixture, Some(&DefaultFontMetrics)), builtin);
        assert_eq!(
            vertical_glyphs(&mut fixture, Some(&DefaultFontMetrics)),
            vertical
        );
    }

    #[test]
    fn host_batch_controls_shaped_advances() {
        let mut fixture = Fixture::new(&[], TestFallback::default(), StageCheck::Placed(|_| {}));
        let builtin = glyphs(&mut fixture, None);
        assert_eq!(builtin.len(), 3);
        assert!(builtin.iter().all(|glyph| glyph.advance == 16.0));
        let provider = FixedAdvances {
            batches: Cell::new(0),
        };
        let hinted = glyphs(&mut fixture, Some(&provider));
        assert!(provider.batches.get() > 0);
        assert!(hinted.iter().all(|glyph| glyph.advance == 20.0));
    }

    #[test]
    fn line_edge_reshaping_uses_the_supplied_provider() {
        let mut fixture = Fixture::new(
            &[latin()],
            TestFallback::default(),
            StageCheck::Placed(|_| {}),
        );
        let provider = FixedAdvances {
            batches: Cell::new(0),
        };
        let mut style = sized(&LATIN, 20.0);
        style.text.word_break = WordBreak::BreakAll;
        let mut layout = Layout::new();
        let mut builder = layout.builder(
            &mut fixture.cx,
            NodeKey(0),
            &ComputedBlockStyle::new(&style),
            BuildOptions::default(),
        );
        builder.text(NodeKey(1), "AVAVAVAV");
        builder.finish_with_metrics(&provider);
        let prepared_calls = provider.batches.get();
        assert!(prepared_calls > 0);
        let mut fresh_context = Context::new(fontwich::Collection::new());
        layout.break_lines_with_metrics(
            &mut fresh_context,
            Area::new(26.0),
            &mut NoExclusions,
            &provider,
        );
        assert!(provider.batches.get() > prepared_calls);
    }
}
