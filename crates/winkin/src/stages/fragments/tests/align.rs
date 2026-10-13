//! Alignment and tab tests, in Ahem's numbers. They pin:
//! - `text-align` and `text-align-last` on every line;
//! - trailing white space as Chrome aligns it, hanging or taking room;
//! - tabs as items reaching to their stops, and a tabbed relayout;
//! - a ruby column's overhang reaching into a tab beside it.

use super::*;

/// A line of 70 px in a band of 100 is set at 0, 15 and 30 by `start`, `center` and `end`.
///
/// `left` and `right` set it at their sides. Its items keep their places in
/// the line. An overflowing line is set at its start.
#[test]
fn a_line_is_set_at_its_start_its_center_or_its_end() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for (align, left) in [
        (TextAlign::Start, 0.0),
        (TextAlign::Center, 15.0),
        (TextAlign::End, 30.0),
        (TextAlign::Left, 0.0),
        (TextAlign::Right, 30.0),
        (TextAlign::Justify, 0.0),
    ] {
        let block = aligned(align, TextAlignLast::Auto);
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem(10.0),
                ..block
            },
            "XXX XXX",
        );
        fixture.lay_out(&mut layout, 100.0);
        assert_eq!(lefts(&layout), [left], "{align:?}");
        assert_eq!(items(&layout, 0).len(), 1, "one run of text");
        assert_eq!(items(&layout, 0)[0].inline, InlineLayoutUnit::ZERO);
        assert_eq!(items(&layout, 0)[0].size, LayoutUnit::from_px(70.0));
        // A line too wide for its band is set at its start, whatever the
        // alignment.
        fixture.lay_out(&mut layout, 50.0);
        let lines = texts(&layout);
        assert_eq!(lines, ["XXX ", "XXX"]);
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem(10.0),
                ..block
            },
            "XXXXXXX",
        );
        fixture.lay_out(&mut layout, 50.0);
        assert_eq!(lefts(&layout), [0.0], "{align:?} overflowing");
    }
    // A band that starts away from the area's zero sets the line in it.
    let block = aligned(TextAlign::Center, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        "XXX XXX",
    );
    let area = Area {
        inline: InlineExtents {
            left: 20.0,
            right: 120.0,
        },
        block_start: 0.0,
        block_end: None,
        room_above: 0.0,
    };
    fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
    assert_eq!(lefts(&layout), [35.0]);
    // Centering in an odd room takes half of it, truncated on the grid.
    fixture.lay_out(&mut layout, 70.0 + 3.0 / 64.0);
    assert_eq!(lefts(&layout), [1.0 / 64.0]);
}

/// `text-align-last` sets a paragraph's last line, before a forced break or at the block's end.
///
/// `auto` takes `text-align`, but `start` for `justify`.
#[test]
fn the_last_line_is_set_by_text_align_last() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = aligned(TextAlign::Center, TextAlignLast::End);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        "XXX XXX XXX",
    );
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(texts(&layout), ["XXX XXX ", "XXX"]);
    assert_eq!(lefts(&layout), [5.0, 50.0]);
    // A line before a forced break is a last line too.
    let block = aligned(TextAlign::Start, TextAlignLast::Right);
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        |b| {
            b.text(NodeKey(1), "XXX XXX XXX");
            b.line_break(NodeKey(2));
            b.text(NodeKey(3), "XX");
        },
    );
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(texts(&layout), ["XXX XXX ", "XXX\n", "XX"]);
    assert_eq!(lefts(&layout), [0.0, 50.0, 60.0]);
    for (last, left) in [
        (TextAlignLast::Auto, 0.0),
        (TextAlignLast::Start, 0.0),
        (TextAlignLast::Center, 25.0),
        (TextAlignLast::Left, 0.0),
        (TextAlignLast::Justify, 0.0),
    ] {
        let block = aligned(TextAlign::Justify, last);
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem(10.0),
                ..block
            },
            "XXX XXX XXX",
        );
        fixture.lay_out(&mut layout, 80.0);
        assert_eq!(lefts(&layout), [0.0, left], "{last:?}");
    }
}

/// Trailing white space affects alignment as in Chrome.
///
/// - At a soft wrap, a collapsible space is removed and preserved white space
///   hangs. Neither moves the line.
/// - Before a forced break or at the block's end, preserved white space hangs
///   only where it overflows. The part that fits moves the line.
/// - Under `break-spaces`, white space is content.
#[test]
fn only_preserved_trailing_spaces_take_room_to_align() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let center = aligned(TextAlign::Center, TextAlignLast::Auto);
    let end = aligned(TextAlign::End, TextAlignLast::Auto);
    // A collapsible space at a soft wrap is removed.
    for (block, left) in [(&center, 10.0), (&end, 20.0)] {
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem(10.0),
                ..*block
            },
            "XXX XXX",
        );
        fixture.lay_out(&mut layout, 50.0);
        assert_eq!(texts(&layout), ["XXX ", "XXX"]);
        assert_eq!(lefts(&layout)[0], left);
        assert_eq!(items(&layout, 0).len(), 1, "the space is no item");
        assert_eq!(
            items(&layout, 0)[0].clusters(),
            ClusterId::new(0)..ClusterId::new(3)
        );
    }
    // A preserved space at a soft wrap hangs past the content.
    let mut pre_wrap = ahem(10.0);
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    for (block, left) in [(&center, 10.0), (&end, 20.0)] {
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &pre_wrap,
                ..*block
            },
            "XXX XXX",
        );
        fixture.lay_out(&mut layout, 50.0);
        assert_eq!(texts(&layout), ["XXX ", "XXX"]);
        assert_eq!(lefts(&layout)[0], left);
        let line = items(&layout, 0);
        assert_eq!(line.len(), 2);
        assert!(line[1].flags.contains(FragmentItemFlags::HANGS));
        assert_eq!(line[1].inline, px(30.0));
        assert_eq!(line[1].size, LayoutUnit::from_px(10.0));
    }
    // Before a forced break, the part of preserved white space that fits
    // counts.
    for (width, center_left, end_left) in [(100.0, 25.0, 50.0), (40.0, 0.0, 0.0), (45.0, 0.0, 0.0)]
    {
        for (block, left) in [(&center, center_left), (&end, end_left)] {
            fixture.block_text(
                &mut layout,
                &ComputedBlockStyle {
                    style: &pre_wrap,
                    ..*block
                },
                "XXX  \nXXX",
            );
            fixture.lay_out(&mut layout, width);
            assert_eq!(texts(&layout)[0], "XXX  \n", "at {width}");
            assert_eq!(lefts(&layout)[0], left, "at {width}");
        }
    }
    // The same holds at the block's end.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &pre_wrap,
            ..center
        },
        "XXX  ",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [25.0]);
    // Under `break-spaces`, spaces are content.
    let mut break_spaces = ahem(10.0);
    break_spaces.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &break_spaces,
            ..center
        },
        "XXX  ",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [25.0]);
    assert!(
        items(&layout, 0)
            .iter()
            .all(|item| !item.flags.contains(FragmentItemFlags::HANGS))
    );
    // Under `pre`, spaces are content too. Chrome 153, with 20 px Ahem in
    // 200 px, right-aligns `XX ` at 140 and sets a line whose three spaces
    // overflow at its start.
    let mut pre = pre_wrap;
    pre.text.wrap_mode = TextWrapMode::NoWrap;
    pre.font.size = 20.0;
    for (text, left) in [("XX ", 140.0), ("XXXXXXXXX   ", 0.0)] {
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle { style: &pre, ..end },
            text,
        );
        fixture.lay_out(&mut layout, 200.0);
        assert_eq!(lefts(&layout), [left], "{text:?}");
        assert!(
            items(&layout, 0)
                .iter()
                .all(|item| !item.flags.contains(FragmentItemFlags::HANGS))
        );
    }
}

/// An other space separator ending a collapsing paragraph hangs whole and moves nothing.
///
/// Preserved white space that wraps hangs only where it overflows, and under
/// `pre` it is content, as CSS Text 4 says. The ideographic space rows are
/// Chrome 153's, with 20 px Ahem in 200 px. An em space hangs as CSS says,
/// though Chrome hangs no space separator but U+3000.
#[test]
fn an_other_space_separator_ending_a_paragraph_hangs_whole() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let right = aligned(TextAlign::Right, TextAlignLast::Auto);
    let center = aligned(TextAlign::Center, TextAlignLast::Auto);
    let style = |collapse, wrap| {
        let mut style = ahem(20.0);
        style.text.white_space_collapse = collapse;
        style.text.wrap_mode = wrap;
        style
    };
    let normal = style(WhiteSpaceCollapse::Collapse, TextWrapMode::Wrap);
    let nowrap = style(WhiteSpaceCollapse::Collapse, TextWrapMode::NoWrap);
    let pre_line = style(WhiteSpaceCollapse::PreserveBreaks, TextWrapMode::Wrap);
    let pre_wrap = style(WhiteSpaceCollapse::Preserve, TextWrapMode::Wrap);
    let pre = style(WhiteSpaceCollapse::Preserve, TextWrapMode::NoWrap);
    for (block, style, text, left) in [
        (&right, &normal, "XX\u{3000}", 160.0),
        (&center, &normal, "XX\u{3000}", 80.0),
        (&right, &normal, "XX\u{3000}\u{3000}", 160.0),
        (&right, &nowrap, "XX\u{3000}", 160.0),
        (&right, &pre_line, "XX\u{3000}", 160.0),
        (&right, &normal, "XX\u{2003}", 160.0),
        (&right, &pre_wrap, "XX\u{3000}", 140.0),
        (&right, &pre_wrap, "XX ", 140.0),
        (&right, &pre, "XX\u{3000}", 140.0),
    ] {
        fixture.block_text(&mut layout, &ComputedBlockStyle { style, ..*block }, text);
        fixture.lay_out(&mut layout, 200.0);
        assert_eq!(lefts(&layout), [left], "{text:?} under {style:?}");
    }
    // It hangs before a forced break too.
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &normal,
            ..right
        },
        |b| {
            b.text(NodeKey(1), "XX\u{3000}");
            b.line_break(NodeKey(2));
            b.text(NodeKey(3), "XX");
        },
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(lefts(&layout), [160.0, 160.0]);
}

/// An ideographic space before preserved spaces ending a paragraph hangs
/// only where none of those spaces fits, as CSS Text 3, section 4.1.3,
/// hangs a character only where all after it hangs.
///
/// Right-aligned 20 px Ahem in 80 px, each line is ideographic spaces where
/// white space collapses, then two `pre-wrap` spaces and a `<br>`. After
/// `X` one preserved space fits, so the ideographic spaces take room and
/// the line sits at the left. Past that the spaces overflow, and all of it
/// hangs.
#[test]
fn an_ideographic_space_before_preserved_spaces_hangs_only_where_they_overflow() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = ahem(20.0);
    let right = ComputedBlockStyle {
        style: &root,
        ..aligned(TextAlign::Right, TextAlignLast::Auto)
    };
    let mut pre_wrap = root;
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    for (before, left) in [("XXXX", 0.0), ("XXX", 20.0), ("XX", 40.0), ("X", 0.0)] {
        fixture.build(&mut layout, &right, |b| {
            b.text(NodeKey(1), before);
            b.text(NodeKey(2), "\u{3000}\u{3000}");
            b.open_box(NodeKey(3), &pre_wrap, None);
            b.text(NodeKey(4), "  ");
            b.close_box();
            b.line_break(NodeKey(5));
        });
        fixture.lay_out(&mut layout, 80.0);
        assert_eq!(lefts(&layout), [left], "{before}");
    }
}

/// White space ending a line hangs through a `<wbr>` or an opportunity that collapsing keeps.
///
/// Both take no room and are opaque to collapsing, and Blink's line breaker
/// passes over them. A U+200B the caller wrote is text, so the space before
/// it stays on the line. The rows and the first case's min-content width are
/// Chrome 153's, with 20 px Ahem in 100 px.
#[test]
fn white_space_hangs_through_a_generated_opportunity() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let right = aligned(TextAlign::Right, TextAlignLast::Auto);
    let wbr = |b: &mut LayoutBuilder<'_>| {
        b.text(NodeKey(1), "XXX ");
        b.break_opportunity();
        b.text(NodeKey(2), "XXX");
    };
    // Before a `<wbr>`, the space is removed and the line is 60 wide.
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(20.0),
            ..right
        },
        wbr,
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XXX \u{200B}", "XXX"]);
    assert_eq!(lefts(&layout), [40.0, 40.0]);
    assert_eq!(
        items(&layout, 0).len(),
        1,
        "neither the space nor the U+200B draws"
    );
    assert_eq!(layout.intrinsic_sizes().min_content, 60.0);
    // A U+200B the caller wrote is text, so the space before it stays.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(20.0),
            ..right
        },
        "XXX \u{200B}XXX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [20.0, 40.0]);
    // A preserved space hangs through the `<wbr>` at a soft wrap.
    let mut pre_wrap = ahem(20.0);
    pre_wrap.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &pre_wrap,
            ..right
        },
        wbr,
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [40.0, 40.0]);
    assert!(
        items(&layout, 0)[1]
            .flags
            .contains(FragmentItemFlags::HANGS)
    );
    // Collapsing keeps an opportunity where white space from a wrapping box
    // joins a run begun in a `nowrap` one. The cases are a `nowrap` span,
    // and a `nowrap` block around a wrapping span.
    let mut nowrap = ahem(20.0);
    nowrap.text.wrap_mode = TextWrapMode::NoWrap;
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(20.0),
            ..right
        },
        |b| {
            b.open_box(NodeKey(1), &nowrap, None);
            b.text(NodeKey(2), "XXX ");
            b.close_box();
            b.text(NodeKey(3), " XXX");
        },
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XXX \u{200B}", "XXX"]);
    assert_eq!(lefts(&layout), [40.0, 40.0]);
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap,
            ..right
        },
        |b| {
            b.text(NodeKey(1), "XXX ");
            b.open_box(NodeKey(2), &ahem(20.0), None);
            b.text(NodeKey(3), " XXX");
            b.close_box();
        },
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XXX \u{200B}", "XXX"]);
    assert_eq!(lefts(&layout), [40.0, 40.0]);
}

/// A line wraps after a space as the box holding the space does.
///
/// `<nowrap>XXX </nowrap>XXX` is one overflowing line. A wrapping span that
/// ends in a space, in a `nowrap` block, breaks after it. The rows are
/// Chrome 153's, with 20 px Ahem in 100 px.
#[test]
fn a_line_wraps_after_a_space_as_its_box_does() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut nowrap = ahem(20.0);
    nowrap.text.wrap_mode = TextWrapMode::NoWrap;
    let block_style = ahem(20.0);
    let block = ComputedBlockStyle::new(&block_style);
    fixture.build(&mut layout, &block, |b| {
        b.open_box(NodeKey(1), &nowrap, None);
        b.text(NodeKey(2), "XXX ");
        b.close_box();
        b.text(NodeKey(3), "XXX");
    });
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XXX XXX"]);
    assert_eq!(
        layout.lines().next().map(|line| line.metrics().width),
        Some(140.0)
    );
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &nowrap,
            ..block
        },
        |b| {
            b.open_box(NodeKey(1), &ahem(20.0), None);
            b.text(NodeKey(2), "XXX ");
            b.close_box();
            b.text(NodeKey(3), "XXX");
        },
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XXX ", "XXX"]);
}

// Tabs ---------------------------------------------------------------------

/// A tab is an item of its own, flagged `TAB`, reaching from where it lands to its stop.
///
/// - In Ahem at 10 px with stops 40 apart, `XX` has its tab from 20 to 40,
///   and the next `XX` is drawn from 40. A reader reads the tab that wide.
/// - Alignment moves the whole line, its stops with it.
/// - A tab at a soft wrap hangs whole. A tab ending the paragraph hangs only
///   where it overflows, so the part that fits moves an aligned line.
/// - A right-to-left line is drawn from its end, the tab as wide.
#[test]
fn a_tab_is_an_item_as_wide_as_it_reaches() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let (none, tab) = (FragmentItemFlags::NONE, FragmentItemFlags::TAB);
    let hanging_tab = FragmentItemFlags::TAB.union(FragmentItemFlags::HANGS);
    for (align, left) in [
        (TextAlign::Start, 0.0),
        (TextAlign::Center, 20.0),
        (TextAlign::Right, 40.0),
    ] {
        let block = aligned(align, TextAlignLast::Auto);
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &tabbed(10.0, 4.0),
                ..block
            },
            "XX\tXX",
        );
        fixture.lay_out(&mut layout, 100.0);
        assert_eq!(lefts(&layout), [left], "{align:?}");
        assert_eq!(
            placed(&layout, 0),
            [(0.0, 20.0, none), (20.0, 20.0, tab), (40.0, 20.0, none)],
            "{align:?}"
        );
        let xs: Vec<f32> = glyphs(&layout, 0).iter().map(|&(_, x)| x.to_px()).collect();
        assert_eq!(xs, [0.0, 10.0, 40.0, 50.0], "{align:?}");
    }
    // Through the public API, the tab's run is a tab. The run and its one
    // cluster are as wide as its item.
    let line = layout.line(0).expect("a line");
    let runs: Vec<crate::TextRun<'_>> = line
        .items()
        .filter_map(|item| match item {
            crate::Item::Text(run) => Some(run),
            _ => None,
        })
        .collect();
    assert_eq!(runs.len(), 3);
    assert!(runs[1].is_tab() && !runs[0].is_tab());
    assert_eq!(runs[1].advance(), 20.0);
    assert_eq!(runs[1].inline(), along(20.0, 40.0));
    let clusters: Vec<f32> = runs[1]
        .clusters()
        .map(|cluster| cluster.advance())
        .collect();
    assert_eq!(clusters, [20.0]);
    assert_eq!(runs[1].glyphs().count(), 0, "a tab draws nothing");
    // At a soft wrap, the tab hangs whole and moves nothing.
    let end = aligned(TextAlign::End, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &tabbed(10.0, 4.0),
            ..end
        },
        "XX\tXX\tXX",
    );
    fixture.lay_out(&mut layout, 70.0);
    assert_eq!(texts(&layout), ["XX\tXX\t", "XX"]);
    assert_eq!(lefts(&layout), [10.0, 50.0]);
    assert_eq!(
        placed(&layout, 0),
        [
            (0.0, 20.0, none),
            (20.0, 20.0, tab),
            (40.0, 20.0, none),
            (60.0, 20.0, hanging_tab)
        ]
    );
    // At the paragraph's end, the part that fits counts: `XXX` and its tab
    // reach 40.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &tabbed(10.0, 4.0),
            ..end
        },
        "XXX\t",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [60.0]);
    assert_eq!(placed(&layout, 0)[1], (30.0, 10.0, hanging_tab));
    fixture.lay_out(&mut layout, 35.0);
    assert_eq!(lefts(&layout), [0.0], "what overflows hangs past the end");
    let center = aligned(TextAlign::Center, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &tabbed(10.0, 4.0),
            ..center
        },
        "XXX\t",
    );
    fixture.lay_out(&mut layout, 45.0);
    assert_eq!(lefts(&layout), [2.5], "the tab counts to 40 of 45");
    // A right-to-left line starts at its right, and its stops count from
    // there.
    let block_style = tabbed(10.0, 4.0);
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&block_style)
    };
    fixture.block_text(&mut layout, &rtl, "XX\tXX");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [40.0]);
    assert_eq!(
        placed(&layout, 0),
        [(0.0, 20.0, none), (20.0, 20.0, tab), (40.0, 20.0, none)]
    );
    assert_eq!(
        items(&layout, 0)[2].clusters(),
        ClusterId::new(0)..ClusterId::new(2)
    );
}

/// A relayout of tabbed lines matches a fresh layout, item for item, at every alignment and width.
///
/// The area starts away from zero, and the stops count from its content edge.
#[test]
fn a_tabbed_relayout_equals_a_fresh_layout() {
    let mut fixture = fixture();
    let style = ComputedStyle {
        text: TextGroup {
            white_space_collapse: WhiteSpaceCollapse::Preserve,
            ..TextGroup::INITIAL
        },
        ..sized(&NARROW, 13.0)
    };
    let text = "name\tvalue\ti i\tnote\n\ta much longer name\tv\t\ti\t ";
    for align in [TextAlign::Start, TextAlign::Center, TextAlign::End] {
        let block = aligned(align, TextAlignLast::Auto);
        let mut layout = Layout::new();
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            text,
        );
        for width in [30.0, 57.5, 90.0, 131.0, 400.0, 3.0] {
            let area = Area {
                inline: InlineExtents {
                    left: 7.5,
                    right: 7.5 + width,
                },
                block_start: 0.0,
                block_end: None,
                room_above: 0.0,
            };
            fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
            let mut fresh = Layout::new();
            fixture.block_text(
                &mut fresh,
                &ComputedBlockStyle {
                    style: &style,
                    ..block
                },
                text,
            );
            fixture.lay_out_with(&mut fresh, area, &mut NoExclusions);
            let (a, b) = (layout.fragments(), fresh.fragments());
            assert_eq!(a.items.as_slice(), b.items.as_slice(), "at {width}");
            // The pieces add up to what the breaker fitted.
            for (id, line) in layout.line_records().lines.iter() {
                let own = layout.fragments().line_items(id);
                let content: InlineLayoutUnit = own
                    .iter()
                    .filter(|item| !item.flags.contains(FragmentItemFlags::HANGS))
                    .filter(|item| item.descendants() == 0)
                    .map(FragmentItem::advance)
                    .fold(InlineLayoutUnit::ZERO, |sum, advance| sum + advance);
                let fitted = content.to_layout();
                assert!(
                    (fitted - line.width).max(line.width - fitted) <= LayoutUnit::EPSILON,
                    "{id:?} at {width}: {fitted:?} against {:?}",
                    line.width
                );
            }
        }
    }
}

/// Under `ruby-overhang: spaces` a ruby column's overhang reaches over the
/// whole run of spaces and tabs beside it, the tabs sized where Chrome's
/// breaker stands: before the column, where they fall; after it, a run of
/// tabs from where the items before it gave back what they took
/// (`CommitPendingEndOverhang`). Chrome 155, 20px Ahem, `tab-size: 50px`,
/// `X` under ten 10px `X`s (a column 100 wide, 40 to spare each side), with
/// `X` before and after the blank: the column and the last `X` start at
/// - `X\t|`: 20 and 120; `X \t|`: 20 and 120; `X\t |`: 30 and 130;
///   `X\t\t|`: 60 and 160;
/// - `|\tX`: 20 and 120; `|\t X`: 20 and 130; `| \tX`: 20 and 130;
///   `|\t\tX`: 20 and 160.
#[test]
fn a_ruby_overhang_reaches_over_runs_of_tabs_and_spaces() {
    use crate::style::RubyOverhang;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut block = tabbed(20.0, 1.0);
    block.text.tab_size = TabSize::Px(50.0);
    let mut reading = block;
    reading.font.size = 10.0;
    let mut ruby = block;
    ruby.ruby.overhang = RubyOverhang::Spaces;
    ruby.ruby.align = RubyAlign::Center;
    for (before, after, column, last) in [
        ("X\t", "X", 20.0, 120.0),
        ("X \t", "X", 20.0, 120.0),
        ("X\t ", "X", 30.0, 130.0),
        ("X\t\t", "X", 60.0, 160.0),
        ("X", "\tX", 20.0, 120.0),
        ("X", "\t X", 20.0, 130.0),
        ("X", " \tX", 20.0, 130.0),
        ("X", "\t\tX", 20.0, 160.0),
    ] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&block), |b| {
            b.text(NodeKey(1), before);
            b.open_ruby(NodeKey(2), &ruby, None);
            b.text(NodeKey(3), "X");
            b.open_annotation(NodeKey(4), &reading, None);
            b.text(NodeKey(5), "XXXXXXXXXX");
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(6), after);
        });
        fixture.lay_out(&mut layout, 1000.0);
        let lefts: Vec<f32> = places(&layout, 0).iter().map(|&(_, left)| left).collect();
        assert_eq!(
            (lefts.get(1).copied(), lefts.last().copied()),
            (Some(column + 40.0), Some(last)),
            "{before:?} {after:?}: {lefts:?}"
        );
        assert_eq!(
            reach(&layout, 0).to_px(),
            last + 20.0,
            "{before:?} {after:?}"
        );
        assert_eq!(
            layout.intrinsic_sizes().max_content,
            last + 20.0,
            "{before:?} {after:?}"
        );
    }
}

/// Under `ruby-overhang: spaces` a ruby column's overhang reaches into a tab
/// beside it, as Chrome's `GetOverhang` and `CommitPendingEndOverhang` reach
/// into a control item. Under `auto` it reaches into no tab.
///
/// In 20px Ahem with stops 20 apart, `X`, a ruby of `X` under six 10px `X`s
/// (a column 60 wide), a tab and `X`: Chrome sets the last `X` at 80, the
/// tab taking the column's last 20, under `center` and `start` alike. With
/// the tab before the column, the column starts over it, at 20, and the last
/// `X` is at 80 too. Under `auto` the column reaches 5 over the `X` before
/// it and none over the tab, which reaches to 100.
#[test]
fn a_ruby_overhang_reaches_into_a_tab_under_spaces() {
    use crate::style::RubyOverhang;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut block = tabbed(20.0, 1.0);
    block.text.tab_size = TabSize::Px(20.0);
    let mut reading = block;
    reading.font.size = 10.0;
    for (before, after, align, overhang, expected) in [
        (
            "X",
            "\tX",
            RubyAlign::SpaceAround,
            RubyOverhang::Spaces,
            [0.0, 40.0, 80.0],
        ),
        (
            "X",
            "\tX",
            RubyAlign::Start,
            RubyOverhang::Spaces,
            [0.0, 20.0, 80.0],
        ),
        (
            "X\t",
            "X",
            RubyAlign::SpaceAround,
            RubyOverhang::Spaces,
            [0.0, 40.0, 80.0],
        ),
        (
            "X",
            "\tX",
            RubyAlign::SpaceAround,
            RubyOverhang::Auto,
            [0.0, 35.0, 100.0],
        ),
    ] {
        let mut ruby = block;
        ruby.ruby.overhang = overhang;
        ruby.ruby.align = align;
        fixture.build(&mut layout, &ComputedBlockStyle::new(&block), |b| {
            b.text(NodeKey(1), before);
            b.open_ruby(NodeKey(2), &ruby, None);
            b.text(NodeKey(3), "X");
            b.open_annotation(NodeKey(4), &reading, None);
            b.text(NodeKey(5), "XXXXXX");
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(6), after);
        });
        fixture.lay_out(&mut layout, 1000.0);
        let lefts: Vec<f32> = places(&layout, 0).iter().map(|&(_, left)| left).collect();
        assert_eq!(
            lefts, expected,
            "{before:?} {after:?} {align:?} {overhang:?}"
        );
        assert_eq!(reach(&layout, 0).to_px(), expected[2] + 20.0);
        assert_eq!(layout.intrinsic_sizes().max_content, expected[2] + 20.0);
    }
}
