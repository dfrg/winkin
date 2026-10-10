//! Bidi tests. They pin:
//! - a line reordered by its levels, two bidi paragraphs included;
//! - a right-to-left line drawn from its end;
//! - a box split by reordering, with its edges on their own sides;
//! - preserved trailing white space at the paragraph's level.

use super::*;

/// A line holding two bidi paragraphs is reordered as one, by its clusters' levels.
///
/// U+001C ends a bidi paragraph but breaks no line. Blink reorders a line by
/// its items' levels the same way. Under `plaintext`, `بت␜XY` draws `␜تب`
/// and then `XY`, left to right, as Chrome 153 draws `אב␜XY`.
#[test]
fn a_line_of_two_bidi_paragraphs_is_reordered_as_one() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&ARABIC, 20.0);
    style.bidi.unicode_bidi = UnicodeBidi::Plaintext;
    let text = alloc::format!("{BEH}{TEH}\u{1C}XY");
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&style), &text);
    fixture.lay_out(&mut layout, 400.0);
    assert_eq!(texts(&layout).len(), 1);
    let input = layout.read_input();
    let record = &layout.line_records().lines[LineId::new(0)];
    let drawn: Vec<ClusterId> = items(&layout, 0)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Text)
        .flat_map(|item| {
            GlyphWalk::new(&input, LineId::new(0), record, item)
                .map(|glyph| glyph.cluster)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(drawn, [2, 1, 0, 3, 4].map(ClusterId::new));
}

/// A right-to-left line is set at its right and drawn from its logical end at the line box's left.
///
/// What hangs at its end hangs past the line box's left.
#[test]
fn a_right_to_left_line_is_drawn_from_its_end() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block_style = sized(&ARABIC, 20.0);
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&block_style)
    };
    let word: String = [BEH, TEH, SEEN, MEEM].iter().collect();
    let text = alloc::format!("{word} {word}");
    fixture.block_text(&mut layout, &block, &text);
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout).len(), 2);
    let line = items(&layout, 0);
    assert_eq!(line.len(), 1);
    assert!(line[0].level.is_rtl());
    // `start` is the right, so the line starts at the band less the word.
    assert_eq!(lefts(&layout), [20.0, 20.0]);
    let clusters: Vec<ClusterId> = {
        let input = layout.read_input();
        let record = &layout.line_records().lines[LineId::new(0)];
        GlyphWalk::new(&input, LineId::new(0), record, &line[0])
            .map(|glyph| glyph.cluster)
            .collect()
    };
    assert_eq!(
        clusters,
        [3, 2, 1, 0].map(ClusterId::new),
        "drawn from the logical end"
    );
    let xs: Vec<InlineLayoutUnit> = glyphs(&layout, 0).iter().map(|&(_, x)| x).collect();
    assert_eq!(xs, [px(0.0), px(10.0), px(20.0), px(30.0)]);
    // A preserved space at the end hangs past the left.
    let mut pre_wrap = sized(&ARABIC, 20.0);
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &pre_wrap,
            ..block
        },
        &text,
    );
    fixture.lay_out(&mut layout, 60.0);
    let line = items(&layout, 0);
    assert_eq!(line.len(), 2);
    assert!(line[0].flags.contains(FragmentItemFlags::HANGS));
    assert_eq!(line[0].inline, px(-10.0));
    assert_eq!(line[1].inline, px(0.0));
    assert_eq!(lefts(&layout)[0], 20.0);
}

/// A line's pieces are reordered by their levels (UAX #9 rule L2), as Blink's `BidiReorder` does.
///
/// `A<b>BCD</b>E`, the box overriding right to left in Ahem at 20 px, draws
/// `A`, then `DCB`, then `E`. The reversed item stands where its text does
/// and is drawn from its logical end. Its level is odd, the others' even.
#[test]
fn a_line_is_reordered_by_level() {
    use crate::style::{Direction, UnicodeBidi};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let overriding = reading(&root, Direction::Rtl, UnicodeBidi::BidiOverride);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &overriding, None);
        b.text(NodeKey(3), "BCD");
        b.close_box();
        b.text(NodeKey(4), "E");
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(
        places(&layout, 0),
        [
            ("A".into(), 0.0),
            ("D".into(), 20.0),
            ("C".into(), 40.0),
            ("B".into(), 60.0),
            ("E".into(), 80.0)
        ]
    );
    let levels: Vec<u8> = items(&layout, 0)
        .iter()
        .map(|item| item.level.get())
        .collect();
    assert_eq!(levels, [0, 1, 0]);
    // In a right-to-left paragraph, the Latin is at level 2 and the override
    // at 3. Reversing at 3, 2 and 1 in turn leaves the same order.
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&root)
    };
    fixture.build(&mut layout, &rtl, |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &overriding, None);
        b.text(NodeKey(3), "BCD");
        b.close_box();
        b.text(NodeKey(4), "E");
    });
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(
        places(&layout, 0),
        [
            ("A".into(), 0.0),
            ("D".into(), 20.0),
            ("C".into(), 40.0),
            ("B".into(), 60.0),
            ("E".into(), 80.0)
        ]
    );
    let levels: Vec<u8> = items(&layout, 0)
        .iter()
        .map(|item| item.level.get())
        .collect();
    assert_eq!(levels, [2, 3, 2]);
}

/// A box's bidi controls split the runs around them by their level, as
/// Blink's level-only line items for bidi controls do.
///
/// In `<b>BC</b><i>D</i>`, `b` overriding right to left and `i` isolated
/// left to right, `BC` is at 1 and `D` at 2. The LRI between them is at 0,
/// so `CB` and `D` reverse apart: `CBD`, where the clusters' levels alone
/// would reverse all three to `DCB`.
#[test]
fn a_box_control_below_its_neighbours_splits_the_runs() {
    use crate::style::{Direction, UnicodeBidi};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let overriding = reading(&root, Direction::Rtl, UnicodeBidi::BidiOverride);
    let isolated = reading(&root, Direction::Ltr, UnicodeBidi::Isolate);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &overriding, None);
        b.text(NodeKey(2), "BC");
        b.close_box();
        b.open_box(NodeKey(3), &isolated, None);
        b.text(NodeKey(4), "D");
        b.close_box();
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(
        places(&layout, 0),
        [("C".into(), 0.0), ("B".into(), 20.0), ("D".into(), 40.0)]
    );
}

/// A box that reordering splits becomes a box part on each side, as in
/// Blink's `UpdateBoxDataFragmentRange`.
///
/// In `A<b>B<i>CD</i></b><i>EF</i>G`, both `i`s override right to left. The
/// two runs join and reverse to `FEDC`, so `EF`, outside the painted box,
/// lands between its `B` and its `CD`. Chrome gives `[40, 80]` and
/// `[160, 240]`. The box's left edge goes on its leftmost part and its right
/// edge on its rightmost (`UpdateFragmentEdges`). So the first part is open
/// on its right and the second on its left.
#[test]
fn a_box_split_by_reordering_has_a_part_either_side() {
    use crate::style::{Direction, UnicodeBidi};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(40.0);
    let painted = ComputedStyle {
        paints: true,
        edges: EdgesGroup {
            border: Sides {
                left: 2.0,
                right: 3.0,
                ..Sides::ZERO
            },
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let overriding = reading(&root, Direction::Rtl, UnicodeBidi::BidiOverride);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &painted, None);
        b.text(NodeKey(3), "B");
        b.open_box(NodeKey(4), &overriding, None);
        b.text(NodeKey(5), "CD");
        b.close_box();
        b.close_box();
        b.open_box(NodeKey(6), &overriding, None);
        b.text(NodeKey(7), "EF");
        b.close_box();
        b.text(NodeKey(8), "G");
    });
    fixture.lay_out(&mut layout, 800.0);
    // The border puts two pixels before `B` and three after `C`, the box's
    // rightmost text.
    assert_eq!(
        box_parts(&layout, 0),
        [(2, 40.0, 82.0, false, true), (2, 162.0, 245.0, true, false)]
    );
    assert_eq!(
        places(&layout, 0),
        [
            ("A".into(), 0.0),
            ("B".into(), 42.0),
            ("F".into(), 82.0),
            ("E".into(), 122.0),
            ("D".into(), 162.0),
            ("C".into(), 202.0),
            ("G".into(), 245.0)
        ]
    );
}

/// A box's edges are physical, whichever way it reads.
///
/// Chrome's `AddBoxData` swaps them for a right-to-left box, so its left edge
/// is on its left and its right on its right. A right-to-left box in a
/// left-to-right paragraph opens with its start, its right side, and closes
/// with its end, its left side. Broken across two lines, the first line has
/// its right edge and the second its left.
#[test]
fn a_box_wears_its_edges_on_their_own_sides() {
    use crate::style::{Direction, UnicodeBidi};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let padded = ComputedStyle {
        paints: true,
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 10.0,
                right: 3.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..reading(&root, Direction::Rtl, UnicodeBidi::Normal)
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_box(NodeKey(2), &padded, None);
        b.text(NodeKey(3), "B");
        b.close_box();
        b.text(NodeKey(4), "C");
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(box_parts(&layout, 0), [(2, 20.0, 53.0, false, false)]);
    assert_eq!(
        places(&layout, 0),
        [("A".into(), 0.0), ("B".into(), 30.0), ("C".into(), 53.0)]
    );
    // Broken, the first line holds `XX` and the start's three. The second
    // holds the end's ten and `YY`.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &padded, None);
        b.text(NodeKey(2), "XX YY");
        b.close_box();
    });
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout), ["XX ", "YY"]);
    assert_eq!(box_parts(&layout, 0), [(1, 0.0, 43.0, true, false)]);
    assert_eq!(box_parts(&layout, 1), [(1, 0.0, 50.0, false, true)]);
    assert_eq!(places(&layout, 1)[0], ("Y".into(), 10.0));
}

/// Preserved white space ending a line takes the paragraph's level (UAX #9 rule L1), as in Chrome.
///
/// So it stays at the line's visual end however the text before it reads.
/// The case is `CD ` overridden right to left in a left-to-right paragraph.
/// The space hangs under `pre-wrap` and is content under `break-spaces`.
/// Either way it ends its item and splits from the text before it, as Blink's
/// `SplitTrailingBidiPreservedSpace` splits it. A collapsible space is
/// removed, and nothing else is reset.
#[test]
fn preserved_trailing_white_space_takes_the_paragraph_level() {
    use crate::style::{Direction, UnicodeBidi};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for collapse in [
        WhiteSpaceCollapse::Preserve,
        WhiteSpaceCollapse::BreakSpaces,
    ] {
        let root = ComputedStyle {
            text: TextGroup {
                white_space_collapse: collapse,
                ..ComputedStyle::initial().text
            },
            ..ahem(20.0)
        };
        let overriding = reading(&root, Direction::Rtl, UnicodeBidi::BidiOverride);
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "AB ");
            b.open_box(NodeKey(2), &overriding, None);
            b.text(NodeKey(3), "CD ");
            b.close_box();
            b.text(NodeKey(4), "EF");
        });
        fixture.lay_out(&mut layout, 120.0);
        assert_eq!(texts(&layout), ["AB CD ", "EF"], "{collapse:?}");
        let line = items(&layout, 0);
        let last = line.last().copied().expect("items");
        assert_eq!(last.clusters(), ClusterId::new(5)..ClusterId::new(6));
        assert_eq!(last.level.get(), 0, "{collapse:?}");
        assert_eq!(last.inline, px(100.0), "{collapse:?}");
        assert_eq!(
            places(&layout, 0),
            [
                ("A".into(), 0.0),
                ("B".into(), 20.0),
                ("D".into(), 60.0),
                ("C".into(), 80.0)
            ]
        );
    }
}

/// Returns line `n`'s text items as clusters, level, and left and right edges
/// in pixels from the area's line-left.
fn text_spans(layout: &Layout, n: usize) -> Vec<(Range<usize>, u8, f32, f32)> {
    let left = lefts(layout)[n];
    items(layout, n)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Text)
        .map(|item| {
            let clusters = item.clusters();
            let from = left + item.inline.to_px();
            (
                clusters.start.get()..clusters.end.get(),
                item.level.get(),
                from,
                from + item.size.to_px(),
            )
        })
        .collect()
}

/// `break-spaces` white space ending a line inside one item's text keeps that text's level, as in Chrome.
///
/// Chrome splits a line's trailing preserved white space off to take the
/// paragraph's level (UAX #9 rule L1) only where it ends an item or is all
/// its item holds on the line. In a right-to-left paragraph in 40 px Ahem
/// and 120 px, `XXX X` sets `XXX ` left to right at level 2, 160 px wide.
/// The line overflows, so it stands at the start, from -40 to 120, however
/// it is aligned. The second line's `X` aligns as asked. `XX  X` fits
/// `XX ` exactly, its space at the right. Left to right, the first line
/// stands from 0 to 160.
#[test]
fn break_spaces_white_space_inside_one_text_keeps_its_level() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut root = ahem(40.0);
    root.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    for (align, second) in [
        (TextAlign::Left, 0.0),
        (TextAlign::Center, 40.0),
        (TextAlign::Right, 80.0),
        (TextAlign::Start, 80.0),
        (TextAlign::End, 0.0),
        (TextAlign::Justify, 80.0),
    ] {
        let block = ComputedBlockStyle {
            direction: BaseDirection::Rtl,
            text_align: align,
            ..ComputedBlockStyle::new(&root)
        };
        fixture.block_text(&mut layout, &block, "XXX X");
        fixture.lay_out(&mut layout, 120.0);
        assert_eq!(texts(&layout), ["XXX ", "X"], "{align:?}");
        assert_eq!(
            text_spans(&layout, 0),
            [(0..4, 2, -40.0, 120.0)],
            "{align:?}"
        );
        assert_eq!(
            text_spans(&layout, 1),
            [(4..5, 2, second, second + 40.0)],
            "{align:?}"
        );
    }
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        text_align: TextAlign::Left,
        ..ComputedBlockStyle::new(&root)
    };
    fixture.block_text(&mut layout, &rtl, "XX  X");
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(texts(&layout), ["XX ", " X"]);
    assert_eq!(text_spans(&layout, 0), [(0..3, 2, 0.0, 120.0)]);
    assert_eq!(text_spans(&layout, 1), [(3..5, 2, 0.0, 80.0)]);
    let ltr = ComputedBlockStyle {
        text_align: TextAlign::Left,
        ..ComputedBlockStyle::new(&root)
    };
    fixture.block_text(&mut layout, &ltr, "XXX X");
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(text_spans(&layout, 0), [(0..4, 0, 0.0, 160.0)]);
}

/// `break-spaces` white space ending its item, or all its item holds on the line, takes the paragraph's level, as in Chrome.
///
/// In a right-to-left paragraph in 40 px Ahem and 120 px, Chrome sets the
/// space of `<span>XXX </span>X` and of `<span>XXX</span><span> X</span>`
/// at level 1, left of `XXX`: the space from -40 to 0 and `XXX` from 0 to
/// 120. Under `pre-wrap` the space of `XXX X` hangs there too, inside one
/// item.
#[test]
fn break_spaces_white_space_ending_its_item_takes_the_paragraph_level() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut root = ahem(40.0);
    root.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&root)
    };
    for (first, second) in [("XXX ", "X"), ("XXX", " X")] {
        fixture.build(&mut layout, &block, |b| {
            b.open_box(NodeKey(1), &root, None);
            b.text(NodeKey(2), first);
            b.close_box();
            b.open_box(NodeKey(3), &root, None);
            b.text(NodeKey(4), second);
            b.close_box();
        });
        fixture.lay_out(&mut layout, 120.0);
        assert_eq!(texts(&layout), ["XXX ", "X"], "{first:?}");
        assert_eq!(
            text_spans(&layout, 0),
            [(3..4, 1, -40.0, 0.0), (0..3, 2, 0.0, 120.0)],
            "{first:?}"
        );
    }
    let mut pre_wrap = root;
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &pre_wrap,
            ..block
        },
        "XXX X",
    );
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(
        text_spans(&layout, 0),
        [(3..4, 1, -40.0, 0.0), (0..3, 2, 0.0, 120.0)]
    );
}
