//! A caller-owned advance cache, passed to both build and break.
//!
//! This example rounds outline advances to device pixels to make the effect
//! visible. A renderer would ask its hinted outline or strike for the value
//! in `pixel_advance` and could keep that outline in the same strike entry.
//!
//! A provider is asked through a shared reference, as harfrust asks its font
//! callbacks, so the cache keeps what it measures behind a `RefCell`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

use fontwich::{Collection, FontKey, LayerBuilder, Role};
use read_fonts::model::metrics::ScaleF32;
use read_fonts::model::{Blob, Font};
use read_fonts::types::{F2Dot14, GlyphId};
use winkin::FontInstance;
use winkin::font::{FontMetricsProvider, GlyphAdvanceBatch};
use winkin::style::{ComputedStyle, FontFamilyName, FontGroup};
use winkin::{Area, BuildOptions, ComputedBlockStyle, Context, Layout, NoExclusions, NodeKey};

/// One font instance at one size, and the advances measured in it.
struct Strike {
    coords: Vec<i16>,
    size: u32,
    embolden: bool,
    skew: Option<u32>,
    /// The font held at the instance's coordinates, read once a strike.
    font: Option<Font>,
    advances: HashMap<u32, i32>,
}

impl Strike {
    fn matches(&self, font: FontInstance<'_>) -> bool {
        self.coords == font.coords
            && self.size == font.size.to_bits()
            && self.embolden == font.embolden
            && self.skew == font.skew.map(f32::to_bits)
    }

    fn new(font: FontInstance<'_>) -> Self {
        let (bytes, _) = font.bytes.clone().into_raw_parts();
        let held = Font::new(Blob::Shared(bytes), font.index).map(|held| {
            held.instance_builder()
                .normalized_coords(font.coords.iter().map(|&bits| F2Dot14::from_bits(bits)))
                .build()
        });
        Self {
            coords: font.coords.to_vec(),
            size: font.size.to_bits(),
            embolden: font.embolden,
            skew: font.skew.map(f32::to_bits),
            font: held,
            advances: HashMap::new(),
        }
    }

    /// `glyph`'s advance at the strike's size, snapped to device pixels
    /// `device_scale` to the CSS pixel, in 16.16 CSS pixels; `None` where
    /// the bytes are not a font.
    fn advance(&mut self, glyph: u32, device_scale: f32, misses: &mut usize) -> Option<i32> {
        if let Some(&advance) = self.advances.get(&glyph) {
            return Some(advance);
        }
        let font = self.font.as_ref()?;
        let size = f32::from_bits(self.size);
        let css_px = font
            .glyph_metrics()
            .scaled(ScaleF32::from_ppem(size, font.units_per_em()))
            .h_advance(GlyphId::new(glyph));
        let snapped = (css_px * device_scale).round() / device_scale;
        let advance = (snapped * 65536.0).round() as i32;
        self.advances.insert(glyph, advance);
        *misses += 1;
        Some(advance)
    }
}

/// The strikes a cache holds, and how many advances it has measured.
#[derive(Default)]
struct Strikes {
    strikes: HashMap<FontKey, Vec<Strike>>,
    misses: usize,
}

/// The strike in `strikes` that `font` is set in, made the first time.
fn font_strike<'a>(
    strikes: &'a mut HashMap<FontKey, Vec<Strike>>,
    font: FontInstance<'_>,
) -> &'a mut Strike {
    let list = strikes.entry(font.key()).or_default();
    let at = match list.iter().position(|strike| strike.matches(font)) {
        Some(at) => at,
        None => {
            list.push(Strike::new(font));
            list.len() - 1
        }
    };
    &mut list[at]
}

struct AdvanceCache {
    /// Device pixels per CSS pixel; fixed for this cache's lifetime.
    device_scale: f32,
    held: RefCell<Strikes>,
}

impl FontMetricsProvider for AdvanceCache {
    fn h_advance_batched<'a>(
        &self,
        font: FontInstance<'_>,
        batch: GlyphAdvanceBatch<'a>,
    ) -> Result<(), GlyphAdvanceBatch<'a>> {
        let mut held = self.held.borrow_mut();
        let Strikes { strikes, misses } = &mut *held;
        let strike = font_strike(strikes, font);
        if strike.font.is_none() {
            return Err(batch);
        }
        for (glyph, advance) in batch.advances() {
            *advance = strike
                .advance(glyph, self.device_scale, misses)
                .unwrap_or_default();
        }
        Ok(())
    }

    fn h_advance(&self, font: FontInstance<'_>, glyph: u32) -> Option<i32> {
        let mut held = self.held.borrow_mut();
        let Strikes { strikes, misses } = &mut *held;
        font_strike(strikes, font).advance(glyph, self.device_scale, misses)
    }
}

fn main() {
    let mut layer = LayerBuilder::new(Role::Application);
    layer
        .add_data(&include_bytes!("../../../support/testing/fonts/Ahem.ttf")[..])
        .expect("Ahem is a font");
    let mut context = Context::new(Collection::new().with_layer(layer.snapshot()));
    let families = [FontFamilyName::Named(Cow::Borrowed("Ahem"))];
    let style = ComputedStyle {
        font: FontGroup {
            families: &families,
            size: 16.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut layout = Layout::new();
    let mut builder = layout.builder(
        &mut context,
        NodeKey(0),
        &ComputedBlockStyle::new(&style),
        BuildOptions::default(),
    );
    builder.text(NodeKey(1), "Ahem Ahem Ahem");

    let cache = AdvanceCache {
        device_scale: 1.25,
        held: RefCell::default(),
    };
    builder.finish_with_metrics(&cache);
    let build_misses = cache.held.borrow().misses;
    layout.break_lines_with_metrics(&mut context, Area::new(80.0), &mut NoExclusions, &cache);
    layout.break_lines_with_metrics(&mut context, Area::new(48.0), &mut NoExclusions, &cache);
    let held = cache.held.borrow();
    assert_eq!(held.misses, build_misses, "relayout reused cached advances");
    println!(
        "{} lines, {} cached strikes, {} advance misses",
        layout.lines().len(),
        held.strikes.len(),
        held.misses
    );
}
