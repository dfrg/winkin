//! Tests of variation sequences in font selection:
//! - Chrome 153's cases, measured on the `probe.html` and `load.html` probe
//!   pages over the probe's own fonts, VsA to VsF, at the probe's 50 px so
//!   that each width is Chrome's;
//! - the cases where winkin chooses otherwise, each saying why;
//! - the emoji cases, and a web font wanted for a sequence;
//! - in every case, harfrust shaping in the chosen font draws the
//!   sequence's own glyph exactly where fontwich says the font has it.
//!
//! The probe's fonts are a document's web fonts here, as they were in
//! Chrome. Fallback never reads a document's fonts. So where the list has
//! no font with a sequence, fallback has none either, as in Chrome.

use super::*;
use crate::tests::VARIATION_FONTS;
use crate::unicode::is_variation_selector;

/// The probe's size in pixels.
///
/// At this size VsA's base glyph is 35 px wide and its variants 36, 37 and
/// 38; VsB's base is 40, VsC's 45 with variants 46 to 48, VsD's 50 and
/// VsF's 60.
const PX: f32 = 50.0;

/// Returns the bytes of the probe font `family`.
fn probe_bytes(family: &str) -> &'static [u8] {
    VARIATION_FONTS
        .iter()
        .find(|(name, _)| *name == family)
        .map(|(_, bytes)| *bytes)
        .expect("a probe font")
}

/// Returns a fixture with the probe's fonts as a document's web fonts.
///
/// Under them is an application layer, which fallback reads, of Ahem,
/// `fonts`, and copies of the probe fonts `readable`, each under a family
/// name of its own (`Fallback VsC`). Fallback puts Ahem first, `han` among
/// Han's script fonts where one is named, and [`keycaps`] as the emoji
/// font, standing in for Segoe UI Emoji, which Chrome's fallback found for
/// ❤ and the keycap bases.
fn probe(fonts: &[TestFont], readable: &[&str], han: Option<&str>) -> Fixture {
    let mut application = LayerBuilder::new(Role::Application);
    assert!(application.add_data(AHEM).is_ok());
    for font in fonts {
        assert!(application.add_data(font.build()).is_ok());
    }
    let mut names = vec![String::from("Ahem")];
    names.extend(fonts.iter().map(|font| font.family.clone()));
    for &family in readable {
        let name = alloc::format!("Fallback {family}");
        let font = fontwich::Font::from_data(probe_bytes(family).to_vec(), 0);
        application.add_face_font(&name, FaceDescriptors::default(), font);
        names.push(name);
    }
    let mut fallback = ahem_fallback().emoji_family("Test Keycaps");
    if let Some(family) = han {
        fallback = fallback.han_family(family);
    }
    application.set_fallback_override(fallback);
    let mut document = LayerBuilder::new(Role::Document);
    for (family, bytes) in VARIATION_FONTS {
        assert!(document.add_data(bytes).is_ok());
        names.push(String::from(family));
    }
    let collection = Collection::new()
        .with_layer(application.snapshot())
        .with_layer(document.snapshot());
    fixture_over(collection, &names)
}

/// Returns `families` as a style names them.
fn listed(families: &[&'static str]) -> Vec<FontFamilyName<'static>> {
    families
        .iter()
        .map(|&family| FontFamilyName::named(family))
        .collect()
}

/// One case: its name on the probe page, the list, the text, the
/// `font-variant-emoji`, the family each cluster is set in, and the width
/// Chrome 153 measured, where it was measured in a probe font.
type Case = (
    &'static str,
    &'static [&'static str],
    &'static str,
    FontVariantEmoji,
    &'static [&'static str],
    Option<f32>,
);

/// Lays out each case in `fixture` at the probe's size and checks the
/// family of each cluster and the width.
///
/// Then checks that harfrust, shaping each cluster in the probe font chosen
/// for it, draws a sequence's own glyph exactly where fontwich says that
/// font has the sequence. It draws a `.notdef` only in case i15, where no
/// font has the base.
fn pin(fixture: &mut Fixture, cases: &[Case]) {
    for &(name, families, text, variant, expected, width) in cases {
        let families = listed(families);
        let mut style = sized(&families, PX);
        style.font.variant_emoji = variant;
        let mut layout = Layout::new();
        fixture.span(&mut layout, &style, text);
        assert_eq!(fixture.families(&layout), expected, "{name}: {text:?}");
        if let Some(width) = width {
            assert_eq!(
                layout.intrinsic_sizes().max_content,
                width,
                "{name}: {text:?}"
            );
        }
        let clusters = &layout.analysis().clusters;
        for (at, family) in fixture.families(&layout).into_iter().enumerate() {
            // A probe font, listed or the fallback's copy.
            let probe = family.strip_prefix("Fallback ").unwrap_or(family);
            let Some(&(_, bytes)) = VARIATION_FONTS.iter().find(|(name, _)| *name == probe) else {
                continue;
            };
            let range = clusters.range(ClusterId::new(at));
            let cluster = &layout.text()[range.start.get()..range.end.get()];
            let drawn = harfrust_glyphs(bytes, cluster);
            let font = fontwich::Font::from_data(bytes.to_vec(), 0);
            let sequences = cluster
                .chars()
                .zip(cluster.chars().skip(1))
                .filter(|&(base, selector)| {
                    is_variation_selector(selector) && font.maps_variation_sequence(base, selector)
                })
                .count();
            assert_eq!(
                drawn.len(),
                cluster.chars().count() - sequences,
                "{name}: harfrust in {family} draws {cluster:?} as {drawn:?}"
            );
            let notdef = drawn.contains(&0);
            assert_eq!(notdef, name == "i15", "{name}: a .notdef in {family}");
        }
    }
}

/// Returns the glyphs harfrust shapes `text` into in the font `bytes` hold,
/// with the buffer set up as shaping sets it up.
fn harfrust_glyphs(bytes: &[u8], text: &str) -> Vec<u32> {
    let font = harfrust::Font::new(bytes.to_vec(), 0).expect("a font");
    let shaper = harfrust::ShaperFont::new(&font);
    let mut buffer = harfrust::Buffer::new();
    buffer.push_str(text);
    buffer.guess_segment_properties();
    buffer.set_cluster_level(harfrust::ClusterLevel::MonotoneCharacters);
    buffer.set_flags(harfrust::BufferFlags::BEGINNING_OF_TEXT);
    harfrust::shape(&shaper, &mut buffer, harfrust::ShapeOptions::new()).expect("shaped");
    buffer
        .glyph_infos()
        .iter()
        .map(|glyph| glyph.glyph_id)
        .collect()
}

const NORMAL: FontVariantEmoji = FontVariantEmoji::Normal;

/// Chrome 153 takes the first font of the whole chain with the sequence,
/// Default UVS included, over list order. With none anywhere, it takes the
/// first font with the base and hides the selector. Each cluster chooses
/// alone. winkin does the same.
#[test]
fn the_first_font_with_the_sequence_wins_else_the_first_with_the_base() {
    let mut fixture = probe(&[], &[], None);
    #[rustfmt::skip]
    let cases: [Case; 18] = [
        // The first font has it.
        ("i1", &["VsA", "VsB"], "\u{845B}\u{E0100}", NORMAL, &["VsA"], Some(36.0)),
        ("s1", &["VsA", "VsB"], "\u{2229}\u{FE00}", NORMAL, &["VsA"], Some(36.0)),
        // A later font has it, over one with the base alone.
        ("i2", &["VsB", "VsA"], "\u{845B}\u{E0100}", NORMAL, &["VsA"], Some(36.0)),
        ("i3", &["VsB", "VsC", "VsA"], "\u{845B}\u{E0100}", NORMAL, &["VsC"], Some(46.0)),
        ("i4", &["VsE", "VsB", "VsA"], "\u{845B}\u{E0100}", NORMAL, &["VsA"], Some(36.0)),
        // A Default UVS entry counts as the sequence: the base's own glyph,
        // in the font that lists it.
        ("i5", &["VsB", "VsF"], "\u{845B}\u{E0100}", NORMAL, &["VsF"], Some(60.0)),
        ("i13", &["VsB", "VsA"], "\u{845B}\u{E0101}", NORMAL, &["VsA"], Some(35.0)),
        // Standardized sequences, ∩ VS1, 0 VS1 and a CJK compatibility one.
        ("s2", &["VsB", "VsA"], "\u{2229}\u{FE00}", NORMAL, &["VsA"], Some(36.0)),
        ("s6", &["VsB", "VsA"], "0\u{FE00}", NORMAL, &["VsA"], Some(36.0)),
        ("s7", &["VsB", "VsA"], "\u{8279}\u{FE00}", NORMAL, &["VsA"], Some(36.0)),
        // Each cluster alone: the sequence to VsA, the base after it to VsB.
        ("i12", &["VsB", "VsA"], "\u{845B}\u{E0100}\u{845B}", NORMAL, &["VsA", "VsB"], Some(76.0)),
        ("i14", &["VsB", "VsA"], "\u{845B}", NORMAL, &["VsB"], Some(40.0)),
        // No font has it: the first with the base draws the base.
        ("i6", &["VsB", "VsD"], "\u{845B}\u{E0100}", NORMAL, &["VsB"], Some(40.0)),
        ("i7", &["VsB", "VsD"], "\u{845B}\u{E01E0}", NORMAL, &["VsB"], Some(40.0)),
        ("i8", &["VsE", "VsD", "VsB"], "\u{845B}\u{E01E0}", NORMAL, &["VsD"], Some(50.0)),
        ("s8", &["VsB", "VsD"], "0\u{FE00}", NORMAL, &["VsB"], Some(40.0)),
        // No font has even the base: the primary font's `.notdef`, the
        // selector hidden.
        ("i15", &["VsE"], "\u{E000}\u{E0100}", NORMAL, &["VsE"], Some(20.0)),
        // A text selector no font lists falls back to the base, as any
        // sequence does.
        ("e3", &["VsB"], "\u{2764}\u{FE0E}", NORMAL, &["VsB"], Some(40.0)),
    ];
    pin(&mut fixture, &cases);
}

/// Where fallback has a font with the sequence, Chrome takes it over a
/// listed font with the base alone only where its system fallback for the
/// base's script and language happens to be that font: Yu Gothic for Han in
/// Japanese (i6ja, s9), Cambria Math for ∩ (s3). winkin agrees where a
/// fallback font for the script has the sequence, here the font named for
/// Han.
///
/// winkin differs elsewhere. Chrome asks system fallback by the base alone,
/// one font per script and language. So with `lang=en` it draws 葛 VS17 in
/// the list's font though Yu Gothic and MS Gothic on the same system have
/// the sequence (i6, i6zh, i6ko, r5), and `0` VS1 though Yu Gothic has it
/// (s8). winkin does not copy that limitation. CSS Fonts 4's cluster
/// matching asks system fallback "to find a font that supports the full
/// sequence". winkin asks every font of its list, and on a miss every
/// installed family.
#[test]
fn fallback_is_asked_for_the_sequence_not_the_base() {
    // i6ja: the script's fallback font has it.
    let mut fixture = probe(&[], &["VsC"], Some("Fallback VsC"));
    #[rustfmt::skip]
    pin(&mut fixture, &[
        ("i6ja", &["VsB", "VsD"], "\u{845B}\u{E0100}", NORMAL, &["Fallback VsC"], Some(46.0)),
        ("s9", &["VsB", "VsD"], "\u{8279}\u{FE00}", NORMAL, &["Fallback VsC"], Some(46.0)),
    ]);
    // Chrome draws the base for i6 and s8. Here the miss reaches a font no
    // script names, which has the sequence and draws it.
    let mut fixture = probe(&[], &["VsC"], None);
    #[rustfmt::skip]
    pin(&mut fixture, &[
        ("i6", &["VsB", "VsD"], "\u{845B}\u{E0100}", NORMAL, &["Fallback VsC"], Some(46.0)),
        ("s8", &["VsB", "VsD"], "0\u{FE00}", NORMAL, &["Fallback VsC"], Some(46.0)),
        // With none anywhere, still the first with the base.
        ("i7", &["VsB", "VsD"], "\u{845B}\u{E0106}", NORMAL, &["VsB"], Some(40.0)),
    ]);
    // The same in every language. Chrome draws the base for i6zh and i6ko,
    // since its fallback for Han in Chinese and Korean lacks the sequence.
    let families = listed(&["VsB", "VsD"]);
    for tag in ["zh-Hans", "ko", "ja", "en"] {
        let mut style = sized(&families, PX);
        style.text.language = Language::parse(tag).ok();
        let mut layout = Layout::new();
        fixture.span(&mut layout, &style, "\u{845B}\u{E0100}");
        assert_eq!(fixture.families(&layout), ["Fallback VsC"], "{tag}");
    }
}

/// Blink asks for the sequence only where Unicode sanctions it: a
/// standardized sequence, an ideographic one after an ideograph with no
/// decomposition, or VS15 and VS16 after an emoji. Any other selector
/// chooses by the base, though a later font lists the sequence (i10, s5,
/// i11, measured).
///
/// winkin asks for the sequence of every selector but VS15 and VS16. A font
/// that lists a sequence draws it, as harfrust would in that font, Unicode
/// or not. Across the 15,673 pairs real fonts carry, this differs from
/// Blink's rule only for five Myanmar pairs no Unicode file sanctions.
#[test]
fn every_selector_but_vs15_and_vs16_asks_for_its_sequence() {
    let mut fixture = probe(&[], &[], None);
    #[rustfmt::skip]
    let cases: [Case; 3] = [
        // 葛 VS1 is in no Unicode file: Chrome draws VsB's base, 40.
        ("i10", &["VsB", "VsA"], "\u{845B}\u{FE00}", NORMAL, &["VsA"], Some(38.0)),
        // ∩ VS2 is not in StandardizedVariants: Chrome draws VsB's base.
        ("s5", &["VsB", "VsA"], "\u{2229}\u{FE01}", NORMAL, &["VsA"], Some(37.0)),
        // 豈 decomposes canonically, so Blink takes no IVS after it: Chrome
        // draws VsB's base.
        ("i11", &["VsB", "VsA"], "\u{F900}\u{E0100}", NORMAL, &["VsA"], Some(36.0)),
    ];
    pin(&mut fixture, &cases);
}

/// The emoji cases, with Chrome 153's answers:
/// - a keycap base is asked its presentation: `#` VS16 goes to the emoji
///   font (e9), and so does a bare digit under `font-variant-emoji: emoji`
///   (e11);
/// - a monochrome font whose `cmap` has ❤ VS16 is taken for emoji, as
///   Blink's glyph callback asks for the sequence before the colour tables
///   (e1);
/// - a selector wins over the property;
/// - a color font lacking VS15's sequence is passed over for text;
/// - 😀 in text goes to a monochrome fallback font, which Chrome found in
///   Segoe UI Symbol (e7, e13).
#[test]
fn keycaps_and_an_emoji_sequence_choose_as_chrome_does() {
    use FontVariantEmoji::{Emoji, Text};
    let faces = TestFont::new("Test Faces", &[(0x1F600, 0x1F600)]);
    let mut fixture = probe(&[keycaps(), faces], &[], None);
    #[rustfmt::skip]
    let cases: [Case; 14] = [
        ("e1", &["VsB", "VsA"], "\u{2764}\u{FE0F}", NORMAL, &["VsA"], Some(36.0)),
        ("e2", &["VsB"], "\u{2764}\u{FE0F}", NORMAL, &["Test Keycaps"], None),
        ("e12", &["Test Keycaps", "VsB"], "\u{2764}\u{FE0F}", NORMAL, &["Test Keycaps"], None),
        ("e4", &["Test Keycaps", "VsB"], "\u{2764}\u{FE0E}", NORMAL, &["VsB"], Some(40.0)),
        ("e10", &["VsB"], "\u{2764}", NORMAL, &["VsB"], Some(40.0)),
        ("e5", &["VsB"], "\u{2764}", Emoji, &["Test Keycaps"], None),
        ("e6", &["Test Keycaps", "VsB"], "\u{2764}", Text, &["VsB"], Some(40.0)),
        ("e8", &["VsB"], "\u{2764}\u{FE0F}", Text, &["Test Keycaps"], None),
        ("e14", &["VsB"], "\u{2764}\u{FE0E}", Emoji, &["VsB"], Some(40.0)),
        ("e9", &["VsB"], "#\u{FE0F}", NORMAL, &["Test Keycaps"], None),
        ("e11", &["VsB"], "0", Emoji, &["Test Keycaps"], None),
        ("e7", &["VsB"], "\u{1F600}", Text, &["Test Faces"], None),
        ("e13", &["VsB"], "\u{1F600}\u{FE0E}", NORMAL, &["Test Faces"], None),
        // Not a Chrome case: a digit in text stays text.
        ("digit", &["VsB"], "0", NORMAL, &["VsB"], Some(40.0)),
    ];
    pin(&mut fixture, &cases);
}

/// A pending web font that has the sequence is wanted. Meanwhile the text
/// is drawn as though no font had the sequence: the base, in the first font
/// with it. When the face loads and the host builds again, the cluster takes
/// the sequence. Chrome 153 keeps the answer it gave while the font loaded,
/// for new text of the same style too, until something restyles it (L1,
/// L2, L3). winkin does not copy that limitation.
#[test]
fn a_web_font_with_the_sequence_is_wanted_and_takes_it_once_loaded() {
    let mut application = LayerBuilder::new(Role::Application);
    assert!(application.add_data(AHEM).is_ok());
    application.set_fallback_override(ahem_fallback());
    let mut document = LayerBuilder::new(Role::Document);
    assert!(document.add_data(probe_bytes("VsB")).is_ok());
    let later = document
        .add_face("Later", FaceDescriptors::default(), None)
        .expect("a face declared");
    let names = [
        String::from("Ahem"),
        String::from("VsB"),
        String::from("Later"),
    ];
    let collection = |document: &LayerBuilder| {
        Collection::new()
            .with_layer(application.snapshot())
            .with_layer(document.snapshot())
    };
    let mut fixture = fixture_over(collection(&document), &names);
    let families = listed(&["VsB", "Later"]);
    let style = sized(&families, PX);
    let mut layout = Layout::new();
    fixture.span(&mut layout, &style, "\u{845B}\u{E0100}");
    assert_eq!(fixture.families(&layout), ["VsB"]);
    assert_eq!(layout.wanted_faces(), [later]);

    document
        .load_face(later, probe_bytes("VsA").to_vec().into(), 0)
        .expect("the face loads");
    let mut fixture = fixture_over(collection(&document), &names);
    fixture.span(&mut layout, &style, "\u{845B}\u{E0100}");
    assert_eq!(fixture.families(&layout), ["Later"]);
    assert_eq!(layout.intrinsic_sizes().max_content, 36.0);
}

/// Text around a sequence keeps its fonts. A font passed over for lacking a
/// sequence is not ruled out for the text after it, nor is one whose
/// base-only cover is the second pass's answer. A run of text in one font
/// ends at a sequence and resumes after it.
#[test]
fn a_sequence_leaves_the_fonts_of_the_text_around_it() {
    let mut fixture = probe(&[], &[], None);
    let families = listed(&["VsB", "VsE", "VsA"]);
    let style = sized(&families, PX);
    let mut layout = Layout::new();
    // x is VsB's; 葛 VS17 is VsA's; 葛 and 辻 VS241, which no font has, are
    // VsB's; 辻 VS17 is VsA's; each x is VsB's.
    fixture.span(
        &mut layout,
        &style,
        "x\u{845B}\u{E0100}\u{845B}x\u{8FBB}\u{E01E0}x\u{8FBB}\u{E0100}x",
    );
    assert_eq!(
        fixture.families(&layout),
        ["VsB", "VsA", "VsB", "VsB", "VsB", "VsB", "VsA", "VsB"]
    );
}

/// A grapheme divided by a span between its base and its selector, where
/// the spans shape alike, is chosen for whole: the sequence, in the font
/// that has it.
#[test]
fn a_sequence_divided_by_a_span_is_chosen_for_whole() {
    let mut fixture = probe(&[], &[], None);
    let families = listed(&["VsB", "VsA"]);
    let style = sized(&families, PX);
    let painted = ComputedStyle {
        paints: true,
        ..style
    };
    let mut layout = Layout::new();
    fixture.spans(
        &mut layout,
        &style,
        &[(&style, "\u{845B}"), (&painted, "\u{E0100}")],
    );
    assert_eq!(fixture.families(&layout), ["VsA", "VsA"]);
}
