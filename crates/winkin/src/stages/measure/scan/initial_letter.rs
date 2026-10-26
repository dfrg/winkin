//! The block's initial letter, measured before the scan charges its edges.

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

/// Measures the block's initial letter in `scan`'s variant, as Blink's
/// `LayoutInitialLetterBox` and `initial_letter_utils` lay it out.
///
/// `advances` are the shaped advances, `strut` the line's strut. Returns
/// `None` where the block has no initial letter.
///
/// - **Along the line:** its content is its glyphs' ink
///   (`ComputeTextInkBounds`, `CalculateInitialLetterBoxInlineSize`), each
///   taken in whole pixels where the pen will draw it, then rounded out to
///   the grid. A tab takes its advance to its stop instead. The ink starts
///   where the content does. A box with no border or padding kerns its
///   margin by a negative side bearing (`ComputeNegativeSideBearings`).
/// - **Across the lines** (`ComputeInitialLetterBoxBlockOffset`): where it
///   sinks no further than it is tall, its baseline is the `size`-th
///   line's, `size` line heights below the first line's top less the
///   strut's descent; otherwise its bottom is on the `sink`-th line's. A
///   raised letter moves the first line down. Its exclusion is its margin
///   box, from the first line's top or above.
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
    // Step each cluster as the scan will, taking its glyphs' ink at the pen.
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
        // A tab takes its advance whole, a control item counting into the box's
        // size (Blink's `CalculateInitialLetterBoxInlineSize`). The prefix holds
        // none of it: line layout sizes it.
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
    // Round the ink out onto the grid (`LogicalRect::EnclosingRect`); empty
    // where the letter draws nothing.
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
    // The box is bare where it has no border or padding.
    let zero = (LayoutUnit::ZERO, LayoutUnit::ZERO);
    let bare = boxes.room == boxes.margin_line && boxes.across == zero;
    // The letter's advance outside its ink at each end, in the box's direction.
    let outside_right = pen - InlineLayoutUnit::from_layout(right);
    let (outside_start, outside_end) =
        direction.line_order(InlineLayoutUnit::from_layout(left), outside_right);
    // Inline kerning: in a horizontal line, a bare box's negative side bearing
    // kerns its start.
    let kern = if bare && writing_mode == WritingMode::HorizontalTb {
        outside_start.to_layout().min(LayoutUnit::ZERO)
    } else {
        LayoutUnit::ZERO
    };
    let start_margin = margin_start + kern;
    let start_room = InlineLayoutUnit::from_layout(room_start + kern) - outside_start;
    let end_room = InlineLayoutUnit::from_layout(room_end) - outside_end;
    let exclusion_width = (room_start + kern + width + room_end).max(LayoutUnit::ZERO);
    // Across the lines, from the block-start side, swapped where lines flip.
    let (margin_before, margin_after) = if writing_mode.flips_lines() {
        (boxes.margin_across.1, boxes.margin_across.0)
    } else {
        boxes.margin_across
    };
    let above = boxes.across.0 + over;
    let below = under + boxes.across.1;
    let block = above + below;
    let line = strut.box_height();
    let size = unit::ceil_count(letter.size);
    let raise = line.times_whole(size.saturating_sub(letter.sink));
    let border_top = if size >= letter.sink {
        // Sit on the `size`-th line's baseline. For `ideographic`, the em box's
        // bottom sits there.
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
        baseline - above + margin_before
    } else {
        line.times_whole(letter.sink) - block + margin_before
    };
    let exclusion_top = (border_top - margin_before).min(LayoutUnit::ZERO);
    let exclusion_bottom = border_top + block + margin_after;
    // The ink is measured about the alphabetic baseline, which a centred
    // vertical line sets under its central one.
    let alphabetic = input
        .primary(text)
        .map_or(LayoutUnit::ZERO, |metrics| metrics.alphabetic);
    Some(InitialLetter {
        node,
        start_room,
        end_room,
        start_margin,
        end_margin: margin_end,
        extent: Extent::new(above - alphabetic, below + alphabetic),
        baseline: border_top + above - alphabetic,
        raise,
        exclusion_top,
        exclusion_height: (exclusion_bottom - exclusion_top).max(LayoutUnit::ZERO),
        exclusion_width,
    })
}
