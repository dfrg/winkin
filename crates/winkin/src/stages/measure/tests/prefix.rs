//! Prefix tests. They pin:
//! - the prefix against shaping's advances, exact at 16.16;
//! - fitting positions as Chrome's ceiling of the sum from each paragraph's
//!   start;
//! - exact differences past the 26.6 range;
//! - `NONMONOTONE` exactly on paragraphs whose prefix goes back.

use super::*;
use crate::style::FirstLineVariant;

/// Ahem's advances land in the prefix exactly: each boundary is the size
/// times the clusters before it, and fitting reads the same values.
#[test]
fn ahem_advances_land_exactly() {
    let mut fixture = fixture();
    for size in [16.0, 13.5, 1.0, 72.0] {
        let mut layout = Layout::new();
        fixture.text(&mut layout, &sized(&AHEM_FAMILY, size), "XpÉ x");
        let text = layout.measured().text(FirstLineVariant::Standard);
        for b in 0..=5 {
            let expected =
                InlineLayoutUnit::from_text(TextUnit::from_px_truncated(size * b as f32));
            assert_eq!(text.prefix.get(at(b)), expected, "{size} px at {b}");
            assert_eq!(
                text.prefix.fit_position(at(0), at(b)),
                expected,
                "{size} px at {b}"
            );
        }
        assert_eq!(text.prefix.fit_width(at(0), at(1), at(4)), lu(3.0 * size));
    }
}

/// Fitting rounds each position from the paragraph's start up to the 1/64
/// grid, as Chrome's cached character positions do (`ToCeil`); painting
/// reads exact differences. A third of an em at 16 px is off the grid, so
/// the two differ.
#[test]
fn fitting_rounds_the_running_sum_up_from_the_paragraph() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = families_style(&NARROW);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "abcdefg");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "hijk");
    });
    let advances = fixture.cx.shaped_advances();
    let text = layout.measured().text(FirstLineVariant::Standard);
    let one = advances[0];
    assert_ne!(one.ceil_to_grid(), one, "a third of an em is off the grid");
    let paragraphs = &layout.analysis().paragraphs;
    for (id, _) in paragraphs.iter() {
        let range = paragraphs.clusters(id);
        let mut sum = InlineLayoutUnit::ZERO;
        let check_at = |b: usize, sum: InlineLayoutUnit| {
            // Chrome's position: the sum from the paragraph's start, rounded
            // up.
            assert_eq!(
                text.prefix.fit_position(range.start, at(b)),
                sum.ceil_to_grid(),
                "at {b}"
            );
            assert_eq!(text.prefix.get(at(b)) - text.prefix.get(range.start), sum);
        };
        let shaped = &advances[range.start.get()..range.end.get()];
        for (b, &advance) in (range.start.get()..).zip(shaped) {
            check_at(b, sum);
            sum += advance;
        }
        check_at(range.end.get(), sum);
    }
    // A fitting width is a difference of rounded positions.
    let sum = |n: usize| {
        advances[..n]
            .iter()
            .fold(InlineLayoutUnit::ZERO, |sum, &advance| sum + advance)
    };
    assert_eq!(
        text.prefix.fit_width(at(0), at(1), at(4)),
        (sum(4).ceil_to_grid() - sum(1).ceil_to_grid()).to_layout()
    );
    // The second paragraph fits from its own start.
    let second = layout
        .analysis()
        .paragraphs
        .clusters(ParagraphId::new(1))
        .start;
    assert_eq!(
        text.prefix.fit_position(second, second),
        InlineLayoutUnit::ZERO
    );
    assert_eq!(
        text.prefix.fit_width(second, second, at(second.get() + 1)),
        one.to_layout()
    );
}

/// Each prefix difference is exactly the cluster's shaped advance, with
/// ligatures, split and raised glyphs, and kerns.
#[test]
fn painting_differences_are_exact() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for size in [16.0, 13.3, 7.7] {
        fixture.text(
            &mut layout,
            &sized(&LATIN, size),
            "office AVAIL fit Quiet R flat",
        );
        let advances = fixture.cx.shaped_advances();
        let stages = layout.stages().variant(FirstLineVariant::Standard);
        let text = stages.measured;
        let mut sum = InlineLayoutUnit::ZERO;
        for (c, &advance) in advances.iter().enumerate() {
            assert_eq!(
                text.prefix.advance(at(c), at(c + 1)),
                advance,
                "{size} px, {c}"
            );
            // Without box edges, the pen is the prefix.
            assert_eq!(pen(&stages, at(c)), sum, "{size} px, {c}");
            sum += advance;
        }
        assert_eq!(text.prefix.get(at(advances.len())), sum);
    }
}

/// A paragraph reaching 1e8 px, about three times the 26.6 range, keeps
/// exact differences. Only a width read on the grid saturates, as Chrome's
/// does.
#[test]
fn a_very_long_paragraph_is_exact() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let count = 10_000;
    let long: String = iter::repeat_n('X', count).collect();
    fixture.text(&mut layout, &sized(&AHEM_FAMILY, 10_000.0), &long);
    let text = layout.measured().text(FirstLineVariant::Standard);
    let em = InlineLayoutUnit::from_text(TextUnit::from_px_truncated(10_000.0));
    for c in 0..count {
        assert_eq!(text.prefix.advance(at(c), at(c + 1)), em, "{c}");
    }
    let end = text.prefix.get(at(count));
    assert_eq!(end.raw(), em.raw() * 10_000);
    assert_eq!(end.to_px(), 1e8);
    // A thousand clusters, 1e7 px, fit the grid exactly.
    assert_eq!(
        text.prefix.fit_width(at(0), at(9_000), at(count)),
        lu(10_000_000.0)
    );
    assert_eq!(
        text.prefix.fit_width(at(0), at(0), at(count)),
        LayoutUnit::MAX
    );
    assert_eq!(layout.measured().intrinsic.max, LayoutUnit::MAX);
}

// Monotonicity -------------------------------------------------------------

/// `NONMONOTONE` marks only paragraphs whose prefix goes back: here through
/// a negative margin and through a large negative kern.
#[test]
fn only_a_prefix_that_goes_back_is_marked() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let pulled = edged(&ahem, [-30.0, 0.0, 0.0], [0.0; 3]);
    let narrow = families_style(&NARROW);
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "plain text");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "a");
        b.open_box(NodeKey(4), &pulled, None);
        b.text(NodeKey(5), "b");
        b.close_box();
        b.line_break(NodeKey(6));
        b.open_box(NodeKey(7), &narrow, None);
        b.text(NodeKey(8), "AV");
        b.close_box();
        b.line_break(NodeKey(9));
        b.text(NodeKey(10), "more");
    });
    let text = layout.measured().text(FirstLineVariant::Standard);
    let marked: Vec<bool> = (0..layout.analysis().paragraphs.len())
        .map(|p| {
            text.paragraph(ParagraphId::new(p))
                .contains(MeasureFlags::NONMONOTONE)
        })
        .collect();
    assert_eq!(marked, [false, true, true, false]);
    fixture.text(&mut layout, &ahem, "plain\ttext at any size");
    let text = layout.measured().text(FirstLineVariant::Standard);
    assert!(
        !text
            .paragraph(ParagraphId::new(0))
            .contains(MeasureFlags::NONMONOTONE)
    );
}

/// An initial letter's negative edge room, which recovers before the next
/// boundary, leaves its paragraph unmarked, since the breaker reads only
/// boundaries. Walking it would double a drop-capped article's break time.
#[test]
fn an_initial_letters_edges_leave_its_paragraph_searched() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = sized(&LATIN, 16.0);
    let mut letter = style;
    letter.line.initial_letter = InitialLetterStyle {
        size: 3.0,
        sink: 3,
        ..InitialLetterStyle::NONE
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.set_first_letter(NodeKey(9), &letter, None);
        b.text(NodeKey(1), "alpha bravo charlie delta");
    });
    let text = layout.measured().text(FirstLineVariant::Standard);
    let measured = text.initial_letter().expect("an initial letter");
    assert!(
        measured.edge_room(true) < InlineLayoutUnit::ZERO
            || measured.edge_room(false) < InlineLayoutUnit::ZERO,
        "its edges give some of its advance back"
    );
    assert!(
        !text
            .paragraph(ParagraphId::new(0))
            .contains(MeasureFlags::NONMONOTONE)
    );
}
