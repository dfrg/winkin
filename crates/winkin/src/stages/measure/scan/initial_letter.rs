//! The block's initial letter, measured before the scan charges its box's
//! edges.

use super::{Scan, ScanWalk};
use crate::data::{Id, IdRange};
use crate::font::FontMetricsProvider;
use crate::stages::analysis::{ClusterAttrs, ClusterClass};
use crate::stages::content::{ItemId, NodeId};
use crate::stages::measure::edges::logical_edges;
use crate::stages::measure::tabs::tab_advance;
use crate::stages::measure::{Extent, InitialLetter};
use crate::stages::shape::ClusterGlyphs;
use crate::style::{InitialLetterAlign, WritingMode};
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::{unit, work};

/// Measures the block's initial letter in `scan`'s variant, as Chrome's
/// `LayoutInitialLetterBox` and `initial_letter_utils` lay it out.
///
/// `advances` holds each cluster's shaped advance, and `strut` is the
/// line's strut. Returns `None` where the block has no initial letter.
///
/// The box is the one the builder kept `initial-letter` on (`BlockFacts`).
/// It opens at the text's start. Its text is set in the size font selection
/// gave its font request.
///
/// - **Along the line:** its content is the ink of its glyphs
///   (`ComputeTextInkBounds`, `CalculateInitialLetterBoxInlineSize`). Each
///   glyph's ink is taken in whole pixels where the scan's pen will draw it,
///   then rounded out onto the grid. A look-ahead with a copy of the walk
///   steps each cluster as the scan will, in its segment's font. A tab in
///   it takes its advance to its stop instead of ink. The text is
///   set so its ink starts where the content does. Where the box has no
///   border or padding, a negative side bearing kerns its margin ("inline
///   kerning", `ComputeNegativeSideBearings`).
/// - **Across the lines** (`ComputeInitialLetterBoxBlockOffset`): where it
///   sinks no further than it is tall, its baseline is on the `size`-th
///   line's baseline: `size` line heights below the first line's top, less
///   the strut's descent. A raised letter moves the first line's text down
///   by the lines it stands above. Where it sinks further, its bottom is on
///   the `sink`-th line's. Its exclusion is its margin box, from the first
///   line's top or above it.
pub(super) fn initial_letter(
    scan: &Scan<'_>,
    provider: Option<&dyn FontMetricsProvider>,
    advances: &[TextUnit],
    strut: Extent,
) -> Option<InitialLetter> {
    let (content, input, variant) = (scan.input.content, scan.input, scan.variant);
    let node = content.block.initial_letter?;
    let (nodes, facts) = (&content.nodes, &content.facts);
    let text = nodes.text_facts(node, variant);
    let letter = facts.request(facts.text_request(text)).initial_letter;
    if !letter.is_set() {
        return None;
    }
    let boxes = facts.box_facts(nodes.box_facts(node, variant));
    let items = nodes.items(node);
    let close = ItemId::new(items.end.get().checked_sub(1)?);
    let item_clusters = &scan.input.analysis.item_clusters;
    let clusters = item_clusters.get(items.start)?..item_clusters.get(close)?;
    // Step each cluster as the scan will, and take the ink of its glyphs
    // where the pen will draw them.
    let mut look = ScanWalk::new(scan);
    let mut pen = InlineLayoutUnit::ZERO;
    // The ink along the line, and across it.
    let mut along: Option<(InlineLayoutUnit, InlineLayoutUnit)> = None;
    let mut across: Option<(LayoutUnit, LayoutUnit)> = None;
    let mut unite_along = |left: InlineLayoutUnit, right: InlineLayoutUnit| {
        along = Some(along.map_or((left, right), |(l, r)| (l.min(left), r.max(right))));
    };
    for at in clusters.ids() {
        work::step();
        look.cross(scan, at, None);
        let class = scan
            .input
            .analysis
            .clusters
            .attrs(at)
            .map_or(ClusterClass::Text, ClusterAttrs::class);
        // A tab takes its advance whole, ink or none, as Chrome counts a
        // control item's inline size into the box's
        // (`CalculateInitialLetterBoxInlineSize`). The prefix holds none of
        // it: line layout sizes it where the line puts it.
        if class == ClusterClass::Tab
            && let Some(stops) = scan.tab_stops(look.text)
        {
            let tab = InlineLayoutUnit::from_layout(tab_advance(LayoutUnit::ZERO, pen, stops));
            unite_along(pen, pen + tab);
            pen += tab;
            continue;
        }
        if let Some(font) = look.segment.and_then(|segment| scan.font(&segment)) {
            let mut unite = |x: InlineLayoutUnit, rise: LayoutUnit, glyph: u32| {
                let Some(glyph) = scan.input.fonts.used.glyph_ink(font, glyph, provider) else {
                    return;
                };
                let left = x + InlineLayoutUnit::from_layout(glyph.left);
                let right = x + InlineLayoutUnit::from_layout(glyph.right);
                unite_along(left, right);
                let (over, under) = (glyph.over + rise, glyph.under - rise);
                across = Some(across.map_or((over, under), |(o, u)| (o.max(over), u.max(under))));
            };
            match scan.shaped.glyphs.glyphs(at) {
                ClusterGlyphs::None => {}
                ClusterGlyphs::One(glyph) => unite(pen, LayoutUnit::ZERO, glyph),
                ClusterGlyphs::Many(glyphs) => {
                    let mut x = pen;
                    for glyph in glyphs {
                        work::step();
                        let (dx, dy) = (glyph.x_offset, glyph.y_offset);
                        let rise = InlineLayoutUnit::from_text(dy).to_layout();
                        unite(x + InlineLayoutUnit::from_text(dx), rise, glyph.id());
                        x += InlineLayoutUnit::from_text(glyph.advance);
                    }
                }
            }
        }
        let advance =
            InlineLayoutUnit::from_text(advances.get(at.get()).copied().unwrap_or_default());
        pen += look.step(scan, class, advance, at);
    }
    // Round the ink out onto the grid (`LogicalRect::EnclosingRect`). It is
    // empty where the letter draws nothing.
    let (left, right) = along.map_or((LayoutUnit::ZERO, LayoutUnit::ZERO), |(left, right)| {
        (-(-left).to_layout(), right.to_layout())
    });
    let (over, under) = across.unwrap_or((LayoutUnit::ZERO, LayoutUnit::ZERO));
    let width = right - left;
    // Along the line, in the box's own direction.
    let writing_mode = scan.writing_mode;
    let direction = boxes.direction();
    let (margin_start, margin_end) = direction.line_order(boxes.margin_line.0, boxes.margin_line.1);
    let (room_start, room_end) = logical_edges(boxes);
    // The box is bare where its border and padding are zero: along the
    // line, where its room is its margin, and across it.
    let zero = (LayoutUnit::ZERO, LayoutUnit::ZERO);
    let bare = boxes.room == boxes.margin_line && boxes.across == zero;
    // The part of the letter's advance outside its ink at its start and its
    // end, with left and right swapped for a right-to-left box.
    let outside_right = pen - InlineLayoutUnit::from_layout(right);
    let (outside_start, outside_end) =
        direction.line_order(InlineLayoutUnit::from_layout(left), outside_right);
    // Inline kerning: in a horizontal line, where nothing lies between the
    // ink and the margin, a negative side bearing kerns the start.
    let kern = if bare && writing_mode == WritingMode::HorizontalTb {
        outside_start.to_layout().min(LayoutUnit::ZERO)
    } else {
        LayoutUnit::ZERO
    };
    let start_margin = margin_start + kern;
    let start_room = InlineLayoutUnit::from_layout(room_start + kern) - outside_start;
    let end_room = InlineLayoutUnit::from_layout(room_end) - outside_end;
    let exclusion_width = (room_start + kern + width + room_end).max(LayoutUnit::ZERO);
    // Across the lines.
    let (margin_over, margin_under) = boxes.margin_across;
    let above = boxes.across.0 + over;
    let below = under + boxes.across.1;
    let block = above + below;
    let line = strut.box_height();
    let size = unit::ceil_count(letter.size);
    let raise = line.times_whole(size.saturating_sub(letter.sink));
    let border_top = if size >= letter.sink {
        // Sit on the `size`-th line's baseline, whatever the lines between
        // hold. For `ideographic`, the em box's bottom sits there.
        let mut baseline = line.times_truncated(letter.size) - strut.descent();
        if letter.align == InitialLetterAlign::Ideographic {
            let em_under = |text| {
                input
                    .primary(text)
                    .map_or(LayoutUnit::ZERO, |metrics| metrics.em_under)
            };
            baseline =
                baseline + em_under(nodes.text_facts(NodeId::BLOCK, variant)) - em_under(text);
        }
        baseline - above + margin_over
    } else {
        line.times_whole(letter.sink) - block + margin_over
    };
    let exclusion_top = (border_top - margin_over).min(LayoutUnit::ZERO);
    let exclusion_bottom = border_top + block + margin_under;
    Some(InitialLetter {
        node,
        start_room,
        end_room,
        start_margin,
        end_margin: margin_end,
        extent: Extent::new(above, below),
        baseline: border_top + above,
        raise,
        exclusion_top,
        exclusion_height: (exclusion_bottom - exclusion_top).max(LayoutUnit::ZERO),
        exclusion_width,
    })
}
