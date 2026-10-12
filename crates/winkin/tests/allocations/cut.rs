//! Hyphens at soft hyphens, clamped blocks and lines cut for ellipses
//! allocate nothing warm.

use super::count_allocations;
use super::test_fonts::{self, TestFont, ahem_fallback};
use fontwich::Collection;
use winkin::config::EllipsisSpace;
use winkin::paint::Decorates;
use winkin::style::{
    BaseDirection, ComputedStyle, Direction, FontFamilyName, FontGroup, LineClamp, TextAlign,
    TextOverflow, TextWrapMode, TextWrapStyle,
};
use winkin::{
    Area, BoxSize, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

/// Ahem, and Latin with ligatures and a kerning pair.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E), (0x2010, 0x2010)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    latin.kerning = vec![('A', 'V', -100)];
    test_fonts::collection(&[latin], ahem_fallback())
}

/// The ways a block may be cut: at its soft hyphens alone, clamped,
/// clamped by height and balanced, cut where a line overflows, read
/// right to left, and justified.
#[derive(Copy, Clone)]
enum How {
    Hyphens,
    Clamped,
    ClampedByHeight,
    Ellipsized,
    RightToLeft,
    Justified,
}

/// The area a block cut as `how` says is broken in: lines `width`
/// long, clamped by height 90 px down where it is.
fn area(how: How, width: f32) -> Area {
    let block_end = matches!(how, How::ClampedByHeight).then_some(90.0);
    Area {
        block_end,
        ..Area::new(width)
    }
}

/// Text full of soft hyphens, in spans that set their own hyphen, a
/// larger size and a hyphen of three characters, the last of which Test
/// Latin lacks and Ahem draws, a run of its own, with an atomic inline,
/// `words` long, set as `how` says; a `::first-line` at another size,
/// so the first line's hyphens and ellipsis are its own.
fn document(layout: &mut Layout, cx: &mut Context, how: How, words: usize) {
    let families = [FontFamilyName::named("Test Latin")];
    let mut root = ComputedStyle {
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    // The block's own properties, made before its style is settled.
    let initial = ComputedStyle::initial();
    let mut block = ComputedBlockStyle::new(&initial);
    match how {
        How::Hyphens => {}
        How::Clamped => block.line_clamp = LineClamp::Lines(3),
        How::ClampedByHeight => {
            block.line_clamp = LineClamp::Auto;
            block.text_wrap_style = TextWrapStyle::Balance;
        }
        How::Ellipsized => {
            block.text_overflow = TextOverflow::Ellipsis;
            root.text.wrap_mode = TextWrapMode::NoWrap;
        }
        How::RightToLeft => {
            block.text_overflow = TextOverflow::Ellipsis;
            block.line_clamp = LineClamp::Lines(2);
            block.direction = BaseDirection::Rtl;
            root.bidi.direction = Direction::Rtl;
        }
        How::Justified => {
            block.text_align = TextAlign::Justify;
            block.line_clamp = LineClamp::Lines(4);
        }
    }
    let big = ComputedStyle {
        font: FontGroup {
            size: 24.0,
            ..root.font
        },
        ..root
    };
    let mut own = root;
    own.text.hyphenate_character = Some("=\u{2010}\u{2022}");
    let first = ComputedStyle {
        font: FontGroup {
            size: 20.0,
            ..root.font
        },
        ..root
    };
    let block = ComputedBlockStyle {
        style: &root,
        first_line: Some(&first),
        ..block
    };
    let mut b = layout.builder(cx, NodeKey(0), &block, BuildOptions::default());
    for at in 0..words {
        let key = 8 * at as u64;
        b.text(NodeKey(key), "of\u{AD}fi\u{AD}cial waf\u{AD}fles ");
        b.open_box(NodeKey(key + 1), &big, None);
        b.text(NodeKey(key + 2), "AVA\u{AD}VA\u{AD}VA ");
        b.close_box();
        b.open_box(NodeKey(key + 3), &own, None);
        b.text(NodeKey(key + 4), "hy\u{AD}phen\u{AD}ated ");
        b.close_box();
        b.atomic(
            NodeKey(key + 5),
            &root,
            None,
            BoxSize {
                inline: 12.0,
                block: 10.0,
                baseline: None,
            },
        );
    }
    assert!(b.finish().is_complete());
}

/// Every line's items, glyphs and paint, read back as a painter does,
/// the hidden tail and the generated runs among them.
fn read(layout: &Layout) -> f32 {
    let mut sum = 0.0;
    for line in layout.lines() {
        let metrics = line.metrics();
        sum += metrics.width + metrics.height();
        sum += f32::from(u8::from(line.is_hyphenated()) + u8::from(line.has_ellipsis()));
        for item in line.all_items() {
            if let Item::Text(run) | Item::Generated(run) = item {
                sum += run.inline().left + run.advance() + run.baseline();
                for glyph in run.glyphs() {
                    sum += glyph.id as f32 + glyph.x + glyph.y + glyph.advance;
                }
            }
        }
        sum += line.paints(|_| Decorates::None).count() as f32;
    }
    sum
}

/// Text hyphenated at its soft hyphens, clamped, by a count or by a
/// height, cut for ellipses and justified, both ways round and under either rule for a space before
/// an ellipsis, rebuilds, breaks again and reads back allocating
/// nothing once warm: the generated texts and their glyphs keep their
/// capacity, and so do a cut line's pieces.
#[test]
fn hyphens_ellipses_and_clamps_allocate_nothing_warm() {
    let widths = [37.0, 90.5, 160.0, 300.0, 11.0, 800.0];
    for how in [
        How::Hyphens,
        How::Clamped,
        How::ClampedByHeight,
        How::Ellipsized,
        How::RightToLeft,
        How::Justified,
    ] {
        for space in [EllipsisSpace::Kept, EllipsisSpace::Hidden] {
            let mut cx = Context::new(collection());
            let mut config = *cx.config();
            config.ellipsis_space = space;
            cx.set_config(config);
            let mut layout = Layout::new();
            let cold = count_allocations(|| document(&mut layout, &mut cx, how, 12));
            assert!(cold > 0, "a cold layout grows");
            for &width in &widths {
                layout.break_lines(&mut cx, area(how, width), &mut NoExclusions);
                read(&layout);
            }
            let warm = count_allocations(|| document(&mut layout, &mut cx, how, 12));
            assert_eq!(warm, 0, "rebuilding allocated");
            for &width in &widths {
                layout.break_lines(&mut cx, area(how, width), &mut NoExclusions);
                read(&layout);
            }
            let warm = count_allocations(|| {
                for &width in &widths {
                    layout.break_lines(&mut cx, area(how, width), &mut NoExclusions);
                }
            });
            assert_eq!(warm, 0, "breaking again allocated");
            for &width in &widths {
                layout.break_lines(&mut cx, area(how, width), &mut NoExclusions);
                let mut sum = 0.0;
                let warm = count_allocations(|| sum = read(&layout));
                assert!(sum.is_finite());
                assert_eq!(warm, 0, "at {width}: reading back allocated");
            }
        }
    }
}
