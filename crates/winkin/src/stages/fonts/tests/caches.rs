//! Tests of web fonts and the context: pending web faces, what a used font
//! keeps, caches across layouts and collections, rebuilds and dropped
//! builders.

use alloc::format;
use core::iter;

use super::*;

// Web fonts --------------------------------------------------------------------

/// A face still downloading that a cluster wants is listed for the host to
/// fetch, once, and the text is set meanwhile in what covers it; the
/// primary font passes over it, as Chrome measures in the fallback while a
/// face loads.
#[test]
fn a_pending_face_is_wanted_and_the_text_falls_back_meanwhile() {
    let mut document = LayerBuilder::new(Role::Document);
    let face = document
        .add_face(
            "Brand",
            FaceDescriptors {
                unicode_range: iter::once(0x0..=0xFF).collect(),
                ..FaceDescriptors::default()
            },
            None,
        )
        .expect("a face declared");
    let collection = collection(&[], ahem_fallback()).with_layer(document.snapshot());
    let mut fixture = fixture_over(collection, &[String::from("Ahem")]);
    let mut layout = Layout::new();
    let brand = [FontFamilyName::named("Brand")];
    fixture.span(&mut layout, &families_style(&brand), "abc \u{4E00}");
    assert_eq!(layout.wanted_faces(), [face]);
    assert_eq!(fixture.families(&layout), ["Ahem"; 5]);
    assert_eq!(fixture.family(&layout, fixture.primary(&layout)), "Ahem");
}

// The context ----------------------------------------------------------------

/// A used font keeps what a reader draws it with: its instance's record,
/// shared with the context's instance and not copied. That holds the font
/// whole, its bytes, its index, its coordinates, its synthesis
/// and its features. Each used font takes a count on the record, and gives
/// it back when the layout is rebuilt.
#[test]
fn a_used_font_keeps_what_a_reader_draws_with() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let ahem = [FontFamilyName::named("Ahem")];
    let style = families_style(&ahem);
    let big = sized(&ahem, 20.0);
    fixture.spans(&mut layout, &style, &[(&style, "a漢"), (&big, "b")]);
    let used: Vec<&UsedFont> = layout.fonts().used.iter().map(|(_, font)| font).collect();
    assert_eq!(used.len(), 3, "Ahem at two sizes, and the Han fallback");
    let instances = fixture.cx.font_context().instances();
    for font in &used {
        let drawn = font.instance.as_ref().expect("set in a font");
        // The layout names no instance: the context's is found by its font.
        let instance = instances
            .first_instance(drawn.key())
            .expect("the context's");
        assert!(Arc::ptr_eq(drawn, instance.used()));
        let coords: Vec<i16> = drawn
            .font
            .normalized_coords()
            .iter()
            .map(|coord| coord.to_bits())
            .collect();
        assert_eq!(*drawn.coords, *coords);
        // Neither font varies, so every instance is its default.
        assert!(drawn.coords.is_empty());
    }
    // Ahem's two used fonts share its instance, so each counts on its record.
    let record = used[0].instance.clone().expect("Ahem");
    let held = Arc::strong_count(&record);
    fixture.span(&mut layout, &style, "a");
    assert_eq!(Arc::strong_count(&record), held - 1);
}

/// A character no installed font maps has its miss walked once a context:
/// the first list to meet it walks every family and notes it, and no list
/// after it, of another `font-family`, walks it again. It is drawn in the
/// primary font, whose `.notdef` is Chrome's missing-glyph box.
#[test]
fn a_character_no_font_maps_is_walked_once_a_context() {
    let mut fixture = fixture_with(&[plain()]);
    let mut layout = Layout::new();
    let listed = [FontFamilyName::named("Test Plain Hyphen")];
    let unmapped = '\u{10FFFD}';
    let walked = |fixture: &Fixture| fixture.cx.font_context().candidate_lists().walked(unmapped);
    fixture.span(&mut layout, &ComputedStyle::initial(), "a\u{10FFFD}");
    assert_eq!(walked(&fixture), (1, true));
    fixture.span(&mut layout, &families_style(&listed), "a\u{10FFFD}");
    assert_eq!(walked(&fixture), (1, true));
    let families = fixture.families(&layout);
    assert!(
        families.iter().all(|&family| family == "Test Plain Hyphen"),
        "{families:?}"
    );
}

/// A new collection of the same snapshots keeps the family lists the
/// context holds; another stack drops them. The font instances of the fonts
/// both collections have survive, so a rebuild in one reads nothing again,
/// and those of fonts the new one lacks go with their faces. A layout built
/// before names none of it.
#[test]
fn a_changed_collection_drops_the_lists_and_the_instances_it_lacks() {
    let mut fixture = fixture_with(&[han()]);
    let mut layout = Layout::new();
    let style = ComputedStyle::initial();
    fixture.span(&mut layout, &style, "a漢");
    let lists = fixture.cx.font_context().candidate_lists().len();
    let instances = fixture.cx.font_context().instances().len();
    assert!(lists > 0 && instances == 2);

    let same = fixture.cx.collection().clone();
    fixture.cx.set_collection(same);
    assert_eq!(fixture.cx.font_context().candidate_lists().len(), lists);

    // The same layers under one more: the lists go, the instances stay.
    let more = fixture
        .cx
        .collection()
        .clone()
        .with_layer(LayerBuilder::new(Role::Document).snapshot());
    fixture.cx.set_collection(more);
    assert_eq!(fixture.cx.font_context().candidate_lists().len(), 0);
    assert_eq!(fixture.cx.font_context().instances().len(), instances);

    fixture.cx.set_collection(Collection::new());
    assert_eq!(fixture.cx.font_context().instances().len(), 0);
    assert_eq!(fixture.cx.font_context().instances().face_count(), 0);
    for run in ordered_runs(&layout) {
        let used = used_font(&layout, run.font);
        assert!(used.instance.as_ref().is_some(), "set in a font of its own");
    }
}

/// Every instance of a font is cut from the font's one face, which what it
/// offers and what its `sups` covers are read from too: a font asked for
/// small capitals and superscripts at two weights is one face, whose tables
/// each of its instances shares, and whose bytes each instance's record
/// takes a count on.
#[test]
fn every_instance_of_a_font_is_cut_from_its_one_face() {
    let mut fixture = fixture_with(&[capitals_variable()]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Capitals Variable")];
    let mut style = families_style(&family);
    style.font.variant_caps = FontVariantCaps::SmallCaps;
    style.font.variant_position = FontVariantPosition::Super;
    let mut bold = style;
    bold.font.weight = FontWeight::new(700.0);
    fixture.spans(&mut layout, &style, &[(&style, "a2"), (&bold, "b3")]);
    let instances = fixture.cx.font_context().instances();
    let faces: Vec<_> = instances.faces().collect();
    let all: Vec<_> = instances.all().collect();
    let mut keys: Vec<_> = all.iter().map(|at| at.used().key()).collect();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(faces.len(), keys.len(), "one face a font");
    let varied = all.iter().filter(|at| !at.used().coords.is_empty()).count();
    assert!(varied > 0 && varied < all.len(), "both weights");
    for instance in all {
        let used = instance.used();
        let face = faces
            .iter()
            .find(|face| face.key == used.key())
            .expect("its font's face");
        assert!(ptr::eq(face.font.tables(), used.font.tables()));
        assert!(Arc::ptr_eq(face.bytes.arc(), used.bytes.arc()));
    }
}

/// A cluster that starts with a mark takes the side of a change of case of
/// the letter before it, as Blink's `SmallCapsIterator` passes over marks,
/// in a cold build as in a warm one. The acute after the soft hyphen, which
/// a grapheme boundary parts from the `a`, is a cluster of its own that
/// Ahem lacks, so a cold walk stops there for Test Only Marks to be matched
/// and starts again at it; neither font has `smcp`, so small capitals are
/// synthesized in both, and the acute is fed uppercased, as the `a` is.
#[test]
fn a_mark_takes_the_side_of_case_before_it_cold_as_warm() {
    let mut fixture = fixture_with(&[only_marks()]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Ahem"),
        FontFamilyName::named("Test Only Marks"),
    ];
    let mut style = families_style(&listed);
    style.font.variant_caps = FontVariantCaps::SmallCaps;
    let text = "a\u{AD}\u{301}";
    fixture.span(&mut layout, &style, text);
    assert_eq!(
        layout.analysis().clusters.len(),
        3,
        "the acute stands alone"
    );
    assert_eq!(fixture.families(&layout)[2], "Test Only Marks");
    let cold = cluster_used_font(&layout, 2).synthesis;
    fixture.span(&mut layout, &style, text);
    let warm = cluster_used_font(&layout, 2).synthesis;
    assert_eq!(warm.case(), CaseMap::Upper, "the acute on the a's side");
    assert_eq!(cold, warm, "a cold build as a warm one");
}

/// Building again selects the same fonts, in a warm context or a fresh one.
#[test]
fn a_rebuild_selects_what_a_fresh_build_does() {
    let text = "Mixed text 漢字 with digits 123, \u{2665} and e\u{301}.";
    let ahem = [FontFamilyName::named("Ahem")];
    let style = families_style(&ahem);
    let mut fixture = fixture_with(&[han(), symbols()]);
    let mut layout = Layout::new();
    fixture.span(&mut layout, &style, text);
    let first = fixture.families(&layout).join(",");
    fixture.span(&mut layout, &style, "something else entirely");
    fixture.span(&mut layout, &style, text);
    assert_eq!(fixture.families(&layout).join(","), first);

    let mut fresh = fixture_with(&[han(), symbols()]);
    let mut other = Layout::new();
    fresh.span(&mut other, &style, text);
    assert_eq!(fresh.families(&other).join(","), first);
}

/// A builder dropped without finishing leaves no fonts, rather than those of
/// the content before.
#[test]
fn a_dropped_builder_leaves_no_fonts() {
    let mut fixture = fixture_with(&[]);
    let mut layout = Layout::new();
    fixture.span(&mut layout, &ComputedStyle::initial(), "abc");
    assert!(!text_runs(&layout).is_empty());
    let mut b = layout.builder(
        &mut fixture.cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&ComputedStyle::initial()),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "never finished");
    drop(b);
    assert!(text_runs(&layout).is_empty());
    assert!(layout.fonts().used.is_empty());
}

/// One build naming far more `font-family` lists than the context keeps sets
/// each span in its own list's font: a build only adds lists, and the lists
/// go back to their limit when the next build starts.
#[test]
fn a_build_naming_more_family_lists_than_the_limit_sets_each_in_its_own() {
    const LISTS: usize = 100;
    let latin = TestFont::new("Test Plain Latin", &[(0x20, 0x7E)]);
    let mut fixture = fixture_with(&[latin]);
    let limit = fixture.cx.cache_limits().family_lists;
    assert!(LISTS > limit);
    let names: Vec<String> = (0..LISTS).map(|n| format!("Missing {n}")).collect();
    let lists: Vec<[FontFamilyName<'_>; 2]> = names
        .iter()
        .zip(["Ahem", "Test Plain Latin"].into_iter().cycle())
        .map(|(name, found)| [FontFamilyName::named(name), FontFamilyName::named(found)])
        .collect();
    let styles: Vec<ComputedStyle<'_>> = lists.iter().map(|list| families_style(list)).collect();
    let spans: Vec<(&ComputedStyle<'_>, &str)> = styles.iter().map(|style| (style, "a")).collect();
    let mut layout = Layout::new();
    let built = fixture.spans(&mut layout, &ComputedStyle::initial(), &spans);
    assert!(built.is_complete());
    let families = fixture.families(&layout);
    assert_eq!(families.len(), LISTS);
    for (n, family) in families.iter().enumerate() {
        let listed = if n % 2 == 0 {
            "Ahem"
        } else {
            "Test Plain Latin"
        };
        assert_eq!(*family, listed, "span {n}");
    }
    assert!(fixture.cx.font_context().counts()[0] >= LISTS);
    // The initial list is never stored, so this build adds none.
    fixture.text(&mut layout, &ComputedStyle::initial(), "a");
    assert_eq!(fixture.cx.font_context().counts()[0], limit);
}
