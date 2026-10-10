//! `text-autospace` tests. They pin:
//! - an eighth of the ideographic advance at each seam;
//! - the item a seam is sized by, and both sides having to take it;
//! - seams across box edges and a first letter, and not past controls or
//!   floats;
//! - conditional punctuation, narrow only in Chinese.

use super::*;
use crate::style::LengthPercentage;

/// A fixture over Ahem, a Japanese font whose `水` is a whole em, and an
/// ASCII font with no `水`, which Han falls back from to the first.
fn cjk() -> Fixture {
    Fixture::new(
        &[
            TestFont::cjk("Test Punct", true),
            TestFont::new("Test ASCII", &[(0x20, 0x7E)]),
        ],
        han_fallback("Test Punct"),
        StageCheck::Built(|_, layout| check(layout)),
    )
}

/// `style` with `text-autospace` as said.
fn spaced(style: &ComputedStyle<'static>, alpha: bool, numeric: bool) -> ComputedStyle<'static> {
    let mut style = *style;
    style.text.autospace = TextAutospace {
        ideograph_alpha: alpha,
        ideograph_numeric: numeric,
    };
    style
}

/// The room `text-autospace` put after each cluster of `spans`, each an
/// inline box of its style holding its text, in a block of `block` whose
/// own style is `root`: each cluster's step against the same spans set in
/// `no-autospace`, in pixels.
fn gaps(
    fixture: &mut Fixture,
    block: &ComputedBlockStyle,
    root: &ComputedStyle<'static>,
    spans: &[(&ComputedStyle<'static>, &str)],
) -> Vec<f32> {
    let steps = |fixture: &mut Fixture, off: bool| {
        let mut layout = Layout::new();
        let plain = |style: &ComputedStyle<'static>| {
            if off {
                spaced(style, false, false)
            } else {
                *style
            }
        };
        let root = plain(root);
        let styles: Vec<ComputedStyle<'static>> = spans.iter().map(|(s, _)| plain(s)).collect();
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &root,
                ..*block
            },
            |b| {
                for (at, (style, (_, text))) in (0u64..).zip(styles.iter().zip(spans)) {
                    b.open_box(NodeKey(2 * at + 1), style, None);
                    b.text(NodeKey(2 * at + 2), text);
                    b.close_box();
                }
            },
        );
        let p = positions(&layout);
        p.windows(2).map(|w| w[1] - w[0]).collect::<Vec<f32>>()
    };
    let on = steps(fixture, false);
    let off = steps(fixture, true);
    on.iter().zip(&off).map(|(a, b)| a - b).collect()
}

/// An eighth of the primary font's ideographic advance, `水`'s, goes after
/// the cluster before each seam where an ideograph meets a letter or a
/// digit, either way round, as Chrome's `TextAutoSpace` puts it: `漢字ABC漢字`
/// at 20px in a font whose `水` is an em has two seams of 2.5px. Where the
/// primary font has no `水`, an eighth of its size.
#[test]
fn autospace_puts_an_eighth_of_the_ideographic_advance_at_each_seam() {
    let mut fixture = cjk();
    let style = spaced(&sized(&PUNCT, 20.0), true, true);
    assert_eq!(
        gaps(
            &mut fixture,
            &ComputedBlockStyle::default(),
            &style,
            &[(&style, "漢字ABC漢字")]
        ),
        [0.0, 2.5, 0.0, 0.0, 2.5, 0.0, 0.0]
    );
    let ascii = spaced(&sized(&ASCII_ONLY, 24.0), true, true);
    assert_eq!(
        gaps(
            &mut fixture,
            &ComputedBlockStyle::default(),
            &ascii,
            &[(&ascii, "漢A")]
        ),
        [3.0, 0.0]
    );
    // Hangul is an ideograph to it, a digit is its own seam, and a full
    // width letter, `、` and a U+200B are none.
    for (text, expected) in [
        ("한A", vec![2.5, 0.0]),
        ("漢1", vec![2.5, 0.0]),
        ("漢Ａ", vec![0.0, 0.0]),
        ("漢、A", vec![0.0, 0.0, 0.0]),
        ("漢\u{200B}A", vec![0.0, 0.0, 0.0]),
    ] {
        assert_eq!(
            gaps(
                &mut fixture,
                &ComputedBlockStyle::default(),
                &style,
                &[(&style, text)]
            ),
            expected,
            "{text}"
        );
    }
}

/// A seam across items takes the room of the earlier item's font where it
/// reads left to right, and the later's where it reads right to left, as
/// Chrome writes it into the one or the other: `<span 20px>漢</span>A` at
/// 40px takes 2.5, `A<span 20px>漢</span>` 5.
#[test]
fn a_seam_across_items_is_sized_by_the_item_it_is_written_into() {
    let mut fixture = cjk();
    let small = spaced(&sized(&PUNCT, 20.0), true, true);
    let large = spaced(&sized(&PUNCT, 40.0), true, true);
    let block = ComputedBlockStyle::default();
    assert_eq!(
        gaps(
            &mut fixture,
            &block,
            &large,
            &[(&small, "漢"), (&large, "A")]
        ),
        [2.5, 0.0]
    );
    assert_eq!(
        gaps(
            &mut fixture,
            &block,
            &large,
            &[(&large, "A"), (&small, "漢")]
        ),
        [5.0, 0.0]
    );
    // A Hebrew letter reads right to left: the room is the later item's.
    assert_eq!(
        gaps(
            &mut fixture,
            &block,
            &large,
            &[(&large, "\u{5D0}"), (&small, "漢")]
        ),
        [2.5, 0.0]
    );
}

/// Both sides of a seam must take it: `no-autospace` on either ends it, as
/// does text set upright in a vertical line; `ideograph-alpha` takes a
/// letter's seam and `ideograph-numeric` a digit's, as CSS Text 4 has them
/// (beyond Chrome, which parses neither).
#[test]
fn both_sides_of_a_seam_must_take_it() {
    let mut fixture = cjk();
    let base = sized(&PUNCT, 20.0);
    let on = spaced(&base, true, true);
    let off = spaced(&base, false, false);
    let block = ComputedBlockStyle::default();
    assert_eq!(
        gaps(
            &mut fixture,
            &block,
            &on,
            &[(&on, "漢"), (&off, "A"), (&on, "漢")]
        ),
        [0.0, 0.0, 0.0]
    );
    let alpha = spaced(&base, true, false);
    let numeric = spaced(&base, false, true);
    assert_eq!(
        gaps(&mut fixture, &block, &alpha, &[(&alpha, "漢A漢1漢")]),
        [2.5, 2.5, 0.0, 0.0, 0.0]
    );
    assert_eq!(
        gaps(&mut fixture, &block, &numeric, &[(&numeric, "漢A漢1漢")]),
        [0.0, 0.0, 2.5, 2.5, 0.0]
    );
    let mut upright = on;
    upright.orientation.text_orientation = TextOrientation::Upright;
    let vertical = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::default()
    };
    assert_eq!(
        gaps(&mut fixture, &vertical, &upright, &[(&upright, "漢A")]),
        [0.0, 0.0]
    );
    assert_eq!(
        gaps(&mut fixture, &vertical, &on, &[(&on, "漢A")]),
        [2.5, 0.0]
    );
}

/// `text-orientation: upright` ends no seam in a sideways mode, where it has
/// no effect.
///
/// Chrome's `TextAutoSpace` ends a seam at text whose font orientation is
/// upright. Chrome's `ComputeFontOrientation` makes it horizontal in every
/// horizontal typographic mode, the sideways modes among them.
#[test]
fn upright_text_in_a_sideways_mode_takes_seams() {
    let mut fixture = cjk();
    let mut upright = spaced(&sized(&PUNCT, 20.0), true, true);
    upright.orientation.text_orientation = TextOrientation::Upright;
    for writing_mode in [WritingMode::SidewaysRl, WritingMode::SidewaysLr] {
        let sideways = ComputedBlockStyle {
            writing_mode,
            ..ComputedBlockStyle::default()
        };
        assert_eq!(
            gaps(&mut fixture, &sideways, &upright, &[(&upright, "漢A")]),
            [2.5, 0.0],
            "{writing_mode:?}"
        );
    }
}

/// A seam reaches across a box's edges, padding and all, as Chrome's runs
/// over the block's whole text; what Chrome writes into that text as a
/// character of its own ends it: the bidi controls of a box with
/// `unicode-bidi`, and a float. Marks are stepped over.
#[test]
fn a_seam_reaches_across_box_edges_and_not_past_controls_or_floats() {
    let mut fixture = cjk();
    let on = spaced(&sized(&PUNCT, 20.0), true, true);
    let block = ComputedBlockStyle::new(&on);
    let mut padded = on;
    padded.edges.padding.left = LengthPercentage::from_px(10.0);
    padded.edges.padding.right = LengthPercentage::from_px(10.0);
    assert_eq!(
        gaps(
            &mut fixture,
            &block,
            &on,
            &[(&on, "漢"), (&padded, "A"), (&on, "漢")]
        ),
        [2.5, 2.5, 0.0]
    );
    let mut isolated = on;
    isolated.bidi.unicode_bidi = UnicodeBidi::Isolate;
    assert_eq!(
        gaps(
            &mut fixture,
            &block,
            &on,
            &[(&on, "漢"), (&isolated, "A"), (&on, "漢")]
        ),
        [0.0, 0.0, 0.0]
    );
    assert_eq!(
        gaps(&mut fixture, &block, &on, &[(&on, "A\u{301}漢")]),
        [2.5, 0.0]
    );
    let mut layout = Layout::new();
    fixture.build(&mut layout, &block, |b| {
        b.text(NodeKey(1), "漢");
        b.float(
            NodeKey(2),
            &on,
            FloatSide::Left,
            BoxSize {
                inline: 10.0,
                block: 10.0,
                baseline: None,
            },
        );
        b.text(NodeKey(3), "A");
    });
    assert_eq!(positions(&layout), [0.0, 20.0, 30.0]);
}

/// A seam reaches across a `::first-letter` box's edge whatever its
/// `unicode-bidi`, which Blink's cascade drops for the pseudo, so it writes
/// no controls: `漢Abc字` whose first letter is `isolate` has its gap after
/// `漢`, as in Chrome 154 (probe `firstletteredges`, row s1).
#[test]
fn a_seam_reaches_across_a_first_letter_whatever_its_bidi() {
    let mut fixture = cjk();
    let on = spaced(&sized(&PUNCT, 20.0), true, true);
    let block = ComputedBlockStyle::new(&on);
    let mut plain = Layout::new();
    fixture.build(&mut plain, &block, |b| b.text(NodeKey(1), "漢Abc字"));
    let mut isolated = on;
    isolated.bidi.unicode_bidi = UnicodeBidi::Isolate;
    let mut layout = Layout::new();
    fixture.build(&mut layout, &block, |b| {
        b.set_first_letter(NodeKey(9), &isolated, None);
        b.text(NodeKey(1), "漢Abc字");
    });
    let seams = positions(&plain);
    assert_eq!(seams.get(1), Some(&22.5));
    assert_eq!(positions(&layout), seams);
}

/// Conditional punctuation, `!` among it, is narrow in a Chinese block and
/// neither side of a seam elsewhere (UTR #59, as Chrome resolves it by the
/// block's language).
#[test]
fn conditional_punctuation_is_narrow_only_in_chinese() {
    let mut fixture = cjk();
    let on = spaced(&sized(&PUNCT, 20.0), true, true);
    let mut chinese = on;
    chinese.text.language = Language::parse("zh-Hant").ok();
    let mut japanese = on;
    japanese.text.language = Language::parse("ja").ok();
    let block = ComputedBlockStyle::default();
    assert_eq!(
        gaps(&mut fixture, &block, &chinese, &[(&chinese, "漢!漢")]),
        [2.5, 2.5, 0.0]
    );
    assert_eq!(
        gaps(&mut fixture, &block, &japanese, &[(&japanese, "漢!漢")]),
        [0.0, 0.0, 0.0]
    );
}

/// Each seam beside a right-to-left run puts its room on its own side of
/// the run, as Chrome's `TextAutoSpace` writes it.
///
/// Where the earlier side reads left to right, the room goes at the end of
/// the earlier item. Where it reads right to left, the room goes at the
/// start of the later item, before its first glyph. So `漢漢אבג漢漢` at 20px
/// draws `漢漢`, 2.5px, `גבא`, 2.5px, `漢漢`, whether or not a span splits
/// the Hebrew.
#[test]
fn a_seam_beside_a_right_to_left_run_takes_room_on_its_own_side() {
    const FAMILIES: [FontFamilyName<'static>; 2] = [
        FontFamilyName::Named(Cow::Borrowed("Test Punct")),
        FontFamilyName::Named(Cow::Borrowed("Test Hebrew")),
    ];
    let mut fixture = Fixture::new(
        &[
            TestFont::cjk("Test Punct", true),
            TestFont::new("Test Hebrew", &[(0x5D0, 0x5EA)]),
        ],
        han_fallback("Test Punct"),
        StageCheck::Built(|_, layout| check(layout)),
    );
    let on = spaced(&sized(&FAMILIES, 20.0), true, true);
    let drawn = |layout: &Layout| {
        let text = &layout.content().text;
        let mut drawn: Vec<(char, f32)> = layout
            .line(0)
            .into_iter()
            .flat_map(|line| line.items())
            .filter_map(|item| match item {
                crate::Item::Text(run) => Some(run),
                _ => None,
            })
            .flat_map(|run| run.glyphs().collect::<Vec<_>>())
            .filter_map(|glyph| Some((text[glyph.text_offset..].chars().next()?, glyph.x)))
            .collect();
        drawn.sort_by(|a, b| a.1.total_cmp(&b.1));
        drawn
    };
    let expected = [
        ('\u{6F22}', 0.0),
        ('\u{6F22}', 20.0),
        ('\u{5D2}', 42.5),
        ('\u{5D1}', 52.5),
        ('\u{5D0}', 62.5),
        ('\u{6F22}', 75.0),
        ('\u{6F22}', 95.0),
    ];
    let block = ComputedBlockStyle::new(&on);
    let mut layout = Layout::new();
    fixture.build(&mut layout, &block, |b| {
        b.text(
            NodeKey(1),
            "\u{6F22}\u{6F22}\u{5D0}\u{5D1}\u{5D2}\u{6F22}\u{6F22}",
        );
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(drawn(&layout), expected);
    fixture.build(&mut layout, &block, |b| {
        b.text(NodeKey(1), "\u{6F22}\u{6F22}");
        b.open_box(NodeKey(2), &on, None);
        b.text(NodeKey(3), "\u{5D0}\u{5D1}");
        b.close_box();
        b.text(NodeKey(4), "\u{5D2}\u{6F22}\u{6F22}");
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(drawn(&layout), expected);
}
