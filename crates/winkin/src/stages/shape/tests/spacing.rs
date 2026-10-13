//! Tests of `text-spacing-trim` in shaping:
//! - a mark gives its blank back through the font's `halt`, or is halved
//!   where the font has none and the config says so;
//! - a pair is found across runs, and across fallback fonts;
//! - a paragraph's opening mark trims under `trim-start`;
//! - a font with `chws` collapses its own pairs;
//! - a font's punctuation classes are read off its glyphs, and the marks
//!   are found as Blink finds them.

use super::*;
use crate::style::FirstLineVariant;

/// A fixture over a Japanese font with `halt` and one without.
fn punct() -> Fixture {
    fixture_with(&[
        TestFont::cjk("Test Punct", true),
        TestFont::cjk("Test Punct Plain", false),
    ])
}

/// Where two full-width marks meet, the one that gives its blank back is
/// set in the font's own `halt`, one character at a time, as Chrome's
/// `HanKerning` sets it: the second of two opening marks, whose blank is
/// before its ink, drawn half an em back; the first of two closing marks;
/// the opening mark after a closing one. Breaking inside the pair is unsafe,
/// so a line starting there is shaped as a start.
#[test]
fn a_mark_gives_its_blank_back_in_the_fonts_halt() {
    let mut fixture = punct();
    let mut layout = Layout::new();
    let style = sized(&PUNCT, 20.0);
    for (text, halved) in [
        ("漢「「漢", 2),
        ("漢」」漢", 1),
        ("漢」「漢", 2),
        ("漢。「漢", 2),
        ("漢・「漢", 2),
        ("漢（「漢", 2),
    ] {
        fixture.span(&mut layout, &style, text);
        let mut expected = vec![px16(20); 4];
        expected[halved] = px16(10);
        assert_eq!(fixture.advances(), expected, "{text}");
        let shaped = layout.shaped().text(FirstLineVariant::Standard);
        // Breaking between the two marks is unsafe.
        assert!(
            shaped.glyphs.word(ClusterId::new(2)).is_unsafe_to_break(),
            "{text}"
        );
        assert!(
            !shaped.glyphs.word(ClusterId::new(1)).is_unsafe_to_break(),
            "{text}"
        );
    }
    // An opening mark is drawn half an em back into the half it keeps.
    fixture.span(&mut layout, &style, "漢「「漢");
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let second = drawn(
        shaped.glyphs.word(ClusterId::new(2)),
        &shaped.glyphs.sidecar,
        0,
    );
    assert_eq!((second[0].1, second[0].3), (-px16(10), px16(10)));
    // A mark beside ideographs keeps its blank.
    fixture.span(&mut layout, &style, "漢「漢」漢");
    assert_eq!(fixture.advances(), vec![px16(20); 5]);
}

/// Nothing is trimmed under `space-all`; nor, as in Chrome, in a font
/// without `halt`, unless `Config::punctuation_trim` says `Always`, beyond
/// Chrome, which halves the mark's own advance instead and moves an opening
/// mark's glyph back into the half it keeps.
#[test]
fn a_font_without_halt_trims_nothing_but_where_the_config_says() {
    let mut fixture = punct();
    let mut layout = Layout::new();
    let mut spaced = sized(&PUNCT, 20.0);
    spaced.text.spacing_trim = TextSpacingTrim::SpaceAll;
    fixture.span(&mut layout, &spaced, "漢「「漢");
    assert_eq!(fixture.advances(), vec![px16(20); 4]);
    let plain = sized(&PLAIN_PUNCT, 20.0);
    fixture.span(&mut layout, &plain, "漢「「漢");
    assert_eq!(fixture.advances(), vec![px16(20); 4]);
    assert!(
        !layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .glyphs
            .word(ClusterId::new(2))
            .is_unsafe_to_break()
    );
    let mut config = *fixture.cx.config();
    config.punctuation_trim = PunctuationTrim::Always;
    fixture.cx.set_config(config);
    fixture.span(&mut layout, &plain, "漢「「漢");
    assert!(
        layout
            .shaped()
            .flags
            .contains(ShapedFlags::HALVES_PUNCTUATION)
    );
    assert_eq!(fixture.advances(), [px16(20), px16(20), px16(10), px16(20)]);
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let second = drawn(
        shaped.glyphs.word(ClusterId::new(2)),
        &shaped.glyphs.sidecar,
        0,
    );
    assert_eq!((second[0].1, second[0].3), (-px16(10), px16(10)));
    fixture.span(&mut layout, &plain, "漢」」漢");
    assert_eq!(fixture.advances(), [px16(20), px16(10), px16(20), px16(20)]);
    // A curly quote, half an em in a Japanese font, is a narrow opening
    // mark, and the full-width one after it gives its blank back.
    fixture.span(&mut layout, &plain, "漢“「漢");
    assert_eq!(fixture.advances(), [px16(20), px16(10), px16(10), px16(20)]);
}

/// A pair is found across a style boundary that stops shaping, the text
/// either side read: the first of two closing marks gives its blank back
/// though the second is in another run, as in Chrome, and a run's first
/// cluster kerned against the run before is unsafe to break before but a
/// reshape may still start there.
#[test]
fn a_pair_is_found_across_runs() {
    let mut fixture = punct();
    let mut layout = Layout::new();
    let style = sized(&PUNCT, 20.0);
    let small = sized(&PUNCT, 10.0);
    fixture.spans(&mut layout, &style, &[(&style, "漢」"), (&small, "」漢")]);
    assert_eq!(fixture.advances(), [px16(20), px16(10), px16(10), px16(10)]);
    fixture.spans(&mut layout, &style, &[(&style, "漢」"), (&small, "「漢")]);
    assert_eq!(fixture.advances(), [px16(20), px16(20), px16(5), px16(10)]);
    // The pair makes the second run's first cluster unsafe to break
    // before, though the runs are shaped apart: a line starting there is
    // reshaped, and a line end's window, which stops at its run's start,
    // does not reach past it.
    let word = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .glyphs
        .word(ClusterId::new(2));
    assert!(word.is_unsafe_to_break());
}

/// Under `trim-start` a paragraph's opening mark gives its blank back at
/// its start, as Chrome's `ShapeText` trims every paragraph's first text
/// item; not under `normal` or `space-first`, which keep it there.
#[test]
fn trim_start_trims_a_paragraphs_opening_mark() {
    let mut fixture = punct();
    let mut layout = Layout::new();
    let mut style = sized(&PUNCT, 20.0);
    for (trim, first) in [
        (TextSpacingTrim::Normal, 20),
        (TextSpacingTrim::SpaceFirst, 20),
        (TextSpacingTrim::TrimStart, 10),
    ] {
        style.text.spacing_trim = trim;
        fixture.span(&mut layout, &style, "「漢");
        assert_eq!(fixture.advances(), [px16(first), px16(20)], "{trim:?}");
    }
}

/// Where the font has `chws`, it collapses the pairs inside a run itself,
/// which every style but `space-all` turns on, and the rule finds none
/// there: a mark gives its blank back once.
#[test]
fn a_font_with_chws_collapses_its_own_pairs() {
    let mut font = TestFont::cjk("Test Punct Contextual", true);
    font.chws = vec![('\u{300D}', '\u{300D}', -500)];
    let family = [FontFamilyName::named("Test Punct Contextual")];
    let mut fixture = fixture_with(&[font]);
    let mut layout = Layout::new();
    let style = sized(&family, 20.0);
    fixture.span(&mut layout, &style, "漢」」漢");
    assert_eq!(fixture.advances(), [px16(20), px16(10), px16(20), px16(20)]);
}

/// A pair in two fallback fonts is trimmed as in one font: a neighbour is
/// classed in the font it is shaped in, as Chrome's `HanKerning` caches its
/// class. Each subset font has `halt` and one mark, so it cannot class the
/// other's mark itself: a dot before a colon gives its blank back, and so
/// does an opening bracket after a dot, whichever font comes first.
#[test]
fn a_pair_in_two_fallback_fonts_is_trimmed() {
    let subset = |family: &str, ch: char| {
        let mut font = TestFont::cjk(family, true);
        font.ranges = vec![(u32::from(ch), u32::from(ch))];
        font.advances.retain(|&(held, _)| held == ch);
        font.halt.retain(|&(held, _, _)| held == ch);
        font
    };
    let mut fixture = fixture_with(&[
        TestFont::cjk("Test Punct", true),
        subset("Test Punct Dot", '\u{3002}'),
        subset("Test Punct Colon", '\u{FF1A}'),
        subset("Test Punct Bracket", '\u{300C}'),
    ]);
    const fn named(family: &'static str) -> FontFamilyName<'static> {
        FontFamilyName::Named(Cow::Borrowed(family))
    }
    const DOT: FontFamilyName<'static> = named("Test Punct Dot");
    const COLON: FontFamilyName<'static> = named("Test Punct Colon");
    const BRACKET: FontFamilyName<'static> = named("Test Punct Bracket");
    const ALL: FontFamilyName<'static> = named("Test Punct");
    const COLON_DOT: [FontFamilyName<'static>; 3] = [COLON, DOT, ALL];
    const DOT_COLON: [FontFamilyName<'static>; 3] = [DOT, COLON, ALL];
    const DOT_BRACKET: [FontFamilyName<'static>; 3] = [DOT, BRACKET, ALL];
    const BRACKET_DOT: [FontFamilyName<'static>; 3] = [BRACKET, DOT, ALL];
    let mut layout = Layout::new();
    for (families, text, trimmed) in [
        (&PUNCT[..], "漢。：漢", 1),
        (&COLON_DOT, "漢。：漢", 1),
        (&DOT_COLON, "漢。：漢", 1),
        (&PUNCT, "漢。「漢", 2),
        (&DOT_BRACKET, "漢。「漢", 2),
        (&BRACKET_DOT, "漢。「漢", 2),
    ] {
        let style = sized(families, 20.0);
        fixture.span(&mut layout, &style, text);
        let mut expected = vec![px16(20); 4];
        expected[trimmed] = px16(10);
        assert_eq!(fixture.advances(), expected, "{families:?} {text}");
    }
}

/// A probe drawn with its ink from `left` to `right` in an advance of 1000.
fn probe(left: f32, right: f32) -> super::trim::TrimProbe {
    super::trim::TrimProbe {
        drawn: true,
        advance: 1000.0,
        left,
        right,
    }
}

/// A font's classes are read off its glyphs as Blink's `FontData` reads
/// them: ink in the left half is a closing mark's, in the right half an
/// opening one's, within the middle half a middle one's; a group whose
/// glyphs differ in advance or class is none; a glyph the font lacks is
/// passed over; and a font whose probes could not be shaped one glyph each
/// cannot be read, and its `halt` goes unused.
#[test]
fn a_fonts_punctuation_classes_are_read_off_its_glyphs() {
    use super::trim::{TrimClass, TrimFont, TrimProbe};
    use crate::unicode::TextSpacingClass;
    let left = probe(100.0, 400.0);
    let right = probe(600.0, 900.0);
    let middle = probe(400.0, 600.0);
    // Japanese: dots and colons in the left half, quotes proportional,
    // whose ink is in the middle of an advance of their own.
    let japanese = [
        left,
        left,
        left,
        left,
        middle,
        middle,
        probe(200.0, 450.0),
        probe(200.0, 450.0),
        probe(50.0, 300.0),
        probe(50.0, 300.0),
    ];
    let font = TrimFont::from_probes(true, false, Some(&japanese));
    assert!(
        font.has_halt()
            && !font.has_chws()
            && font.class(TextSpacingClass::OpenQuote) != TrimClass::Open
    );
    assert_eq!(
        (
            font.class(TextSpacingClass::Dot),
            font.class(TextSpacingClass::Colon),
            font.class(TextSpacingClass::Semicolon)
        ),
        (TrimClass::Close, TrimClass::Middle, TrimClass::Middle)
    );
    assert_eq!(
        font.class(TextSpacingClass::OpenQuote),
        TrimClass::OpenNarrow
    );
    // Traditional Chinese: dots in the middle, quotes full width.
    let chinese = [
        middle, middle, middle, middle, middle, middle, right, right, left, left,
    ];
    let font = TrimFont::from_probes(true, true, Some(&chinese));
    assert!(font.class(TextSpacingClass::OpenQuote) == TrimClass::Open && font.has_chws());
    assert_eq!(font.class(TextSpacingClass::Dot), TrimClass::Middle);
    assert_eq!(font.class(TextSpacingClass::CloseQuote), TrimClass::Close);
    // Dots that disagree are none; one the font lacks is passed over.
    let mut mixed = japanese;
    mixed[1] = right;
    assert_eq!(
        TrimFont::from_probes(true, false, Some(&mixed)).class(TextSpacingClass::Dot),
        TrimClass::Other
    );
    let mut wider = japanese;
    wider[2].advance = 1200.0;
    assert_eq!(
        TrimFont::from_probes(true, false, Some(&wider)).class(TextSpacingClass::Dot),
        TrimClass::Other
    );
    let mut missing = japanese;
    missing[0] = TrimProbe::default();
    assert_eq!(
        TrimFont::from_probes(true, false, Some(&missing)).class(TextSpacingClass::Dot),
        TrimClass::Close
    );
    // Probes that could not be read: the font's `halt` goes unused.
    assert!(!TrimFont::from_probes(true, false, None).has_halt());
}

/// The marks of a range, as Blink's `HanKerning::AppendFontFeatures`
/// finds them: pairs inside it, where the font has no `chws`; its first
/// character against the one before it, unless it starts a line; its last
/// against the one after; and at a line's edges the marks the edges ask to
/// be trimmed whatever is beside them.
#[test]
fn the_marks_are_found_as_blink_finds_them() {
    use super::trim::{TrimFont, TrimMarks, TrimProbe, class_beside};
    // A font with `halt` and no classes of its own: none of its probes is
    // drawn.
    let undrawn = [TrimProbe::default(); 10];
    let font = TrimFont::from_probes(true, false, Some(&undrawn));
    let mut found = TrimMarks::default();
    let mut marks = |chars: &str,
                     before: Option<char>,
                     after: Option<char>,
                     font: &TrimFont,
                     edges: ShapingEdges| {
        found.clear();
        let beside = |ch: Option<char>| ch.map(|ch| class_beside(ch, font, || None));
        found.find(chars, 100, beside(before), beside(after), font, edges);
        (
            found
                .marks()
                .iter()
                .map(|m| m.start() - 100)
                .collect::<Vec<u32>>(),
            found
                .unsafe_before()
                .iter()
                .map(|at| at - 100)
                .collect::<Vec<u32>>(),
        )
    };
    // 漢「「漢: the second opening mark, byte 6; unsafe before it.
    assert_eq!(
        marks("漢「「漢", None, None, &font, ShapingEdges::default()),
        (vec![6], vec![6])
    );
    // 」」: the first; unsafe before the second.
    assert_eq!(
        marks("」」", None, None, &font, ShapingEdges::default()),
        (vec![0], vec![3])
    );
    // Against the text either side.
    assert_eq!(
        marks("「漢", Some('」'), None, &font, ShapingEdges::default()),
        (vec![0], vec![0])
    );
    assert_eq!(
        marks("漢」", None, Some('」'), &font, ShapingEdges::default()),
        (vec![3], vec![])
    );
    // A line's start takes nothing from before it.
    let start = ShapingEdges {
        line_start: true,
        ..ShapingEdges::default()
    };
    assert_eq!(
        marks("「漢", Some('」'), None, &font, start),
        (vec![], vec![])
    );
    // The edges asked for: the first and the last, whatever they meet.
    let both = ShapingEdges {
        line_start: true,
        trim_start: true,
        trim_end: true,
    };
    assert_eq!(
        marks("「漢」", None, None, &font, both),
        (vec![0, 6], vec![0])
    );
    // With `chws` only the run's edges are found.
    let contextual = TrimFont::from_probes(true, true, Some(&undrawn));
    assert_eq!(
        marks("」」漢", None, None, &contextual, ShapingEdges::default()),
        (vec![], vec![])
    );
    assert_eq!(
        marks(
            "漢」",
            None,
            Some('」'),
            &contextual,
            ShapingEdges::default()
        ),
        (vec![3], vec![])
    );
}
