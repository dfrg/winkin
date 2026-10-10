//! Shaping run tests: where runs split, default ignorables and kerns,
//! first letters, divided graphemes, Arabic joining across a split, and
//! the language and language system the shaping plan gets.

use super::*;

/// Boundary discovery must produce exactly the cluster scan's runs and glyphs,
/// including paragraph tails, transparent boxes, first-line variants and the
/// exceptional paragraphs beside ordinary ones.
#[test]
fn finding_runs_at_boundaries_matches_scanning_every_cluster() {
    let mut fixture = Fixture::new(
        &[latin_hebrew(), arabic(), han()],
        han_fallback("Test Han"),
        StageCheck::Built(|_, _| {}),
    );
    for collapse in [WhiteSpaceCollapse::Collapse, WhiteSpaceCollapse::Preserve] {
        for text in [
            "",
            "\n",
            "\n\n",
            "AV fi ffi\nAV fi\n",
            "AV\u{85}fi\u{2028}AV",
            "AV\u{200B}fi\u{AD}AV\u{2060}fi",
            "AV\u{200C}fi\nplain",
            "AV\tfi\nplain\n\t",
            "AV日本fi\nمرحبا AV\nשלום",
        ] {
            let mut style = families_style(&LATIN_HEBREW);
            style.text.white_space_collapse = collapse;
            let first = ComputedStyle {
                font: FontGroup {
                    size: 22.0,
                    ..style.font
                },
                ..style
            };
            let painted = ComputedStyle {
                paints: true,
                ..style
            };
            let padded = ComputedStyle {
                edges: EdgesGroup {
                    padding: Sides::from_px(2.0),
                    ..EdgesGroup::INITIAL
                },
                ..style
            };
            let mut layout = Layout::new();
            fixture.build(
                &mut layout,
                &ComputedBlockStyle {
                    first_line: Some(&first),
                    ..ComputedBlockStyle::new(&style)
                },
                |b| {
                    b.text(NodeKey(1), text);
                    b.line_break(NodeKey(2));
                    b.text(NodeKey(3), "A");
                    b.open_box(NodeKey(4), &painted, None);
                    b.text(NodeKey(5), "V");
                    b.close_box();
                    b.line_break(NodeKey(6));
                    b.text(NodeKey(7), "A");
                    b.open_box(NodeKey(8), &padded, None);
                    b.text(NodeKey(9), "V");
                    b.close_box();
                    b.break_opportunity();
                    b.atomic(NodeKey(10), &style, None, crate::BoxSize::default());
                    b.line_break(NodeKey(11));
                    b.text(NodeKey(12), "AV fi");
                },
            );
            let input = ShapeInput {
                content: layout.content(),
                analysis: layout.analysis(),
                fonts: layout.fonts(),
                punctuation_trim: fixture.cx.config().punctuation_trim,
            };
            let mut shape = || {
                let mut out = Shaped::new();
                shape_runs(
                    &input,
                    &mut ShapeSession::new(fixture.cx.shaping(), None),
                    &mut out,
                );
                let advances = out.advances_into(Advances::new(Vec::new(), Vec::new()));
                (out, advances.into_parts())
            };
            let (fast, fast_advances) = shape();
            let (general, general_advances) = work::general_paths(shape);
            assert_eq!(fast_advances, general_advances, "{text:?}: {collapse:?}");
            assert_eq!(fast.flags, general.flags);
            for variant in [FirstLineVariant::Standard, FirstLineVariant::FirstLine] {
                let (fast, general) = (fast.text(variant), general.text(variant));
                let runs = |text: &ShapedText| {
                    text.runs
                        .iter()
                        .map(|(_, run, range)| (*run, range))
                        .collect::<Vec<_>>()
                };
                assert_eq!(
                    runs(fast),
                    runs(general),
                    "{text:?}: {collapse:?}: {variant:?}"
                );
                assert_eq!(
                    fast.glyphs.words.as_slice(),
                    general.glyphs.words.as_slice()
                );
                assert_eq!(
                    fast.glyphs.sidecar.as_slice(),
                    general.glyphs.sidecar.as_slice()
                );
                let combined = |text: &ShapedText| {
                    text.combined
                        .iter()
                        .map(|(run, fit)| (*run, fit.scale, fit.middle))
                        .collect::<Vec<_>>()
                };
                assert_eq!(combined(fast), combined(general));
            }
        }
    }
}

/// Tabs, separators, atomic inlines and a `<wbr>`'s U+200B are not shaped:
/// each group of them is a run of its own, with no glyphs, and the text
/// either side is shaped as runs of its own. A soft hyphen and a lone
/// control are shaped with their text, hidden, as Blink shapes them: the
/// LRI goes with the `f` before it, and the run ends after it only because
/// the `g` it isolates is at another level.
#[test]
fn clusters_not_shaped_are_runs_of_their_own() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = families_style(&LATIN_HEBREW);
    // So the tab is kept.
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&style),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "ab\tc");
    b.break_opportunity();
    b.text(NodeKey(2), "d\u{AD}e");
    b.atomic(NodeKey(3), &style, None, crate::BoxSize::default());
    b.text(NodeKey(4), "f\u{2066}g");
    b.line_break(NodeKey(5));
    b.text(NodeKey(6), "h");
    assert!(b.finish(&mut fixture.cx).is_complete());
    check(&mut fixture.cx, &layout);
    let starts: Vec<usize> = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .runs
        .iter()
        .map(|(_, _, clusters)| clusters.start.get())
        .collect();
    // a b | tab | c | ZWSP | d SHY e | FFFC | f LRI | g | LF | h
    assert_eq!(starts, [0, 2, 3, 4, 5, 8, 9, 11, 12, 13]);
    let shy = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .glyphs
        .word(ClusterId::new(6));
    assert_ne!(shy, GlyphWord::EMPTY, "the soft hyphen is shaped");
    assert_eq!(fixture.advances(&layout)[6], 0, "and hidden");
    let lri = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .glyphs
        .word(ClusterId::new(10));
    assert_ne!(lri, GlyphWord::EMPTY, "the isolate is shaped");
    assert_eq!(fixture.advances(&layout)[10], 0, "and hidden");
}

/// A default-ignorable the caller wrote is shaped with the text around it
/// and hidden, as Blink keeps it in its text item: `AV` kerns across U+200B,
/// U+2060, U+200E, U+FEFF, U+2061, U+034F, U+00AD and U+200D, under every
/// `white-space-collapse`, as Chrome 153 kerns `AV` in Arial across each of
/// them on the `shaping.html` probe page. A `<wbr>`'s U+200B, which Blink
/// makes a control item, parts them, as it does in Chrome.
#[test]
fn a_kern_survives_a_default_ignorable_the_caller_wrote() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = sized(&LATIN_HEBREW, 100.0);
    fixture.span(&mut layout, &style, "AV");
    let kerned = fixture.advances(&layout)[0];
    fixture.span(&mut layout, &style, "A");
    let apart = fixture.advances(&layout)[0];
    assert!(kerned < apart, "{kerned} against {apart}");
    let modes = [
        WhiteSpaceCollapse::Collapse,
        WhiteSpaceCollapse::PreserveBreaks,
        WhiteSpaceCollapse::Preserve,
        WhiteSpaceCollapse::BreakSpaces,
        WhiteSpaceCollapse::PreserveSpaces,
    ];
    for ignorable in [
        '\u{200B}', '\u{2060}', '\u{200E}', '\u{FEFF}', '\u{2061}', '\u{34F}', '\u{AD}', '\u{200D}',
    ] {
        for mode in modes {
            let mut style = style;
            style.text.white_space_collapse = mode;
            fixture.span(&mut layout, &style, &format!("A{ignorable}V"));
            assert_eq!(
                fixture.advances(&layout)[0],
                kerned,
                "U+{:04X} under {mode:?}",
                u32::from(ignorable)
            );
        }
    }
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle::new(&style),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "A");
    b.break_opportunity();
    b.text(NodeKey(1), "V");
    b.finish(&mut fixture.cx);
    check(&mut fixture.cx, &layout);
    assert_eq!(fixture.advances(&layout)[0], apart, "a <wbr> parts them");
}

/// A ZWNJ parts a kern only where its white space is kept: Chrome 153 kerns
/// `AV` in Arial across one under `normal`, `nowrap` and `pre-line`, and not
/// under `pre`, `pre-wrap` or `break-spaces`, whose ZWNJ Blink makes a
/// control item, on the `shaping.html` probe page. A ligature stops at it
/// everywhere, which is the shaper's own rule: GSUB does not skip a ZWNJ.
#[test]
fn a_zero_width_non_joiner_parts_a_kern_only_where_white_space_is_kept() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let font = latin_hebrew();
    let style = sized(&LATIN_HEBREW, 100.0);
    fixture.span(&mut layout, &style, "AV");
    let kerned = fixture.advances(&layout)[0];
    fixture.span(&mut layout, &style, "A");
    let apart = fixture.advances(&layout)[0];
    for (mode, parted) in [
        (WhiteSpaceCollapse::Collapse, false),
        (WhiteSpaceCollapse::PreserveBreaks, false),
        (WhiteSpaceCollapse::Preserve, true),
        (WhiteSpaceCollapse::BreakSpaces, true),
        (WhiteSpaceCollapse::PreserveSpaces, true),
    ] {
        let mut style = style;
        style.text.white_space_collapse = mode;
        fixture.span(&mut layout, &style, "A\u{200C}V");
        let expected = if parted { apart } else { kerned };
        assert_eq!(fixture.advances(&layout)[0], expected, "{mode:?}");
        fixture.span(&mut layout, &style, "f\u{200C}i");
        assert_eq!(ids(&layout)[1], [font.glyph('i')], "{mode:?}: no ligature");
    }
}

/// A shaping run ends where the used font changes, and where analysis says
/// shaping stops -- here, at a box with padding -- but not at a box that
/// only paints, whose text is shaped across: the kern across it survives.
#[test]
fn runs_end_at_fonts_and_shaping_breaks() {
    let mut fixture = fixture();
    let style = families_style(&LATIN_HEBREW);
    let painted = ComputedStyle {
        paints: true,
        ..style
    };
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(2.0),
            ..EdgesGroup::INITIAL
        },
        ..style
    };
    let mut whole = Layout::new();
    fixture.span(&mut whole, &style, "AVA");
    let mut layout = Layout::new();
    fixture.spans(&mut layout, &style, &[(&style, "A"), (&painted, "VA")]);
    assert_eq!(
        layout.shaped().text(FirstLineVariant::Standard).runs.len(),
        1,
        "a painting box is shaped across"
    );
    let kerned = fixture.advances(&whole);
    assert_eq!(fixture.advances(&layout), kerned, "and the kern survives");
    assert_eq!(kerned[0], (500 - 100) * 16 * 65_536 / 1000);
    fixture.spans(&mut layout, &style, &[(&style, "A"), (&padded, "VA")]);
    let starts: Vec<usize> = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .runs
        .iter()
        .map(|(_, _, clusters)| clusters.start.get())
        .collect();
    assert_eq!(starts, [0, 1], "a padded box stops shaping");
    assert_eq!(
        fixture.advances(&layout)[0],
        8 * 65_536,
        "and the kern is lost"
    );
    // A Han fallback run between Latin ones: three runs, three fonts.
    let ahem = families_style(&AHEM_FAMILY);
    fixture.span(&mut layout, &ahem, "ab漢字cd");
    let fonts: Vec<UsedFontId> = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .runs
        .iter()
        .map(|(_, r, _)| r.font)
        .collect();
    assert_eq!(fonts.len(), 3);
    assert_ne!(fonts[0], fonts[1]);
    assert_eq!(fonts[0], fonts[2]);
}

/// A `::first-letter` box is shaped across whatever its `unicode-bidi` and
/// `direction`, which Blink's cascade drops for the pseudo: Arial's `AV`
/// kern across a first letter set `isolate`, or `bidi-override` in `rtl`,
/// survives in Chrome 154, and so does one set `rtl` with padding on its
/// left, which is its start, before its letter, in its parent's direction
/// (probe `firstletteredges`, rows k1, k2 and p1).
#[test]
fn a_first_letter_is_shaped_across_whatever_its_bidi() {
    let mut fixture = fixture();
    let style = families_style(&LATIN_HEBREW);
    let mut whole = Layout::new();
    fixture.text(&mut whole, &style, "AVA");
    let kerned = fixture.advances(&whole);
    let isolated = ComputedStyle {
        bidi: BidiGroup {
            unicode_bidi: UnicodeBidi::Isolate,
            ..style.bidi
        },
        ..style
    };
    let overridden = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::BidiOverride,
        },
        ..style
    };
    let padded = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            ..style.bidi
        },
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 8.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..style
    };
    let mut layout = Layout::new();
    for letter in [isolated, overridden, padded] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
            b.set_first_letter(NodeKey(9), &letter, None);
            b.text(NodeKey(1), "AVA");
        });
        assert_eq!(
            layout.shaped().text(FirstLineVariant::Standard).runs.len(),
            1,
            "{:?}",
            letter.bidi
        );
        assert_eq!(fixture.advances(&layout), kerned, "{:?}", letter.bidi);
    }
}

/// A style boundary divides a grapheme where its later part is in a box of
/// its own. Where the parts' styles shape alike -- a bare box, or one that
/// only paints -- the grapheme is shaped whole, in one run; where a shaping
/// property differs, the parts are separate runs, as in Chrome, even where
/// they are set in the same font.
#[test]
fn a_divided_grapheme_is_split_only_by_a_shaping_property() {
    let mut fixture = fixture();
    let style = families_style(&LATIN_HEBREW);
    let painted = ComputedStyle {
        paints: true,
        ..style
    };
    let larger = sized(&LATIN_HEBREW, 20.0);
    let bolder = ComputedStyle {
        font: FontGroup {
            weight: FontWeight::BOLD,
            ..style.font
        },
        ..style
    };
    let spaced = ComputedStyle {
        text: TextGroup {
            letter_spacing: 1.0,
            ..style.text
        },
        ..style
    };
    let mut layout = Layout::new();
    for (other, splits) in [
        (&style, false),
        (&painted, false),
        (&larger, true),
        (&bolder, true),
        (&spaced, true),
    ] {
        fixture.spans(&mut layout, &style, &[(&style, "xe"), (other, "\u{301}y")]);
        let clusters = &layout.analysis().clusters;
        assert!(
            clusters.is_continuation(ClusterId::new(2)),
            "the mark continues its grapheme"
        );
        let runs = &layout.shaped().text(FirstLineVariant::Standard).runs;
        let mark_run = layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .runs
            .containing(ClusterId::new(2))
            .unwrap();
        let base_run = layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .runs
            .containing(ClusterId::new(1))
            .unwrap();
        assert_eq!(mark_run != base_run, splits, "{:?}", other.font.weight);
        if !splits {
            assert_eq!(runs.len(), 1);
        }
    }
    // A heavier weight the font reaches without faking bold resolves to
    // the same font at the same size, so the runs are in one used font and
    // still apart. (Bold would be faux bold here, an instance of its own.)
    let heavier = ComputedStyle {
        font: FontGroup {
            weight: FontWeight::new(500.0),
            ..style.font
        },
        ..style
    };
    fixture.spans(
        &mut layout,
        &style,
        &[(&style, "xe"), (&heavier, "\u{301}y")],
    );
    let fonts: Vec<UsedFontId> = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .runs
        .iter()
        .map(|(_, r, _)| r.font)
        .collect();
    assert_eq!(fonts.len(), 2);
    assert_eq!(fonts[0], fonts[1]);
}

/// Arabic joins across a shaping-run split: letters either side of a box
/// whose letter-spacing ends the run keep their joining forms, as the
/// context either side carries the paragraph's letters, as Chrome gives
/// HarfBuzz the whole text. Across a box that only paints there is no split
/// at all. In a right-to-left paragraph, where the Arabic is at the
/// paragraph's own level.
#[test]
fn arabic_joins_across_a_run_split() {
    let mut fixture = fixture();
    let font = arabic();
    let style = families_style(&ARABIC);
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let painted = ComputedStyle {
        paints: true,
        ..style
    };
    let spaced = ComputedStyle {
        text: TextGroup {
            letter_spacing: 2.0,
            ..style.text
        },
        ..style
    };
    let word: String = [BEH, TEH, SEEN, MEEM].iter().collect();
    let joined = vec![
        vec![font.form_glyph(BEH, Form::Initial)],
        vec![font.form_glyph(TEH, Form::Medial)],
        vec![font.form_glyph(SEEN, Form::Medial)],
        vec![font.form_glyph(MEEM, Form::Final)],
    ];
    let mut layout = Layout::new();
    fixture.build_spans(&mut layout, &rtl, &[(&style, &word)]);
    assert_eq!(ids(&layout), joined);
    let (head, tail): (String, String) = (
        word.chars().take(2).collect(),
        word.chars().skip(2).collect(),
    );
    fixture.build_spans(&mut layout, &rtl, &[(&style, &head), (&painted, &tail)]);
    assert_eq!(
        layout.shaped().text(FirstLineVariant::Standard).runs.len(),
        1,
        "a painting box does not split"
    );
    assert_eq!(ids(&layout), joined);
    fixture.build_spans(&mut layout, &rtl, &[(&style, &head), (&spaced, &tail)]);
    assert_eq!(
        layout.shaped().text(FirstLineVariant::Standard).runs.len(),
        2,
        "letter-spacing splits"
    );
    assert_eq!(ids(&layout), joined, "and the letters still join");
    // Without the context the split would break the join: each side alone
    // takes its isolated, initial or final forms.
    fixture.build_spans(&mut layout, &rtl, &[(&style, &head)]);
    assert_eq!(
        ids(&layout),
        [
            vec![font.form_glyph(BEH, Form::Initial)],
            vec![font.form_glyph(TEH, Form::Final)]
        ]
    );
}

/// The content language reaches the plan: Turkish text takes the font's
/// Turkish `i`, and English its own.
#[test]
fn the_language_reaches_the_plan() {
    let mut fixture = fixture();
    let font = latin_hebrew();
    let mut layout = Layout::new();
    for (language, glyph) in [
        ("tr", font.localized_glyph()),
        ("en", font.glyph('i')),
        ("und", font.glyph('i')),
    ] {
        let mut style = families_style(&LATIN_HEBREW);
        style.text.language = Language::parse(language).ok();
        fixture.span(&mut layout, &style, "xi");
        assert_eq!(ids(&layout)[1], [glyph], "{language}");
    }
}

/// Text whose style names no language is planned for the config's default,
/// as Chrome shapes in its default locale (`LocaleOrDefault`): under a
/// Turkish default it takes the font's Turkish `i`, text that says it is
/// English keeps its own, and under `und` nothing is localized. A line edge
/// reshaped later is planned for the default the build took, whatever the
/// config says by then, so it cannot disagree with the paragraph.
#[test]
fn text_naming_no_language_is_planned_for_the_default() {
    let mut fixture = fixture();
    let font = latin_hebrew();
    let turkish = Language::parse("tr").expect("a tag");
    let mut layout = Layout::new();
    for (default, own, glyph) in [
        (turkish, None, font.localized_glyph()),
        (turkish, Language::parse("en").ok(), font.glyph('i')),
        (Language::UND, None, font.glyph('i')),
    ] {
        let mut config = Config::chrome_windows();
        config.default_language = default;
        fixture.cx.set_config(config);
        let mut style = families_style(&LATIN_HEBREW);
        style.text.language = own;
        fixture.span(&mut layout, &style, "xi");
        assert_eq!(ids(&layout)[1], [glyph], "{default:?} {own:?}");
    }
    // Built under the Turkish default, then reshaped under `und`.
    let mut config = Config::chrome_windows();
    config.default_language = turkish;
    fixture.cx.set_config(config);
    fixture.span(&mut layout, &families_style(&LATIN_HEBREW), "xi");
    config.default_language = Language::UND;
    fixture.cx.set_config(config);
    let piece = fixture.reshape(
        &layout,
        ShapedRunId::new(0),
        ClusterId::new(1)..ClusterId::new(2),
    );
    let reshaped: Vec<u32> = drawn(piece.words[ClusterId::new(0)], &piece.sidecar, 0)
        .iter()
        .map(|g| g.0)
        .collect();
    assert_eq!(reshaped, [font.localized_glyph()]);
}

/// `font-language-override` names the language system itself, whatever the
/// content language: English text overridden to `TRK ` takes the Turkish
/// `i`, and a run ends where the override changes, since it shapes
/// differently.
#[test]
fn font_language_override_names_the_system() {
    let mut fixture = fixture();
    let font = latin_hebrew();
    let mut english = families_style(&LATIN_HEBREW);
    english.text.language = Language::parse("en").ok();
    let turkish = ComputedStyle {
        font: FontGroup {
            language_override: FontLanguageOverride::System(Tag::new(b"TRK ")),
            ..english.font
        },
        ..english
    };
    let mut layout = Layout::new();
    fixture.spans(&mut layout, &english, &[(&english, "xi"), (&turkish, "xi")]);
    let ids = ids(&layout);
    assert_eq!(ids[1], [font.glyph('i')]);
    assert_eq!(ids[3], [font.localized_glyph()]);
    let starts: Vec<usize> = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .runs
        .iter()
        .map(|(_, _, clusters)| clusters.start.get())
        .collect();
    assert_eq!(starts, [0, 2]);
}

/// `font-language-override` keeps the tag's case: `trk ` is not the Turkish
/// system `TRK `, so it keeps the font's own `i`.
#[test]
fn font_language_override_keeps_the_tag_case() {
    let mut fixture = fixture();
    let font = latin_hebrew();
    let mut layout = Layout::new();
    for (tag, glyph) in [
        (b"trk ", font.glyph('i')),
        (b"TRK ", font.localized_glyph()),
    ] {
        let style = families_style(&LATIN_HEBREW);
        let style = ComputedStyle {
            font: FontGroup {
                language_override: FontLanguageOverride::System(Tag::new(tag)),
                ..style.font
            },
            ..style
        };
        fixture.span(&mut layout, &style, "xi");
        assert_eq!(ids(&layout)[1], [glyph], "{tag:?}");
    }
}
