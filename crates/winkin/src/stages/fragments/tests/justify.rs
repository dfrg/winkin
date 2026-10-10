//! Justification and spacing tests. They pin:
//! - a justified line spending exactly its room, where `text-justify` says;
//! - justified tabs, and reading a justified line in linear steps;
//! - `text-group-align` and `line-padding`;
//! - hanging punctuation outside the line box;
//! - letter-spacing turning ligatures off;
//! - valid items under any spacing.

use super::*;

/// Returns each cluster of line `n`'s text items as the cluster walk reads it, `(left, advance)` in pixels.
fn cluster_steps(layout: &Layout, n: usize) -> Vec<(f32, f32)> {
    let input = layout.read_input();
    let id = LineId::new(n);
    let line = &layout.line_records().lines[id];
    items(layout, n)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Text)
        .flat_map(|item| {
            ClusterWalk::new(&input, id, line, item)
                .map(|(_, left, step)| (left.to_px(), step.to_px()))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Returns each cluster of line `n`'s text items as the cluster walk reads it, in logical order.
///
/// Each is `(cluster, left, advance)`, exactly.
fn cluster_walk(layout: &Layout, n: usize) -> Vec<(ClusterId, InlineLayoutUnit, InlineLayoutUnit)> {
    let input = layout.read_input();
    let id = LineId::new(n);
    let line = &layout.line_records().lines[id];
    let mut walked: Vec<_> = items(layout, n)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Text)
        .flat_map(|item| ClusterWalk::new(&input, id, line, item).collect::<Vec<_>>())
        .collect();
    walked.sort_by_key(|&(cluster, _, _)| cluster);
    walked
}

/// A justified line ends exactly at its band's edge.
///
/// The division's leftover goes to the opportunity Chrome hands out last
/// (`ShapeResultSpacing::NextExpansion`). That is the last opportunity of the
/// logically last item that has one. Where that item reads right to left, it
/// is the first, since Chrome walks an item's glyphs in visual order. The
/// fonts are Ahem at 10 px and Test Arabic, whose letters are half an em.
#[test]
fn a_justified_line_ends_exactly_at_its_band_edge() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = aligned(TextAlign::Justify, TextAlignLast::Justify);
    let raw = InlineLayoutUnit::from_raw;
    // With 1 px over three spaces, the last takes a third plus the 1/65536 px
    // the thirds leave. So the last `X` is at 61 exactly.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        "X X X X",
    );
    fixture.lay_out(&mut layout, 71.0);
    let justify = layout.fragments().justified()[0].justification;
    let per = raw(65536 / 3);
    assert_eq!(
        (justify.per, justify.leftover(), justify.last),
        (per, raw(1), ClusterId::new(5))
    );
    let xs: Vec<_> = glyphs(&layout, 0).iter().map(|&(_, x)| x).collect();
    assert_eq!(xs[4], px(40.0) + per + per);
    assert_eq!(xs[6], px(61.0));
    assert_eq!(reach(&layout, 0), px(71.0));
    // Latin in a right-to-left paragraph is a left-to-right item at level 2.
    // The leftover goes to its last space, the rightmost. The room is 10 px
    // over three.
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..block
    };
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..rtl
        },
        "XX XX XX XX",
    );
    fixture.lay_out(&mut layout, 120.0);
    let justify = layout.fragments().justified()[0].justification;
    let per = raw(655_360 / 3);
    assert_eq!(
        (justify.per, justify.leftover(), justify.last),
        (per, raw(1), ClusterId::new(8))
    );
    let walk = cluster_walk(&layout, 0);
    assert_eq!(walk[8].2, px(10.0) + per + raw(1));
    assert_eq!((walk[2].2, walk[5].2), (px(10.0) + per, px(10.0) + per));
    assert_eq!(walk[9].1, px(90.0) + per + per + per + raw(1));
    assert_eq!(walk[0].1, InlineLayoutUnit::ZERO);
    assert_eq!(reach(&layout, 0), px(120.0));
    // Arabic reads right to left, and Chrome walks its item from its logical
    // end back, so the leftover goes to its first space, the rightmost.
    let words = "\u{628}\u{62A} \u{628}\u{62A} \u{628}\u{62A} \u{628}\u{62A}";
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..rtl
        },
        words,
    );
    fixture.lay_out(&mut layout, 100.5);
    let justify = layout.fragments().justified()[0].justification;
    assert_ne!(justify.leftover(), InlineLayoutUnit::ZERO);
    assert_eq!(justify.last, ClusterId::new(2));
    let walk = cluster_walk(&layout, 0);
    assert_eq!(walk[2].2 - walk[5].2, justify.leftover());
    assert_eq!(walk[5].2, walk[8].2);
    assert_eq!(walk[10].1, InlineLayoutUnit::ZERO);
    assert_eq!(reach(&layout, 0), px(100.5));
    // An ideograph after a letter has room before it. That is the line's
    // last opportunity, since the room after the line's last cluster is
    // dropped. So its glyph moves by the leftover too. The room is 40 px
    // over three.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        "X X X\u{65E5}",
    );
    fixture.lay_out(&mut layout, 100.0);
    let justify = layout.fragments().justified()[0].justification;
    let per = raw(40 * 65536 / 3);
    assert_eq!(
        (justify.per, justify.leftover(), justify.last),
        (per, raw(1), ClusterId::new(5))
    );
    let ideograph = glyphs(&layout, 0).last().map(|&(_, x)| x);
    assert_eq!(ideograph, Some(px(90.0)));
    assert_eq!(reach(&layout, 0), px(100.0));
    // In a left-to-right line whose last item reads right to left, the
    // leftover goes to that item's first space, whatever fonts divide it.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        "XX \u{628}\u{62A} \u{633}\u{645} \u{628}\u{62A}",
    );
    fixture.lay_out(&mut layout, 100.5);
    let justify = layout.fragments().justified()[0].justification;
    assert_ne!(justify.leftover(), InlineLayoutUnit::ZERO);
    assert_eq!(justify.last, ClusterId::new(5));
    let walk = cluster_walk(&layout, 0);
    assert_eq!(walk[5].2 - walk[8].2, justify.leftover());
    assert_eq!(reach(&layout, 0), px(100.5));
}

/// Every justified line spends exactly the room it was given, at any width.
///
/// The paragraph mixes proportional fonts of several sizes in spans, with a
/// box's edges among them. A line's room is the band less its width on
/// Chrome's fitting grid, as Chrome's `space` is the available width less
/// the line's snapped width. So it ends where Chrome's does. That is the
/// band's edge, off by the gap between its exact and grid widths, which a
/// centered line of the same text shows.
#[test]
fn every_justified_line_spends_exactly_its_room() {
    let mut fixture = fixture();
    let mut justified = Layout::new();
    let mut centered = Layout::new();
    let root = sized(&LATIN, 13.0);
    let narrow = sized(&NARROW, 17.3);
    let boxed = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(2.3),
            margin: Sides::from_px(0.7),
            ..EdgesGroup::INITIAL
        },
        ..sized(&LATIN, 11.0)
    };
    let calls = |b: &mut LayoutBuilder<'_>| {
        b.text(NodeKey(1), "AVAVA office waffle ");
        b.open_box(NodeKey(2), &narrow, None);
        b.text(NodeKey(3), "iiw wi iwiw w ");
        b.close_box();
        b.open_box(NodeKey(4), &boxed, None);
        b.text(NodeKey(5), "fluff to the flat ");
        b.close_box();
        b.text(NodeKey(6), "AV iw fi ffi wave after wave of it");
    };
    let justify = aligned(TextAlign::Justify, TextAlignLast::Justify);
    let center = aligned(TextAlign::Center, TextAlignLast::Center);
    fixture.build(
        &mut justified,
        &ComputedBlockStyle {
            style: &root,
            ..justify
        },
        calls,
    );
    fixture.build(
        &mut centered,
        &ComputedBlockStyle {
            style: &root,
            ..center
        },
        calls,
    );
    let mut lines = 0;
    for width in [61.0, 77.3, 90.0, 103.7, 131.0, 157.9, 200.0] {
        fixture.lay_out(&mut justified, width);
        fixture.lay_out(&mut centered, width);
        assert_eq!(texts(&justified), texts(&centered), "{width}");
        let band = InlineLayoutUnit::from_layout(LayoutUnit::from_px(width));
        for row in justified.fragments().justified() {
            let line = row.line;
            let fitted = justified.line_records().lines[line].width;
            let exact = reach(&centered, line.get());
            let residue = exact - InlineLayoutUnit::from_layout(fitted);
            assert_eq!(
                reach(&justified, line.get()),
                band + residue,
                "{width}: {line:?}"
            );
            lines += 1;
        }
    }
    assert!(lines > 10, "{lines}");
}

/// A justified line divides its spare room evenly among its opportunities, as Chrome's `ShapeResultSpacing` does.
///
/// Each share is the room over the count, truncated. In Ahem at 10 px, `XX XX
/// XX` is 80 long in a band of 100, so each of its two spaces takes 10. The
/// glyphs after them move along, and the line fills its band. The last line
/// is set at its start. A reader walks what was placed.
#[test]
fn a_justified_line_spends_its_room_on_its_spaces() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = aligned(TextAlign::Justify, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        "XX XX XX XX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XX XX XX ", "XX"]);
    let justify = layout.fragments().justified();
    assert_eq!(justify.len(), 1);
    assert_eq!(justify[0].line, LineId::new(0));
    assert_eq!(justify[0].justification.per, px(10.0));
    // Ahem draws its spaces as glyphs of their own.
    assert_eq!(
        xs(&layout, 0),
        [0.0, 10.0, 20.0, 40.0, 50.0, 60.0, 80.0, 90.0]
    );
    assert_eq!(
        cluster_steps(&layout, 0),
        [
            (0.0, 10.0),
            (10.0, 10.0),
            (20.0, 20.0),
            (40.0, 10.0),
            (50.0, 10.0),
            (60.0, 20.0),
            (80.0, 10.0),
            (90.0, 10.0)
        ]
    );
    // Its content fills its band.
    assert_eq!(reach(&layout, 0), px(100.0));
    assert_eq!(xs(&layout, 1), [0.0, 10.0]);
    // Room that does not divide evenly gives each a truncated third, and
    // the leftover to the last. So the line ends exactly at its band.
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..block
        },
        "X X X X XXXXXXXXXXX",
    );
    fixture.lay_out(&mut layout, 71.0);
    assert_eq!(texts(&layout)[0], "X X X X ");
    let per = layout.fragments().justified()[0].justification.per;
    assert_eq!(
        per,
        InlineLayoutUnit::from_raw(px(1.0 / 3.0 * 3.0).raw() / 3)
    );
    assert_eq!(reach(&layout, 0), px(71.0));
    // A right-to-left line fills its band from its right.
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..block
    };
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..rtl
        },
        "XX XX XX XX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout)[0], 0.0);
    let reach = cluster_steps(&layout, 0)
        .iter()
        .map(|&(left, step)| left + step)
        .fold(0.0, f32::max);
    assert_eq!(reach, 100.0);
}

/// A justified line with nowhere to put its room is set at its start, with no record kept.
///
/// That covers a line with no opportunity, an overflowing line, and
/// `text-justify: none`. `text-align-last: justify` justifies a paragraph's
/// last line too.
#[test]
fn a_line_with_nowhere_to_put_the_room_is_set_at_its_start() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let last = aligned(TextAlign::Justify, TextAlignLast::Justify);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &ahem(10.0),
            ..last
        },
        "XX XX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(xs(&layout, 0), [0.0, 10.0, 20.0, 80.0, 90.0]);
    for text in ["XXXX", "XXXXXXXXXXXX"] {
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem(10.0),
                ..last
            },
            text,
        );
        fixture.lay_out(&mut layout, 100.0);
        assert!(layout.fragments().justified().is_empty(), "{text}");
        assert_eq!(lefts(&layout), [0.0], "{text}");
    }
    let mut none = ahem(10.0);
    none.text.justify = TextJustify::None;
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &none,
            ..last
        },
        "XX XX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert!(layout.fragments().justified().is_empty());
    assert_eq!(xs(&layout, 0), [0.0, 10.0, 20.0, 30.0, 40.0]);
}

/// Reading a justified line costs linear steps in its text and items.
///
/// A reader must not rescan the whole line for each item.
#[cfg(debug_assertions)]
#[test]
fn reading_a_justified_line_costs_linear_steps() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    let block = ComputedBlockStyle {
        text_align: TextAlign::Justify,
        text_align_last: TextAlignLast::Justify,
        ..ComputedBlockStyle::new(&style)
    };
    let mut steps = |count: u64| {
        fixture.build(&mut layout, &block, |builder| {
            for n in 0..count {
                builder.text(NodeKey(n + 1), "word word ");
            }
        });
        fixture.lay_out(&mut layout, 100_000.0);
        assert_eq!(layout.lines().len(), 1);
        work::take();
        let mut glyphs = 0;
        let mut fragments = 0;
        for line in layout.lines() {
            for item in line.items() {
                if let crate::Item::Text(run) = item {
                    assert!(run.advance() > 0.0);
                    assert!(run.inline().right > run.inline().left);
                    glyphs += run.glyphs().count();
                    fragments += 1;
                }
            }
        }
        assert_eq!(fragments, count);
        assert!(glyphs > 0);
        work::take()
    };
    let small = steps(64);
    let large = steps(256);
    assert!(small > 0);
    assert!(
        large <= small * 5,
        "four times the content took {large} steps against {small}"
    );
}

/// `text-justify` chooses where a justified line's room goes.
///
/// - `auto`: a space, a tab and U+00A0 each take a share after them. An
///   ideograph takes one after it, and one before it where a letter precedes
///   it. The line's first cluster gets none before, and its last none after.
/// - `inter-word`: only the spaces take a share.
/// - `inter-character`: every cluster but the last takes a share after it.
/// - U+2007 and the zero-width space take none, and the zero-width space is
///   invisible to its neighbours.
#[test]
fn text_justify_chooses_where_the_room_goes() {
    use crate::style::TextJustify;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = aligned(TextAlign::Justify, TextAlignLast::Justify);
    let counts = |layout: &Layout| -> Vec<u32> {
        let line = &layout.line_records().lines[LineId::new(0)];
        let opportunities = crate::stages::measure::JustifyOpportunities::from_line(
            &layout.stages().variant(FirstLineVariant::Standard),
            line.clusters().start..line.content_end(&layout.analysis().clusters),
            None,
        )
        .expect("justified");
        (line.clusters().start..line.content_end(&layout.analysis().clusters))
            .ids()
            .map(|cluster| opportunities.count(cluster))
            .collect()
    };
    for (justify, text, expected) in [
        (
            TextJustify::Auto,
            "ab\u{65E5}\u{672C}\u{8A9E}",
            vec![0, 0, 2, 1, 0],
        ),
        (
            TextJustify::InterWord,
            "ab\u{65E5}\u{672C}\u{8A9E}",
            vec![0; 5],
        ),
        (
            TextJustify::InterCharacter,
            "ab\u{65E5}\u{672C}\u{8A9E}",
            vec![1, 1, 1, 1, 0],
        ),
        (TextJustify::Auto, "\u{65E5}\u{672C} a", vec![1, 1, 1, 0]),
        (
            TextJustify::Auto,
            "a\u{A0}b\u{2007}c d",
            vec![0, 1, 0, 0, 0, 1, 0],
        ),
        (TextJustify::Auto, "\u{65E5}\u{200B}\u{672C}", vec![1, 0, 0]),
        (TextJustify::InterWord, "a b\u{3000}c", vec![0, 1, 0, 0, 0]),
    ] {
        let mut style = ahem(10.0);
        style.text.justify = justify;
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            text,
        );
        fixture.lay_out(&mut layout, 1000.0);
        assert_eq!(counts(&layout), expected, "{justify:?} {text:?}");
    }
    // `inter-character` spreads a word, here 60 over three gaps.
    let mut style = ahem(10.0);
    style.text.justify = TextJustify::InterCharacter;
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..block
        },
        "XXXX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(xs(&layout, 0), [0.0, 30.0, 60.0, 90.0]);
}

/// Each cluster takes room under its own text's `text-justify`.
///
/// Chrome 155 reads the property item by item. In Ahem at 20 px on a line
/// 220 wide, justified as its last:
/// - `X <span>X X</span> X` with the span under `none` puts its `X`s at 0,
///   80, 120 and 200: the span's space takes nothing;
/// - the block under `none` and the span under `inter-word` puts them at 0,
///   40, 160 and 200: the span's space takes all the room;
/// - `XX <span>XX</span> XX` with the span under `inter-character` gives
///   each of its two `X`s a share of 15, as each space has.
#[test]
fn each_cluster_takes_room_under_its_own_text_justify() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let styled = |justify| {
        let mut style = ahem(20.0);
        style.text.justify = justify;
        style
    };
    let cases = [
        (TextJustify::Auto, TextJustify::None, ["X ", "X X", " X"]),
        (
            TextJustify::None,
            TextJustify::InterWord,
            ["X ", "X X", " X"],
        ),
    ];
    let expected = [[0.0, 80.0, 120.0, 200.0], [0.0, 40.0, 160.0, 200.0]];
    for ((block, span, [before, inside, after]), expected) in cases.into_iter().zip(expected) {
        let (block_style, span_style) = (styled(block), styled(span));
        let style = ComputedBlockStyle {
            style: &block_style,
            ..aligned(TextAlign::Justify, TextAlignLast::Justify)
        };
        fixture.build(&mut layout, &style, |b| {
            b.text(NodeKey(1), before);
            b.open_box(NodeKey(2), &span_style, None);
            b.text(NodeKey(3), inside);
            b.close_box();
            b.text(NodeKey(4), after);
        });
        fixture.lay_out(&mut layout, 220.0);
        let xs: Vec<f32> = xs(&layout, 0).into_iter().step_by(2).collect();
        assert_eq!(xs, expected, "{block:?} {span:?}");
    }
    let (block_style, span_style) = (
        styled(TextJustify::Auto),
        styled(TextJustify::InterCharacter),
    );
    let style = ComputedBlockStyle {
        style: &block_style,
        ..aligned(TextAlign::Justify, TextAlignLast::Justify)
    };
    fixture.build(&mut layout, &style, |b| {
        b.text(NodeKey(1), "XX ");
        b.open_box(NodeKey(2), &span_style, None);
        b.text(NodeKey(3), "XX");
        b.close_box();
        b.text(NodeKey(4), " XX");
    });
    fixture.lay_out(&mut layout, 220.0);
    assert_eq!(
        xs(&layout, 0),
        [0.0, 20.0, 40.0, 75.0, 110.0, 145.0, 180.0, 200.0]
    );
}

/// `inter-character` leaves a run of atomic inlines, and a cursive script's
/// letters, whole, and the character after either takes room before it.
///
/// Measured in Chrome 155:
/// - `X`, two 20 px inline blocks and `X` in Ahem at 20 px on a line 100
///   wide puts the blocks at 30 and 50 and the last `X` at 80: one share
///   after the first `X`, one before the last;
/// - two Arabic words, a space and three Latin letters take room only at
///   the space (before and after it), the first letter (before and after
///   it) and the second, as Chrome leaves Arabic, Syriac, Mongolian, N'Ko,
///   Mandaic, Hanifi Rohingya and Phags-pa whole.
#[test]
fn inter_character_leaves_atomic_inlines_and_cursive_letters_whole() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = ahem(20.0);
    style.text.justify = TextJustify::InterCharacter;
    let block = ComputedBlockStyle {
        style: &style,
        ..aligned(TextAlign::Justify, TextAlignLast::Justify)
    };
    let size = BoxSize {
        inline: 20.0,
        block: 20.0,
        baseline: None,
    };
    fixture.build(&mut layout, &block, |b| {
        b.text(NodeKey(1), "X");
        b.atomic(NodeKey(2), &style, None, size);
        b.atomic(NodeKey(3), &style, None, size);
        b.text(NodeKey(4), "X");
    });
    fixture.lay_out(&mut layout, 100.0);
    let atomics: Vec<_> = items(&layout, 0)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Atomic)
        .map(|item| item.inline)
        .collect();
    assert_eq!(atomics, [px(30.0), px(50.0)]);
    assert_eq!(xs(&layout, 0), [0.0, 80.0]);
    let mut arabic = style;
    arabic.font.families = &ARABIC;
    let words = alloc::format!("{BEH}{TEH} {BEH}{TEH}XXX");
    fixture.build(&mut layout, &block, |b| {
        b.open_box(NodeKey(1), &arabic, None);
        b.text(NodeKey(2), &words);
        b.close_box();
    });
    fixture.lay_out(&mut layout, 1000.0);
    let line = &layout.line_records().lines[LineId::new(0)];
    let clusters = line.clusters().start..line.content_end(&layout.analysis().clusters);
    let opportunities = crate::stages::measure::JustifyOpportunities::from_line(
        &layout.stages().variant(FirstLineVariant::Standard),
        clusters.clone(),
        None,
    )
    .expect("justified");
    let counts: Vec<u32> = clusters
        .ids()
        .map(|cluster| opportunities.count(cluster))
        .collect();
    assert_eq!(counts, [0, 0, 2, 0, 0, 2, 1, 0]);
}

/// Tabs take a share of a justified line's room, as Chrome's do, unless the config keeps their stops.
///
/// A tab's width rounds up onto the grid, as Chrome snaps a tab. Under
/// `TabJustification::KeepStops`, the spaces take all the room. In Ahem at
/// 10 px with stops 40 apart, `X⇥X X` is 70 long.
#[test]
fn a_justified_tab_stretches_unless_it_keeps_its_stop() {
    use crate::config::{Config, TabJustification};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = aligned(TextAlign::Justify, TextAlignLast::Justify);
    let mut style = ahem(10.0);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    style.text.tab_size = TabSize::Spaces(4.0);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..block
        },
        "X\tX X",
    );
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(xs(&layout, 0), [0.0, 105.0, 115.0, 190.0]);
    let tab = items(&layout, 0)
        .iter()
        .find(|item| item.flags.contains(FragmentItemFlags::TAB))
        .copied()
        .expect("a tab");
    assert_eq!(tab.size, LayoutUnit::from_px(95.0));
    fixture.cx.set_config(Config {
        tab_justification: TabJustification::KeepStops,
        ..Config::default()
    });
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(xs(&layout, 0), [0.0, 40.0, 50.0, 190.0]);
}

/// `text-group-align` aligns the lines as one group.
///
/// It narrows every line's band, on the side it names, by the least room any
/// line spares. Each line is then aligned in its narrowed band. With `XX` and
/// `XXXXX` in Ahem at 10 px in a band of 100, the least room is 50.
#[test]
fn group_alignment_moves_the_lines_as_one() {
    use crate::style::TextGroupAlign;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for (group, align, expected) in [
        (TextGroupAlign::None, TextAlign::Start, [0.0, 0.0]),
        (TextGroupAlign::Center, TextAlign::Start, [25.0, 25.0]),
        (TextGroupAlign::Right, TextAlign::Start, [50.0, 50.0]),
        (TextGroupAlign::Right, TextAlign::Center, [65.0, 50.0]),
        (TextGroupAlign::End, TextAlign::Start, [50.0, 50.0]),
    ] {
        let block = ComputedBlockStyle {
            text_group_align: group,
            ..aligned(align, TextAlignLast::Auto)
        };
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem(10.0),
                ..block
            },
            |b| {
                b.text(NodeKey(1), "XX");
                b.line_break(NodeKey(2));
                b.text(NodeKey(3), "XXXXX");
            },
        );
        fixture.lay_out(&mut layout, 100.0);
        assert_eq!(lefts(&layout), expected, "{group:?} {align:?}");
    }
    // A line that overflows spares nothing, so the group spares nothing.
    let block_style = ahem(10.0);
    let block = ComputedBlockStyle {
        text_group_align: TextGroupAlign::Center,
        ..ComputedBlockStyle::new(&block_style)
    };
    fixture.block_text(&mut layout, &block, "XXXXXXXXXXXX");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [0.0]);
}

/// `line-padding` is room inside each end of every line.
///
/// The innermost box at that end covers it. The text starts that far in, a
/// right-aligned line stops that far short, and a box at a line's end is that
/// much wider.
#[test]
fn line_padding_pads_both_ends_of_every_line() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut padded = ahem(10.0);
    padded.line.padding = 5.0;
    let painted = ComputedStyle {
        paints: true,
        ..padded
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&padded), |b| {
        b.open_box(NodeKey(1), &painted, None);
        b.text(NodeKey(2), "XX");
        b.close_box();
        b.text(NodeKey(3), " XX");
    });
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(xs(&layout, 0), [5.0, 15.0, 25.0, 35.0, 45.0]);
    let part = items(&layout, 0)
        .iter()
        .find(|item| item.kind() == FragmentItemKind::Box)
        .copied()
        .expect("the painted box");
    assert_eq!(part.inline, InlineLayoutUnit::ZERO);
    assert_eq!(part.size, LayoutUnit::from_px(25.0));
    let right = aligned(TextAlign::Right, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &padded,
            ..right
        },
        "XX XX",
    );
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(lefts(&layout), [40.0]);
    assert_eq!(
        xs(&layout, 0),
        [5.0, 15.0, 25.0, 35.0, 45.0],
        "from the line box"
    );
    // Every line pays for both ends, so 50 no longer fits `XX XX`.
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&padded), "XX XX");
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(texts(&layout), ["XX ", "XX"]);
}

/// A hanging mark is drawn outside the line box, in Ahem at 10 px.
///
/// - Under `first`, the opening mark sits before the line's left, or in the
///   indent where there is one. The text after it starts at the edge.
/// - Under `force-end`, a comma hangs past a right-aligned line's right edge.
/// - A justified line spreads only what is inside its edges.
#[test]
fn a_hanging_mark_is_drawn_outside_the_line_box() {
    use crate::style::{HangEnd, HangingPunctuation};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut first = ahem(10.0);
    first.text.hanging_punctuation = HangingPunctuation {
        first: true,
        last: false,
        end: HangEnd::None,
    };
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&first), "(XX XX");
    fixture.lay_out(&mut layout, 50.0);
    assert_eq!(xs(&layout, 0), [-10.0, 0.0, 10.0, 20.0, 30.0, 40.0]);
    let indented = ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage {
                px: 20.0,
                fraction: 0.0,
            },
            hanging: false,
            each_line: false,
        },
        ..ComputedBlockStyle::new(&first)
    };
    fixture.block_text(&mut layout, &indented, "(XX XX");
    fixture.lay_out(&mut layout, 70.0);
    assert_eq!(xs(&layout, 0), [10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
    let mut end = ahem(10.0);
    end.text.hanging_punctuation = HangingPunctuation {
        first: false,
        last: false,
        end: HangEnd::Force,
    };
    let right = aligned(TextAlign::Right, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &end,
            ..right
        },
        "XX XX, XX",
    );
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(texts(&layout)[0], "XX XX, ");
    assert_eq!(lefts(&layout)[0], 10.0);
    assert_eq!(xs(&layout, 0), [0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    let justified = aligned(TextAlign::Justify, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &end,
            ..justified
        },
        "XX XX, XX",
    );
    fixture.lay_out(&mut layout, 60.0);
    assert_eq!(xs(&layout, 0), [0.0, 10.0, 20.0, 40.0, 50.0, 60.0]);
}

/// Letter-spacing turns off the common ligatures.
///
/// So `office` in Test Latin is six glyphs, not four, each spaced, the last
/// included.
#[test]
fn letter_spaced_text_draws_no_ligature() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let plain = sized(&LATIN, 20.0);
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&plain), "office");
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(
        glyphs(&layout, 0).len(),
        4,
        "o, f, fi and c, e: ffi's glyph"
    );
    let mut spaced = plain;
    spaced.text.letter_spacing = 2.0;
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&spaced), "office");
    fixture.lay_out(&mut layout, 500.0);
    assert_eq!(xs(&layout, 0), [0.0, 12.0, 24.0, 36.0, 48.0, 60.0]);
}

/// Any spacing, padding, hanging, indent or justification values lay out into valid items at any width.
#[test]
fn any_spacing_or_justification_gives_valid_items() {
    use crate::style::{HangEnd, HangingPunctuation, TextGroupAlign, TextJustify};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for bad in [f32::NAN, f32::INFINITY, -1e30, f32::MAX, 0.25] {
        let mut style = sized(&LATIN, 20.0);
        style.text.letter_spacing = bad;
        style.text.word_spacing = LengthPercentage {
            px: bad,
            fraction: bad,
        };
        style.line.padding = bad;
        style.text.hanging_punctuation = HangingPunctuation {
            first: true,
            last: true,
            end: HangEnd::Allow,
        };
        for justify in [TextJustify::Auto, TextJustify::InterCharacter] {
            style.text.justify = justify;
            for direction in [BaseDirection::Ltr, BaseDirection::Rtl] {
                let block = ComputedBlockStyle {
                    direction,
                    text_group_align: TextGroupAlign::Center,
                    text_indent: TextIndent {
                        amount: LengthPercentage {
                            px: bad,
                            fraction: bad,
                        },
                        hanging: true,
                        each_line: true,
                    },
                    ..aligned(TextAlign::Justify, TextAlignLast::Justify)
                };
                fixture.build(
                    &mut layout,
                    &ComputedBlockStyle {
                        style: &style,
                        ..block
                    },
                    |b| {
                        b.text(NodeKey(1), "(office AVAVA,\t\u{65E5}\u{672C} waffle.");
                        b.line_break(NodeKey(2));
                        b.text(NodeKey(3), "x)");
                    },
                );
                for width in [0.0, 30.0, 120.0, 1000.0, f32::NAN] {
                    fixture.lay_out(&mut layout, width);
                }
            }
        }
    }
}
