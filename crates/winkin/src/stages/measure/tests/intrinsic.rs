//! Intrinsic size and tab tests. They pin:
//! - min-content as the widest run no line may break inside, and max-content
//!   as the widest paragraph;
//! - tabs reaching the next stop past where they land, on and off the grid;
//! - white space that hangs conditionally counted in max-content;
//! - hanging punctuation left out, `allow-end` from min-content alone.

use super::*;
use crate::style::FirstLineVariant;

/// On Ahem at 10 px: min-content is the widest run no line may break
/// inside, and max-content the widest paragraph, each without the spaces
/// that hang at its end.
#[test]
fn intrinsic_sizes_are_the_widest_unbreakable_run_and_paragraph() {
    let mut fixture = fixture();
    let ahem = sized(&AHEM_FAMILY, 10.0);
    let mut layout = Layout::new();
    fixture.text(&mut layout, &ahem, "XX XXXX XXX");
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (40.0, 110.0));

    // The widest paragraph, and the widest word in any.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "XXX XX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "X XXXXX X X X");
        b.line_break(NodeKey(4));
        b.text(NodeKey(5), "XX");
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (50.0, 130.0));

    // `overflow-wrap: anywhere` lowers min-content to the widest cluster,
    // and `break-word` does not; `nowrap` offers no
    // opportunity at all.
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

    // A box closing after a hanging space: the space hangs and the edge
    // counts; an atomic inline is as wide as its
    // margin box.
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

/// A tab reaches the first stop past where it lands, stops being multiples
/// of `tab-size` from the content edge: from the edge, a whole interval; on
/// a stop, on to the next; nearer the next than half a space, on to the one
/// after, as Chrome's `Font::TabWidth` has it; its width rounded up onto the
/// grid. A length is the interval itself. Stops nothing apart make nothing,
/// and a `tab-size` no computed value has is taken as none.
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
    // The stops are the content edge's, wherever the line starts.
    assert_eq!(tab(15.0, px(0.0), eight), 65.0);
    assert_eq!(tab(15.0, px(65.0), eight), 80.0);
    // Where it lands between the grid's points, it ends at the stop rounded
    // up: 13.3203125 from the edge reaches 46.6875.
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
    // Before the content edge it reaches the stop past it, as Chrome 155
    // ends a tab starting 10 px before the edge at the edge, and one
    // starting 110 px before at 100 px before, under 100 px stops (WPT's
    // `text-indent-tab-positions-001`).
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
    // In a font with no space, stops of spaces are nothing apart.
    let spaceless = TabStops::new(
        TabSize::Spaces(8.0),
        InlineLayoutUnit::ZERO,
        TextUnit::from_raw(0),
        TextUnit::from_raw(0),
    );
    assert_eq!(tab(0.0, px(15.0), spaceless), 0.0);
}

/// Stops and positions at the ends of the grid give a width, saturating
/// where they must, and never panic.
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

/// A tab's stops count in its style's space: its primary font's U+0020 at
/// the used size, a whole em in Ahem and a third of one in the narrow font.
/// Where there is no font to read, a quarter of an em.
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

/// The intrinsic sizes size each tab where it lands: from the paragraph's
/// start for max-content, and from its min-content line's for min-content,
/// where every opportunity ends a line. A tab that hangs, as under
/// `pre-wrap`, counts in no min-content line, and ending a paragraph, where
/// it hangs only where
/// it overflows, counts in max-content; under `break-spaces` it is content;
/// under `pre` it is content and nothing breaks, so the two are one. In
/// Ahem at 10 px with stops 40 apart, as Chrome 153 sizes them.
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
    // `X` to 10, a tab to 40, `XX` to 60, a tab to 80, `X` to 90; the
    // widest min-content line is `XX` and its tab, which hangs.
    assert_eq!(
        sizes(&mut fixture, &mut layout, &style, "X\tXX\tX"),
        (20.0, 90.0)
    );
    // A tab ending the paragraph hangs where it overflows, which a line of
    // max-content has room for, and one ending a min-content line hangs.
    assert_eq!(
        sizes(&mut fixture, &mut layout, &style, "XXX\t"),
        (30.0, 40.0)
    );
    // A paragraph's whole trailing run hangs so, though an opportunity
    // after its space parts it: max-content is what the breaker measures
    // of the line, `XX`, its space and its tab.
    assert_eq!(
        sizes(&mut fixture, &mut layout, &style, "XX \t"),
        (20.0, 40.0)
    );
    // Each min-content line starts the tab stops again: `XXXXX ` ends at 60,
    // its space hanging, and on the next line `X` reaches 10 and its tab 40.
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
    // Each paragraph's tabs from its own start.
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "XXXXX\tX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "\tX");
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (50.0, 90.0));
}

/// White space ending a paragraph counts in max-content where it hangs only
/// conditionally, kept and wrapping (`pre-wrap`), and in neither size where
/// it hangs whole, as other space separators do under `normal`; kept and not
/// wrapping (`pre`) it does not hang, and counts in both; min-content leaves
/// out all that hangs (CSS Text 3, "trimming and positioning"). These are
/// Chrome 153's widths in Ahem at 20 px, but for an ideographic space, which
/// Chrome counts in both where it hangs, and which hangs here as CSS says.
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

/// A punctuation mark that hangs is left out of the intrinsic sizes, as
/// CSS Text 4 says: under `first`, `last` and `force-end` from both, under
/// `allow-end`, which hangs only where the line would not fit, from
/// min-content alone. A mark smaller than its text gives back its own
/// advance. On Ahem at 10 px.
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
    // A mark at half the text's size hangs by its own advance.
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

/// Nothing built, or a builder dropped without finishing, asks for nothing,
/// and holds no measurements of content it no longer has.
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
