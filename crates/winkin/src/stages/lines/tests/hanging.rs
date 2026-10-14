//! Forced break and hanging tests. They pin:
//! - forced breaks ending lines and starting paragraphs;
//! - control characters and every BK and NL character, as CSS Text 3 has
//!   them;
//! - trailing white space hanging, and a box edge among it staying content;
//! - a cloned box paying for its edges on every line;
//! - intrinsic sizes the text fits.

use super::*;
use crate::style::FirstLineVariant;

/// A forced break ends its line, with the separator on it, and starts a
/// paragraph.
///
/// A text ending at a forced break has no empty line after it, as in
/// Chrome. Of two breaks in a row, the second makes a line alone.
#[test]
fn forced_breaks_end_lines_and_start_paragraphs() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem(20.0)), |b| {
        b.text(NodeKey(1), "XX");
        b.line_break(NodeKey(2));
        b.line_break(NodeKey(3));
        b.text(NodeKey(4), "YY YY");
        b.line_break(NodeKey(5));
    });
    fixture.lay_out(&mut layout, 1000.0);
    assert_eq!(texts(&layout), ["XX\n", "\n", "YY YY\n"]);
    assert_eq!(end_kinds(&layout), [EndKind::Forced; 3]);
    let paragraphs: Vec<usize> = layout.lines().map(|line| line.paragraph()).collect();
    assert_eq!(paragraphs, [0, 1, 2]);
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout), ["XX\n", "\n", "YY ", "YY\n"]);
    assert_eq!(
        end_kinds(&layout),
        [
            EndKind::Forced,
            EndKind::Forced,
            EndKind::Soft,
            EndKind::Forced
        ]
    );
    fixture.text(&mut layout, &ahem(20.0), "XX");
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(end_kinds(&layout)[0], EndKind::TextEnd);
    // An empty text has no lines; a lone break has one.
    fixture.text(&mut layout, &ahem(20.0), "");
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(layout.lines().len(), 0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem(20.0)), |b| {
        b.line_break(NodeKey(1));
    });
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout), ["\n"]);
}

// Hanging --------------------------------------------------------------------

/// Trailing white space hangs and does not count toward the fit, whether
/// collapsible or preserved.
///
/// Before a forced break, preserved spaces hang only where they overflow,
/// and the line flags this for alignment.
#[test]
fn trailing_white_space_hangs() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.text(&mut layout, &ahem(20.0), "XX XX");
    fixture.lay_out(&mut layout, 40.0);
    assert_eq!(texts(&layout), ["XX ", "XX"]);
    assert_eq!(layout.line(0).expect("a line").metrics().hang, 20.0);
    assert_eq!(
        records(&layout)[0].content_end(&layout.analysis().clusters),
        at(2)
    );
    let mut pre = ahem(20.0);
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    fixture.text(&mut layout, &pre, "XX   XX");
    fixture.lay_out(&mut layout, 40.0);
    assert_eq!(texts(&layout), ["XX   ", "XX"]);
    let first = records(&layout)[0].clone();
    assert!(!first.flags.contains(LineFlags::OVERFLOWS));
    assert_eq!(
        (first.width, first.hang.space),
        (LayoutUnit::from_px(40.0), LayoutUnit::from_px(60.0))
    );
    assert!(!first.flags.contains(LineFlags::CONDITIONAL_HANG));
    fixture.build(&mut layout, &ComputedBlockStyle::new(&pre), |b| {
        b.text(NodeKey(1), "XX  ");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "XX");
    });
    fixture.lay_out(&mut layout, 40.0);
    assert_eq!(texts(&layout), ["XX  \n", "XX"]);
    let first = records(&layout)[0].clone();
    assert_eq!(first.hang.space, LayoutUnit::from_px(40.0));
    assert!(first.flags.contains(LineFlags::CONDITIONAL_HANG));
}

/// Control characters break as CSS Text 3 says.
///
/// Each case is `XX`, the character, then `XX`, in Ahem.
/// - A preserved lone CR is a space, which hangs and breaks as one: `XX_`
///   over `XX` in 60 px under `pre-wrap`. Chrome drops it.
/// - VT, FF, NEL, U+2028 and U+2029 force a break, under `normal` too.
///   Chrome breaks at none. Spaces around one collapse: `XX` over `XX`.
/// - VT, FF and NEL are drawn, so they take room at their line's end.
///   U+2028 and U+2029 draw nothing and hang, as a `<br>` does.
#[test]
fn control_characters_break_and_draw_as_css_has_them() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut pre_wrap = ahem(20.0);
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    fixture.text(&mut layout, &pre_wrap, "XX\rXX");
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout), ["XX ", "XX"]);
    assert_eq!(layout.line(0).expect("a line").metrics().hang, 20.0);
    fixture.lay_out(&mut layout, 40.0);
    assert_eq!(texts(&layout), ["XX ", "XX"], "the space hangs");
    for separator in ['\u{B}', '\u{C}', '\u{85}', '\u{2028}', '\u{2029}'] {
        let drawn = matches!(separator, '\u{B}' | '\u{C}' | '\u{85}');
        for style in [ahem(20.0), pre_wrap] {
            fixture.text(&mut layout, &style, &format!("XX{separator}XX"));
            fixture.lay_out(&mut layout, 200.0);
            let what = format!("U+{:04X}", u32::from(separator));
            assert_eq!(
                texts(&layout),
                [format!("XX{separator}"), "XX".into()],
                "{what}"
            );
            let first = records(&layout)[0].clone();
            let content = if drawn { at(3) } else { at(2) };
            assert_eq!(
                first.content_end(&layout.analysis().clusters),
                content,
                "{what}"
            );
            let advance = layout
                .shaped()
                .text(FirstLineVariant::Standard)
                .glyphs
                .word(at(2));
            assert_eq!(advance != GlyphWord::EMPTY, drawn, "{what} is drawn");
        }
        fixture.text(&mut layout, &ahem(20.0), &format!("XX {separator} XX"));
        fixture.lay_out(&mut layout, 200.0);
        assert_eq!(
            texts(&layout),
            [format!("XX{separator}"), "XX".into()],
            "U+{:04X} takes the spaces either side",
            u32::from(separator)
        );
    }
}

/// Every character of line-breaking class BK or NL forces a break, whatever
/// the white space (WPT `line-breaking-022`).
///
/// Chrome fails it, breaking at none of them.
#[test]
fn every_bk_and_nl_character_forces_a_break() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(25.0);
    let parted = [
        ("1", "\u{C}"),
        ("2", "\u{B}"),
        ("3", "\u{2028}"),
        ("4", "\u{2029}"),
        ("5", "\u{85}"),
        ("6", "\n"),
    ];
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "\n");
        for (at, (digit, separator)) in (0u64..).zip(parted) {
            b.open_box(NodeKey(10 + 3 * at), &style, None);
            b.text(NodeKey(11 + 3 * at), digit);
            b.close_box();
            b.text(NodeKey(12 + 3 * at), separator);
        }
    });
    fixture.lay_out(&mut layout, 800.0);
    assert_eq!(
        texts(&layout),
        ["1\u{C}", "2\u{B}", "3\u{2028}", "4\u{2029}", "5\u{85}", "6"]
    );
}

/// A cloned box pays both its edges on every line it reaches, and a sliced
/// one only where it opens and closes.
///
/// Chrome's breaker charges the cloned edges as `cloned_box_decorations_*`.
#[test]
fn a_cloned_box_pays_for_its_edges_on_every_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let boxed = |decoration_break| ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 10.0,
                right: 10.0,
                ..Sides::ZERO
            }
            .into(),
            decoration_break,
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    for (decoration_break, lines, wide) in [
        (
            BoxDecorationBreak::Slice,
            &["XX XX ", "XX XX"][..],
            &[110.0, 110.0][..],
        ),
        (
            BoxDecorationBreak::Clone,
            &["XX ", "XX ", "XX ", "XX"][..],
            &[60.0, 60.0, 60.0, 60.0][..],
        ),
    ] {
        let style = boxed(decoration_break);
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.open_box(NodeKey(1), &style, None);
            b.text(NodeKey(2), "XX XX XX XX");
            b.close_box();
        });
        fixture.lay_out(&mut layout, 115.0);
        assert_eq!(texts(&layout), lines, "{decoration_break:?}");
        assert_eq!(widths(&layout), wide, "{decoration_break:?}");
    }
}

/// A box closing after a hanging space keeps its edge on the line as
/// content, and the space still hangs.
///
/// The intrinsic sizes measure the borders too.
#[test]
fn a_box_edge_among_what_hangs_stays_content() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let bordered = ComputedStyle {
        edges: EdgesGroup {
            border: Sides::all(1.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX");
        b.open_box(NodeKey(2), &bordered, None);
        b.text(NodeKey(3), " ");
        b.close_box();
        b.text(NodeKey(4), "XX");
    });
    fixture.lay_out(&mut layout, 42.0);
    assert_eq!(texts(&layout), ["XX ", "XX"]);
    assert_eq!(widths(&layout), [42.0, 40.0]);
    assert_eq!(layout.line(0).expect("a line").metrics().hang, 20.0);
    fixture.lay_out(&mut layout, 41.0);
    assert_eq!(texts(&layout), ["XX ", "XX"]);
    assert!(records(&layout)[0].flags.contains(LineFlags::OVERFLOWS));
    let intrinsic = layout.intrinsic_sizes();
    fixture.lay_out(&mut layout, intrinsic.max_content);
    assert_eq!(layout.lines().len(), 1);
    fixture.lay_out(&mut layout, intrinsic.min_content);
    assert!(
        records(&layout)
            .iter()
            .all(|line| !line.flags.contains(LineFlags::OVERFLOWS))
    );
}

/// At max-content every paragraph is one line, and at min-content no line
/// overflows.
#[test]
fn the_intrinsic_sizes_are_widths_the_text_fits() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = sized(&LATIN, 16.0);
    let boxed = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(2.0),
            padding: Sides::from_px(1.25),
            ..EdgesGroup::INITIAL
        },
        ..sized(&NARROW, 13.0)
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "The quick brown fox ");
        b.open_box(NodeKey(2), &boxed, None);
        b.text(NodeKey(3), "jumps over ");
        b.close_box();
        b.text(NodeKey(4), "the lazy dog.");
        b.line_break(NodeKey(5));
        b.text(NodeKey(6), "An office of flat AVAIL fit.");
    });
    let intrinsic = layout.intrinsic_sizes();
    fixture.lay_out(&mut layout, intrinsic.max_content);
    assert_eq!(layout.lines().len(), 2);
    fixture.lay_out(&mut layout, intrinsic.min_content);
    assert!(
        records(&layout)
            .iter()
            .all(|line| !line.flags.contains(LineFlags::OVERFLOWS))
    );
}
