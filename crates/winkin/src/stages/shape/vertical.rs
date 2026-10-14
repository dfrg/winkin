//! A font's vertical metrics as Chrome hands them to its shaper, for upright
//! text in a vertical line.
//!
//! Blink does not let HarfBuzz read vertical metrics itself. It answers the
//! vertical advance and origin from its own reading of the tables
//! (`OpenTypeVerticalData`, called from `HarfBuzzFace`'s
//! `HarfBuzzGetGlyphVerticalAdvance` and `…Origin`). The two readings differ
//! wherever a font lacks a table. So a call shaping down the line gives
//! harfrust these answers instead (`harfrust::FontFuncs`), at the call's
//! size:
//! - **Advance**: `vmtx`'s, scaled by size over em, where the font has
//!   `vhea` and `vmtx`. Otherwise the font's line height, its ascent and
//!   descent each rounded to a whole pixel (Blink's `height_fallback_`).
//! - **Origin across**: half the glyph's `hmtx` advance.
//! - **Origin up**: `VORG`'s, using its default for unlisted glyphs and
//!   glyph 0. Without `VORG`, the glyph's bounds' top, rounded out to a
//!   whole pixel as Skia does, plus its top side bearing. Without `vmtx`
//!   either, the font's ascent.
//! - Every value is a float in pixels truncated to 16.16, as
//!   `SkiaScalarToHarfBuzzPosition` does. Like Blink, this reads `hmtx`,
//!   `vmtx` and `VORG` raw, without `HVAR` or `VVAR`.
//!
//! A host metric provider, where one is given, answers first, and these
//! answer what it leaves out (see `provider`). Everything else harfrust asks
//! is its own or the provider's. Horizontal text never reads these.
//!
//! A glyph's two answers depend only on its instance, the size and the
//! fallback. So the context caches a [`VerticalFont`] per instance, size and
//! fallback, and a [`VerticalGlyph`] per glyph asked ([`VerticalGlyphs`]),
//! as Blink keeps vertical data with the font at its size. A call reads the
//! tables ([`VerticalTables`]) only at the first glyph the cache lacks, so a
//! warm call reads none. Without `VORG`, a `CFF` or `gvar` font draws the
//! outline to find a glyph's bounds, which makes that reading costly.

use core::cell::{OnceCell, RefCell};

use fontwich::FontKey;
use harfrust::{FontFuncs, GlyphId, ShaperFont};
use hashbrown::HashTable;
use read_fonts::TableProvider;
use read_fonts::model::Font;
use read_fonts::model::metrics::{ScaleF32, ScaledGlyphMetrics};
use read_fonts::tables::hmtx::Hmtx;
use read_fonts::tables::vmtx::Vmtx;
use read_fonts::tables::vorg::Vorg;

use super::context::is_instance;
use crate::data::{define_id, hash_one, heap_bytes};
use crate::stages::fonts::UsedInstance;
use crate::unit::{self, LayoutUnit, TextUnit};

define_id! {
    /// Names a font instance at a size in the context's table of
    /// [`VerticalFont`]s.
    pub(super) struct VerticalFontId(u32);
}

/// The line ascent and height Blink uses where a font has no vertical
/// metrics.
///
/// Both are whole pixels, from the font's line metrics, and alphabetic
/// whatever baseline the line is centred on.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(super) struct VerticalFallback {
    ascent: LayoutUnit,
    height: LayoutUnit,
}

impl VerticalFallback {
    /// Returns the fallback of a font whose line's ascent and descent around
    /// its alphabetic baseline are `ascent` and `descent`.
    pub(super) fn new(ascent: LayoutUnit, descent: LayoutUnit) -> Self {
        Self {
            ascent,
            height: ascent + descent,
        }
    }
}

/// The cache key of a font set down the line: the font, its size and its
/// fallback, but not its coordinates, which are compared apart.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct VerticalKey {
    font: FontKey,
    /// harfrust's scale, the size in 1/65536 px.
    scale: i32,
    fallback: VerticalFallback,
}

impl VerticalKey {
    /// Returns the key of `font` set at `scale`, with `fallback`.
    pub(super) fn new(font: FontKey, scale: i32, fallback: VerticalFallback) -> Self {
        Self {
            font,
            scale,
            fallback,
        }
    }
}

/// A font instance set down the line at one size.
///
/// Its glyphs' cached metrics are keyed by it.
#[derive(Clone)]
pub(super) struct VerticalFont {
    key: VerticalKey,
    /// The font at the instance's coordinates, which are compared.
    font: Font,
    /// The font's units per em, never zero.
    upem: u16,
}

impl VerticalFont {
    /// Returns the font `key` names, held as `font`.
    ///
    /// Returns `None` where its em is zero, so harfrust's own metrics are
    /// used.
    pub(super) fn new(font: &Font, key: VerticalKey) -> Option<Self> {
        let upem = font.units_per_em();
        (upem != 0).then(|| Self {
            key,
            font: font.clone(),
            upem,
        })
    }

    /// Returns the key of the font it sets.
    pub(super) fn key(&self) -> FontKey {
        self.key.font
    }

    /// Returns whether it is the font `key` names, at the coordinates of
    /// `used`.
    pub(super) fn is(&self, key: &VerticalKey, used: &UsedInstance) -> bool {
        self.key == *key && is_instance(key.font, &self.font, used)
    }
}

/// One glyph's metrics down the line in a [`VerticalFont`], as harfrust is
/// answered, in 16.16 pixels.
///
/// The advance is negative down the line, since HarfBuzz's y grows up. The
/// origin is the vertical origin's offset from the horizontal one, across
/// and up.
#[derive(Copy, Clone, Debug)]
struct VerticalGlyph {
    font: VerticalFontId,
    glyph: u32,
    advance: i32,
    across: i32,
    up: i32,
}

/// The context's cache of glyph metrics down the line, by vertical font and
/// glyph.
///
/// A call reads a font's tables only for a glyph the cache lacks.
pub(super) struct VerticalGlyphs {
    glyphs: HashTable<VerticalGlyph>,
}

impl VerticalGlyphs {
    /// Returns an empty cache, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            glyphs: HashTable::new(),
        }
    }

    /// Drops every glyph, keeping the capacity.
    pub(super) fn clear(&mut self) {
        self.glyphs.clear();
    }

    /// Drops the glyphs of every font for which `keep` returns false.
    pub(super) fn retain(&mut self, keep: impl Fn(VerticalFontId) -> bool) {
        self.glyphs.retain(|held| keep(held.font));
    }

    /// Returns how many glyphs it holds.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Returns glyph `glyph` of `font`, or `None` where no call has read it.
    fn get(&self, font: VerticalFontId, glyph: u32) -> Option<VerticalGlyph> {
        self.glyphs
            .find(hash_one(&(font, glyph)), |held| {
                held.font == font && held.glyph == glyph
            })
            .copied()
    }

    /// Inserts `read`, which it must not hold yet.
    fn insert(&mut self, read: VerticalGlyph) {
        let hash = hash_one(&(read.font, read.glyph));
        self.glyphs
            .insert_unique(hash, read, |held| hash_one(&(held.font, held.glyph)));
    }
}

heap_bytes! {
    VerticalGlyphs { glyphs }
}

/// A font's vertical metric tables at one size, read as Blink reads them.
///
/// A glyph the cache lacks is measured with these.
struct VerticalTables<'a> {
    hmtx: Option<Hmtx<'a>>,
    /// The font's `vmtx`, read only where it has `vhea`; `None` also where
    /// it is unreadable.
    vmtx: Option<Vmtx<'a>>,
    /// The font's `VORG`, read only where it has `vhea`, as Blink reads it.
    vorg: Option<Vorg<'a>>,
    /// The glyphs' bounds at the size and the instance's coordinates.
    bounds: ScaledGlyphMetrics<'a, 'static, ScaleF32>,
    /// The size over the em, in pixels per unit, as Blink's `size_per_unit_`.
    per_unit: f32,
    fallback: VerticalFallback,
}

impl<'a> VerticalTables<'a> {
    /// Reads the tables of `font`, with `upem` units to the em, at the
    /// instance's coordinates.
    ///
    /// `scale` is the size in 1/65536 px, and `fallback` applies where the
    /// font lacks vertical metrics.
    fn new(font: &'a Font, scale: i32, upem: u16, fallback: VerticalFallback) -> Self {
        let px = TextUnit::from_raw(scale).to_px();
        let tables = font.tables();
        // Blink finds `VORG` only where `vhea` is there to read, and `vmtx`
        // only where `vhea` says how many long metrics it holds.
        let has_vhea = tables
            .vhea()
            .is_ok_and(|vhea| vhea.number_of_long_ver_metrics() > 0);
        let vmtx = has_vhea.then(|| tables.vmtx().ok()).flatten();
        let vorg = has_vhea.then(|| tables.vorg().ok()).flatten();
        Self {
            hmtx: tables.hmtx().ok(),
            vmtx,
            vorg,
            bounds: font.glyph_metrics().scaled(ScaleF32::from_ppem(px, upem)),
            per_unit: px / unit::whole_to_f32(upem),
            fallback,
        }
    }

    /// Converts `units` of the font to pixels, as Blink multiplies a value
    /// it reads by the size over the em.
    fn px(&self, units: impl Into<i64>) -> f32 {
        unit::whole_to_f32(units) * self.per_unit
    }

    /// Returns the glyph's vertical advance, in pixels.
    fn advance_px(&self, glyph: GlyphId) -> f32 {
        match self.vmtx.as_ref().and_then(|vmtx| vmtx.advance(glyph)) {
            Some(advance) => self.px(advance),
            None => self.fallback.height.to_px(),
        }
    }

    /// Returns the glyph's vertical origin from its horizontal one, in
    /// pixels: across, and up.
    fn origin_px(&self, glyph: GlyphId) -> (f32, f32) {
        let width = self
            .hmtx
            .as_ref()
            .and_then(|hmtx| hmtx.advance(glyph))
            .map_or(0.0, |advance| self.px(advance));
        let up = if let Some(vorg) = &self.vorg {
            let units = if glyph.to_u32() == 0 {
                vorg.default_vert_origin_y()
            } else {
                vorg.vertical_origin_y(glyph)
            };
            self.px(units)
        } else if let Some(vmtx) = &self.vmtx {
            // A glyph past the bearings takes the last, as Blink indexes
            // them. Its bounds' top rounds out to a whole pixel, as Skia
            // bounds a glyph, and is zero where it has no bounds.
            let bearing = vmtx
                .side_bearing(glyph)
                .or_else(|| vmtx.top_side_bearings().last().map(|tsb| tsb.get()))
                .or_else(|| vmtx.v_metrics().last().map(|metric| metric.side_bearing()))
                .unwrap_or(0);
            let top = self.bounds.extents(glyph).map_or(0.0, |bounds| {
                LayoutUnit::from_px(bounds.y_bearing).ceil_px().to_px()
            });
            self.px(bearing) + top
        } else {
            self.fallback.ascent.to_px()
        };
        (width / 2.0, up)
    }

    /// Reads glyph `glyph` of `font` as harfrust is answered, each value a
    /// float in pixels truncated to 16.16.
    fn read(&self, font: VerticalFontId, glyph: GlyphId) -> VerticalGlyph {
        let (across, up) = self.origin_px(glyph);
        VerticalGlyph {
            font,
            glyph: glyph.to_u32(),
            // Down the line is negative, as HarfBuzz's y grows up.
            advance: TextUnit::from_px_truncated(-self.advance_px(glyph)).raw(),
            across: TextUnit::from_px_truncated(across).raw(),
            up: TextUnit::from_px_truncated(up).raw(),
        }
    }
}

/// The functions a call shaping down the line hands harfrust.
///
/// Each glyph's vertical advance and origin come from the context's cache,
/// or from the font's tables where the cache lacks them. Everything else is
/// harfrust's own, scaled by its own dispatch.
pub(super) struct VerticalFuncs<'a> {
    font: VerticalFontId,
    /// The context's cache, behind a cell, since harfrust asks through a
    /// shared reference.
    glyphs: RefCell<&'a mut VerticalGlyphs>,
    /// The font held at the instance's coordinates, whose tables are read.
    held: &'a Font,
    scale: i32,
    upem: u16,
    fallback: VerticalFallback,
    /// The font's tables, read at the first glyph the cache does not hold.
    tables: OnceCell<VerticalTables<'a>>,
}

impl<'a> VerticalFuncs<'a> {
    /// Returns the functions for `font`, which is `vertical` in the context
    /// and held as `held`, caching its glyphs' metrics in `glyphs`.
    pub(super) fn new(
        font: VerticalFontId,
        vertical: &VerticalFont,
        held: &'a Font,
        glyphs: &'a mut VerticalGlyphs,
    ) -> Self {
        Self {
            font,
            glyphs: RefCell::new(glyphs),
            held,
            scale: vertical.key.scale,
            upem: vertical.upem,
            fallback: vertical.key.fallback,
            tables: OnceCell::new(),
        }
    }

    /// Returns the glyph's metrics down the line from the cache, or reads
    /// and caches them.
    fn glyph(&self, glyph: GlyphId) -> VerticalGlyph {
        let Ok(mut glyphs) = self.glyphs.try_borrow_mut() else {
            return self.read(glyph);
        };
        if let Some(held) = glyphs.get(self.font, glyph.to_u32()) {
            return held;
        }
        let read = self.read(glyph);
        glyphs.insert(read);
        read
    }

    /// Reads the glyph's metrics down the line from the font's tables.
    fn read(&self, glyph: GlyphId) -> VerticalGlyph {
        self.tables
            .get_or_init(|| VerticalTables::new(self.held, self.scale, self.upem, self.fallback))
            .read(self.font, glyph)
    }
}

impl FontFuncs for VerticalFuncs<'_> {
    fn glyph_v_advance(&self, _: &ShaperFont, glyph: GlyphId) -> i32 {
        self.glyph(glyph).advance
    }

    fn glyph_v_origin(&self, _: &ShaperFont, glyph: GlyphId) -> (i32, i32) {
        let read = self.glyph(glyph);
        (read.across, read.up)
    }
}
