//! `text-wrap-style: balance` and `pretty` allocate nothing warm.

use super::test_fonts::{self, ahem_fallback, latin};
use super::{Floats, count_allocations};
use fontwich::Collection;
use winkin::config::{Config, Pretty};
use winkin::style::{
    ComputedStyle, FontFamilyName, FontGroup, InitialLetter, LineClamp, TextAlign, TextWrapStyle,
};
use winkin::{
    Area, BoxSize, BuildOptions, ComputedBlockStyle, Context, FloatSide, Layout, NodeKey,
};

/// Ahem, and Latin with ligatures and a kerning pair, so reshaped edges
/// are among what the trials take back.
fn collection() -> Collection {
    test_fonts::collection(&[latin()], ahem_fallback())
}

/// The ways a block's lines are chosen here.
#[derive(Copy, Clone, Debug)]
enum How {
    /// Balanced, each paragraph by the scorer where it is six lines or
    /// fewer, and by halving the room where it is more or holds a
    /// float.
    Balanced,
    /// Balanced with a restyled first line and an initial letter, and
    /// clamped.
    BalancedFirstLine,
    /// Pretty, justified.
    Pretty,
}

/// Paragraphs of every length a relayout meets, some holding floats,
/// each ending in a lone short word, set as `how` says.
fn document(layout: &mut Layout, cx: &mut Context, how: How, paragraphs: u64) {
    let families = [FontFamilyName::named("Test Latin")];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut block = ComputedBlockStyle::new(&root);
    let first = ComputedStyle {
        font: FontGroup {
            size: 20.0,
            ..root.font
        },
        ..root
    };
    let mut letter = root;
    letter.line.initial_letter = InitialLetter {
        size: 2.0,
        sink: 2,
        ..InitialLetter::NONE
    };
    match how {
        How::Balanced => block.text_wrap_style = TextWrapStyle::Balance,
        How::BalancedFirstLine => {
            block.text_wrap_style = TextWrapStyle::Balance;
            block.line_clamp = LineClamp::Lines(9);
            block.first_line = Some(&first);
        }
        How::Pretty => {
            block.text_wrap_style = TextWrapStyle::Pretty;
            block.text_align = TextAlign::Justify;
        }
    }
    let size = |inline, block| BoxSize {
        inline,
        block,
        baseline: None,
    };
    let mut b = layout.builder(cx, NodeKey(0), &block, BuildOptions::default());
    if matches!(how, How::BalancedFirstLine) {
        b.open_box(NodeKey(1), &letter, None);
        b.text(NodeKey(2), "O");
        b.close_box();
    }
    for at in 0..paragraphs {
        let key = 16 * (at + 1);
        b.text(
            NodeKey(key),
            "official waffles AVAVA flee to an affable fiefdom at",
        );
        if at % 3 == 1 {
            b.float(NodeKey(key + 1), &root, FloatSide::Left, size(40.0, 30.0));
        }
        for more in 0..at % 5 {
            b.text(NodeKey(key + 2 + more), " fine AVA waffles officially");
        }
        b.text(NodeKey(key + 8), " a");
        b.line_break(NodeKey(key + 9));
    }
    assert!(b.finish().is_complete());
}

/// Balanced and pretty text, under either rule for pretty, breaks again
/// allocating nothing once warm, at any width broken at before (plan
/// step 26): the trials the driver takes back, their reshaped edges and
/// floats, the scorer's candidates and the plan keep their capacity,
/// and so does the host.
#[test]
fn balanced_and_pretty_text_allocates_nothing_warm() {
    let widths = [120.0, 300.0, 61.5, 900.0, 17.0, 180.0];
    for how in [How::Balanced, How::BalancedFirstLine, How::Pretty] {
        for pretty in [Pretty::Limited, Pretty::Even] {
            let mut cx = Context::new(collection());
            let mut config = Config::chrome_windows();
            config.pretty = pretty;
            cx.set_config(config);
            let mut layout = Layout::new();
            document(&mut layout, &mut cx, how, 12);
            let mut host = Floats {
                width: 0.0,
                placed: Vec::with_capacity(64),
                lowers: false,
            };
            let mut relayout = |layout: &mut Layout, cx: &mut Context| {
                let mut read = 0;
                for &width in &widths {
                    host.width = width;
                    host.placed.clear();
                    layout.break_lines(cx, Area::new(width), &mut host);
                    read += layout.lines().len();
                }
                read
            };
            let cold = count_allocations(|| {
                relayout(&mut layout, &mut cx);
            });
            assert!(cold > 0, "a first break grows the lines");
            let warm = count_allocations(|| {
                assert!(relayout(&mut layout, &mut cx) > 0);
            });
            assert_eq!(warm, 0, "{how:?}, {pretty:?}: breaking again allocated");
            let warm = count_allocations(|| document(&mut layout, &mut cx, how, 12));
            assert_eq!(warm, 0, "{how:?}, {pretty:?}: rebuilding allocated");
            let warm = count_allocations(|| {
                relayout(&mut layout, &mut cx);
            });
            assert_eq!(
                warm, 0,
                "{how:?}, {pretty:?}: breaking the rebuild allocated"
            );
        }
    }
}
