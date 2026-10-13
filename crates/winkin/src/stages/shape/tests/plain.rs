//! Tests that the plain path writes exactly what the cluster-by-cluster path
//! writes, over random text, styles, directions, orientations and first
//! lines.

use super::*;
use crate::style::{FirstLineVariant, LengthPercentage, TextCase, TextTransform};

/// What a shaping wrote, word by word: each cluster's word, the glyphs it
/// draws and its advance, and where each run starts and in which font.
fn written(
    words: &[(ClusterId, GlyphWord)],
    sidecar: &Table<SidecarGlyphId, SidecarGlyph>,
    advances: &[i32],
) -> Vec<(GlyphWord, Vec<Drawn>)> {
    words
        .iter()
        .map(|&(cluster, word)| {
            let advance = advances.get(cluster.get()).copied().unwrap_or(0);
            (word, drawn(word, sidecar, advance))
        })
        .collect()
}

/// Shaping writes a call's output by a plain path where no mark is kerned or
/// halved and its glyphs run forward along a horizontal line; it must write
/// exactly what the path that asks of every cluster writes. Over random text
/// of ligatures, kerned pairs, glyphs split and raised, marks, Hebrew and
/// Arabic, ideographs and the punctuation `text-spacing-trim` trims, in
/// random styles, directions and orientations, the shaped text each builds,
/// its first line's in capitals, its advances, and the ranges the breaker
/// would reshape are the same either way.
#[test]
fn the_plain_path_writes_what_asking_every_cluster_does() {
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
    let fonts = [
        latin_hebrew(),
        arabic(),
        han(),
        TestFont::cjk("Test Punct", true),
        TestFont::cjk("Test Punct Plain", false),
    ];
    let mut fixture = fixture_with(&fonts);
    // The shaping check holds no first line's table, so a first line is
    // built unchecked.
    let mut lined = Fixture::new(&fonts, han_fallback("Test Han"), StageCheck::Built(|_| {}));
    let lists: [&[FontFamilyName<'static>]; 5] =
        [&LATIN_HEBREW, &ARABIC, &AHEM_FAMILY, &PUNCT, &PLAIN_PUNCT];
    let arabic_word: String = [BEH, TEH, SEEN, MEEM].iter().collect();
    let hebrew_word: String = [ALEF, LAMED, BET, GIMEL].iter().collect();
    let pieces = [
        "fi",
        "ffi",
        "fl",
        "office",
        "AV",
        "Q",
        "R",
        "i",
        " ",
        "a",
        "e\u{301}",
        "\u{300}",
        "\u{AD}",
        "\u{200B}",
        "\t",
        "漢字",
        "、",
        "「",
        "」",
        "。",
        "（",
        "）",
        "・",
        "1",
        "ß",
        &arabic_word,
        &hebrew_word,
    ];
    let trims = [
        TextSpacingTrim::Normal,
        TextSpacingTrim::SpaceAll,
        TextSpacingTrim::TrimStart,
    ];
    let mut random = Lcg(5);
    for round in 0..300 {
        let style_of = |random: &mut Lcg| {
            let mut style = sized(
                lists[random.next(lists.len())],
                [16.0, 20.0, 13.3][random.next(3)],
            );
            if random.next(4) == 0 {
                style.font.variant_caps = FontVariantCaps::SmallCaps;
            }
            if random.next(4) == 0 {
                style.text.letter_spacing = LengthPercentage::from_px(2.0);
            }
            if random.next(3) == 0 {
                style.text.language = Language::parse(["tr", "ja"][random.next(2)]).ok();
            }
            style.text.spacing_trim = trims[random.next(trims.len())];
            style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
            style
        };
        let root = style_of(&mut random);
        // Every third round sets the first line in capitals, where `ß`
        // makes two characters of one cluster.
        let capitalize = |style: &ComputedStyle<'static>| {
            let mut capitals = *style;
            capitals.text.transform = TextTransform {
                case: TextCase::Uppercase,
                ..TextTransform::NONE
            };
            capitals
        };
        let capitals = capitalize(&root);
        let first_line = round % 3 == 0;
        let spans: Vec<(ComputedStyle<'_>, String)> = (0..1 + random.next(4))
            .map(|_| {
                let style = style_of(&mut random);
                let text: String = (0..1 + random.next(10))
                    .map(|_| pieces[random.next(pieces.len())])
                    .collect();
                (style, text)
            })
            .collect();
        let mut block = ComputedBlockStyle::new(&root);
        match random.next(4) {
            0 => block.direction = BaseDirection::Rtl,
            1 => block.writing_mode = WritingMode::VerticalRl,
            _ => {}
        }
        if first_line {
            block.first_line = Some(&capitals);
        }
        let firsts: Vec<ComputedStyle<'_>> =
            spans.iter().map(|(style, _)| capitalize(style)).collect();
        let spans: Vec<(&ComputedStyle<'_>, &str, &ComputedStyle<'_>)> = spans
            .iter()
            .zip(&firsts)
            .map(|((style, text), first)| (style, text.as_str(), first))
            .collect();
        // All the shaped text, and a range of each run as the breaker shapes it.
        let shape = |fixture: &mut Fixture, marked: bool, random: &mut Lcg| {
            shaper::MARKED_ONLY.with(|only| only.set(marked));
            let mut layout = Layout::new();
            fixture.build(&mut layout, &block, |b| {
                for (at, &(style, text, first)) in (0u64..).zip(&spans) {
                    b.open_box(NodeKey(2 * at + 1), style, first_line.then_some(first));
                    b.text(NodeKey(2 * at + 2), text);
                    b.close_box();
                }
            });
            let advances = fixture.advances();
            let shaped = layout.shaped().text(FirstLineVariant::Standard);
            let words: Vec<(ClusterId, GlyphWord)> =
                shaped.glyphs.iter().map(|(c, &w)| (c, w)).collect();
            let mut said = vec![format!(
                "{:?}",
                written(&words, &shaped.glyphs.sidecar, &advances)
            )];
            if let Some(first) = layout.shaped().first_line() {
                let words: Vec<(ClusterId, GlyphWord)> =
                    first.glyphs.iter().map(|(c, &w)| (c, w)).collect();
                let advances: Vec<i32> = first.advances.iter().map(|a| a.raw()).collect();
                said.push(format!(
                    "first line {:?}",
                    written(&words, &first.glyphs.sidecar, &advances)
                ));
            }
            let runs: Vec<(ShapedRunId, Range<ClusterId>)> = shaped
                .runs
                .iter()
                .map(|(id, _, clusters)| (id, clusters))
                .collect();
            for (run, clusters) in runs {
                said.push(format!("{run:?} {clusters:?}"));
                let len = clusters.end.get() - clusters.start.get();
                let from = clusters.start.get() + random.next(len);
                let to = from + 1 + random.next(clusters.end.get() - from);
                let piece = fixture.reshape(&layout, run, ClusterId::new(from)..ClusterId::new(to));
                let words: Vec<(ClusterId, GlyphWord)> =
                    piece.words.iter().map(|(c, &w)| (c, w)).collect();
                let advances: Vec<i32> = piece.advances.iter().map(|a| a.raw()).collect();
                said.push(format!(
                    "{from}..{to} {:?} {:?}",
                    written(&words, &piece.sidecar, &advances),
                    piece.start
                ));
            }
            shaper::MARKED_ONLY.with(|only| only.set(false));
            said
        };
        // The same random ranges either way.
        let fixture = if first_line { &mut lined } else { &mut fixture };
        let seed = random.0;
        let plain = shape(fixture, false, &mut random);
        random.0 = seed;
        let marked = shape(fixture, true, &mut random);
        assert_eq!(plain, marked, "round {round}: {spans:?}");
    }
}
