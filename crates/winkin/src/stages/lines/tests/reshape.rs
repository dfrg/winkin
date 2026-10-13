//! Unsafe edge tests. They pin:
//! - a kerned pair, a ligature and joined Arabic broken inside measured as
//!   reshaped, with the paragraph's glyphs untouched;
//! - a line end reshaped back to its run and no further;
//! - a break at a space reshaped only where the end must be exact;
//! - a removed space leaving the letter before it unkerned.

use super::*;
use crate::style::FirstLineVariant;

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
/// `A` before `V` is kerned back two pixels at 20 px. So `AVA` measures
/// 26 px in the paragraph and 28 as a line, whose last `A` has no `V` after
/// it. At 26 px the line does not fit as drawn, and the breaker steps back
/// to `AV`. At 28 it takes `AVA`, and the next line starts at the unsafe
/// `V`, reshaped on its own. Every line is as wide as its text shaped
/// whole, and the paragraph's glyphs are untouched.
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
    // second `A`, as Chrome's `ShapingLineBreaker` does. The lone `A` is
    // drawn a whole half em, unkerned, after the paragraph's `AV`.
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
    // A line that starts at an unsafe `V` and ends before another. Its
    // start window ends at the safe `A`, where its end window starts, so
    // the line is two pieces meeting there. The first line has a band of
    // 30, the others 20.
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

/// A kern across a caller's U+200B, which is shaped with its text, is
/// reshaped at a break there.
///
/// `A` is kerned against the `V` past it, so the paragraph measures `A`
/// 8 px at 20 px. A line breaking after the U+200B ends with the `A`
/// reshaped alone, a whole half em, as Chrome's `ShapingLineBreaker`
/// reshapes it. The paragraph's glyphs are untouched.
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

/// Floats a host has placed, as bands that change down the block.
///
/// Each `(top, width)` is the band's width from `top` on.
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
/// `ffi` is one glyph of 15 px at 20 px on its first `f`. So the paragraph
/// measures `o|f` as 25 px and the rest of the ligature as nothing. At
/// 12 px every line holds one letter as drawn alone: `f`, `f`, then `i`.
/// The paragraph's measure would give `ffi` a line that overflows.
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
/// The reshaped pieces are shaped with the paragraph either side as
/// context, as Chrome gives HarfBuzz the whole text. So the letters at the
/// break stay medial, and the lines are exactly as wide as the joined
/// forms. A letter set alone would be wider.
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
/// Here the family's first font draws the spaces, so every word and space
/// is its own shaping run. A piece starting at its run's start joins what
/// is before it as the run did. Widening it would pull the space and the
/// word before into the Arabic font, whose space is half an em, not a
/// third. The line would then measure 60 px, too wide for its room.
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
    // three joined letters of 10 px, a space of 6.66 and two more letters.
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

/// A break at a space is reshaped only where the alignment needs the
/// line's exact end, as Chrome's `SetDontReshapeEndIfAtSpace` decides.
///
/// `text-align: end`, `center` and `justify` need it. A space kerned
/// against the letter after it shows the difference.
#[test]
fn a_break_at_a_space_is_reshaped_only_where_the_end_must_be_exact() {
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
    let style = sized(&SPACED, 20.0);
    let mut layout = Layout::new();
    for (align, reshaped) in [
        (TextAlign::Start, false),
        (TextAlign::Left, false),
        (TextAlign::End, true),
        (TextAlign::Center, true),
        (TextAlign::Justify, true),
    ] {
        let block = ComputedBlockStyle {
            text_align: align,
            ..ComputedBlockStyle::new(&style)
        };
        fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), "AA VV"));
        assert!(
            layout
                .shaped()
                .text(FirstLineVariant::Standard)
                .glyphs
                .word(at(3))
                .is_unsafe_to_break()
        );
        fixture.lay_out(&mut layout, 25.0);
        assert_eq!(texts(&layout), ["AA ", "VV"], "{align:?}");
        let first = &records(&layout)[0];
        assert_eq!(first.shapes.is_empty(), !reshaped, "{align:?}");
        // What hangs is its own advance as drawn: 10 px shaped alone, and 6
        // in the paragraph, kerned against the `V` after it.
        let hang = if reshaped { 10.0 } else { 6.0 };
        assert_eq!(first.hang.space, LayoutUnit::from_px(hang), "{align:?}");
        assert_eq!(first.width, LayoutUnit::from_px(20.0), "{align:?}");
    }
}

/// A line's removed end space no longer kerns the letter before it.
///
/// CSS Text 3 removes the collapsible space a line ends with. Where the
/// paragraph kerned the letter before it against the space, the line is
/// reshaped without it. The letter keeps its own advance, as at a
/// paragraph's end. The paragraph's glyphs are never touched.
///
/// - In a font whose `Y` kerns 4 px closer to a space at 20 px, `XY XY XY`
///   in 50 px ends its first line with a 10 px `Y`. The line is 46 px of
///   text where the paragraph's shaping made 42, at any alignment.
/// - The first `Y`, mid-line, keeps its kern. A preserved space stays on
///   the line, so it keeps the kern too and nothing is reshaped.
/// - Chrome 153 does the same where the line needs its exact end: a
///   background, a decoration, or an alignment other than start. Elsewhere
///   it keeps the kern. In Times New Roman at 20 px, whose `Y` kerns
///   0.74 px against a space, `XRAY ` ends at 917.45 with a background and
///   916.70 without (probe `transformbreak` block 4).
#[test]
fn a_removed_space_leaves_the_letter_before_it_unkerned() {
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
    let collapsed = sized(&SPACED, 20.0);
    let mut preserved = collapsed;
    preserved.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut layout = Layout::new();
    for (style, align, width, reshaped) in [
        (&collapsed, TextAlign::Start, 46.0, true),
        (&collapsed, TextAlign::Center, 46.0, true),
        (&collapsed, TextAlign::End, 46.0, true),
        (&preserved, TextAlign::Start, 42.0, false),
    ] {
        let block = ComputedBlockStyle {
            text_align: align,
            ..ComputedBlockStyle::new(style)
        };
        fixture.build(&mut layout, &block, |b| b.text(NodeKey(1), "XY XY XY"));
        let glyphs = glyph_words(&layout);
        fixture.lay_out(&mut layout, 50.0);
        let says = alloc::format!("{:?} {align:?}", style.text.white_space_collapse);
        assert_eq!(texts(&layout), ["XY XY ", "XY"], "{says}");
        assert_eq!(widths(&layout), [width, 20.0], "{says}");
        let first = &records(&layout)[0];
        assert_eq!(!first.shapes.is_empty(), reshaped, "{says}");
        assert_eq!(glyph_words(&layout), glyphs, "{says}");
    }
}
