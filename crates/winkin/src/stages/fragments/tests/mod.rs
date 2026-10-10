//! Line layout tests.
//!
//! This file holds the fonts, the fixture, [`check`] and the shared views.
//! It pins the record sizes, line placement kept in the context, a relayout
//! against a fresh layout, and areas that must not panic. The children pin:
//! - `align`: alignment, trailing white space and tabs;
//! - `glyphs`: glyph positions, reshaped edges and divided graphemes;
//! - `boxes`: kept and culled boxes, atomic inlines and box edges;
//! - `bidi`: reordering, right-to-left lines and split boxes;
//! - `justify`: justification, `line-padding` and hanging marks;
//! - `block`: the block's end, baselines, shifts and `text-box-trim`;
//! - `cut`: hyphens, ellipses and clamps;
//! - `costs`: timings and step counts.
//!
//! Every layout goes through [`Fixture::lay_out`], which runs [`check`] on it.

use crate::config::RubyBreakWithin;
use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::build::BoxSize;
use crate::config::{EmphasisRoom, Pretty};
use crate::paint::Decorates;
use crate::stages::LineStages;
use crate::stages::analysis::{ClusterClass, ClusterId, ParagraphFlags};
use crate::stages::content::{ContentFlags, ItemFlags, NodeKind};
use crate::stages::fonts::Generated;
use crate::stages::lines::{
    self, Area, EdgeShape, Exclusions, InlineExtents, LineFlags, Lines, NoExclusions,
};
use crate::stages::shape::{ClusterGlyphs, ShapeSession};
use crate::style::{
    BaseDirection, BidiGroup, BoxDecorationBreak, ComputedStyle, Direction, EdgesGroup,
    FirstLineVariant, FontFamilyName, FontGroup, LengthPercentage, LineGroup, LineHeight,
    RubyAlign, RubyPosition, Sides, TabSize, TextAlign, TextAlignLast, TextBoxTrim, TextGroup,
    TextIndent, TextJustify, TextWrapMode, UnicodeBidi, VerticalAlign, WhiteSpaceCollapse,
    WordBreak,
};
use crate::tests::{
    ARABIC, BEH, Fixture, LATIN, MEEM, NARROW, SEEN, StageCheck, TEH, TestFont, ahem,
    ahem_fallback, along, arabic, latin, px, sized, tabbed, texts, vertically,
};
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::{BuildOptions, ComputedBlockStyle, Context, Layout, LayoutBuilder, NodeKey};
// Read only by the step-count tests, which run in debug builds.
use crate::data::IdRange;
#[cfg(debug_assertions)]
use crate::work;

// Fonts --------------------------------------------------------------------

/// A font with ASCII a third of an em wide, `i` a tenth, and zero-width combining marks.
///
/// Its running sums fall between layout's 1/64 grid at 16 px.
fn narrow() -> TestFont {
    let mut font = TestFont::new("Test Narrow", &[(0x20, 0x7E), (0x300, 0x36F)]);
    font.advances = (0x20u8..=0x7E).map(|b| (char::from(b), 333)).collect();
    font.advances.push(('i', 100));
    font.advances
        .extend((0x300..=0x36F).filter_map(char::from_u32).map(|ch| (ch, 0)));
    font
}

/// A font with ASCII and zero-width combining marks, some ligated and some raised.
///
/// `e` and a cedilla form one ligature glyph, and the acute is drawn raised.
/// So a mark is drawn either by its own offset glyph or by its base's.
fn marks() -> TestFont {
    let mut font = TestFont::new("Test Marks", &[(0x20, 0x7E), (0x300, 0x36F)]);
    font.advances = (0x300..=0x36F)
        .filter_map(char::from_u32)
        .map(|ch| (ch, 0))
        .collect();
    font.ligatures = vec![vec!['e', '\u{327}']];
    font.raised = vec![('\u{301}', 250)];
    font
}

const MARKS: [FontFamilyName<'static>; 1] = [FontFamilyName::Named(Cow::Borrowed("Test Marks"))];

// Fixtures -----------------------------------------------------------------

/// Returns a fixture over Ahem and the fonts above, with Ahem first in every fallback chain.
///
/// It runs [`check`] on every layout it lays out.
fn fixture() -> Fixture {
    Fixture::new(
        &[latin(), narrow(), marks(), arabic()],
        ahem_fallback(),
        StageCheck::Placed(check),
    )
}

/// Asserts the invariants of line layout's items and the block result.
///
/// - Each line has a `Line` item heading its items, and the lines follow one another.
/// - Boxes head the items inside them and end inside their own parent.
/// - Leaves hold their line's clusters in order, apart, covering its content.
/// - Only leaves past the content end are flagged as hanging.
/// - Every text item's glyphs are readable and lie in its line.
/// - The block's end and baselines come from the lines.
fn check(layout: &Layout) {
    let fragments = layout.fragments();
    let lines = layout.line_records();
    let block = layout.line_records().block;
    let input = layout.read_input();
    let items = fragments.items.as_slice();
    assert_eq!(fragments.line_heads.len(), lines.lines.len());
    // The justified lines' amounts are sorted by line and never negative.
    // The leftover goes to a content cluster of the line that has an
    // opportunity.
    let justify = fragments.justified();
    assert!(justify.windows(2).all(|pair| pair[0].line < pair[1].line));
    for &JustifiedLine {
        line,
        justification: justify,
        content_end,
    } in justify
    {
        let record = lines.lines.get(line).expect("a line's");
        assert!(justify.per >= InlineLayoutUnit::ZERO, "{line:?}");
        assert!(justify.leftover() >= InlineLayoutUnit::ZERO, "{line:?}");
        let opportunities = crate::stages::measure::JustifyOpportunities::from_line(
            &layout.stages().variant(FirstLineVariant::Standard),
            record.clusters().start..content_end,
            None,
        )
        .expect("a justified line has opportunities");
        assert!(
            (record.clusters().start..content_end).contains(&justify.last)
                && opportunities.count(justify.last) > 0,
            "{line:?}: {justify:?}"
        );
    }
    // Nothing is trimmed unless the block asks for it and has a line.
    if layout.content().block.text_box_trim == TextBoxTrim::None || lines.lines.is_empty() {
        assert_eq!(
            (block.trim_start, block.trim_end),
            (LayoutUnit::ZERO, LayoutUnit::ZERO)
        );
    }
    if let Some(last) = lines.lines.as_slice().last() {
        let top = fragments.items[fragments.line_heads[LineId::new(lines.lines.len() - 1)]].block;
        assert_eq!(
            block.block_end,
            top + last.extent.box_height() - block.trim_end
        );
    }
    // Where nothing is shifted, every item stands on its line's baseline.
    // An initial letter is excluded, since its measure places it.
    let flags = layout.content().flags;
    let shifted = flags.contains(ContentFlags::VERTICAL_ALIGN)
        || flags.contains(ContentFlags::INITIAL_LETTER)
        || !layout
            .measured()
            .text(FirstLineVariant::Standard)
            .shifts()
            .is_empty();
    let records = lines.lines.as_slice();
    // Which clusters a ruby annotation holds.
    let mut annotated = vec![false; layout.analysis().clusters.len()];
    let item_clusters = &layout.analysis().item_clusters;
    for (id, item) in layout.content().items.iter() {
        if item.flags.contains(ItemFlags::ANNOTATION) {
            let clusters = item_clusters.range(id);
            let (from, to) = (clusters.start.get(), clusters.end.get());
            for slot in annotated.get_mut(from..to).unwrap_or_default() {
                *slot = true;
            }
        }
    }
    assert_eq!(
        block.first_baseline,
        records.first().map(
            |line| fragments.items[fragments.line_heads[LineId::new(0)]].block + line.ascent()
        )
    );
    assert_eq!(
        block.last_baseline,
        records.last().map(|line| fragments.items
            [fragments.line_heads[LineId::new(records.len() - 1)]]
        .block
            + line.ascent())
    );
    let mut at = 0;
    for (id, line) in lines.lines.iter() {
        let head = fragments.line_heads[id];
        assert_eq!(head.get(), at, "{id:?} follows the line before");
        let item = items[at];
        assert_eq!(item.kind(), FragmentItemKind::Line);
        assert_eq!(item.level, line.level(&layout.stages().analysis.paragraphs));
        if id == LineId::new(0) {
            assert_eq!(block.first_baseline, Some(item.block + line.ascent()));
        }
        if id.get() + 1 == lines.lines.len() {
            assert_eq!(block.last_baseline, Some(item.block + line.ascent()));
        }
        let own = fragments.line_items(id);
        assert_eq!(own.len(), item.descendants());
        if !shifted {
            for fragment in own {
                assert_eq!(fragment.block, line.ascent(), "{id:?}: {fragment:?}");
            }
        }
        // Pre-order: a box heads the items inside it and ends inside its
        // parent.
        let mut boxes: Vec<(usize, FragmentItem)> = Vec::new();
        let slack = InlineLayoutUnit::from_layout(LayoutUnit::EPSILON);
        for (k, fragment) in own.iter().enumerate() {
            while boxes.last().is_some_and(|&(end, _)| end <= k) {
                boxes.pop();
            }
            let end = k + 1 + fragment.descendants();
            assert!(end <= own.len(), "{id:?}: item {k} reaches past its line");
            if let Some(&(outer, parent)) = boxes.last() {
                assert!(end <= outer, "{id:?}: item {k} ends outside its box");
                let (left, right) = (
                    parent.inline,
                    parent.inline + InlineLayoutUnit::from_layout(parent.size),
                );
                let reach = fragment.inline + InlineLayoutUnit::from_layout(fragment.size);
                // A box covers what hangs inside it too. A ruby container's
                // box is its columns' room, which its base may reach past,
                // as in Chrome.
                let ruby = layout.content().nodes.kind(parent.node) == Some(NodeKind::Ruby);
                assert!(
                    ruby || (fragment.inline + slack >= left && reach <= right + slack),
                    "{id:?}: item {k} lies outside its box"
                );
            }
            match fragment.kind() {
                FragmentItemKind::Box => boxes.push((end, *fragment)),
                FragmentItemKind::Text | FragmentItemKind::Atomic | FragmentItemKind::Generated => {
                }
                kind => panic!("{id:?}: no {kind:?} item among a line's own"),
            }
        }
        // A hyphenated line has one hyphen, and a line cut for an ellipsis at
        // most one ellipsis, owned by the block. Each is as wide as its shaped
        // text. Only a line cut for an ellipsis hides items.
        let generated = layout.measured().generated();
        let (mut hyphens, mut ellipses) = (0, 0);
        for fragment in own {
            if fragment.flags.contains(FragmentItemFlags::HIDDEN) {
                assert!(
                    line.flags.contains(LineFlags::ELLIPSIS),
                    "{id:?}: {fragment:?}"
                );
            }
            let Some(text) = fragment.generated() else {
                continue;
            };
            let shaped = generated
                .get(text)
                .expect("generated text the measure stage has");
            assert_eq!(fragment.advance(), shaped.advance);
            match shaped.kind {
                Generated::Hyphen => hyphens += 1,
                Generated::Ellipsis => {
                    ellipses += 1;
                    assert_eq!(fragment.node, NodeId::BLOCK, "{id:?}");
                }
            }
        }
        assert_eq!(
            hyphens,
            usize::from(line.flags.contains(LineFlags::HYPHENATED)),
            "{id:?}"
        );
        assert!(
            ellipses <= usize::from(line.flags.contains(LineFlags::ELLIPSIS)),
            "{id:?}"
        );
        // The box items are linked in paint order from the line's item.
        // Boxes go in tree order, each box's items left to right, each once.
        let mut painted: Vec<(NodeId, usize)> = own
            .iter()
            .enumerate()
            .filter(|(_, f)| f.kind() == FragmentItemKind::Box)
            .map(|(k, f)| (f.node, k))
            .collect();
        painted.sort();
        let mut linked = Vec::new();
        // A link is the next box item's id, which maps to its place among
        // the line's own items.
        let place = |next: FragmentItemId| next.get().checked_sub(head.get() + 1);
        let mut link = item.painted_next().and_then(place);
        while let Some(k) = link {
            assert!(linked.len() < painted.len(), "{id:?}: the links end");
            linked.push((own[k].node, k));
            link = own[k].painted_next().and_then(place);
        }
        assert_eq!(linked, painted, "{id:?}: the boxes in paint order");
        // The leaves hold the line's clusters, apart and in order. They cover
        // all of its content, and past it only preserved white space.
        let mut leaves: Vec<FragmentItem> = own
            .iter()
            .filter(|f| matches!(f.kind(), FragmentItemKind::Text | FragmentItemKind::Atomic))
            .copied()
            .collect();
        leaves.sort_by_key(|f| f.clusters().start);
        let range = line.clusters();
        let mut next = range.start;
        let clusters = &layout.analysis().clusters;
        // A ruby annotation's clusters belong to its annotation line.
        let skip = |next: &mut ClusterId, to: ClusterId| {
            while *next < to && annotated.get(next.get()).copied().unwrap_or(false) {
                *next = ClusterId::new(next.get() + 1);
            }
        };
        for leaf in &leaves {
            let held = leaf.clusters();
            skip(&mut next, held.start);
            assert!(held.start < held.end, "{id:?}: a leaf holds a cluster");
            assert!(
                held.start >= next && held.end <= range.end,
                "{id:?}: {held:?}"
            );
            if held.start < line.content_end(&layout.analysis().clusters) {
                assert_eq!(held.start, next, "{id:?}: the content is covered");
                assert!(!leaf.flags.contains(FragmentItemFlags::HANGS));
            } else {
                assert!(leaf.flags.contains(FragmentItemFlags::HANGS), "{id:?}");
            }
            next = held.end;
        }
        skip(&mut next, range.end);
        assert!(
            next >= line.content_end(&layout.analysis().clusters),
            "{id:?}: the content is covered"
        );
        for cluster in (next..range.end).ids() {
            if annotated.get(cluster.get()).copied().unwrap_or(false) {
                continue;
            }
            let class = clusters.attrs(cluster).map(|attrs| attrs.class());
            assert!(
                matches!(
                    class,
                    Some(
                        ClusterClass::Separator
                            | ClusterClass::Space
                            | ClusterClass::BreakOpportunity
                    )
                ),
                "{id:?}: only a separator, a removed space or a generated opportunity is left out"
            );
        }
        // Every glyph can be read. A text item's walked clusters add up to
        // its stored advance, a justified line's room included.
        for fragment in own.iter().filter(|f| f.kind() == FragmentItemKind::Text) {
            for glyph in GlyphWalk::new(&input, id, line, fragment) {
                assert!(fragment.clusters().contains(&glyph.cluster));
            }
            let walk = ClusterWalk::new(&input, id, line, fragment);
            if walk.len() > 0 {
                let walked = walk.fold(InlineLayoutUnit::ZERO, |sum, (_, _, step)| sum + step);
                assert_eq!(walked, fragment.advance(), "{id:?}: {fragment:?}");
            }
        }
        // The line's annotation lines follow its own items. Each heads text
        // items of annotation clusters only, which lie over its column and
        // add up as the line's own do.
        let annotations = fragments.line_annotations(id);
        let mut k = 0;
        while let Some(head) = annotations.get(k) {
            assert_eq!(
                head.kind(),
                FragmentItemKind::AnnotationLine,
                "{id:?}: {head:?}"
            );
            assert!(head.ruby_level().is_some());
            let text = annotations
                .get(k + 1..k + 1 + head.descendants())
                .expect("an annotation line's text is after it");
            let (left, right) = (
                head.inline,
                head.inline + InlineLayoutUnit::from_layout(head.size),
            );
            for fragment in text {
                assert_eq!(
                    fragment.kind(),
                    FragmentItemKind::Text,
                    "{id:?}: {fragment:?}"
                );
                assert!(fragment.flags.contains(FragmentItemFlags::ANNOTATION));
                for cluster in fragment.clusters().ids() {
                    assert!(
                        annotated[cluster.get()],
                        "{id:?}: {cluster:?} is annotation text"
                    );
                }
                let slack = InlineLayoutUnit::from_layout(LayoutUnit::EPSILON);
                let reach = fragment.inline + InlineLayoutUnit::from_layout(fragment.size);
                assert!(
                    fragment.inline + slack >= left && reach <= right + slack,
                    "{id:?}: {fragment:?} lies over its column"
                );
                for glyph in GlyphWalk::new(&input, id, line, fragment) {
                    assert!(fragment.clusters().contains(&glyph.cluster));
                }
                let walk = ClusterWalk::new(&input, id, line, fragment);
                if walk.len() > 0 {
                    let walked = walk.fold(InlineLayoutUnit::ZERO, |sum, (_, _, step)| sum + step);
                    assert_eq!(walked, fragment.advance(), "{id:?}: {fragment:?}");
                }
            }
            k += 1 + head.descendants();
        }
        at += 1 + own.len() + annotations.len();
    }
    assert_eq!(at, items.len(), "every item is a line's");
}

/// Returns a block style aligned `align`, with its last line aligned `last`.
fn aligned(align: TextAlign, last: TextAlignLast) -> ComputedBlockStyle<'static> {
    ComputedBlockStyle {
        text_align: align,
        text_align_last: last,
        ..ComputedBlockStyle::default()
    }
}

/// Returns each line box's left edge, in pixels.
fn lefts(layout: &Layout) -> Vec<f32> {
    let fragments = layout.fragments();
    fragments
        .line_heads
        .as_slice()
        .iter()
        .map(|&head| fragments.items[head].inline.to_px())
        .collect()
}

/// Returns line `n`'s items.
fn items(layout: &Layout, n: usize) -> &[FragmentItem] {
    layout.fragments().line_items(LineId::new(n))
}

/// Returns line `n`'s glyphs in paint order as `(glyph id, x)`.
///
/// `x` is exact, from the line box's left.
fn glyphs(layout: &Layout, n: usize) -> Vec<(u32, InlineLayoutUnit)> {
    let input = layout.read_input();
    let id = LineId::new(n);
    let line = &layout.line_records().lines[id];
    items(layout, n)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Text)
        .flat_map(|item| GlyphWalk::new(&input, id, line, item).map(|glyph| (glyph.id, glyph.x)))
        .collect()
}

/// Returns each glyph's x on line `n`, in pixels.
fn xs(layout: &Layout, n: usize) -> Vec<f32> {
    glyphs(layout, n).iter().map(|&(_, x)| x.to_px()).collect()
}

/// Returns how far line `n`'s content reaches from its line box's left, exactly.
///
/// The reach is the farthest end of its leaves that do not hang.
fn reach(layout: &Layout, n: usize) -> InlineLayoutUnit {
    items(layout, n)
        .iter()
        .filter(|item| {
            matches!(
                item.kind(),
                FragmentItemKind::Text | FragmentItemKind::Atomic
            ) && !item.flags.contains(FragmentItemFlags::HANGS)
        })
        .map(|item| item.inline + item.advance())
        .max()
        .unwrap_or_default()
}

/// Returns each item of line `n` as `(inline, size, flags)`, in pixels.
fn placed(layout: &Layout, n: usize) -> Vec<(f32, f32, FragmentItemFlags)> {
    items(layout, n)
        .iter()
        .map(|item| (item.inline.to_px(), item.size.to_px(), item.flags))
        .collect()
}

/// Returns each visible cluster of line `n` as its text and where its first
/// glyph stands, in pixels, left to right.
///
/// A cluster's own extent can take in room `ruby-align` spreads beside it;
/// its glyph stands where the text is set.
fn places(layout: &Layout, n: usize) -> Vec<(String, f32)> {
    let text = &layout.content().text;
    let mut found: Vec<(String, f32)> = layout
        .line(n)
        .into_iter()
        .flat_map(|line| line.items())
        .filter_map(|item| match item {
            crate::Item::Text(run) => Some(run),
            _ => None,
        })
        .flat_map(|run| {
            let glyphs: Vec<_> = run.glyphs().collect();
            run.clusters()
                .map(|cluster| {
                    let range = cluster.text_range();
                    let x = glyphs
                        .iter()
                        .find(|glyph| glyph.text_offset == range.start)
                        .map_or(cluster.inline().left, |glyph| glyph.x);
                    (range, x)
                })
                .collect::<Vec<_>>()
        })
        .filter(|(range, _)| !text[range.clone()].trim().is_empty())
        .map(|(range, x)| (String::from(&text[range]), x))
        .collect();
    found.sort_by(|a, b| a.1.total_cmp(&b.1));
    found
}

/// Returns line `n`'s box items as node, left and right in pixels, and open sides.
fn box_parts(layout: &Layout, n: usize) -> Vec<(usize, f32, f32, bool, bool)> {
    items(layout, n)
        .iter()
        .filter(|item| item.kind() == FragmentItemKind::Box)
        .map(|item| {
            let left = item.inline;
            let right = left + InlineLayoutUnit::from_layout(item.size);
            (
                item.node.get(),
                left.to_px(),
                right.to_px(),
                item.flags.contains(FragmentItemFlags::OPEN_LEFT),
                item.flags.contains(FragmentItemFlags::OPEN_RIGHT),
            )
        })
        .collect()
}

/// Returns line `n`'s box items as `(left, right)` from the area's line-left.
fn box_spans(layout: &Layout, n: usize) -> Vec<(f32, f32)> {
    let left = lefts(layout)[n];
    box_parts(layout, n)
        .into_iter()
        .map(|(_, from, to, _, _)| (left + from, left + to))
        .collect()
}

/// Returns `style` reading in `direction` under `unicode_bidi`.
fn reading<'a>(
    style: &ComputedStyle<'a>,
    direction: Direction,
    unicode_bidi: UnicodeBidi,
) -> ComputedStyle<'a> {
    ComputedStyle {
        bidi: BidiGroup {
            direction,
            unicode_bidi,
        },
        ..*style
    }
}

/// Line placement facts live in the context's scratch, not in a layout.
///
/// They stay correct when one context lays out several documents. Clamping and
/// pretty's rewinds keep each fact with its chosen row. Justified readers keep
/// their own content end after the scratch is reused.
#[test]
fn line_placement_survives_rewinds_and_lives_in_the_context() {
    use crate::style::{LineClamp, TextWrapStyle};
    let mut fixture = fixture();
    let style = ahem(10.0);
    let block = ComputedBlockStyle {
        style: &style,
        text_align: TextAlign::Justify,
        text_align_last: TextAlignLast::Justify,
        text_box_trim: TextBoxTrim::TrimBoth,
        text_wrap_style: TextWrapStyle::Pretty,
        line_clamp: LineClamp::Auto,
        ..ComputedBlockStyle::new(&style)
    };
    let text = "XX XX XX XX XX XX XX XX XX XX XX XX";
    let mut first = Layout::new();
    fixture.block_text(&mut first, &block, text);
    let area = Area {
        block_start: 7.0,
        block_end: Some(37.0),
        ..Area::new(85.0)
    };
    first.break_lines(&mut fixture.cx, area, &mut NoExclusions);
    check(&first);
    assert!(fixture.cx.placing().0.is_empty());
    let before: Vec<_> = (0..first.lines().len())
        .map(|n| glyphs(&first, n))
        .collect();
    let metrics = first.metrics();
    let mut second = Layout::new();
    fixture.text(&mut second, &style, "X");
    fixture.lay_out(&mut second, 100.0);
    assert!(fixture.cx.placing().0.is_empty());
    assert_eq!(first.metrics(), metrics);
    assert_eq!(
        (0..first.lines().len())
            .map(|n| glyphs(&first, n))
            .collect::<Vec<_>>(),
        before
    );
    for width in [45.0, 130.0, 85.0] {
        let area = Area {
            inline: InlineExtents {
                left: 0.0,
                right: width,
            },
            ..area
        };
        first.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        check(&first);
        let mut fresh = Layout::new();
        fixture.block_text(&mut fresh, &block, text);
        fresh.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        assert_eq!(first.metrics(), fresh.metrics());
        assert_eq!(
            first.fragments().items.as_slice(),
            fresh.fragments().items.as_slice()
        );
        assert_eq!(first.fragments().justified(), fresh.fragments().justified());
        assert!(fixture.cx.placing().0.is_empty());
    }
}

// The records --------------------------------------------------------------

/// Line layout's records keep their sizes: a fragment item is 40 bytes, and a line head an item id.
#[test]
fn records_keep_their_sizes() {
    assert_eq!(size_of::<FragmentItem>(), 40);
    assert_eq!(size_of::<FragmentItemId>(), 4);
    assert_eq!(size_of::<FragmentItemFlags>(), 1);
    assert_eq!(size_of::<LineJustification>(), 32);
    assert_eq!(size_of::<JustifiedLine>(), 40);
}

// Relayout -----------------------------------------------------------------

/// A relayout at another width equals a fresh layout at that width, item for item.
///
/// The text mixes two fonts, kept and culled boxes, reshaped edges, ruby
/// with annotation lines, emphasis marks and alignment. A rebuild leaves no
/// items from the previous content.
#[test]
fn a_relayout_equals_a_fresh_layout() {
    let mut fixture = fixture();
    let block = aligned(TextAlign::Center, TextAlignLast::End);
    let mut root = sized(&LATIN, 20.0);
    root.text.word_break = WordBreak::BreakAll;
    let larger = ComputedStyle {
        paints: true,
        ..sized(&LATIN, 24.0)
    };
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(2.5),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let small = sized(&LATIN, 10.0);
    let mut marked = root;
    marked.text.emphasis.marks = true;
    let build = |fixture: &mut Fixture, layout: &mut Layout| {
        fixture.build(
            layout,
            &ComputedBlockStyle {
                style: &root,
                ..block
            },
            |b| {
                b.text(NodeKey(1), "office AVAVA ");
                b.open_box(NodeKey(2), &larger, None);
                b.text(NodeKey(3), "flat fit ");
                b.open_box(NodeKey(4), &padded, None);
                b.text(NodeKey(5), "ffi AVAIL");
                b.close_box();
                b.close_box();
                b.line_break(NodeKey(6));
                b.text(NodeKey(7), "waffle ");
                b.open_ruby(NodeKey(8), &root, None);
                b.open_box(NodeKey(9), &marked, None);
                b.text(NodeKey(10), "fifi");
                b.close_box();
                b.open_annotation(NodeKey(11), &small, None);
                b.text(NodeKey(12), "AVAVA office");
                b.close_annotation();
                b.text(NodeKey(13), "fl");
                b.open_annotation(NodeKey(14), &small, None);
                b.text(NodeKey(15), "x");
                b.close_ruby();
                b.text(NodeKey(16), " end");
            },
        );
    };
    let mut layout = Layout::new();
    build(&mut fixture, &mut layout);
    for width in [12.0, 26.0, 28.0, 57.5, 90.0, 400.0, 3.0, 26.0] {
        fixture.lay_out(&mut layout, width);
        let mut fresh = Layout::new();
        build(&mut fixture, &mut fresh);
        fixture.lay_out(&mut fresh, width);
        let (a, b) = (layout.fragments(), fresh.fragments());
        assert_eq!(a.items.as_slice(), b.items.as_slice(), "at {width}");
        assert_eq!(a.line_heads.as_slice(), b.line_heads.as_slice());
        assert_eq!(layout.line_records().block, fresh.line_records().block);
    }
    fixture.block_text(
        &mut layout,
        &ComputedBlockStyle {
            style: &root,
            ..block
        },
        "x",
    );
    assert!(
        layout.fragments().items.is_empty(),
        "a rebuild clears the items"
    );
    assert_eq!(layout.line_records().block, BlockResult::EMPTY);
}

// What must not panic ------------------------------------------------------

/// Any area gives valid items, under every alignment and in either direction.
///
/// - The widths are zero, negative, NaN, infinite and narrower than a cluster.
/// - The text has boxes, reshaped edges and preserved white space, or none.
/// - Ruby is nested, malformed, under, spread every way, emphasized and
///   right to left.
///
/// A layout never built has no items.
#[test]
fn any_area_gives_valid_items() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.lay_out(&mut layout, 100.0);
    assert!(layout.fragments().items.is_empty());
    let mut pre = ahem(20.0);
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let mut anywhere = sized(&LATIN, 20.0);
    anywhere.text.word_break = WordBreak::BreakAll;
    let roomy = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(-3.0),
            padding: Sides::from_px(5.0),
            ..EdgesGroup::INITIAL
        },
        ..anywhere
    };
    let small = sized(&LATIN, 9.0);
    let mut marked = anywhere;
    marked.text.emphasis.marks = true;
    let arabic = sized(&ARABIC, 20.0);
    let aligns = [
        (RubyAlign::SpaceAround, RubyPosition::Over),
        (RubyAlign::SpaceBetween, RubyPosition::Under),
        (RubyAlign::Center, RubyPosition::Alternate),
        (RubyAlign::Start, RubyPosition::Over),
    ];
    let widths = [
        0.0,
        -10.0,
        f32::NAN,
        f32::INFINITY,
        f32::MAX,
        0.001,
        3.0,
        45.0,
    ];
    for direction in [BaseDirection::Ltr, BaseDirection::Rtl] {
        for align in [
            TextAlign::Start,
            TextAlign::Center,
            TextAlign::Right,
            TextAlign::Justify,
        ] {
            let block = ComputedBlockStyle {
                direction,
                text_align: align,
                ..ComputedBlockStyle::default()
            };
            let builds: [&dyn Fn(&mut LayoutBuilder<'_>); 7] = [
                &|b| b.text(NodeKey(1), "  XX   XX  \t \n X"),
                &|b| {
                    b.text(NodeKey(1), "office ");
                    b.open_box(NodeKey(2), &roomy, None);
                    b.text(NodeKey(3), "AVAVA ");
                    b.close_box();
                    b.open_box(NodeKey(4), &roomy, None);
                    b.close_box();
                    b.text(NodeKey(5), "waffle");
                },
                &|b| b.text(NodeKey(1), ""),
                &|b| {
                    b.line_break(NodeKey(1));
                    b.line_break(NodeKey(2));
                },
                &|b| {
                    b.text(NodeKey(1), "XX ");
                    b.open_ruby(NodeKey(2), &roomy, None);
                    b.text(NodeKey(3), "YY");
                    b.open_annotation(NodeKey(4), &roomy, None);
                    b.text(NodeKey(5), "zz");
                    b.close_annotation();
                    b.close_ruby();
                    b.text(NodeKey(6), " WW");
                },
                &|b| {
                    // This covers every way to spread, both sides, a level
                    // on a level, a ruby in a base, marks, a break, empty
                    // parts, and an annotation with no ruby.
                    b.open_annotation(NodeKey(1), &small, None);
                    b.text(NodeKey(2), "a b ");
                    b.close_annotation();
                    for (n, &(align, position)) in aligns.iter().enumerate() {
                        let mut ruby = anywhere;
                        ruby.ruby.align = align;
                        ruby.ruby.position = position;
                        let key = 20 * (n as u64 + 1);
                        b.open_ruby(NodeKey(key), &ruby, None);
                        b.open_box(NodeKey(key + 1), &marked, None);
                        b.text(NodeKey(key + 2), "A B");
                        b.line_break(NodeKey(key + 3));
                        b.close_box();
                        b.open_annotation(NodeKey(key + 4), &small, None);
                        b.text(NodeKey(key + 5), "xx yy zz");
                        b.close_annotation();
                        b.open_annotation(NodeKey(key + 6), &small, None);
                        b.close_annotation();
                        b.open_ruby(NodeKey(key + 7), &ruby, None);
                        b.open_annotation(NodeKey(key + 8), &marked, None);
                        b.text(NodeKey(key + 9), "q");
                        b.close_ruby();
                        b.text(NodeKey(key + 10), " C ");
                    }
                    b.close_ruby();
                    b.text(NodeKey(99), "D");
                },
                &|b| {
                    // Right-to-left bases and annotations.
                    b.open_ruby(NodeKey(1), &arabic, None);
                    b.text(NodeKey(2), "\u{628}\u{62A}");
                    b.open_annotation(NodeKey(3), &arabic, None);
                    b.text(NodeKey(4), "\u{633}\u{645} \u{628}\u{62A}\u{633}");
                    b.close_annotation();
                    b.text(NodeKey(5), "\u{645}");
                    b.open_annotation(NodeKey(6), &small, None);
                    b.text(NodeKey(7), "ab");
                    b.close_ruby();
                    b.text(NodeKey(8), " \u{628}");
                },
            ];
            for (n, calls) in builds.iter().enumerate() {
                let style = if n == 0 { &pre } else { &anywhere };
                fixture.build(&mut layout, &ComputedBlockStyle { style, ..block }, |b| {
                    calls(b)
                });
                for width in widths {
                    fixture.lay_out(&mut layout, width);
                }
            }
        }
    }
}

mod align;
mod bidi;
mod block;
mod boxes;
mod costs;
mod cut;
mod glyphs;
mod justify;
