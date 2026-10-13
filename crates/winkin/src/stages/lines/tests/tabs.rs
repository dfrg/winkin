//! Tab tests. They pin:
//! - a tab measured where it lands, before the break is chosen;
//! - tab stops at the content edges, beside floats too;
//! - stops counted in the block container's spaces, or the space a space
//!   falls back to where its primary font maps none;
//! - any `tab-size` giving valid lines;
//! - an annotation's tab taking no room on the base line;
//! - tabbed intrinsic sizes the text fits.

use super::*;
use crate::style::LengthPercentage;
use crate::tests::ahem_fallback;

/// A tab is measured where it lands while the break is chosen, not after.
///
/// In Ahem at 10 px with stops 40 apart, `XX` reaches 20 and its tab 40.
/// The next `XX` reaches 60 and its tab 80. At 90 the second tab hangs at a
/// soft wrap and the line fits but for it. At 59 the first tab hangs, and
/// each line starts its stops again.
#[test]
fn a_tab_is_measured_where_it_lands_before_the_break_is_chosen() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.text(&mut layout, &tabbed(10.0, 4.0), "XX\tXX\tXX");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XX\tXX\tXX"]);
    assert_eq!(widths(&layout), [100.0]);
    for width in [90.0, 79.0] {
        fixture.lay_out(&mut layout, width);
        assert_eq!(texts(&layout), ["XX\tXX\t", "XX"], "at {width}");
        assert_eq!(widths(&layout), [60.0, 20.0], "at {width}");
        let first = &records(&layout)[0];
        assert_eq!(first.hang.space, LayoutUnit::from_px(20.0));
        assert!(!first.flags.contains(LineFlags::CONDITIONAL_HANG));
    }
    fixture.lay_out(&mut layout, 59.0);
    assert_eq!(texts(&layout), ["XX\t", "XX\t", "XX"]);
    assert_eq!(widths(&layout), [20.0, 20.0, 20.0]);
    // A tab ending the paragraph hangs only where it overflows.
    fixture.text(&mut layout, &tabbed(10.0, 4.0), "XXX\t");
    fixture.lay_out(&mut layout, 100.0);
    let only = &records(&layout)[0];
    assert_eq!(only.width, LayoutUnit::from_px(30.0));
    assert_eq!(only.hang.space, LayoutUnit::from_px(10.0));
    assert!(only.flags.contains(LineFlags::CONDITIONAL_HANG));
    // Under `break-spaces` a tab is content, with an opportunity after it.
    let mut spaces = tabbed(10.0, 4.0);
    spaces.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    fixture.text(&mut layout, &spaces, "XX\tXX\tXX");
    fixture.lay_out(&mut layout, 90.0);
    assert_eq!(texts(&layout), ["XX\tXX\t", "XX"]);
    assert_eq!(widths(&layout), [80.0, 20.0]);
    // No width lets a line's content past its band.
    fixture.text(
        &mut layout,
        &tabbed(20.0, 8.0),
        "aa\tbb\tcc\tdd\tee\tff\tgg\thh\tii\tjj\tkk\tll",
    );
    for width in [200.0, 300.0, 400.0, 600.0] {
        fixture.lay_out(&mut layout, width);
        assert!(
            records(&layout)
                .iter()
                .all(|line| !line.flags.contains(LineFlags::OVERFLOWS)),
            "at {width}"
        );
    }
}

/// Tab stops count from the block's content edge, not the band's.
///
/// A line a float pushes in 15 px keeps the block's stops, so a tab
/// starting it reaches only 25 px to the first. The same holds on the start
/// side of a right-to-left paragraph. An area away from zero is its own
/// content edge.
#[test]
fn tab_stops_are_the_content_edges() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = tabbed(10.0, 4.0);
    fixture.text(&mut layout, &style, "\tX\tX");
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(widths(&layout), [90.0]);
    let side = FloatSide::Left;
    fixture.lay_out_with(&mut layout, Area::new(200.0), &mut Pushed(15.0, side));
    assert_eq!(widths(&layout), [75.0], "stops at 25 and 65 of the band");
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    fixture.build(&mut layout, &rtl, |b| b.text(NodeKey(1), "\tX\tX"));
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(widths(&layout), [90.0]);
    let side = FloatSide::Right;
    fixture.lay_out_with(&mut layout, Area::new(200.0), &mut Pushed(15.0, side));
    assert_eq!(widths(&layout), [75.0], "from the right, the start side");
    let side = FloatSide::Left;
    fixture.lay_out_with(&mut layout, Area::new(200.0), &mut Pushed(15.0, side));
    assert_eq!(
        widths(&layout),
        [90.0],
        "a float on the end side moves none"
    );
    fixture.text(&mut layout, &style, "\tX\tX");
    let away = Area {
        inline: InlineExtents {
            left: 33.0,
            right: 233.0,
        },
        block_start: 0.0,
        block_end: None,
        room_above: 0.0,
    };
    fixture.lay_out_with(&mut layout, away, &mut NoExclusions);
    assert_eq!(widths(&layout), [90.0]);
}

/// A `tab-size` in spaces counts the block container's space, with its
/// letter-spacing and word-spacing, not the tab's own font's, as Chrome
/// sizes a tab in `InlineNode::FontForTab`. The count is the tab's own.
///
/// The block is Ahem at 10 px with 1 px of letter-spacing and 2 of
/// word-spacing: a space of 13. A 40 px span with `tab-size: 2` stops at 26,
/// not at the 80 its own space would give.
#[test]
fn tab_stops_count_the_block_containers_spaces() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut block = tabbed(10.0, 4.0);
    block.text.letter_spacing = LengthPercentage::from_px(1.0);
    block.text.word_spacing = LengthPercentage {
        px: 2.0,
        fraction: 0.0,
    };
    let span = tabbed(40.0, 2.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&block), |b| {
        b.open_box(NodeKey(1), &span, None);
        b.text(NodeKey(2), "	X");
        b.close_box();
    });
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(widths(&layout), [66.0]);
}

/// Where the block's primary font maps no space, a `tab-size` in spaces
/// counts the space of the first listed font that maps one, as Chrome sizes
/// such a tab by the font a space falls back to (WPT
/// `tab-size-integer-005`).
///
/// Chrome 155, over a primary with no space and a 1 em `.notdef` at 40 px,
/// puts a `tab-size: 8` tab's stop at 70.47 with Gentium Plus next in the
/// list, eight of its spaces, and at 192 with Courier New next.
///
/// Here the primary's `.notdef` is 5 px at 10 px, and the next font's space
/// 3 px: four of those put the stop at 12, not 20.
#[test]
fn tab_stops_count_the_space_a_space_falls_back_to() {
    let spaceless = TestFont::new("Test Spaceless", &[(0x58, 0x58)]);
    let mut spaced = TestFont::new("Test Spaced", &[(0x20, 0x7E)]);
    spaced.advances = vec![(' ', 300)];
    let mut fixture = Fixture::new(
        &[spaceless, spaced],
        ahem_fallback(),
        StageCheck::Placed(check),
    );
    let families = [
        FontFamilyName::Named(Cow::Borrowed("Test Spaceless")),
        FontFamilyName::Named(Cow::Borrowed("Test Spaced")),
    ];
    let mut style = sized(&families, 10.0);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    style.text.tab_size = TabSize::Spaces(4.0);
    let mut layout = Layout::new();
    fixture.text(&mut layout, &style, "\tX");
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(widths(&layout), [17.0]);
    // A primary that maps a space counts its own.
    let families = [FontFamilyName::Named(Cow::Borrowed("Test Spaced"))];
    let mut style = sized(&families, 10.0);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    style.text.tab_size = TabSize::Spaces(4.0);
    fixture.text(&mut layout, &style, "\tX");
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(widths(&layout), [17.0]);
}

/// Every `tab-size` breaks into valid lines, even those no computed value
/// has.
///
/// So does a tab so far along its line that its stop is at the grid's end.
#[test]
fn any_tab_size_gives_valid_lines() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for size in [
        TabSize::Spaces(0.0),
        TabSize::Px(0.0),
        TabSize::Spaces(-8.0),
        TabSize::Px(f32::NAN),
        TabSize::Spaces(f32::INFINITY),
        TabSize::Spaces(1e30),
        TabSize::Px(f32::MAX),
        TabSize::Px(1.0 / 65536.0),
    ] {
        let mut style = tabbed(10.0, 4.0);
        style.text.tab_size = size;
        fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
            b.text(NodeKey(1), "XX\tXX\t\tX");
            let huge = BoxSize {
                inline: 3.0e7,
                block: 10.0,
                baseline: None,
            };
            b.atomic(NodeKey(2), &style, None, huge);
            b.text(NodeKey(3), "\tX\t");
        });
        for width in [0.0, 25.0, 1000.0, f32::INFINITY] {
            fixture.lay_out(&mut layout, width);
        }
        let intrinsic = layout.intrinsic_sizes();
        assert!(intrinsic.min_content >= 0.0 && intrinsic.max_content >= intrinsic.min_content);
    }
}

/// A tab in ruby annotation text takes no room.
///
/// The annotation `\t\tz\t` is as narrow as its `z`, so its column is its
/// base's width. The breaker, line layout and the intrinsic sizes all agree.
/// A tab in the base is sized where it lands.
#[test]
fn an_annotations_tab_takes_no_room_on_the_base_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = tabbed(10.0, 4.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XX");
        b.open_ruby(NodeKey(2), &style, None);
        b.text(NodeKey(3), "\tYY");
        b.open_annotation(NodeKey(4), &style, None);
        b.text(NodeKey(5), "\t\tz\t");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), "XX");
    });
    fixture.lay_out(&mut layout, 1000.0);
    // `XX` to 20, the base's tab to 40, `YY` to 60, `XX` to 80.
    assert_eq!(widths(&layout), [80.0]);
    assert_eq!(layout.intrinsic_sizes().max_content, 80.0);
}

/// With tabs, the intrinsic sizes are widths the text fits.
///
/// At max-content every paragraph is one line, and at min-content none
/// overflows.
#[test]
fn tabbed_intrinsic_sizes_are_widths_the_text_fits() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ComputedStyle {
        text: TextGroup {
            white_space_collapse: WhiteSpaceCollapse::Preserve,
            ..TextGroup::INITIAL
        },
        ..sized(&NARROW, 13.0)
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "name\tvalue\tnote i i\t\n");
        b.text(NodeKey(2), "a much longer name\tv\ti\n\tindented\ti");
    });
    let intrinsic = layout.intrinsic_sizes();
    fixture.lay_out(&mut layout, intrinsic.max_content);
    assert_eq!(layout.lines().len(), 3);
    fixture.lay_out(&mut layout, intrinsic.min_content);
    assert!(
        records(&layout)
            .iter()
            .all(|line| !line.flags.contains(LineFlags::OVERFLOWS))
    );
}
