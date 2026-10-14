//! Intrinsic size and tab tests. They pin:
//! - min-content as the widest unbreakable run, max-content as the widest
//!   paragraph;
//! - tab widths, on and off the grid;
//! - conditionally hanging white space counted in max-content;
//! - hanging punctuation left out, `allow-end` from min-content only.

use super::*;
use crate::style::FirstLineVariant;

/// Min-content is the widest unbreakable run and max-content the widest
/// paragraph, both without trailing hanging spaces. Ahem at 10 px.
#[test]
fn intrinsic_sizes_are_the_widest_unbreakable_run_and_paragraph() {
    let mut fixture = fixture();
    let ahem = sized(&AHEM_FAMILY, 10.0);
    let mut layout = Layout::new();
    fixture.text(&mut layout, &ahem, "XX XXXX XXX");
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (40.0, 110.0));

    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "XXX XX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "X XXXXX X X X");
        b.line_break(NodeKey(4));
        b.text(NodeKey(5), "XX");
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (50.0, 130.0));

    // `overflow-wrap: anywhere` lowers min-content to the widest cluster;
    // `break-word` doesn't, and `nowrap` allows no break.
    for (wrap, mode, min) in [
        (OverflowWrap::Anywhere, TextWrapMode::Wrap, 10.0),
        (OverflowWrap::BreakWord, TextWrapMode::Wrap, 40.0),
        (OverflowWrap::Anywhere, TextWrapMode::NoWrap, 110.0),
    ] {
        let mut style = ahem;
        style.text.overflow_wrap = wrap;
        style.text.wrap_mode = mode;
        fixture.text(&mut layout, &style, "XX XXXX XXX");
        let sizes = layout.intrinsic_sizes();
        assert_eq!(
            (sizes.min_content, sizes.max_content),
            (min, 110.0),
            "{wrap:?} {mode:?}"
        );
    }

    // A box closing after a hanging space: the space hangs, the edge
    // counts. An atomic inline counts its margin box.
    let padded = edged(&ahem, [0.0; 3], [0.0, 0.0, 5.0]);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "XX");
        b.open_box(NodeKey(2), &padded, None);
        b.text(NodeKey(3), "XX ");
        b.close_box();
        b.text(NodeKey(4), "XXX ");
        let size = BoxSize {
            inline: 42.0,
            block: 10.0,
            baseline: None,
        };
        b.atomic(NodeKey(5), &ahem, None, size);
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (45.0, 137.0));
}

// Tabs ---------------------------------------------------------------------

/// A tab reaches the first stop past its start, or the one after where that
/// is nearer than half a space, as Chrome's `Font::TabWidth`; the width
/// rounds up to the grid. A zero or invalid interval gives zero width.
#[test]
fn a_tab_reaches_the_next_stop_past_where_it_lands() {
    let space = px(10.0);
    let eight = TabStops::new(
        TabSize::Spaces(8.0),
        space,
        TextUnit::from_raw(0),
        TextUnit::from_raw(0),
    );
    assert_eq!(eight.interval(), px(80.0));
    let tab = |origin: f32, position: InlineLayoutUnit, stops: TabStops| {
        tab_advance(lu(origin), position, stops).to_px()
    };
    assert_eq!(tab(0.0, px(0.0), eight), 80.0, "from the edge");
    assert_eq!(tab(0.0, px(15.0), eight), 65.0);
    assert_eq!(tab(0.0, px(80.0), eight), 80.0, "on a stop, to the next");
    assert_eq!(tab(0.0, px(150.0), eight), 10.0);
    assert_eq!(tab(0.0, px(75.0), eight), 5.0, "half a space is enough");
    assert_eq!(tab(0.0, px(76.0), eight), 84.0, "less goes on to the next");
    // Stops count from the content edge, wherever the line starts.
    assert_eq!(tab(15.0, px(0.0), eight), 65.0);
    assert_eq!(tab(15.0, px(65.0), eight), 80.0);
    // Off the grid, it ends at the stop rounded up: 13.3203125 from the edge
    // reaches 46.6875.
    let off_grid = InlineLayoutUnit::from_raw(872_960);
    assert_eq!(
        tab(
            0.0,
            off_grid,
            TabStops::new(
                TabSize::Px(60.0),
                space,
                TextUnit::from_raw(0),
                TextUnit::from_raw(0)
            )
        ),
        46.6875
    );
    // Before the content edge, with 100 px stops, Chrome 155 ends a tab
    // starting 10 px before the edge at the edge, and one starting 110 px
    // before at 100 px before (WPT `text-indent-tab-positions-001`).
    assert_eq!(tab(0.0, px(-10.0), eight), 10.0);
    assert_eq!(tab(0.0, px(-90.0), eight), 10.0);
    assert_eq!(tab(0.0, px(-2.0), eight), 82.0, "less than half a space");
    let fixed = TabStops::new(
        TabSize::Px(100.0),
        space,
        TextUnit::from_raw(0),
        TextUnit::from_raw(0),
    );
    assert_eq!(fixed.interval(), px(100.0));
    assert_eq!(tab(0.0, px(15.0), fixed), 85.0);
    for none in [
        TabSize::Spaces(0.0),
        TabSize::Px(0.0),
        TabSize::Spaces(-4.0),
        TabSize::Px(-40.0),
        TabSize::Spaces(f32::NAN),
        TabSize::Px(f32::INFINITY),
        TabSize::Spaces(f32::NEG_INFINITY),
    ] {
        let stops = TabStops::new(none, space, TextUnit::from_raw(0), TextUnit::from_raw(0));
        assert_eq!(stops.interval(), InlineLayoutUnit::ZERO, "{none:?}");
        assert_eq!(tab(0.0, px(15.0), stops), 0.0, "{none:?}");
    }
    // In a font with no space, a `tab-size` in spaces is zero.
    let spaceless = TabStops::new(
        TabSize::Spaces(8.0),
        InlineLayoutUnit::ZERO,
        TextUnit::from_raw(0),
        TextUnit::from_raw(0),
    );
    assert_eq!(tab(0.0, px(15.0), spaceless), 0.0);
}

/// Stops and positions at the grid's ends saturate and don't panic.
#[test]
fn a_tab_at_the_ends_of_the_grid_is_still_a_width() {
    let space = px(10.0);
    let huge = TabStops::new(
        TabSize::Spaces(1e30),
        space,
        TextUnit::from_raw(0),
        TextUnit::from_raw(0),
    );
    let far = InlineLayoutUnit::from_raw(i64::MAX);
    let near = InlineLayoutUnit::from_raw(i64::MIN);
    for stops in [
        huge,
        TabStops::new(
            TabSize::Px(f32::MAX),
            space,
            TextUnit::from_raw(0),
            TextUnit::from_raw(0),
        ),
        TabStops::new(
            TabSize::Px(1.0 / 65536.0),
            space,
            TextUnit::from_raw(0),
            TextUnit::from_raw(0),
        ),
        TabStops::new(
            TabSize::Spaces(8.0),
            InlineLayoutUnit::from_raw(i64::MAX),
            TextUnit::from_raw(0),
            TextUnit::from_raw(0),
        ),
        TabStops::new(
            TabSize::Spaces(8.0),
            InlineLayoutUnit::from_raw(i64::MIN),
            TextUnit::from_raw(0),
            TextUnit::from_raw(0),
        ),
    ] {
        for origin in [LayoutUnit::MIN, LayoutUnit::ZERO, LayoutUnit::MAX] {
            for position in [near, InlineLayoutUnit::ZERO, far] {
                let width = tab_advance(origin, position, stops);
                assert!(
                    width >= LayoutUnit::ZERO,
                    "{stops:?} {origin:?} {position:?}"
                );
            }
        }
    }
    assert_eq!(
        tab_advance(LayoutUnit::ZERO, px(0.0), huge),
        LayoutUnit::MAX
    );
}

/// The space is the primary font's U+0020 advance: an em in Ahem, a third
/// of one in the narrow font.
#[test]
fn a_space_is_its_primary_fonts_advance() {
    let space_of = |layout: &Layout| {
        let block = layout
            .content()
            .nodes
            .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
        let primary = primary(layout, block);
        primary.map_or(TextUnit::from_px(0.0), |used| used.metrics.space)
    };
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for (families, size, expected) in [
        (&AHEM_FAMILY, 10.0, 10.0),
        (&AHEM_FAMILY, 13.0, 13.0),
        (&NARROW, 12.0, 3.996),
    ] {
        fixture.text(&mut layout, &sized(families, size), "a\tb");
        let space = space_of(&layout);
        assert!(
            (space.to_px() - expected).abs() < 1e-4,
            "{space:?} against {expected}"
        );
    }
    // No font at all.
    let mut empty = Context::new(Collection::new());
    let style = sized(&AHEM_FAMILY, 20.0);
    let mut b = layout.builder(
        &mut empty,
        NodeKey(0),
        &ComputedBlockStyle::new(&style),
        BuildOptions::default(),
    );
    b.text(NodeKey(1), "a\tb");
    b.finish();
    let space = space_of(&layout);
    assert_eq!(space.to_px(), 5.0);
}

/// Intrinsic sizes size each tab from its line's start: the paragraph's for
/// max-content, each min-content line's for min-content. Under `pre-wrap` a
/// hanging tab counts in no min-content line, but one ending a paragraph
/// counts in max-content. Under `break-spaces` and `pre` a tab is content.
/// Chrome 153's widths, Ahem at 10 px with 40 px stops.
#[test]
fn intrinsic_sizes_count_each_tab_where_it_lands() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&AHEM_FAMILY, 10.0);
    style.text.tab_size = TabSize::Spaces(4.0);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let sizes = |fixture: &mut Fixture, layout: &mut Layout, style, text| {
        fixture.text(layout, style, text);
        let sizes = layout.intrinsic_sizes();
        (sizes.min_content, sizes.max_content)
    };
    // `X` 10, tab 40, `XX` 60, tab 80, `X` 90. The widest min-content line
    // is `XX` with its hanging tab.
    assert_eq!(
        sizes(&mut fixture, &mut layout, &style, "X\tXX\tX"),
        (20.0, 90.0)
    );
    // A final tab hangs only where it overflows, so max-content counts it.
    assert_eq!(
        sizes(&mut fixture, &mut layout, &style, "XXX\t"),
        (30.0, 40.0)
    );
    // So does the whole trailing space-and-tab run, as the breaker measures
    // it.
    assert_eq!(
        sizes(&mut fixture, &mut layout, &style, "XX \t"),
        (20.0, 40.0)
    );
    // Each min-content line restarts the stops: `X` then reaches 10 and its
    // tab 40.
    assert_eq!(
        sizes(&mut fixture, &mut layout, &style, "XXXXX X\tX"),
        (50.0, 90.0)
    );
    let mut spaces = style;
    spaces.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    assert_eq!(
        sizes(&mut fixture, &mut layout, &spaces, "X\tXX"),
        (40.0, 60.0)
    );
    let mut pre = style;
    pre.text.wrap_mode = TextWrapMode::NoWrap;
    assert_eq!(
        sizes(&mut fixture, &mut layout, &pre, "X\tXX\tX"),
        (90.0, 90.0)
    );
    assert_eq!(
        sizes(&mut fixture, &mut layout, &pre, "XXX\t"),
        (40.0, 40.0)
    );
    // Each paragraph's tabs count from its own start.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XXXXX\tX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "\tX");
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (50.0, 90.0));
}

/// Trailing white space counts in max-content where it hangs conditionally
/// (`pre-wrap`), in neither size where it always hangs (`normal`), and in
/// both under `pre`; min-content leaves out all that hangs (CSS Text 3,
/// "trimming and positioning"). Chrome 153's widths in Ahem at 20 px,
/// except that U+3000 hangs here as CSS says, where Chrome counts it.
#[test]
fn trailing_white_space_counts_in_max_content_where_it_hangs_conditionally() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = |collapse, wrap| {
        let mut style = sized(&AHEM_FAMILY, 20.0);
        style.text.white_space_collapse = collapse;
        style.text.wrap_mode = wrap;
        style
    };
    let pre_wrap = style(WhiteSpaceCollapse::Preserve, TextWrapMode::Wrap);
    let pre = style(WhiteSpaceCollapse::Preserve, TextWrapMode::NoWrap);
    let break_spaces = style(WhiteSpaceCollapse::BreakSpaces, TextWrapMode::Wrap);
    let normal = style(WhiteSpaceCollapse::Collapse, TextWrapMode::Wrap);
    for (style, text, expected) in [
        (&pre_wrap, "XX ", (40.0, 60.0)),
        (&pre_wrap, "XX  ", (40.0, 80.0)),
        (&pre_wrap, "XX \nY", (40.0, 60.0)),
        (&pre_wrap, "XX YY", (40.0, 100.0)),
        (&pre_wrap, "XX \u{3000}", (40.0, 80.0)),
        (&pre_wrap, "XX\u{3000}", (40.0, 60.0)),
        (&pre, "XX ", (60.0, 60.0)),
        (&pre, "XX  ", (80.0, 80.0)),
        (&pre, "XX \nY", (60.0, 60.0)),
        (&pre, "XX\u{3000}", (60.0, 60.0)),
        (&break_spaces, "XX ", (60.0, 60.0)),
        (&break_spaces, "XX  ", (60.0, 80.0)),
        (&normal, "XX\u{3000}", (40.0, 40.0)),
        (&normal, "XX\u{3000}YY", (40.0, 100.0)),
    ] {
        fixture.text(&mut layout, style, text);
        let sizes = layout.intrinsic_sizes();
        assert_eq!(
            (sizes.min_content, sizes.max_content),
            expected,
            "{text:?} under {:?}, {:?}",
            style.text.white_space_collapse,
            style.text.wrap_mode
        );
    }
}

/// A hanging mark is left out of both intrinsic sizes, or of min-content
/// only under `allow-end`, as CSS Text 4 says. A smaller mark gives back
/// its own advance. Ahem at 10 px.
#[test]
fn a_hanging_mark_is_left_out_of_the_intrinsic_sizes() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let hanging = |first: bool, last: bool, end: HangEnd| {
        let mut style = sized(&AHEM_FAMILY, 10.0);
        style.text.hanging_punctuation = HangingPunctuation { first, last, end };
        style
    };
    for (style, text, expected) in [
        (
            hanging(false, false, HangEnd::None),
            "(XXXX XX)",
            (50.0, 90.0),
        ),
        (
            hanging(true, false, HangEnd::None),
            "(XXXX XX)",
            (40.0, 80.0),
        ),
        (
            hanging(false, true, HangEnd::None),
            "XX XXXX)",
            (40.0, 70.0),
        ),
        (
            hanging(false, false, HangEnd::Force),
            "XXXX, XX.",
            (40.0, 80.0),
        ),
        (
            hanging(false, false, HangEnd::Allow),
            "XXXX, XX.",
            (40.0, 90.0),
        ),
    ] {
        fixture.text(&mut layout, &style, text);
        let sizes = layout.intrinsic_sizes();
        assert_eq!(
            (sizes.min_content, sizes.max_content),
            expected,
            "{text:?} under {:?}",
            style.text.hanging_punctuation
        );
    }
    let big = hanging(true, true, HangEnd::None);
    let mut small = sized(&AHEM_FAMILY, 5.0);
    small.text.hanging_punctuation = big.text.hanging_punctuation;
    for (before, after, expected) in [("(", "", 40.0), ("", ")", 40.0)] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&big), |b| {
            b.open_box(NodeKey(1), &small, None);
            b.text(NodeKey(2), before);
            b.close_box();
            b.text(NodeKey(3), "XXXX");
            b.open_box(NodeKey(4), &small, None);
            b.text(NodeKey(5), after);
            b.close_box();
        });
        let sizes = layout.intrinsic_sizes();
        assert_eq!(
            (sizes.min_content, sizes.max_content),
            (expected, expected),
            "{before}XXXX{after}"
        );
    }
}

/// An unbuilt layout, or one whose builder was dropped unfinished, has zero
/// intrinsic sizes and no stale measurements.
#[test]
fn a_layout_with_no_content_asks_for_nothing() {
    let layout = Layout::new();
    assert_eq!(layout.intrinsic_sizes(), IntrinsicSizes::default());
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.text(&mut layout, &families_style(&AHEM_FAMILY), "Some text");
    assert!(layout.intrinsic_sizes().max_content > 0.0);
    let b = layout.builder(
        &mut fixture.cx,
        NodeKey(0),
        &ComputedBlockStyle::new(&ComputedStyle::initial()),
        BuildOptions::default(),
    );
    drop(b);
    assert_eq!(layout.intrinsic_sizes(), IntrinsicSizes::default());
    assert!(
        layout
            .measured()
            .text(FirstLineVariant::Standard)
            .prefix
            .is_empty()
    );
    // Empty content has one boundary and an empty paragraph.
    fixture.text(&mut layout, &families_style(&AHEM_FAMILY), "");
    assert_eq!(positions(&layout), [0.0]);
    assert_eq!(layout.intrinsic_sizes(), IntrinsicSizes::default());
}
