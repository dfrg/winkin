//! Vertical line tests. They pin:
//! - upright and sideways glyphs placed along a vertical line;
//! - a `sideways-lr` line's over side on its left;
//! - combined text fitted to one em, centred, and spaced as one character,
//!   keeping its em whatever its white space;
//! - a generated hyphen standing as its text does.

use super::*;
use crate::style::LengthPercentage;

/// Returns each glyph of line 0's text runs, where it is drawn.
fn glyph_places(layout: &Layout) -> Vec<(f32, f32)> {
    runs(layout, 0)
        .iter()
        .flat_map(TextRun::glyphs)
        .map(|glyph| (glyph.x, glyph.y))
        .collect()
}

/// In a vertical line a run says how its glyphs stand, and its glyphs are
/// placed line-relative, along the line from its top and across it from
/// its right, its over side. 30 px Ahem, whose line is 24 over its
/// alphabetic baseline and 6 under, is centred: 15 and 15 about the
/// central baseline, 15 from the line box's over side. An upright glyph,
/// which has no vertical metrics, stands 24 along from its pen and half its
/// width under the centre, so drawn unturned from there it fills the line
/// across; a glyph on its side stands on its alphabetic baseline, 9 under
/// the centre, so turned with the line its ascent and descent fill it too.
/// Both runs' text is the line's own 30 across. In a horizontal line
/// nothing moves.
#[test]
fn a_vertical_line_places_upright_and_sideways_glyphs() {
    use crate::RunOrientation::{Horizontal, Sideways, Upright};
    use crate::style::WritingMode;
    let mut cx = context();
    let mut layout = Layout::new();
    let style = sized(&AHEM_FAMILY, 30.0);
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&style)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "\u{6C34}X");
    });
    layout.break_lines(&mut cx, Area::new(300.0), &mut NoExclusions);
    let line = runs(&layout, 0);
    let orientations: Vec<_> = line.iter().map(TextRun::orientation).collect();
    assert_eq!(orientations, [Upright, Sideways]);
    assert_eq!(glyph_places(&layout), [(24.0, 30.0), (30.0, 24.0)]);
    for run in &line {
        assert_eq!(run.baseline(), 15.0);
        assert_eq!(run.block(), across(0.0, 30.0));
    }
    let mut layout = Layout::new();
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&style),
        |b| {
            b.text(NodeKey(1), "\u{6C34}X");
        },
    );
    layout.break_lines(&mut cx, Area::new(300.0), &mut NoExclusions);
    assert!(
        runs(&layout, 0)
            .iter()
            .all(|run| run.orientation() == Horizontal)
    );
    assert_eq!(glyph_places(&layout), [(0.0, 24.0), (30.0, 24.0)]);
}

/// A `sideways-lr` line's over side is its left, where its lines stack
/// from (CSS Writing Modes 4, section 6.4; Blink's `LineOver`), so what is set over
/// its text takes room at the top of its line box, as across a horizontal
/// line, and what is under at its bottom: the frame a host turns the line
/// with puts the top on the left. Chrome 153, Ahem at 20 px on lines of 20
/// with a 10 px annotation, sets ruby `over` on the left, the base from 10
/// to 30 across the block and the annotation from 0 to 10, and `under` on
/// the right, the base from 0 and the annotation from 20.
///
/// Emphasis marks go where `text-emphasis-position` says by `over` and
/// `under` there, which CSS Text Decoration 4, section 3.4, reads in horizontal
/// typographic modes, the sideways ones among them, keeping `right` and
/// `left` for vertical typographic modes. Chrome 153 reads `right` and
/// `left` in the sideways modes too (`GetTextEmphasisLineLogicalSide`), so
/// it sets the initial `over right` on the right in `sideways-lr`, under
/// the line: not copied.
#[test]
fn a_sideways_lr_line_is_over_on_its_left() {
    use crate::style::{RubyPosition, WritingMode};
    let mut cx = context();
    let mut layout = Layout::new();
    let mut base = sized(&AHEM_FAMILY, 20.0);
    base.line.height = LineHeight::Px(20.0);
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::SidewaysLr,
        ..ComputedBlockStyle::new(&base)
    };
    let small = sized(&AHEM_FAMILY, 10.0);
    for (position, text, annotation) in [
        (RubyPosition::Over, across(10.0, 30.0), across(0.0, 10.0)),
        (RubyPosition::Under, across(0.0, 20.0), across(20.0, 30.0)),
    ] {
        let mut container = base;
        container.ruby.position = position;
        build(&mut cx, &mut layout, &block, |b| {
            b.open_ruby(NodeKey(2), &container, None);
            b.text(NodeKey(3), "XX");
            b.open_annotation(NodeKey(4), &small, None);
            b.text(NodeKey(5), "yy");
            b.close_annotation();
            b.close_ruby();
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        let line = layout.line(0).expect("a line");
        assert_eq!(line.metrics().height(), 30.0, "{position:?}");
        assert_eq!(runs(&layout, 0)[0].block(), text, "{position:?}");
        let over = line.annotations().next().expect("an annotation");
        let run = over.runs().next().expect("its text");
        assert_eq!(run.block(), annotation, "{position:?}");
    }
    for (side, text) in [
        (EmphasisSide::Over, across(10.0, 30.0)),
        (EmphasisSide::Under, across(0.0, 20.0)),
    ] {
        let mut style = marked(20.0, EmphasisSkip::INITIAL);
        style.line.height = LineHeight::Px(20.0);
        style.text.emphasis.position.side = side;
        build(
            &mut cx,
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            |b| b.text(NodeKey(1), "XX"),
        );
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        let line = layout.line(0).expect("a line");
        assert_eq!(line.metrics().height(), 30.0, "{side:?}");
        assert_eq!(runs(&layout, 0)[0].block(), text, "{side:?}");
    }
}

// Combined text ------------------------------------------------------------

/// A context over a font whose digits are half an em wide, with narrower
/// forms of them under `hwid` (a quarter of an em), `twid` (a sixth) and
/// `qwid` (an eighth), and whose ideographs and hiragana are an em.
fn combining_context() -> Context {
    let mut font = TestFont::new(
        "Test Digits",
        &[(0x20, 0x7E), (0x3042, 0x3042), (0x6C34, 0x6C34)],
    );
    font.advances = vec![('\u{3042}', 1000), ('\u{6C34}', 1000)];
    let digits = |advance: u16| ('0'..='9').map(|d| (d, advance)).collect::<Vec<_>>();
    font.alternates = vec![
        (*b"hwid", digits(250)),
        (*b"twid", digits(166)),
        (*b"qwid", digits(125)),
    ];
    let fallback = TestFallback::new().family("Test Digits");
    Context::new(collection(&[font], fallback))
}

const DIGITS: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed("Test Digits"))];

/// A `vertical-rl` block of 20 px text in the digits font, `text` in it
/// combined whole between two ideographs, `style` changing its own style,
/// broken in `width`.
fn combined_line(
    cx: &mut Context,
    text: &str,
    width: f32,
    style: impl Fn(&mut ComputedStyle<'static>),
) -> Layout {
    use crate::style::{TextCombineUpright, WritingMode};
    let mut layout = Layout::new();
    let mut root = sized(&DIGITS, 20.0);
    style(&mut root);
    let mut combined = root;
    combined.orientation.text_combine_upright = TextCombineUpright::All;
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&root)
    };
    build(cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "\u{6C34}");
        b.open_box(NodeKey(2), &combined, None);
        b.text(NodeKey(3), text);
        b.close_box();
        b.text(NodeKey(4), "\u{6C34}");
    });
    layout.break_lines(cx, Area::new(width), &mut NoExclusions);
    layout
}

/// Returns line 0's combined run.
fn combined_run(layout: &Layout) -> TextRun<'_> {
    runs(layout, 0)
        .into_iter()
        .find(|run| run.orientation() == crate::RunOrientation::Combined)
        .expect("a combined run")
}

/// Combined text takes one em of its line, whatever it holds, and is fitted
/// to its em and a tenth as Blink fits a `LayoutTextCombine`: set as it is
/// where it fits, else in the first narrower form that does, half, third
/// or quarter widths, else narrowed across by the ratio. Its glyphs stand
/// upright, their baseline the font's ascent down the em (16 of 20 here,
/// the font having no internal leading), laid across it and centred on the
/// line's centre.
#[test]
fn combined_text_is_fitted_to_one_em() {
    let mut cx = combining_context();
    let plain = |_: &mut ComputedStyle<'static>| {};
    // "12" is 20 wide, which fits 22.
    let layout = combined_line(&mut cx, "12", 400.0, plain);
    let line = layout.line(0).expect("a line");
    assert_eq!(line.metrics().width, 60.0);
    let run = combined_run(&layout);
    assert_eq!(run.inline(), along(20.0, 40.0));
    assert_eq!(run.combine_scale(), 1.0);
    let glyphs: Vec<(f32, f32, f32)> = run
        .glyphs()
        .map(|glyph| (glyph.x, glyph.y, glyph.advance))
        .collect();
    let centre = run.baseline();
    assert_eq!(glyphs, [(36.0, centre + 10.0, 0.0), (36.0, centre, 20.0)]);
    // Each narrower form in turn: `2026` at half widths is 20, six digits
    // at thirds 19.92, eight at quarters 20; ten at quarters are 25, and
    // are set as they are, 100 wide, narrowed to 22.
    for (text, advance, scale) in [
        ("2026", 5.0, 1.0),
        ("123456", 3.32, 1.0),
        ("12345678", 2.5, 1.0),
        ("1234567890", 10.0, 0.22),
    ] {
        let layout = combined_line(&mut cx, text, 400.0, plain);
        assert_eq!(
            layout.line(0).expect("a line").metrics().width,
            60.0,
            "{text}"
        );
        let run = combined_run(&layout);
        assert!(
            (run.combine_scale() - scale).abs() < 1e-4,
            "{text}: {}",
            run.combine_scale()
        );
        let across: Vec<f32> = run.glyphs().map(|glyph| glyph.y).collect();
        let step = across[0] - across[1];
        assert!((step - advance * scale).abs() < 1e-3, "{text}: {step}");
        // Centred: the first glyph's left is as far under the centre as
        // the last glyph's right is over it.
        let width = advance * scale * text.len() as f32;
        assert!(
            (across[0] - run.baseline() - width / 2.0).abs() < 1e-3,
            "{text}"
        );
    }
}

/// Combined text is centred between its font's text-over and text-under
/// baselines (CSS Writing Modes 3, section 9.1.2), which in a centred line of odd
/// height, its odd pixel over, is half a pixel over the central baseline:
/// Blink sets a `LayoutTextCombine` at its text's top, as wide as the font
/// is tall, and centres the text in it, or narrowed, `ComputeInlineSpacing`
/// in from its edge. A font of Yu Gothic's height, 2017 units over its
/// alphabetic baseline and 619 under in an em of 2048, is 39 and 12 whole
/// pixels at 40 px, 26 over the central baseline and 25 under; at 25 px,
/// 25 and 8, 17 over and 16 under. Chrome 153, Yu Gothic on lines twice its
/// size, in `vertical-rl` and `vertical-lr` alike: at 40 px `12` combined
/// from 20.5 to 60.5 px across the 80 px line box, its font box 15 to 66,
/// and `ABCDEFG`,
/// narrowed to 44, from 18.5 to 62.48; at 25 px `12` from 13 to 38 across
/// 50. Under `text-orientation: sideways` the line is on its alphabetic
/// baseline, and the unit is set where it was, the middle of the font box:
/// 20.5 to 60.5. The carets inside the unit are where its characters meet.
/// Across here is from the line box's over side, its right.
#[test]
fn combined_text_is_centred_between_its_fonts_text_over_and_under() {
    use crate::selection::{Affinity, Position};
    use crate::style::{TextCombineUpright, TextOrientation, WritingMode};
    let mut font = TestFont::new("Test Tall", &[(0x20, 0x7E), (0x6C34, 0x6C34)]);
    font.upem = 2048;
    font.win = (2017, 619);
    font.hhea = (2017, -619, 0);
    font.typo = (2017, -619, 0);
    font.advances = vec![('\u{6C34}', 2048)];
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(font.build()).is_ok());
    layer.set_fallback_override(TestFallback::new().family("Test Tall"));
    let mut cx = Context::new(Collection::new().with_layer(layer.snapshot()));
    const TALL: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed("Test Tall"))];
    let mut layout = Layout::new();
    for mode in [WritingMode::VerticalRl, WritingMode::VerticalLr] {
        for (size, orientation, text, first, last) in [
            (40.0, TextOrientation::Mixed, "12", 20.5, 60.5),
            (40.0, TextOrientation::Mixed, "ABCDEFG", 18.5, 62.5),
            (25.0, TextOrientation::Mixed, "12", 13.0, 38.0),
            (40.0, TextOrientation::Sideways, "12", 20.5, 60.5),
        ] {
            let case = (mode, size, orientation, text);
            let mut root = sized(&TALL, size);
            root.line.height = LineHeight::Px(2.0 * size);
            root.orientation.text_orientation = orientation;
            let mut combined = root;
            combined.orientation.text_combine_upright = TextCombineUpright::All;
            let block = ComputedBlockStyle {
                writing_mode: mode,
                ..ComputedBlockStyle::new(&root)
            };
            build(&mut cx, &mut layout, &block, |b| {
                b.text(NodeKey(1), "\u{6C34}");
                b.open_box(NodeKey(2), &combined, None);
                b.text(NodeKey(3), text);
                b.close_box();
                b.text(NodeKey(4), "\u{6C34}");
            });
            layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
            let line = 2.0 * size;
            // The unit's first glyph's left, and its last's right, from the
            // over side, where Chrome's range over its text reaches.
            let run = combined_run(&layout);
            let scale = run.combine_scale();
            let across: Vec<(f32, f32)> = run
                .glyphs()
                .map(|glyph| (glyph.y, size / 2.0 * scale))
                .collect();
            let (from, _) = across[0];
            let (to, width) = across[across.len() - 1];
            // To the 1/64 the narrowing's 16.16 scale leaves, as Chrome's
            // `ApplyScaleX` truncates its 62.48.
            let near = |got: f32, want: f32| (got - want).abs() < 1.0 / 64.0;
            assert!(
                near(line - from, first) && near(line - (to - width), last),
                "{case:?}: {} to {}",
                line - from,
                line - (to - width)
            );
            // The carets inside the unit, on the grid across the line: its
            // start, and between its first two characters.
            let caret = |at: usize, want: f32| {
                layout
                    .caret(Position::new(at, Affinity::Downstream))
                    .is_some_and(|caret| {
                        caret.block.over == caret.block.under && near(caret.block.over, want)
                    })
            };
            let start = "\u{6C34}".len();
            assert!(caret(start, from), "{case:?}");
            assert!(caret(start + 1, from - size / 2.0 * scale), "{case:?}");
        }
    }
}

/// Combined text is one character: letter-spacing goes after the text
/// around it and not inside it or after it, as Chrome 153 spaces it; a
/// justified line gives it an ideograph's
/// room, one share after it; and it takes one emphasis mark, in the
/// middle of its em, whatever its characters, as Blink marks a
/// `LayoutTextCombine`.
#[test]
fn combined_text_is_spaced_justified_and_marked_as_one_character() {
    use crate::style::{TextCombineUpright, WritingMode};
    let mut cx = combining_context();
    let layout = combined_line(&mut cx, "12", 400.0, |style| {
        style.text.letter_spacing = LengthPercentage::from_px(5.0);
    });
    // Two ideographs spaced, and the unit not.
    assert_eq!(layout.line(0).expect("a line").metrics().width, 70.0);
    assert_eq!(combined_run(&layout).inline(), along(25.0, 45.0));
    let layout = combined_line(&mut cx, "!?", 400.0, |style| {
        style.text.emphasis.marks = true;
    });
    let marks: Vec<f32> = runs(&layout, 0)
        .iter()
        .flat_map(TextRun::emphasis_marks)
        .map(|mark| mark.x)
        .collect();
    assert_eq!(marks, [10.0, 30.0, 50.0]);
    // Justified, 水 [12] 水 水 in 90 of a line of five: three ideographs'
    // opportunities, the unit's among them, share the 10 left over.
    let mut layout = Layout::new();
    let root = sized(&DIGITS, 20.0);
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        text_align: TextAlign::Justify,
        ..ComputedBlockStyle::new(&root)
    };
    let mut combined = root;
    combined.orientation.text_combine_upright = TextCombineUpright::All;
    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "\u{6C34}");
        b.open_box(NodeKey(2), &combined, None);
        b.text(NodeKey(3), "12");
        b.close_box();
        b.text(NodeKey(4), "\u{6C34}\u{6C34}\u{6C34}");
    });
    layout.break_lines(&mut cx, Area::new(90.0), &mut NoExclusions);
    let InlineExtents { left, right } = combined_run(&layout).inline();
    let share = 10.0 / 3.0;
    assert!((left - (20.0 + share)).abs() < 1e-3, "{left}");
    assert!((right - (40.0 + 2.0 * share)).abs() < 1e-3, "{right}");
}

/// Combined text keeps its em whatever its white space, as Blink's
/// `LayoutTextCombine` is an inline block: a unit of one collapsible space
/// is one em, between ideographs and at the line's end, as WPT's
/// `tcy-white-space-processing-003` asks and Chrome draws; and a kept
/// segment break inside a unit is a space, not a forced break
/// (`text-combine-upright-break-inside-001a`).
#[test]
fn combined_text_keeps_its_em_whatever_its_white_space() {
    use crate::style::{TextCombineUpright, WhiteSpaceCollapse, WritingMode};
    let mut cx = combining_context();
    let layout = combined_line(&mut cx, " ", 400.0, |_| {});
    assert_eq!(layout.line(0).expect("a line").metrics().width, 60.0);
    assert_eq!(combined_run(&layout).inline(), along(20.0, 40.0));
    let layout = combined_line(&mut cx, "1\n2", 400.0, |style| {
        style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    });
    assert_eq!(layout.lines().len(), 1);
    assert_eq!(layout.line(0).expect("a line").metrics().width, 60.0);
    // At the line's end.
    let mut layout = Layout::new();
    let root = sized(&DIGITS, 20.0);
    let block = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&root)
    };
    let mut combined = root;
    combined.orientation.text_combine_upright = TextCombineUpright::All;
    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "\u{6C34}");
        b.open_box(NodeKey(2), &combined, None);
        b.text(NodeKey(3), " ");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(layout.line(0).expect("a line").metrics().width, 40.0);
}

/// A generated hyphen stands as the text it ends stands, as Chrome shapes a
/// hyphen in its style's font orientation: upright under `text-orientation:
/// upright`, and on its side under `mixed`, whose hyphen is sideways by its
/// Vertical_Orientation, and under `sideways`.
#[test]
fn a_generated_hyphen_stands_as_its_text_does() {
    use crate::RunOrientation::{Sideways, Upright};
    use crate::style::{TextOrientation, WritingMode};
    let mut cx = context();
    let mut layout = Layout::new();
    for (orientation, want) in [
        (TextOrientation::Upright, Upright),
        (TextOrientation::Mixed, Sideways),
        (TextOrientation::Sideways, Sideways),
    ] {
        let mut style = sized(&AHEM_FAMILY, 20.0);
        style.orientation.text_orientation = orientation;
        let block = ComputedBlockStyle {
            writing_mode: WritingMode::VerticalRl,
            ..ComputedBlockStyle::new(&style)
        };
        build(&mut cx, &mut layout, &block, |b| {
            b.text(NodeKey(1), "XX\u{AD}XXXXXX");
        });
        layout.break_lines(&mut cx, Area::new(70.0), &mut NoExclusions);
        let line = layout.line(0).expect("a line");
        assert!(line.is_hyphenated(), "{orientation:?}");
        let hyphen: Vec<_> = line
            .items()
            .filter_map(|item| match item {
                Item::Generated(run) => Some(run.orientation()),
                _ => None,
            })
            .collect();
        assert_eq!(hyphen, [want], "{orientation:?}");
    }
}
