//! Tests that choosing fonts for many clusters at once chooses exactly what
//! asking the list at every cluster chooses, over random text and styles,
//! variation sequences among them.

use core::iter;

use super::*;
use crate::tests::VARIATION_FONTS;

/// Returns what font selection made for `layout`, by what each font is rather than by the
/// context's ids: every run of the text and of the first line, and every
/// used font's family, sizes, synthesis, coordinates and features, each
/// style's primary font, the generated text and the faces wanted.
fn selection(fixture: &Fixture, layout: &Layout) -> Vec<String> {
    use alloc::format;
    let fonts = layout.fonts();
    let mut said = Vec::new();
    for variant in [Standard, FirstLineVariant::FirstLine] {
        for (_, run) in fonts.runs(variant).iter() {
            said.push(format!("{variant:?} run {run:?}"));
        }
    }
    for (id, used) in fonts.used.iter() {
        let drawn = used.instance.as_ref().map(|shaping| {
            let features: Vec<_> = shaping
                .features
                .iter()
                .map(|feature| (feature.tag.to_bytes(), feature.value))
                .collect();
            format!(
                "{} {:?} {:?} {:?} {:?}",
                fixture.family(layout, id),
                shaping.coords,
                features,
                shaping.embolden,
                shaping.skew
            )
        });
        said.push(format!(
            "{id:?} {:?} {:?} {:?} {drawn:?}",
            used.size,
            used.glyph_size(),
            used.synthesis
        ));
    }
    for text in layout.content().facts.text_ids() {
        let primary = primary_id(layout, text);
        said.push(format!("primary {text:?} {primary:?}"));
    }
    for generated in fonts.generated().iter() {
        said.push(format!("generated {generated:?}"));
    }
    said.push(format!("wanted {:?}", fonts.wanted));
    said
}

/// The run loop takes many clusters at once where it can see that the list
/// gives each the font the cluster before took. It must choose exactly what
/// asking at every cluster chooses.
///
/// The text is random, of every script, class and form the fixture's fonts
/// cover in part. It has fallback runs ending where an earlier font covers
/// again, emoji and symbols with and without selectors, keycap bases, and
/// variation sequences a later font or no font has. It has graphemes
/// divided by spans that shape alike and ones that don't, composed and
/// decomposed forms, what a font fakes, clusters that draw nothing, and a
/// pending face wanted.
///
/// The styles are random too: sizes, lists, small capitals synthesized and
/// not, positions covered and synthesized, emoji forced either way,
/// languages, and a first line restyled. Each build makes what the walk that
/// asks at every cluster makes, cold and warm, in two contexts fed alike.
#[test]
fn choosing_for_many_clusters_at_once_matches_asking_every_cluster() {
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self, below: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(self.0 >> 33).unwrap_or(0) % below.max(1)
        }
    }
    let fixture = || {
        let mut document = LayerBuilder::new(Role::Document);
        document
            .add_face(
                "Brand",
                FaceDescriptors {
                    unicode_range: iter::once(0x41..=0x5A).collect(),
                    ..FaceDescriptors::default()
                },
                None,
            )
            .expect("a face declared");
        // The variation-sequence probe's fonts, as web fonts, which fallback
        // never reads.
        for (_, bytes) in &VARIATION_FONTS[..3] {
            assert!(document.add_data(*bytes).is_ok());
        }
        let fonts = [
            han(),
            plain(),
            marks(),
            symbols(),
            emoji(),
            pages(),
            capitals(),
            superscripts(),
            keycaps(),
        ];
        let mut names = vec![String::from("Ahem")];
        names.extend(fonts.iter().map(|font| font.family.clone()));
        names.extend(
            VARIATION_FONTS[..3]
                .iter()
                .map(|(name, _)| String::from(*name)),
        );
        // Emoji fall back to their own font, which text never does: an
        // emoji's candidate list and the text's part after the fonts the
        // style lists.
        let fallback = han_fallback("Test Han Acute").emoji_family("Test Emoji");
        let collection = collection(&fonts, fallback).with_layer(document.snapshot());
        Fixture::from_collection(collection, &names, StageCheck::Placed(|_| {}))
    };
    let named = |names: &[&'static str]| -> Vec<FontFamilyName<'static>> {
        names
            .iter()
            .map(|&name| FontFamilyName::named(name))
            .collect()
    };
    let lists = [
        named(&["Ahem"]),
        named(&["Test Plain Hyphen", "Test Acute Marks"]),
        named(&["Test Pages", "Test Plain Hyphen"]),
        named(&["Test Symbols", "Test Emoji"]),
        named(&["Test Emoji", "Test Plain Hyphen"]),
        named(&["Test Capitals"]),
        named(&["Test Superscripts", "Test Han Acute"]),
        named(&["Test Superscripts"]),
        named(&["Test Capitals", "Test Superscripts"]),
        named(&["Brand", "Test Plain Hyphen"]),
        named(&["Test Plain Hyphen", "Brand", "Test Acute Marks"]),
        named(&["Test Han Acute"]),
        named(&[
            "Test Plain Hyphen",
            "Test Han Acute",
            "Test Acute Marks",
            "Test Pages",
        ]),
        named(&["VsB", "VsA"]),
        named(&["VsB", "Test Plain Hyphen", "VsC", "Test Emoji"]),
        named(&["Test Plain Hyphen", "VsA"]),
        named(&["Test Keycaps", "Test Plain Hyphen"]),
        named(&["Test Plain Hyphen", "Test Keycaps", "VsB"]),
        Vec::new(),
    ];
    let pieces = [
        "a",
        "b",
        "Z",
        "Q",
        " ",
        "  ",
        "1",
        "2",
        "fi",
        "AV",
        ".",
        "é",
        "e\u{301}",
        "\u{301}",
        "\u{302}",
        "ß",
        "漢",
        "字",
        "\u{4E01}",
        "か",
        "カ",
        "\u{3000}",
        "、",
        "\u{A0}",
        "\u{2011}",
        "\u{2003}",
        "\u{AD}",
        "\t",
        "\u{200B}",
        "\u{2060}",
        "\u{200D}",
        "\u{2665}",
        "\u{2665}\u{FE0F}",
        "\u{2665}\u{FE0E}",
        "\u{1F600}",
        "\u{1F600}\u{FE0E}",
        "1\u{FE0F}\u{20E3}",
        "\u{101}",
        "\u{17F}",
        "\u{628}",
        "\u{42F}",
        "²",
        "\u{10FFFD}",
        "\n",
        "word",
        "漢字 12漢字",
        "×",
        "漢😀×",
        "12",
        "²3",
        "x2",
        "\u{845B}",
        "\u{845B}\u{E0100}",
        "\u{845B}\u{E01E0}",
        "\u{8FBB}\u{E0101}",
        "\u{2229}\u{FE00}",
        "0\u{FE00}",
        "0",
        "#",
        "#\u{FE0F}",
        "*\u{FE0E}",
        "\u{2764}",
        "\u{2764}\u{FE0F}",
        "x\u{E0100}",
        "\u{E0100}",
    ];
    let plain = ["1", "2", "12", "a", "b", " ", "AV", "x2"];
    let languages = [None, Language::parse("ja").ok(), Language::parse("tr").ok()];
    let caps = [
        FontVariantCaps::Normal,
        FontVariantCaps::SmallCaps,
        FontVariantCaps::AllSmallCaps,
        FontVariantCaps::Unicase,
    ];
    let positions = [
        FontVariantPosition::Normal,
        FontVariantPosition::Super,
        FontVariantPosition::Sub,
    ];
    let emojis = [
        FontVariantEmoji::Normal,
        FontVariantEmoji::Text,
        FontVariantEmoji::Emoji,
        FontVariantEmoji::Unicode,
    ];
    let collapses = [WhiteSpaceCollapse::Collapse, WhiteSpaceCollapse::Preserve];
    let sizes = [16.0, 30.0, 13.3];
    let mut random = Lcg(11);
    let mut shortcut = fixture();
    let mut asked = fixture();
    for round in 0..600 {
        let style_of = |random: &mut Lcg| {
            let list = &lists[random.next(lists.len())];
            let mut style = sized(list, sizes[random.next(sizes.len())]);
            // Mostly the plain case, where the shortcut keeps the most clusters.
            if random.next(3) == 0 {
                style.font.variant_caps = caps[random.next(caps.len())];
            }
            if random.next(3) == 0 {
                style.font.variant_position = positions[random.next(positions.len())];
            }
            if random.next(3) == 0 {
                style.font.variant_emoji = emojis[random.next(emojis.len())];
            }
            style.text.language = languages[random.next(languages.len())];
            style.paints = random.next(2) == 0;
            style.text.white_space_collapse = collapses[random.next(collapses.len())];
            style
        };
        // A first line mostly restyled in size and spacing alone, whose runs
        // are mapped from the text's, and else in anything.
        fn resized<'a>(style: &ComputedStyle<'a>, random: &mut Lcg) -> ComputedStyle<'a> {
            let mut style = *style;
            style.font.size = [20.0, 32.0, 13.3][random.next(3)];
            style.text.letter_spacing = [0.0, 0.0, 2.0][random.next(3)];
            style
        }
        let root = style_of(&mut random);
        let first_line = match random.next(2) {
            0 => resized(&root, &mut random),
            _ => style_of(&mut random),
        };
        let mut spans: Vec<(ComputedStyle<'_>, Option<ComputedStyle<'_>>, String)> = Vec::new();
        for _ in 0..1 + random.next(6) {
            // Half the spans go on in the style before, or one that only
            // paints otherwise, so that a grapheme a span divides is often
            // chosen for whole; and a quarter start with a mark, dividing
            // one.
            let style = match spans.last() {
                Some((before, _, _)) if random.next(2) == 0 => ComputedStyle {
                    paints: random.next(2) == 0,
                    ..*before
                },
                _ => style_of(&mut random),
            };
            let first = match random.next(4) {
                0 => Some(style_of(&mut random)),
                1 | 2 => Some(resized(&style, &mut random)),
                _ => None,
            };
            // A third of them in plain text alone, whose position runs
            // hold nothing the shortcut does not see.
            let pool = if random.next(3) == 0 {
                &plain[..]
            } else {
                &pieces[..]
            };
            let mut text = String::from(["", "", "", "\u{301}"][random.next(4)]);
            for _ in 0..random.next(14) {
                text.push_str(pool[random.next(pool.len())]);
            }
            spans.push((style, first, text));
        }
        let block = ComputedBlockStyle {
            first_line: (round % 3 == 0).then_some(&first_line),
            ..ComputedBlockStyle::new(&root)
        };
        let calls = |b: &mut LayoutBuilder<'_>| {
            for (at, (style, first, text)) in (0u64..).zip(&spans) {
                b.open_box(NodeKey(2 * at + 1), style, first.as_ref());
                b.text(NodeKey(2 * at + 2), text);
                b.close_box();
            }
        };
        let mut layouts = [Layout::new(), Layout::new()];
        for pass in ["cold", "warm"] {
            let [by_shortcut, by_cluster] = &mut layouts;
            shortcut.build(by_shortcut, &block, calls);
            select_kept::PER_CLUSTER.with(|per_cluster| per_cluster.set(true));
            asked.build(by_cluster, &block, calls);
            select_kept::PER_CLUSTER.with(|per_cluster| per_cluster.set(false));
            assert_eq!(
                selection(&shortcut, by_shortcut),
                selection(&asked, by_cluster),
                "round {round}, {pass}: {:?}",
                spans
                    .iter()
                    .map(|(style, _, text)| (style.font.families, text))
                    .collect::<Vec<_>>()
            );
        }
    }
}
