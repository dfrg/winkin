//! Shaping boundary and script run tests. They pin:
//! - where shaping stops: edges with room, shifts, bidi, ruby marks and a zero
//!   width non-joiner;
//! - runs split by script and language, with common and inherited characters
//!   taking their neighbours' script.

use super::*;
use crate::style::LengthPercentage;

/// Shaping stops at a box edge with margin, border or padding on that inline
/// side, `vertical-align` other than `baseline`, or any `unicode-bidi`. A box
/// that only paints is shaped across.
#[test]
fn shaping_stops_at_edges_with_room_or_a_shift_or_bidi() {
    let stops = |style: &ComputedStyle<'_>, block: &ComputedBlockStyle<'_>| {
        let layout = build_with(block, |b| {
            b.text(key(1), "ab");
            b.open_box(key(2), style, None);
            b.text(key(3), "cd");
            b.close_box();
            b.text(key(4), "ef");
        });
        ends_with(&layout, ClusterAttrs::SHAPE_BREAK_AFTER)
    };
    let initial = ComputedStyle::initial();
    let horizontal = ComputedBlockStyle::new(&initial);
    let vertical = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..horizontal
    };
    let left = styled(|s| s.edges.padding.left = LengthPercentage::from_px(2.0));
    let right_margin = styled(|s| s.edges.margin.right = LengthPercentage::from_px(2.0));
    let top_border = styled(|s| s.edges.border.top = 1.0);
    let rtl_left = styled(|s| {
        s.edges.padding.left = LengthPercentage::from_px(2.0);
        s.bidi.direction = Direction::Rtl;
    });
    assert_eq!(stops(&left, &horizontal), [2], "the start side, before");
    assert_eq!(
        stops(&right_margin, &horizontal),
        [4],
        "the end side, after"
    );
    assert_eq!(stops(&rtl_left, &horizontal), [4], "left is the end in rtl");
    assert_eq!(
        stops(&top_border, &horizontal),
        [] as [usize; 0],
        "not an inline side"
    );
    assert_eq!(
        stops(&top_border, &vertical),
        [2],
        "the start side in vertical"
    );
    assert_eq!(stops(&left, &vertical), [] as [usize; 0]);
    let shifted = styled(|s| s.line.vertical_align = VerticalAlign::Super);
    assert_eq!(stops(&shifted, &horizontal), [2, 4]);
    let isolated = styled(|s| s.bidi.unicode_bidi = UnicodeBidi::Isolate);
    assert_eq!(stops(&isolated, &horizontal), [2, 4]);
    let painted = styled(|s| {
        s.paints = true;
        s.font.weight = FontWeight::BOLD;
    });
    assert_eq!(stops(&painted, &horizontal), [] as [usize; 0]);
}

#[test]
fn shaping_stops_at_ruby_marks_and_a_zero_width_non_joiner() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.open_ruby(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "b");
        b.open_annotation(key(4), &ComputedStyle::initial(), None);
        b.text(key(5), "c");
        b.close_annotation();
        b.close_ruby();
        b.text(key(6), "d");
    });
    assert_eq!(
        ends_with(&layout, ClusterAttrs::SHAPE_BREAK_AFTER),
        [1, 2, 3]
    );
    let layout = pre("a\u{200C}b");
    assert_eq!(clusters(&layout), ["a\u{200C}", "b"]);
    assert_eq!(ends_with(&layout, ClusterAttrs::SHAPE_BREAK_AFTER), [4]);
}

/// A ZWNJ stops shaping where its white space is preserved. Blink makes it a
/// control item under `preserve` and `break-spaces` alone, and the analysis
/// does the same under `preserve-spaces`. Where white space collapses, the
/// shaper runs past it. The style of the ZWNJ's own item decides.
#[test]
fn a_zero_width_non_joiner_stops_shaping_only_where_white_space_is_kept() {
    use WhiteSpaceCollapse as W;
    for (mode, stops) in [
        (W::Collapse, false),
        (W::PreserveBreaks, false),
        (W::Discard, false),
        (W::Preserve, true),
        (W::BreakSpaces, true),
        (W::PreserveSpaces, true),
    ] {
        let layout = root_text(&white_space(mode), "a\u{200C}b");
        let expected: &[usize] = if stops { &[4] } else { &[] };
        assert_eq!(
            ends_with(&layout, ClusterAttrs::SHAPE_BREAK_AFTER),
            expected,
            "{mode:?}"
        );
    }
    // A lone one, after a box edge, is a cluster of its own, and its item's
    // style is the one read.
    let layout = pieces(&[
        ("a", &ComputedStyle::initial()),
        ("\u{200C}b", &white_space(W::Preserve)),
    ]);
    assert_eq!(clusters(&layout), ["a", "\u{200C}", "b"]);
    assert_eq!(ends_with(&layout, ClusterAttrs::SHAPE_BREAK_AFTER), [4]);
}

// Runs ---------------------------------------------------------------------

fn latn(text: &str) -> (&str, String) {
    (text, String::from("Latn"))
}

fn hani(text: &str) -> (&str, String) {
    (text, String::from("Hani"))
}

#[test]
fn runs_split_by_script() {
    assert_eq!(
        scripts(&plain("abc 日本語 def")),
        [latn("abc "), hani("日本語 "), latn("def")]
    );
}

/// Common characters take the script before them, or the one after at a
/// run's start, in one look-ahead.
#[test]
fn common_takes_the_script_around_it() {
    assert_eq!(scripts(&plain("123 abc")), [latn("123 abc")]);
    assert_eq!(scripts(&plain("abc 123")), [latn("abc 123")]);
    assert_eq!(scripts(&plain("!?")), [("!?", String::from("Zyyy"))]);
    // Inherited marks alone at the start take what follows too.
    assert_eq!(scripts(&plain("\u{301}abc")), [latn("\u{301}abc")]);
}

/// Script_Extensions narrow a shared character toward its neighbour. One
/// that fits neither side starts a run of its own.
#[test]
fn extensions_narrow_toward_the_neighbour() {
    // U+3001 may be Han, Hiragana, Katakana and others: it stays with Han.
    assert_eq!(scripts(&plain("日本、abc")), [hani("日本、"), latn("abc")]);
    // It cannot be Latin, so it takes the Han after it.
    assert_eq!(scripts(&plain("abc、日本")), [latn("abc"), hani("、日本")]);
    // Arabic-Indic digits are Arabic or Thaana, never Latin.
    assert_eq!(
        scripts(&plain("abc \u{661}\u{662}")),
        [latn("abc "), ("\u{661}\u{662}", String::from("Arab"))]
    );
}

/// A closing bracket takes its opener's script, as Blink's
/// `ScriptRunIterator` does.
#[test]
fn a_bracket_takes_its_openers_script() {
    assert_eq!(
        scripts(&plain("abc (日本) def")),
        [latn("abc ("), hani("日本"), latn(") def")]
    );
    assert_eq!(
        scripts(&plain("日本「abc」です")),
        [
            hani("日本「"),
            latn("abc"),
            hani("」"),
            (("です"), String::from("Hira"))
        ]
    );
    // Nested past what is remembered, without panicking.
    let deep = format!("{}日本{}", "(".repeat(40), ")".repeat(40));
    let layout = plain(&deep);
    assert!(!runs(&layout).is_empty());
}

#[test]
fn runs_split_by_language() {
    let en = language("en");
    let ja = language("ja");
    // The space after the Japanese is Han, as the script before it, and
    // English, as its node, as Blink's items would have it.
    let layout = pieces(&[("abc ", &en), ("日本", &ja), (" def", &en)]);
    assert_eq!(
        runs(&layout),
        [
            ("abc ", String::from("Latn"), String::from("en")),
            ("日本", String::from("Hani"), String::from("ja")),
            (" ", String::from("Hani"), String::from("en")),
            ("def", String::from("Latn"), String::from("en")),
        ]
    );
    // A Common run takes the script that follows it across a language seam,
    // as Blink's script runs cross items.
    let layout = pieces(&[("\"", &en), ("日本", &ja)]);
    assert_eq!(
        runs(&layout),
        [
            ("\"", String::from("Hani"), String::from("en")),
            ("日本", String::from("Hani"), String::from("ja")),
        ]
    );
}

/// Runs never cross a paragraph, but a script carries across one, as in
/// Blink: digits after Han are Han, which is what font fallback asks for.
#[test]
fn runs_stop_at_paragraphs_and_scripts_carry_over() {
    assert_eq!(scripts(&pre("abc\ndef")), [latn("abc\n"), latn("def")]);
    assert_eq!(scripts(&pre("日本\n123")), [hani("日本\n"), hani("123")]);
    assert_eq!(scripts(&pre("123\n日本")), [hani("123\n"), hani("日本")]);
}
