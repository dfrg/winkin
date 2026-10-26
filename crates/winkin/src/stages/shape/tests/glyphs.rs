//! Advances and glyph storage tests: exact advances at 16.16 for any
//! size, compact words, the sidecar, ligature continuations,
//! right-to-left order, `.notdef` and broken layout tables.

use super::*;
use crate::style::{FirstLineVariant, FontVariantPosition};

// Advances -----------------------------------------------------------------

/// Ahem's every glyph is a square of the em, so every cluster advances by
/// the size.
///
/// Text is shaped at harfrust scale `size × 65536`, as Chrome sets HarfBuzz
/// up. The advance is exactly the size in 16.16 where harfrust's arithmetic
/// lands there, and matches harfrust's arithmetic to the unit everywhere.
/// The size is Chrome's effective one, floored to a hundredth of a pixel in
/// `f32`: Chrome sets a hundred of Ahem's squares at 16.3 px 1629 px wide.
/// Every cluster is compact, 4 bytes.
#[test]
fn square_glyphs_advance_exactly_at_16_16() {
    let mut fixture = fixture();
    for (size, effective) in [
        (16.0, 16.0),
        (13.5, 13.5),
        (1.0, 1.0),
        (16.3, 16.29),
        (72.0, 72.0),
        (10_000.0, 10_000.0),
    ] {
        let mut layout = Layout::new();
        fixture.span(&mut layout, &sized(&AHEM_FAMILY, size), "XpÉ x");
        let scale = TextUnit::from_px_truncated(effective).raw();
        let em = scaled(1000, 1000, i64::from(scale));
        let advances = fixture.advances();
        assert!(advances.iter().all(|&a| a == em), "{size} px: {advances:?}");
        let shaped = layout.shaped().text(FirstLineVariant::Standard);
        assert!(shaped.glyphs.sidecar.is_empty(), "{size} px: compact");
        assert!(
            shaped
                .glyphs
                .iter()
                .all(|(_, w)| !w.is_expanded() && !w.is_unsafe_to_break())
        );
    }
    // Where the size is on the grid harfrust's scale divides evenly, the
    // advance is the size itself.
    for size in [16.0, 13.5, 1.0, 72.0, 10_000.0] {
        let mut layout = Layout::new();
        fixture.span(&mut layout, &sized(&AHEM_FAMILY, size), "X");
        assert_eq!(
            fixture.advances(),
            [TextUnit::from_px_truncated(size).raw()],
            "{size} px"
        );
    }
}

/// A zero, negative, non-finite or tiny size and one past the largest Chrome
/// allows each shape. Nothing is wide at zero, and a size past ten thousand
/// pixels is ten thousand, whose em still fits 16.16.
#[test]
fn any_font_size_gives_valid_advances() {
    let mut fixture = fixture();
    // A size that is not finite is zero, as is one too small for 16.16.
    for size in [0.0, -3.0, f32::NAN, 1e-9, 50_000.0, f32::INFINITY] {
        let mut layout = Layout::new();
        fixture.span(&mut layout, &sized(&AHEM_FAMILY, size), "XpÉ x");
        let expected = if size == 50_000.0 { 10_000 * 65_536 } else { 0 };
        let advances = fixture.advances();
        assert!(
            advances.iter().all(|&a| a == expected),
            "{size}: {advances:?}"
        );
    }
    // A cluster of several glyphs at ten thousand pixels measures exactly:
    // its glyphs are summed in 64 bits, and the sum fits 16.16.
    let mut layout = Layout::new();
    fixture.span(&mut layout, &sized(&LATIN_HEBREW, 50_000.0), "Q");
    let q = fixture.cx.shaped_advances();
    assert_eq!(q[0].raw(), 7_500 * 65_536, "three quarters of an em");
}

// Glyphs -------------------------------------------------------------------

/// Latin set in a font with nothing to substitute or move is all compact,
/// 4 bytes a cluster, with nothing in the sidecar.
#[test]
fn text_with_nothing_to_substitute_or_move_is_compact() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let font = latin_hebrew();
    fixture.span(
        &mut layout,
        &families_style(&LATIN_HEBREW),
        "The quick brown dog jumps over the lazy cat.",
    );
    let advances = fixture.advances();
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    assert!(shaped.glyphs.sidecar.is_empty());
    let text = &layout.content().text;
    for ((cluster, word), ch) in shaped.glyphs.iter().zip(text.chars()) {
        assert_eq!(
            word.glyphs(&shaped.glyphs.sidecar),
            ClusterGlyphs::One(font.glyph(ch)),
            "{cluster:?}"
        );
        assert_eq!(advances[cluster.get()], 8 * 65_536, "half an em at 16 px");
    }
    assert_eq!(shaped.runs.len(), 1, "one run");
}

/// A font of more than 65,536 glyphs is shaped and stored whole: a
/// character mapped past glyph 65,535 is a compact cluster holding its id,
/// and `sups` substitutes its alternate past it from a 24-bit single
/// substitution, with the alternate's advance from `HMTX`.
#[test]
fn glyphs_past_65535_are_shaped_and_stored() {
    let mut font = TestFont::new("Test Wide", &[(0x20, 0x7E), (0x20000, 0x20000 + 65_600)]);
    let wide = '\u{2FFDC}';
    font.alternates = vec![(*b"sups", vec![(wide, 300)])];
    let (plain, alternate) = (font.glyph(wide), font.alternate_glyph(*b"sups", wide));
    assert!(plain > 65_535 && alternate > plain);
    let mut fixture = fixture_with(&[font]);
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Wide")];
    let mut style = families_style(&family);
    fixture.span(&mut layout, &style, "\u{2FFDC}");
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    assert!(shaped.glyphs.sidecar.is_empty(), "compact");
    assert_eq!(ids(&layout), [vec![plain]]);
    assert_eq!(fixture.advances(), [px16(8)], "half an em at 16 px");
    style.font.variant_position = FontVariantPosition::Super;
    fixture.span(&mut layout, &style, "\u{2FFDC}");
    assert_eq!(ids(&layout), [vec![alternate]]);
    let size = i64::from(px16(16));
    assert_eq!(fixture.advances(), [scaled(300, 1000, size)]);
}

/// A character drawn as two glyphs and a glyph drawn raised are expanded.
///
/// Their glyphs are in the sidecar, 16 bytes each, the last marked, and the
/// cluster's advance is their sum. Everything else stays compact.
#[test]
fn several_glyphs_or_an_offset_expand_a_cluster() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let font = latin_hebrew();
    fixture.span(&mut layout, &families_style(&LATIN_HEBREW), "aQbRc");
    let advances = fixture.advances();
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let word = |at| shaped.glyphs.word(ClusterId::new(at));
    assert!(!word(0).is_expanded() && !word(2).is_expanded() && !word(4).is_expanded());
    let q = drawn(word(1), &shaped.glyphs.sidecar, advances[1]);
    assert_eq!(
        q,
        [
            (font.glyph('Q'), 0, 0, 8 * 65_536),
            (font.split_glyph('Q'), 0, 0, 4 * 65_536)
        ]
    );
    assert_eq!(advances[1], 12 * 65_536);
    let r = drawn(word(3), &shaped.glyphs.sidecar, advances[3]);
    assert_eq!(
        r,
        [(font.glyph('R'), 0, 200 * 16 * 65_536 / 1000, 8 * 65_536)]
    );
    assert_eq!(shaped.glyphs.sidecar.len(), 3);
}

/// A ligature's glyph is its first cluster's. The clusters inside it are
/// continuations with no glyphs and no advance, unsafe to break before.
#[test]
fn a_ligature_leaves_continuations() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let font = latin_hebrew();
    fixture.span(&mut layout, &families_style(&LATIN_HEBREW), "office flat");
    let advances = fixture.advances();
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let expected = [
        vec![font.glyph('o')],
        vec![font.ligature_glyph(0)],
        vec![],
        vec![],
        vec![font.glyph('c')],
        vec![font.glyph('e')],
        vec![font.glyph(' ')],
        vec![font.ligature_glyph(2)],
        vec![],
        vec![font.glyph('a')],
        vec![font.glyph('t')],
    ];
    assert_eq!(ids(&layout), expected);
    for at in [2, 3, 8] {
        let word = shaped.glyphs.word(ClusterId::new(at));
        assert!(word.is_continuation() && word.is_unsafe_to_break(), "{at}");
        assert_eq!(advances[at], 0);
    }
    assert_eq!(advances[1], 12 * 65_536, "three quarters of an em");
    assert!(!shaped.glyphs.word(ClusterId::new(1)).is_continuation());
}

/// A ligature that takes a grapheme's first character takes the rest of
/// that grapheme too, as HarfBuzz's grapheme cluster level merges them.
///
/// The mark after a ligated `i` is the ligature's cluster's, and the `i`'s
/// cluster is a continuation. The same holds right to left.
#[test]
fn a_ligature_takes_the_rest_of_a_grapheme() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let font = latin_hebrew();
    let style = families_style(&LATIN_HEBREW);
    fixture.span(&mut layout, &style, "fi\u{301}x");
    let expected = [
        vec![font.ligature_glyph(1), font.glyph('\u{301}')],
        vec![],
        vec![font.glyph('x')],
    ];
    assert_eq!(ids(&layout), expected);
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    assert!(shaped.glyphs.word(ClusterId::new(1)).is_continuation());
    assert!(!shaped.glyphs.word(ClusterId::new(2)).is_continuation());
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let text: String = [ALEF, LAMED, '\u{301}', BET].iter().collect();
    fixture.build_spans(&mut layout, &rtl, &[(&style, &text)]);
    let expected = [
        vec![font.glyph('\u{301}'), font.ligature_glyph(3)],
        vec![],
        vec![font.split_glyph(BET), font.glyph(BET)],
    ];
    assert_eq!(ids(&layout), expected);
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    assert!(shaped.glyphs.word(ClusterId::new(1)).is_continuation());
}

/// In a right-to-left run the words are still in cluster order, a ligature
/// on its first cluster. A cluster's several glyphs are in the order they
/// are drawn, which for Hebrew is the reverse of the text's.
///
/// Each run is shaped in its level's direction. Latin in a right-to-left
/// paragraph resolves to level 2 and is shaped left to right, so it ligates
/// as it would anywhere.
#[test]
fn right_to_left_is_stored_in_cluster_order() {
    let mut fixture = fixture();
    let style = families_style(&LATIN_HEBREW);
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let font = latin_hebrew();
    let text: String = [ALEF, LAMED, BET, GIMEL].iter().collect();
    let mut layout = Layout::new();
    fixture.build_spans(&mut layout, &rtl, &[(&style, &text)]);
    assert_eq!(
        layout
            .analysis()
            .runs
            .get(ScriptRunId::new(0))
            .unwrap()
            .level
            .get(),
        1
    );
    let expected = [
        vec![font.ligature_glyph(3)],
        vec![],
        vec![font.split_glyph(BET), font.glyph(BET)],
        vec![font.glyph(GIMEL)],
    ];
    assert_eq!(ids(&layout), expected);
    assert!(
        layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .glyphs
            .word(ClusterId::new(1))
            .is_continuation()
    );
    assert_eq!(
        fixture.advances(),
        [12 * 65_536, 0, 12 * 65_536, 8 * 65_536]
    );
    // Latin in the same paragraph is at level 2 and set left to right:
    // ligated, in cluster order, its split's glyphs as drawn.
    fixture.build_spans(&mut layout, &rtl, &[(&style, "fiQb")]);
    assert_eq!(
        layout
            .analysis()
            .runs
            .get(ScriptRunId::new(0))
            .unwrap()
            .level
            .get(),
        2
    );
    let expected = [
        vec![font.ligature_glyph(1)],
        vec![],
        vec![font.glyph('Q'), font.split_glyph('Q')],
        vec![font.glyph('b')],
    ];
    assert_eq!(ids(&layout), expected);
}

/// Each run is shaped in the direction of its level.
///
/// Arabic in a left-to-right paragraph resolves to level 1 and joins exactly
/// as in a right-to-left one, its first letter initial and its last final.
/// Under a left-to-right override it is shaped left to right, against its
/// direction, as HarfBuzz does, and draws differently from the unforced
/// text.
#[test]
fn a_run_is_shaped_in_its_bidi_levels_direction() {
    let mut fixture = fixture();
    let style = families_style(&ARABIC);
    let text: String = [BEH, TEH, SEEN].iter().collect();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let mut layout = Layout::new();
    fixture.build_spans(&mut layout, &rtl, &[(&style, &text)]);
    let right_to_left = ids(&layout);
    fixture.spans(&mut layout, &style, &[(&style, &text)]);
    assert_eq!(
        layout
            .analysis()
            .runs
            .get(ScriptRunId::new(0))
            .unwrap()
            .level
            .get(),
        1
    );
    assert_eq!(ids(&layout), right_to_left);
    let font = arabic();
    assert_eq!(
        right_to_left,
        [
            vec![font.form_glyph(BEH, Form::Initial)],
            vec![font.form_glyph(TEH, Form::Medial)],
            vec![font.form_glyph(SEEN, Form::Final)],
        ]
    );
    let forced = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Ltr,
            unicode_bidi: UnicodeBidi::BidiOverride,
        },
        ..style
    };
    fixture.spans(&mut layout, &style, &[(&forced, &text)]);
    assert_eq!(
        layout
            .analysis()
            .runs
            .get(ScriptRunId::new(0))
            .unwrap()
            .level
            .get(),
        2
    );
    assert_ne!(ids(&layout), right_to_left);
}

/// A cluster no font covers is drawn in its style's primary font as
/// `.notdef`, glyph 0, compact, with its advance.
#[test]
fn a_cluster_no_font_covers_draws_notdef() {
    let mut fixture = fixture_with(&[latin_hebrew()]);
    let mut layout = Layout::new();
    fixture.span(&mut layout, &families_style(&LATIN_HEBREW), "a\u{10400}b");
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    assert_eq!(
        shaped.glyphs.glyphs(ClusterId::new(1)),
        ClusterGlyphs::One(0)
    );
    assert_eq!(
        fixture.advances()[1],
        8 * 65_536,
        "the .notdef's half an em"
    );
}

/// With no font at all, every cluster keeps its place with no glyphs and no
/// advance, in runs that still tile the clusters.
#[test]
fn no_font_at_all_draws_nothing() {
    let mut cx = Context::new(Collection::new());
    let mut layout = Layout::new();
    let mut b = layout.builder(
        &mut cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&ComputedStyle::initial()),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "Nothing\tto draw\u{200B} with");
    b.line_break(NodeKey(2));
    b.text(NodeKey(3), "at all");
    assert!(b.finish().is_complete());
    check(&layout, &cx);
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    assert!(shaped.glyphs.iter().all(|(_, &w)| w == GlyphWord::EMPTY));
    assert!(advances(&cx).iter().all(|&a| a == 0));
    assert!(!shaped.runs.is_empty());
}

/// A font whose `GSUB` and `GPOS` cannot be read shapes with its nominal
/// glyphs: no ligature, no kern, nothing raised, and nothing panics.
#[test]
fn unreadable_layout_tables_shape_nominally() {
    let mut font = latin_hebrew();
    font.family = "Test Latin Hebrew Broken".into();
    font.broken_layout = true;
    let mut fixture = fixture_with(slice::from_ref(&font));
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Latin Hebrew Broken")];
    fixture.span(&mut layout, &families_style(&family), "fiAVQR");
    let expected: Vec<Vec<u32>> = "fiAVQR".chars().map(|ch| vec![font.glyph(ch)]).collect();
    assert_eq!(ids(&layout), expected);
    assert!(fixture.advances().iter().all(|&a| a == 8 * 65_536));
    assert!(!layout.shaped().flags.contains(ShapedFlags::HAS_UNSAFE));
}
