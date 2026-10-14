//! Tab tests. They pin:
//! - a tab measured where it lands, before the break is chosen;
//! - tab stops at the content edges, beside floats too;
//! - stops counted in the block container's spaces, or in the fallback
//!   font's space where its primary font maps none;
//! - any `tab-size` giving valid lines;
//! - an annotation's tab taking no room on the base line;
//! - tabbed intrinsic sizes the text fits.

use super::*;
use crate::style::LengthPercentage;
use crate::tests::ahem_fallback;

/// A tab is measured where it lands while the break is chosen.
///
/// With stops 40 apart, the tabs in `XX\tXX\tXX` end at 40 and 80. At 90
/// and 79 the line breaks after the second, which hangs. Each line starts
/// its stops again.
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
    // Under `break-spaces` a tab is content with an opportunity after it.
    let mut spaces = tabbed(10.0, 4.0);
    spaces.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    fixture.text(&mut layout, &spaces, "XX\tXX\tXX");
    fixture.lay_out(&mut layout, 90.0);
    assert_eq!(texts(&layout), ["XX\tXX\t", "XX"]);
    assert_eq!(widths(&layout), [80.0, 20.0]);
    // At no width does a line's content overflow its band.
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
/// A line a float pushes in 15 px keeps the block's stops, in either
/// direction. An area offset from zero starts its stops at its left edge.
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
/// letter-spacing and word-spacing, as Chrome's `InlineNode::FontForTab`
/// does.
///
/// A 40 px span with `tab-size: 2` in a block whose spaced space is 13 px
/// stops at 26, not 80.
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
/// counts the space of the first listed font that maps one.
///
/// Chrome sizes such a tab by the font a space falls back to (WPT
/// `tab-size-integer-005`). Chrome 155, with a 40 px spaceless primary,
/// puts a `tab-size: 8` stop at 70.47 with Gentium Plus next in the list,
/// and at 192 with Courier New. Here four 3 px spaces put the stop at 12.
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

/// Every `tab-size` gives valid lines, including values no computed style
/// produces.
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
/// The breaker, line layout and the intrinsic sizes agree. A tab in the
/// base is sized where it lands.
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
/// At max-content every paragraph is one line, and at min-content no line
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
