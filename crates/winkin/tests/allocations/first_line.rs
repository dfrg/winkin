//! A first line restyled and transformed in its own variant allocates
//! nothing warm.

use super::count_allocations;
use super::test_fonts::{self, TestFont, ahem_fallback};
use fontwich::Collection;
use winkin::style::LengthPercentage;
use winkin::style::{
    ComputedStyle, FontFamilyName, FontGroup, InitialLetter, LineHeight, TextCase, VerticalAlign,
    WordBreak,
};
use winkin::{
    Area, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

/// Ahem, and Latin with ligatures and a kerning pair.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E), (0xDF, 0xDF)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    latin.kerning = vec![('A', 'V', -100)];
    test_fonts::collection(&[latin], ahem_fallback())
}

/// Builds a block of `words` words whose `::first-line` restyles its font,
/// spacing, height and transform, opening with an initial letter.
///
/// It holds the block's text, a span in capitals that the first line sets
/// as written, a span raised on the first line only, and text that breaks
/// anywhere, so the first line ends inside ligatures and kerned pairs and
/// reshapes its edges.
fn document(layout: &mut Layout, cx: &mut Context, words: usize) {
    let families = [FontFamilyName::named("Test Latin")];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut anywhere = root;
    anywhere.text.word_break = WordBreak::BreakAll;
    let mut first = ComputedStyle {
        font: FontGroup {
            size: 26.0,
            ..root.font
        },
        ..root
    };
    first.line.height = LineHeight::Factor(1.8);
    first.text.letter_spacing = LengthPercentage::from_px(1.25);
    first.text.transform.case = TextCase::Uppercase;
    let mut upper = root;
    upper.text.transform.case = TextCase::Uppercase;
    let mut upper_first = first;
    upper_first.text.transform.case = TextCase::None;
    let mut raised_first = first;
    raised_first.line.vertical_align = VerticalAlign::Super;
    let mut anywhere_first = first;
    anywhere_first.text.word_break = WordBreak::BreakAll;
    // A first letter, larger on the first line, dropped into three lines:
    // sized to them and fitted to its outline's ink, in Ahem, hinted at
    // each size where the platform hints.
    let ahem = [FontFamilyName::named("Ahem")];
    let dropped = InitialLetter {
        size: 3.0,
        sink: 2,
        ..InitialLetter::NONE
    };
    let mut letter = upper;
    letter.font.families = &ahem;
    letter.font.size = 40.0;
    letter.line.initial_letter = dropped;
    let mut letter_first = first;
    letter_first.font.families = &ahem;
    letter_first.font.size = 52.0;
    letter_first.line.initial_letter = dropped;
    let mut b = layout.builder(
        cx,
        NodeKey(0),
        &ComputedBlockStyle {
            first_line: Some(&first),
            ..ComputedBlockStyle::new(&root)
        },
        BuildOptions::default(),
    );
    b.set_first_letter(NodeKey(u64::MAX), &letter, Some(&letter_first));
    for at in 0..words {
        let key = 8 * at as u64;
        b.text(NodeKey(key), "office straße ");
        b.open_box(NodeKey(key + 1), &upper, Some(&upper_first));
        b.text(NodeKey(key + 2), "flat AVAV ");
        b.close_box();
        b.open_box(NodeKey(key + 3), &root, Some(&raised_first));
        b.text(NodeKey(key + 4), "up ");
        b.close_box();
        b.open_box(NodeKey(key + 5), &anywhere, Some(&anywhere_first));
        b.text(NodeKey(key + 6), "affluent waffles ");
        b.close_box();
    }
    assert!(b.finish().is_complete());
}

/// Every line's glyphs and clusters, read back as a painter does.
fn read(layout: &Layout) -> f32 {
    let mut sum = 0.0;
    for line in layout.lines() {
        let metrics = line.metrics();
        sum += metrics.width + metrics.height();
        for item in line.all_items() {
            if let Item::Text(run) = item {
                sum += run.inline().left + run.advance() + run.baseline();
                for glyph in run.glyphs() {
                    sum += glyph.id as f32 + glyph.x + glyph.y + glyph.advance;
                }
                for cluster in run.clusters() {
                    sum += cluster.inline().left + cluster.advance();
                }
            }
        }
    }
    sum
}

/// A block whose first line restyles its font, spacing, height, shift
/// and transform rebuilds, breaks again and reads back allocating
/// nothing once warm: the first line's text and map, its runs, its
/// shaping and its measurements keep their capacity, and so do the
/// reshaped edges of its lines.
#[test]
fn a_restyled_first_line_allocates_nothing_warm() {
    let widths = [37.0, 90.5, 160.0, 300.0, 11.0, 800.0];
    let mut cx = Context::new(collection());
    let mut layout = Layout::new();
    let cold = count_allocations(|| document(&mut layout, &mut cx, 12));
    assert!(cold > 0, "a cold layout grows");
    for &width in &widths {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        read(&layout);
    }
    let warm = count_allocations(|| document(&mut layout, &mut cx, 12));
    assert_eq!(warm, 0, "rebuilding allocated");
    let warm = count_allocations(|| document(&mut layout, &mut cx, 9));
    assert_eq!(warm, 0, "rebuilding something smaller allocated");
    document(&mut layout, &mut cx, 12);
    for &width in &widths {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        read(&layout);
    }
    let warm = count_allocations(|| {
        for &width in &widths {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        }
    });
    assert_eq!(warm, 0, "breaking again allocated");
    for &width in &widths {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        let mut sum = 0.0;
        let warm = count_allocations(|| sum = read(&layout));
        assert!(sum.is_finite());
        assert_eq!(warm, 0, "at {width}: reading back allocated");
    }
}
