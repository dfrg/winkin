//! Unsafe edge tests. They pin:
//! - a kerned pair, a kern across U+200B, a ligature and joined Arabic
//!   broken inside measured as reshaped, with the paragraph's glyphs
//!   untouched;
//! - a line end reshaped back to its run and no further;
//! - hanging spaces at a break keeping the paragraph's glyphs, past the
//!   first only where the end must be exact, and a `break-spaces` space
//!   reshaped wherever it must;
//! - the text before end spaces reshaped without them only where the end
//!   must be exact, outlines aside, and always before removed spaces at a
//!   paragraph's end.

use super::*;
use crate::style::{FirstLineVariant, TextAlignLast};

/// Returns the glyph ids a line's piece over `clusters` draws, in cluster
/// order.
fn piece_ids(layout: &Layout, line: &LineRecord, clusters: Range<usize>) -> Vec<u32> {
    let lines = layout.line_records();
    let shape = lines
        .edges()
        .shapes
        .slice(line.shapes.clone())
        .iter()
        .find(|shape| shape.clusters() == (at(clusters.start)..at(clusters.end)))
        .expect("a piece over those clusters");
    (0..clusters.len())
        .flat_map(|k| {
            let word = lines.edges().words[EdgeClusterId::new(shape.first.get() + k)];
            match word.glyphs(&lines.edges().glyphs) {
                ClusterGlyphs::None => Vec::new(),
                ClusterGlyphs::One(id) => vec![id],
                ClusterGlyphs::Many(glyphs) => glyphs.iter().map(|g| g.id()).collect(),
            }
        })
        .collect()
}

/// A kerned pair broken by `break-all` is measured as drawn.
///
/// `A` before `V` kerns 2 px at 20 px, so `AVA` is 26 px in the paragraph
/// but 28 px as a line. At 26 px the line ends at `AV`; at 28 it takes
/// `AVA`, and the next line starts at the unsafe `V`.
#[test]
fn a_kerned_pair_broken_apart_is_measured_as_reshaped() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = break_all(&sized(&LATIN, 20.0));
    fixture.text(&mut layout, &style, "AVAVAVAV");
    let glyphs = glyph_words(&layout);
    assert!(
        layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .glyphs
            .word(at(1))
            .is_unsafe_to_break()
    );
    fixture.lay_out(&mut layout, 26.0);
    assert_eq!(texts(&layout), ["AV", "AV", "AV", "AV"]);
    fixture.lay_out(&mut layout, 28.0);
    assert_eq!(texts(&layout), ["AVA", "VAV", "AV"]);
    assert_eq!(widths(&layout), [28.0, 28.0, 18.0]);
    // The line's end is reshaped from the last safe-to-break cluster, the
    // second `A`, as Chrome's `ShapingLineBreaker` does. That `A` is drawn
    // unkerned, a whole half em, after the paragraph's `AV`.
    let first = records(&layout)[0].clone();
    let lines = layout.line_records();
    let shapes = lines.edges().shapes.slice(first.shapes.clone());
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].clusters(), at(2)..at(3));
    let latin = latin();
    assert_eq!(piece_ids(&layout, &first, 2..3), [latin.glyph('A')]);
    let last = lines.edges().advances[EdgeClusterId::new(shapes[0].first.get())];
    assert_eq!(last, TextUnit::from_px_truncated(10.0));
    for line in records(&layout).to_vec() {
        assert_eq!(line.width, shaped_whole(&mut fixture, &layout, &line));
    }
    // A line that starts at an unsafe `V` and ends before another is two
    // pieces. Its start piece ends at the safe `A`, where its end piece
    // starts. The first line's band is 30 px wide, the others' 20.
    let steps = Steps(vec![(0.0, 30.0), (20.0, 20.0)]);
    fixture.lay_out_with(&mut layout, Area::new(500.0), &mut { steps });
    assert_eq!(texts(&layout), ["AVA", "VA", "VA", "V"]);
    let second = records(&layout)[1].clone();
    let shapes = layout.line_records().edges().shapes.slice(second.shapes);
    let pieces: Vec<Range<ClusterId>> = shapes.iter().map(EdgeShape::clusters).collect();
    assert_eq!(pieces, [at(3)..at(4), at(4)..at(5)]);
    assert_eq!(second.width, LayoutUnit::from_px(20.0));
    for line in records(&layout).to_vec() {
        assert_eq!(line.width, shaped_whole(&mut fixture, &layout, &line));
    }
    assert_eq!(glyph_words(&layout), glyphs);
}

/// A kern across a U+200B in the text, which is shaped with it, is reshaped
/// at a break there.
///
/// A line breaking after the U+200B ends with the `A` reshaped alone, 10 px
/// instead of 8, as Chrome's `ShapingLineBreaker` reshapes it.
#[test]
fn a_kern_across_a_zero_width_space_is_reshaped_where_a_line_breaks_there() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.text(&mut layout, &sized(&LATIN, 20.0), "A\u{200B}V");
    let glyphs = glyph_words(&layout);
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(widths(&layout), [18.0], "kerned on one line");
    fixture.lay_out(&mut layout, 12.0);
    assert_eq!(texts(&layout), ["A\u{200B}", "V"]);
    assert_eq!(widths(&layout), [10.0, 10.0]);
    for line in records(&layout).to_vec() {
        assert_eq!(line.width, shaped_whole(&mut fixture, &layout, &line));
    }
    assert_eq!(glyph_words(&layout), glyphs);
}

/// Exclusions whose band width changes down the block, as placed floats
/// make it.
///
/// Each `(top, width)` sets the band's width from `top` down.
struct Steps(Vec<(f32, f32)>);

impl Exclusions for Steps {
    fn band(&self, _line: usize, block: BlockExtents) -> InlineExtents {
        let right = self
            .0
            .iter()
            .rev()
            .find(|(top, _)| block.start >= *top)
            .map_or(f32::INFINITY, |&(_, width)| width);
        InlineExtents { left: 0.0, right }
    }
    fn below(&self, _top: f32) -> Option<f32> {
        None
    }
    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        NoExclusions.place(float)
    }
    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(0)
    }
    fn rewind(&mut self, _to: ExclusionsCheckpoint) {}
}

/// A ligature broken inside by `break-all` is reshaped letter by letter.
///
/// `ffi` is one 15 px glyph at 20 px, so the paragraph gives its last two
/// letters no width. At 12 px each line holds one letter drawn alone.
#[test]
fn a_ligature_broken_inside_is_measured_as_reshaped() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = break_all(&sized(&LATIN, 20.0));
    fixture.text(&mut layout, &style, "office");
    let glyphs = glyph_words(&layout);
    fixture.lay_out(&mut layout, 12.0);
    assert_eq!(texts(&layout), ["o", "f", "f", "i", "c", "e"]);
    assert!(
        records(&layout)
            .iter()
            .all(|line| !line.flags.contains(LineFlags::OVERFLOWS))
    );
    let latin = latin();
    let lines = records(&layout).to_vec();
    assert_eq!(piece_ids(&layout, &lines[1], 1..2), [latin.glyph('f')]);
    assert_eq!(piece_ids(&layout, &lines[2], 2..3), [latin.glyph('f')]);
    assert_eq!(piece_ids(&layout, &lines[3], 3..4), [latin.glyph('i')]);
    for line in &lines {
        assert_eq!(line.width, shaped_whole(&mut fixture, &layout, line));
    }
    // At 26 px the break falls after the ligature, where it is safe, so
    // nothing is reshaped.
    fixture.lay_out(&mut layout, 26.0);
    assert_eq!(texts(&layout), ["offi", "ce"]);
    assert!(layout.line_records().edges().shapes.is_empty());
    assert_eq!(glyph_words(&layout), glyphs);
}

/// Arabic broken inside a word keeps its joining forms.
///
/// The reshaped pieces take the paragraph on either side as context, as
/// Chrome gives HarfBuzz the whole text, so letters at a break stay medial.
#[test]
fn arabic_broken_inside_a_word_keeps_its_joining() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = break_all(&sized(&ARABIC, 20.0));
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let word: String = [BEH, TEH, SEEN, MEEM, BEH].iter().collect();
    fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), &word));
    let glyphs = glyph_words(&layout);
    fixture.lay_out(&mut layout, 20.0);
    let ranges: Vec<Range<ClusterId>> = records(&layout).iter().map(LineRecord::clusters).collect();
    assert_eq!(ranges, [at(0)..at(2), at(2)..at(4), at(4)..at(5)]);
    assert_eq!(widths(&layout), [20.0, 20.0, 10.0]);
    let arabic = arabic();
    let lines = records(&layout).to_vec();
    assert_eq!(
        piece_ids(&layout, &lines[0], 0..2),
        [
            arabic.form_glyph(BEH, Form::Initial),
            arabic.form_glyph(TEH, Form::Medial)
        ]
    );
    assert_eq!(
        piece_ids(&layout, &lines[1], 2..4),
        [
            arabic.form_glyph(SEEN, Form::Medial),
            arabic.form_glyph(MEEM, Form::Medial)
        ]
    );
    for line in &lines {
        assert_eq!(line.width, shaped_whole(&mut fixture, &layout, line));
    }
    assert_eq!(glyph_words(&layout), glyphs);
}

/// A line ending inside an Arabic word is reshaped from the word's run
/// start and no further back.
///
/// The family's first font draws the spaces, so each word and space is its
/// own run. Reshaping further back would set the space in the Arabic font,
/// half an em instead of a third, and the line would overflow at 60 px.
#[test]
fn a_line_end_is_reshaped_back_to_its_run_and_no_further() {
    const NARROW_THEN_ARABIC: [FontFamilyName<'static>; 2] = [
        FontFamilyName::Named(Cow::Borrowed("Test Narrow")),
        FontFamilyName::Named(Cow::Borrowed("Test Arabic")),
    ];
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = break_all(&sized(&NARROW_THEN_ARABIC, 20.0));
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let text: String = [BEH, TEH, SEEN, ' ', MEEM, BEH, TEH].iter().collect();
    fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), &text));
    let glyphs = glyph_words(&layout);
    let runs = &layout.shaped().text(FirstLineVariant::Standard).runs;
    assert_eq!(runs.len(), 3, "each word and the space a run");
    // Room for the first word, the space and two letters of the second:
    // three joined 10 px letters, a 6.66 px space and two more letters.
    fixture.lay_out(&mut layout, 57.0);
    let ranges: Vec<Range<ClusterId>> = records(&layout).iter().map(LineRecord::clusters).collect();
    assert_eq!(ranges, [at(0)..at(6), at(6)..at(7)]);
    let lines = records(&layout).to_vec();
    let edges = layout.line_records().edges();
    let pieces: Vec<Range<ClusterId>> = edges
        .shapes
        .slice(lines[0].shapes.clone())
        .iter()
        .map(EdgeShape::clusters)
        .collect();
    assert_eq!(pieces, [at(4)..at(6)]);
    let arabic = arabic();
    assert_eq!(
        piece_ids(&layout, &lines[0], 4..6),
        [
            arabic.form_glyph(MEEM, Form::Initial),
            arabic.form_glyph(BEH, Form::Medial)
        ]
    );
    let width = widths(&layout)[0];
    assert!((width - 56.66).abs() < 1.0 / 64.0, "{width}");
    assert_eq!(glyph_words(&layout), glyphs);
}

/// A break after spaces kerned against the letter after them is reshaped
/// as Chrome's `ShapingLineBreaker` and `TruncateLineEndResult` decide.
///
/// Spaces that hang keep the paragraph's glyphs, past a second one only
/// where the line needs its exact end. Under `break-spaces` the space is
/// reshaped wherever the line needs its exact end.
///
/// In a font whose space kerns 4 px closer to a `V` at 20 px, `AA VV` and
/// `AA  VV` in 30 px end their first line after the spaces. A space is
/// 6 px kerned and 10 shaped alone.
///
/// Chrome 155, Times New Roman at 20 px, whose space is 3.91 px kerned
/// against an `A` and 5 alone:
/// - `XX AXX` in 60 px keeps its space at 3.91 px under `pre-wrap`,
///   `normal` and `pre-line`, at every alignment and with a background or
///   an underline;
/// - `XX  AXX` under `pre-wrap` in 62 px reshapes its second space to 5 px
///   at `center` or with a background, and keeps it at 3.91 at `start`;
/// - under `break-spaces`, `XX AXX` keeps its space at 3.92 px at `start`
///   and `left`, and reshapes it to 5.02 at `right`, `center`, `end` and
///   `justify`, with `text-align-last: center`, and with a background or an
///   underline.
#[test]
fn a_break_at_a_space_is_reshaped_only_where_chrome_reshapes_it() {
    let mut font = TestFont::new("Test Spaced", &[(0x20, 0x7E)]);
    font.kerning = vec![(' ', 'V', -200)];
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(font.build()).is_ok());
    let mut fixture = Fixture::from_collection(
        Collection::new().with_layer(layer.snapshot()),
        &[],
        StageCheck::Placed(check),
    );
    const SPACED: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Spaced"))];
    let collapse = sized(&SPACED, 20.0);
    let mut pre_wrap = collapse;
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut break_spaces = collapse;
    break_spaces.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    let painted = |style: ComputedStyle<'static>| ComputedStyle {
        paints: true,
        ..style
    };
    let decorated = |style: ComputedStyle<'static>| ComputedStyle {
        decorates: true,
        ..style
    };
    let (start, center) = (TextAlign::Start, TextAlign::Center);
    let one = "AA VV";
    let two = "AA  VV";
    let mut layout = Layout::new();
    // The style, the text, the alignment, whether the line is reshaped, and
    // the hanging space and the line's width in px.
    for (style, text, align, reshaped, hang, width) in [
        (collapse, one, start, false, 6.0, 20.0),
        (collapse, one, center, false, 6.0, 20.0),
        (collapse, one, TextAlign::End, false, 6.0, 20.0),
        (collapse, one, TextAlign::Justify, false, 6.0, 20.0),
        (painted(collapse), one, start, false, 6.0, 20.0),
        (pre_wrap, one, start, false, 6.0, 20.0),
        (pre_wrap, one, center, false, 6.0, 20.0),
        (painted(pre_wrap), one, start, false, 6.0, 20.0),
        (decorated(pre_wrap), one, center, false, 6.0, 20.0),
        (pre_wrap, two, start, false, 16.0, 20.0),
        (pre_wrap, two, center, true, 20.0, 20.0),
        (painted(pre_wrap), two, start, true, 20.0, 20.0),
        (break_spaces, one, start, false, 0.0, 26.0),
        (break_spaces, one, TextAlign::Left, false, 0.0, 26.0),
        (break_spaces, one, TextAlign::Right, true, 0.0, 30.0),
        (break_spaces, one, center, true, 0.0, 30.0),
        (painted(break_spaces), one, start, true, 0.0, 30.0),
        (decorated(break_spaces), one, start, true, 0.0, 30.0),
    ] {
        let block = ComputedBlockStyle {
            text_align: align,
            ..ComputedBlockStyle::new(&style)
        };
        fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), text));
        let says = alloc::format!(
            "{text:?} {align:?} {:?} paints {} decorates {}",
            style.text.white_space_collapse,
            style.paints,
            style.decorates
        );
        let glyphs = glyph_words(&layout);
        let first_v = text.len() - 2;
        assert!(
            layout
                .shaped()
                .text(FirstLineVariant::Standard)
                .glyphs
                .word(at(first_v))
                .is_unsafe_to_break(),
            "{says}"
        );
        fixture.lay_out(&mut layout, 30.0);
        let line = &text[..first_v];
        assert_eq!(texts(&layout), [line, "VV"], "{says}");
        let first = &records(&layout)[0];
        assert_eq!(!first.shapes.is_empty(), reshaped, "{says}");
        assert_eq!(first.hang.space, LayoutUnit::from_px(hang), "{says}");
        assert_eq!(first.width, LayoutUnit::from_px(width), "{says}");
        assert_eq!(glyph_words(&layout), glyphs, "{says}");
    }
}

/// How [`the_text_before_end_spaces_is_reshaped_only_where_the_line_needs_its_exact_end`]
/// builds `XY XY XY`.
#[derive(Copy, Clone, Debug)]
enum Spaced {
    /// In the block's own style.
    Plain,
    /// In one box of the style.
    Around,
    /// With the second `XY` alone in a box of the style.
    Word,
    /// In a plain box, in a box of the style.
    Outer,
    /// With the space after the second `XY` in a text node of its own.
    OwnNode,
    /// With the last `XY` alone in a box of the style.
    Split,
}

/// The text before the spaces a line ends with is reshaped without them
/// only where the line needs its exact end, as Chrome's
/// `TruncateLineEndResult` and `ShapingLineBreaker` decide.
///
/// In a font whose `Y` kerns 4 px closer to a space at 20 px, `XY XY XY`
/// in 50 px ends its first line with a space that hangs. Reshaped without
/// it, the line is 46 px of text, its last `Y` 10 px. Kept, it is 42, as
/// the paragraph shaped it. The paragraph's glyphs are never touched.
///
/// Chrome 155 does the same with `XRAY bravo` in Times New Roman at 20 px,
/// 90 px wide, whose `Y` is 0.75 px narrower kerned against a space:
/// - reshaped under `right`, `end`, `center` and `justify`, and under
///   `start` with `text-align-last: center`;
/// - reshaped with a background or an underline on the block, a background
///   on a span holding `XRAY `, or an underline on a span around a span
///   holding it;
/// - reshaped at `center` with `bravo` in a span of its own;
/// - kept under `start` and `left`, with emphasis marks, with an outline on
///   the block or on a span around the text, with a background on a span
///   holding only `XRAY`, and at `center` with the space in a span of its
///   own.
///
/// Under `pre-wrap` the space stays and hangs, and Chrome 155 reshapes the
/// `Y` as it does a removed space's, except at `center` with `bravo` in a
/// span of its own, where it keeps the kern: the text before it fits whole
/// and is not broken.
#[test]
fn the_text_before_end_spaces_is_reshaped_only_where_the_line_needs_its_exact_end() {
    let mut font = TestFont::new("Test Spaced", &[(0x20, 0x7E)]);
    font.kerning = vec![('Y', ' ', -200)];
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(font.build()).is_ok());
    let mut fixture = Fixture::from_collection(
        Collection::new().with_layer(layer.snapshot()),
        &[],
        StageCheck::Placed(check),
    );
    const SPACED: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Spaced"))];
    let plain = sized(&SPACED, 20.0);
    let mut preserved = plain;
    preserved.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut marked = plain;
    marked.text.emphasis.marks = true;
    let painted = ComputedStyle {
        paints: true,
        ..plain
    };
    let decorated = ComputedStyle {
        decorates: true,
        ..plain
    };
    let outlined = ComputedStyle {
        outlines: true,
        ..plain
    };
    let painted_preserved = ComputedStyle {
        paints: true,
        ..preserved
    };
    let (start, last) = (TextAlign::Start, TextAlignLast::Auto);
    let mut layout = Layout::new();
    for (style, spaced, align, reshaped) in [
        (&plain, Spaced::Plain, (start, last), false),
        (&plain, Spaced::Plain, (TextAlign::Left, last), false),
        (&plain, Spaced::Plain, (TextAlign::Right, last), true),
        (&plain, Spaced::Plain, (TextAlign::Center, last), true),
        (&plain, Spaced::Plain, (TextAlign::End, last), true),
        (&plain, Spaced::Plain, (TextAlign::Justify, last), true),
        (&plain, Spaced::Plain, (start, TextAlignLast::Center), true),
        (&plain, Spaced::Plain, (start, TextAlignLast::Left), false),
        (&painted, Spaced::Plain, (start, last), true),
        (&decorated, Spaced::Plain, (start, last), true),
        (&marked, Spaced::Plain, (start, last), false),
        (&painted, Spaced::Around, (start, last), true),
        (&painted, Spaced::Word, (start, last), false),
        (&decorated, Spaced::Outer, (start, last), true),
        (&decorated, Spaced::Word, (start, last), false),
        (&plain, Spaced::OwnNode, (TextAlign::Center, last), false),
        (&outlined, Spaced::Plain, (start, last), false),
        (&outlined, Spaced::Around, (start, last), false),
        (&plain, Spaced::Split, (TextAlign::Center, last), true),
        (&preserved, Spaced::Plain, (start, last), false),
        (&preserved, Spaced::Plain, (TextAlign::Center, last), true),
        (&preserved, Spaced::Plain, (TextAlign::Justify, last), true),
        (&painted_preserved, Spaced::Plain, (start, last), true),
        (&preserved, Spaced::Split, (TextAlign::Center, last), false),
        (
            &preserved,
            Spaced::OwnNode,
            (TextAlign::Center, last),
            false,
        ),
    ] {
        let root = if matches!(spaced, Spaced::Plain | Spaced::Split | Spaced::OwnNode) {
            style
        } else {
            &plain
        };
        let block = ComputedBlockStyle {
            text_align: align.0,
            text_align_last: align.1,
            ..ComputedBlockStyle::new(root)
        };
        fixture.build(&mut layout, &block, |b| match spaced {
            Spaced::Plain => b.text(NodeKey(1), "XY XY XY"),
            Spaced::Around => {
                b.open_box(NodeKey(1), style, None);
                b.text(NodeKey(2), "XY XY XY");
                b.close_box();
            }
            Spaced::Word => {
                b.text(NodeKey(1), "XY ");
                b.open_box(NodeKey(2), style, None);
                b.text(NodeKey(3), "XY");
                b.close_box();
                b.text(NodeKey(4), " XY");
            }
            Spaced::Outer => {
                b.open_box(NodeKey(1), style, None);
                b.open_box(NodeKey(2), &plain, None);
                b.text(NodeKey(3), "XY XY XY");
                b.close_box();
                b.close_box();
            }
            Spaced::OwnNode => {
                b.text(NodeKey(1), "XY XY");
                b.text(NodeKey(2), " ");
                b.text(NodeKey(3), "XY");
            }
            Spaced::Split => {
                b.text(NodeKey(1), "XY XY ");
                b.open_box(NodeKey(2), style, None);
                b.text(NodeKey(3), "XY");
                b.close_box();
            }
        });
        let glyphs = glyph_words(&layout);
        fixture.lay_out(&mut layout, 50.0);
        let says = alloc::format!(
            "{spaced:?} {align:?} paints {} outlines {} decorates {} {:?}",
            style.paints,
            style.outlines,
            style.decorates,
            style.text.white_space_collapse
        );
        assert_eq!(texts(&layout), ["XY XY ", "XY"], "{says}");
        let first = &records(&layout)[0];
        assert_eq!(!first.shapes.is_empty(), reshaped, "{says}");
        if align.0 != TextAlign::Justify {
            let width = if reshaped { 46.0 } else { 42.0 };
            assert_eq!(widths(&layout), [width, 20.0], "{says}");
        }
        assert_eq!(glyph_words(&layout), glyphs, "{says}");
    }
}

/// A removed space at a paragraph's end is reshaped away at any alignment.
///
/// Chrome removes the collapsible spaces a block or a forced break ends
/// with before it shapes, so nothing kerns against them. Chrome 155,
/// `XRAY ` in Times New Roman at 20 px, start-aligned: the `Y` is 14.45 px
/// at the block's end and before a `<br>`, its own advance, where it is
/// 13.70 kerned before a space mid-line.
#[test]
fn a_removed_space_at_a_paragraphs_end_is_reshaped_away() {
    let mut font = TestFont::new("Test Spaced", &[(0x20, 0x7E)]);
    font.kerning = vec![('Y', ' ', -200)];
    let mut layer = LayerBuilder::new(Role::Application);
    assert!(layer.add_data(font.build()).is_ok());
    let mut fixture = Fixture::from_collection(
        Collection::new().with_layer(layer.snapshot()),
        &[],
        StageCheck::Placed(check),
    );
    const SPACED: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Spaced"))];
    let style = sized(&SPACED, 20.0);
    let block = ComputedBlockStyle::new(&style);
    let mut layout = Layout::new();
    fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), "XY XY "));
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(widths(&layout), [46.0]);
    fixture.build(&mut layout, &block, |b| {
        b.text(NodeKey(1), "XY XY ");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "XY");
    });
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(widths(&layout), [46.0, 20.0]);
}
