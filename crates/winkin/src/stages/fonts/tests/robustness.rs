//! Robustness tests: an empty collection, broken tables, bytes that are
//! no font, and a full table of used fonts.

use super::*;

/// With no fonts at all, every cluster still has a used font. It has no
/// instance, draws nothing and measures by the documented defaults.
#[test]
fn an_empty_collection_gives_every_cluster_a_font_that_draws_nothing() {
    let mut fixture = fixture_over(Collection::new(), &[]);
    let mut layout = Layout::new();
    let style = ComputedStyle::initial();
    let built = fixture.span(&mut layout, &style, "Hello, 漢字 \u{1F600}");
    assert!(built.is_complete());
    let runs = ordered_runs(&layout);
    assert_eq!(runs.len(), 1);
    let font = used_font(&layout, runs[0].font);
    assert!(font.instance.as_ref().is_none());
    // 0.8 and 0.2 of 16 px, rounded, and no gap.
    assert_eq!(font.metrics.ascent.to_px(), 13.0);
    assert_eq!(font.metrics.descent.to_px(), 3.0);
    assert_eq!(font.metrics.line_gap.to_px(), 0.0);
}

/// A font whose metric tables cannot be read still draws what its `cmap`
/// maps, and measures by the defaults.
#[test]
fn a_font_with_broken_tables_measures_by_the_defaults() {
    let mut broken = TestFont::new("Test Broken", &[(0x20, 0x7E)]);
    broken.broken = true;
    let mut fixture = fixture_with(&[broken]);
    let mut layout = Layout::new();
    let listed = [FontFamilyName::named("Test Broken")];
    fixture.span(&mut layout, &families_style(&listed), "abc");
    assert_eq!(fixture.families(&layout), ["Test Broken"; 3]);
    let metrics = used_font(&layout, fixture.primary(&layout)).metrics;
    assert_eq!(metrics.ascent.to_px(), 13.0);
    assert_eq!(metrics.descent.to_px(), 3.0);
}

/// Bytes that are no font are refused as the layer is built. A family that
/// did not make it in is skipped like any missing family.
#[test]
fn bytes_that_are_no_font_are_skipped() {
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(vec![0u8; 64]).is_err());
    assert!(layer.add_data(AHEM).is_ok());
    let mut fixture = fixture_over(
        Collection::new().with_layer(layer.snapshot()),
        &[String::from("Ahem")],
    );
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Garbage"),
        FontFamilyName::named("Ahem"),
    ];
    fixture.span(&mut layout, &families_style(&listed), "abc");
    assert_eq!(fixture.families(&layout), ["Ahem"; 3]);
}

/// Where a build asks for more used fonts than a layout's table holds, the
/// rest take a font already there. The layout is still whole, and the build
/// reports how many were replaced.
#[test]
fn a_full_table_of_used_fonts_falls_back_and_says_so() {
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let root = families_style(&ahem);
    // Sizes a 64th of a pixel apart, each with synthesized capitals and so
    // two used fonts of its own. The used fonts fill before the font
    // requests, which have as many ids (`BuildReport::replaced_styles`).
    let mut size = 1.0;
    let styles: Vec<ComputedStyle<'_>> = (0..UsedFontId::MAX / 2 + 10)
        .map(|_| {
            size += 1.0 / 64.0;
            let mut style = sized(&ahem, size);
            style.font.variant_caps = FontVariantCaps::SmallCaps;
            style
        })
        .collect();
    let mut b = layout.builder(
        &mut fixture.cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&root),
        BuildOptions::default(),
    );
    for (at, style) in (0u64..).zip(&styles) {
        b.open_box(NodeKey(2 * at + 1), style, None);
        b.text(NodeKey(2 * at + 2), "a");
        b.close_box();
    }
    let built = b.finish();
    check(&layout, &fixture.cx);
    assert_eq!(layout.fonts().used.len(), UsedFontId::MAX);
    assert!(built.replaced_fonts > 0);
    assert_eq!(built.replaced_styles, 0);
    assert_eq!(built.dropped_bytes + built.dropped_nodes, 0);
}
