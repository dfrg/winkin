//! Glyph position tests. They pin:
//! - glyphs at the exact differences of the prefix;
//! - a line's reshaped edges drawn from its own pieces;
//! - a grapheme a span divides, an item in each part.

use super::*;
use crate::style::FirstLineVariant;

/// Every glyph stands exactly where the prefix puts its cluster's pen, from the line's start.
///
/// This holds across runs in two fonts, in a padded box and on every line.
/// The font's running sums fall between layout's grid.
#[test]
fn glyphs_stand_at_the_exact_differences_of_the_prefix() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = sized(&NARROW, 16.0);
    let larger = sized(&NARROW, 21.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(3.3),
            margin: Sides::from_px(1.7),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "iiiw wiii ");
        b.open_box(NodeKey(2), &larger, None);
        b.text(NodeKey(3), "wwi iw ");
        b.close_box();
        b.open_box(NodeKey(4), &padded, None);
        b.text(NodeKey(5), "wiw iwi");
        b.close_box();
        b.text(NodeKey(6), " iiii wwww iwiw");
    });
    for width in [1000.0, 80.0, 57.3] {
        fixture.lay_out(&mut layout, width);
        let measured = layout.measured().text(FirstLineVariant::Standard);
        let stages = layout.stages().variant(FirstLineVariant::Standard);
        for (id, line) in layout.line_records().lines.iter() {
            let origin = measured.prefix.get(line.clusters().start);
            let input = layout.read_input();
            let mut count = 0;
            for item in items(&layout, id.get()) {
                if item.kind() != FragmentItemKind::Text {
                    continue;
                }
                for glyph in GlyphWalk::new(&input, id, line, item) {
                    let pen = pen(&stages, glyph.cluster);
                    assert_eq!(glyph.x, pen - origin, "{id:?} at {width}: {glyph:?}");
                    count += 1;
                }
            }
            assert!(count > 0);
        }
    }
    // The runs in two fonts are two items, and their sums are not on the
    // grid.
    fixture.lay_out(&mut layout, 1000.0);
    let line = items(&layout, 0);
    assert!(
        line.iter()
            .filter(|item| item.kind() == FragmentItemKind::Text)
            .count()
            >= 3
    );
    assert!(
        line.iter()
            .any(|item| item.inline.ceil_to_grid() != item.inline)
    );
}

/// A line broken inside a ligature or a kerned pair draws its edges from its own reshaped pieces.
///
/// It draws `f` where the paragraph has `ffi`. The last `A` of `AVA` is a
/// whole half em, unkerned, as the breaker measured it.
#[test]
fn a_line_draws_its_reshaped_edges_from_its_own_pieces() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&LATIN, 20.0);
    style.text.word_break = WordBreak::BreakAll;
    let font = latin();
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&style), "office");
    let paragraph = layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .glyphs
        .word(ClusterId::new(1));
    fixture.lay_out(&mut layout, 12.0);
    assert_eq!(texts(&layout), ["o", "f", "f", "i", "c", "e"]);
    assert_eq!(
        glyphs(&layout, 1),
        [(font.glyph('f'), InlineLayoutUnit::ZERO)]
    );
    assert_eq!(
        glyphs(&layout, 2),
        [(font.glyph('f'), InlineLayoutUnit::ZERO)]
    );
    assert_eq!(
        glyphs(&layout, 3),
        [(font.glyph('i'), InlineLayoutUnit::ZERO)]
    );
    // The paragraph's own glyphs still draw the ligature there.
    assert_eq!(
        layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .glyphs
            .word(ClusterId::new(1)),
        paragraph
    );
    assert_ne!(
        paragraph.glyphs(
            &layout
                .shaped()
                .text(FirstLineVariant::Standard)
                .glyphs
                .sidecar
        ),
        ClusterGlyphs::One(font.glyph('f'))
    );
    // `AVA` is one piece. Its first `A` is kerned against the `V`, and its
    // last `A` has nothing after it.
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&style), "AVAVAVAV");
    fixture.lay_out(&mut layout, 28.0);
    assert_eq!(texts(&layout), ["AVA", "VAV", "AV"]);
    let (a, v) = (font.glyph('A'), font.glyph('V'));
    assert_eq!(
        glyphs(&layout, 0),
        [(a, px(0.0)), (v, px(8.0)), (a, px(18.0))]
    );
    // `VAV` starts at an unsafe `V` and ends before another. It is drawn as
    // it shapes on its own, as wide as the breaker measured it. So are the
    // other lines.
    assert_eq!(
        glyphs(&layout, 1),
        [(v, px(0.0)), (a, px(10.0)), (v, px(18.0))]
    );
    // Line 1's start alone is reshaped, so its item mixes the piece's
    // advances with the paragraph's.
    let line = &layout.line_records().lines[LineId::new(1)];
    let shapes = layout.line_records().edges.shapes.as_slice();
    let held: Vec<Range<ClusterId>> = shapes
        .get(line.shapes.start.get()..line.shapes.end.get())
        .unwrap_or_default()
        .iter()
        .map(EdgeShape::clusters)
        .collect();
    assert_eq!(held, [ClusterId::new(3)..ClusterId::new(4)]);
    for (n, width) in [28.0, 28.0, 18.0].into_iter().enumerate() {
        assert_eq!(
            items(&layout, n)[0].size,
            LayoutUnit::from_px(width),
            "line {n}"
        );
    }
    // A right-aligned line's reshaped end meets the band's edge.
    let block = aligned(TextAlign::End, TextAlignLast::Auto);
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..block
        },
        "AVAVAVAV",
    );
    fixture.lay_out(&mut layout, 28.0);
    assert_eq!(lefts(&layout), [0.0, 0.0, 10.0]);
    assert_eq!(items(&layout, 0)[0].size, LayoutUnit::from_px(28.0));
}

/// A text item as [`text_items`] reads it.
///
/// It holds its clusters, its node's key, and its glyphs as `(glyph id,
/// cluster, raised)`.
type TextItem = ((Range<ClusterId>, NodeKey), Vec<(u32, ClusterId, bool)>);

/// Returns line 0's text items.
///
/// It asserts that every item and glyph stands at the exact prefix difference
/// from the line's start.
fn text_items(layout: &Layout) -> Vec<TextItem> {
    let stages = layout.stages().variant(FirstLineVariant::Standard);
    let measured = layout.measured().text(FirstLineVariant::Standard);
    let input = layout.read_input();
    let record = &layout.line_records().lines[LineId::new(0)];
    let origin = measured.prefix.get(record.clusters().start);
    items(layout, 0)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Text)
        .map(|item| {
            let start = item.clusters().start;
            assert_eq!(item.inline, pen(&stages, start) - origin);
            let glyphs = GlyphWalk::new(&input, LineId::new(0), record, item)
                .map(|glyph| {
                    let pen = pen(&stages, glyph.cluster);
                    assert_eq!(glyph.x, pen - origin, "{glyph:?}");
                    (glyph.id, glyph.cluster, glyph.y > TextUnit::from_raw(0))
                })
                .collect();
            (
                (item.clusters(), stages.content.nodes.key(item.node)),
                glyphs,
            )
        })
        .collect()
}

/// A grapheme a span divides gives an item to each part's node.
///
/// Chrome gives the span's text an item of its own. This holds whether the
/// parts shape together or a shaping property parts them. `we`, the span's
/// acute and the following `w` are three items. The span's item draws the
/// raised acute at its own cluster's pen. So the span is found where the
/// mark is, culled or kept.
#[test]
fn a_grapheme_a_span_divides_is_an_item_in_each_part() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let font = marks();
    let root = sized(&MARKS, 16.0);
    let paints = ComputedStyle {
        paints: true,
        ..root
    };
    // A larger size is a shaping property, so the mark is a run of its own.
    let larger = ComputedStyle {
        paints: true,
        ..sized(&MARKS, 20.0)
    };
    let cases = [
        ("bare", root, true),
        ("painting", paints, true),
        ("larger", larger, false),
    ];
    for (name, style, together) in cases {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "we");
            b.open_box(NodeKey(2), &style, None);
            b.text(NodeKey(3), "\u{301}");
            b.close_box();
            b.text(NodeKey(4), "w");
        });
        fixture.lay_out(&mut layout, 500.0);
        let mark = ClusterId::new(2);
        assert!(
            layout.analysis().clusters.is_continuation(mark),
            "the mark continues its grapheme"
        );
        let shaped = layout.shaped().text(FirstLineVariant::Standard);
        assert_eq!(
            shaped.runs.containing(ClusterId::new(1)) == shaped.runs.containing(mark),
            together,
            "{name}: shaped together"
        );
        let (w, e, acute) = (font.glyph('w'), font.glyph('e'), font.glyph('\u{301}'));
        let c = ClusterId::new;
        assert_eq!(
            text_items(&layout),
            [
                (
                    (c(0)..c(2), NodeKey(1)),
                    vec![(w, c(0), false), (e, c(1), false)]
                ),
                ((c(2)..c(3), NodeKey(3)), vec![(acute, c(2), true)]),
                ((c(3)..c(4), NodeKey(4)), vec![(w, c(3), false)]),
            ],
            "{name}"
        );
        let found: Vec<_> = layout.box_fragments(NodeKey(2)).collect();
        assert_eq!(found.len(), 1, "{name}");
        assert_eq!(found[0].is_culled(), !style.paints, "{name}");
        assert_eq!(found[0].inline(), along(16.0, 16.0), "{name}: after `we`");
    }
}

/// A mark the font drew with its base leaves the span's item empty.
///
/// Here `e` and the span's cedilla form one ligature glyph. The glyph belongs
/// to the first part's cluster, as the shaper gave it. The span's item holds
/// the cedilla's cluster and draws nothing. It still follows the ligature, so
/// the span is still found.
#[test]
fn a_mark_the_font_drew_with_its_base_leaves_its_item_empty() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let font = marks();
    let root = sized(&MARKS, 16.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "we");
        b.open_box(NodeKey(2), &root, None);
        b.text(NodeKey(3), "\u{327}");
        b.close_box();
        b.text(NodeKey(4), "w");
    });
    fixture.lay_out(&mut layout, 500.0);
    let c = ClusterId::new;
    assert!(
        layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .glyphs
            .word(c(2))
            .is_continuation(),
        "the cedilla's glyph is its base's"
    );
    let (w, ligature) = (font.glyph('w'), font.ligature_glyph(0));
    assert_eq!(
        text_items(&layout),
        [
            (
                (c(0)..c(2), NodeKey(1)),
                vec![(w, c(0), false), (ligature, c(1), false)]
            ),
            ((c(2)..c(3), NodeKey(3)), vec![]),
            ((c(3)..c(4), NodeKey(4)), vec![(w, c(3), false)]),
        ]
    );
    // The box starts after `w` (half an em) and the ligature (three
    // quarters).
    let found: Vec<_> = layout.box_fragments(NodeKey(2)).collect();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].inline(), along(20.0, 20.0));
}

/// Returns the pen position for drawing `cluster`, seeking its item first (`LineStages::pen`).
fn pen(stages: &LineStages<'_>, cluster: ClusterId) -> InlineLayoutUnit {
    stages.pen(
        cluster,
        stages
            .analysis
            .item_clusters
            .cursor_containing(cluster)
            .id(),
    )
}
