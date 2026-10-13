//! Fixed box shifts and retained box extents.

use super::{Extent, MeasureInput, MeasuredText, TextMetrics};
use crate::config::{DominantBaselines, SuperSubPosition};
use crate::data::{Id, IdRange, Table};
use crate::stages::analysis::ClusterId;
use crate::stages::content::{
    BoxFlags, ContentFlags, ItemFlags, ItemId, ItemKind, NodeId, NodeKind, TextFactsId,
};
use crate::stages::fonts::FontLineMetrics;
use crate::style::{FirstLineVariant, VerticalAlign};
use crate::unit::LayoutUnit;
use crate::work;

/// Prepares fixed shifts and retained box extents in node order. Ruby's
/// look-ahead reads the shifts, so this runs before the cluster scan. The
/// initial letter has already been measured and keeps its ink extent. Nodes
/// opening past `reach`, where the scan stops, are left out.
pub(super) fn prepare_boxes(
    input: &MeasureInput<'_>,
    variant: FirstLineVariant,
    reach: ClusterId,
    metrics: &Table<TextFactsId, TextMetrics>,
    out: &mut MeasuredText,
) {
    let content = input.content;
    let (nodes, facts) = (&content.nodes, &content.facts);
    let clusters = &input.analysis.item_clusters;
    let dominant = input.dominant_baseline == DominantBaselines::Applied
        && content.flags.contains(ContentFlags::DOMINANT_BASELINE);
    let shifting = dominant || content.flags.contains(ContentFlags::VERTICAL_ALIGN);
    let letter = out.initial_letter().copied();
    for node in (NodeId::new(1)..NodeId::new(nodes.len())).ids() {
        work::step();
        let opens = nodes.items(node).start;
        if clusters.get(opens).is_some_and(|first| first > reach) {
            break;
        }
        let Some(item) = content.items.get(opens) else {
            continue;
        };
        let kind = nodes.kind(node);
        let atomic = kind == Some(NodeKind::Atomic);
        let shifts = shifting
            && !item.flags.contains(ItemFlags::ANNOTATION)
            && matches!(
                kind,
                Some(NodeKind::Box | NodeKind::FirstLetter | NodeKind::Atomic)
            );
        // A ruby container always keeps its box, as Chrome keeps one for a
        // box whose annotations set fonts of their own. An anonymous one has
        // no element to answer for.
        let ruby = item.kind == ItemKind::RubyOpen && !nodes.is_anonymous_ruby(node);
        // An annotation keeps its box too: its annotation line, as wide as
        // its column, across which its border box stands on its baseline.
        let annotation = item.kind == ItemKind::AnnotationOpen;
        let keeps = item.kind == ItemKind::Open || ruby || annotation;
        if !shifts && !keeps {
            continue;
        }
        let text = nodes.text_facts(node, variant);
        let parent = nodes.text_facts(nodes.parent(node), variant);
        let boxes = facts.box_facts(nodes.box_facts(node, variant));
        let own = input.primary(text);
        let parent_metrics = input.primary(parent);
        let shift = if shifts {
            baseline_shift(
                input,
                metrics,
                text,
                parent,
                boxes.align,
                atomic,
                dominant,
                own,
                parent_metrics,
            )
        } else {
            LayoutUnit::ZERO
        };
        if shift != LayoutUnit::ZERO {
            out.rare_mut().shifts.push((node, shift));
        }
        if !keeps {
            continue;
        }
        if let Some(letter) = letter
            && letter.node == node
        {
            out.rare_mut().kept_boxes.boxes.push_bounded(
                (node, letter.extent),
                "no more kept boxes than nodes, which a NodeId names",
            );
            continue;
        }
        let same_metrics = match (own, parent_metrics) {
            (Some(own), Some(parent)) => own.same_line_metrics(parent),
            (own, parent) => own.is_none() && parent.is_none(),
        };
        let close = nodes.items(node).end.get().checked_sub(1).map(ItemId::new);
        let empty = close.is_some_and(|close| clusters.get(opens) == clusters.get(close));
        let kept = ruby
            || annotation
            || boxes.has(BoxFlags::PAINTS)
            || boxes.has(BoxFlags::HAS_EDGES)
            || !matches!(boxes.align, VerticalAlign::Baseline)
            || shift != LayoutUnit::ZERO
            || boxes.has(BoxFlags::TRIMS_OVER)
            || boxes.has(BoxFlags::TRIMS_UNDER)
            || empty
            || !same_metrics;
        if kept {
            // `text-box-trim` brings its trimmed edges to `text-box-edge`'s,
            // as Chrome's `InlineBoxState::AdjustEdges` does.
            let (trims_over, trims_under, edge) = boxes.text_box();
            let (ascent, descent) = own.map_or((LayoutUnit::ZERO, LayoutUnit::ZERO), |metrics| {
                (
                    if trims_over {
                        metrics.over_edge(edge)
                    } else {
                        metrics.ascent
                    },
                    if trims_under {
                        metrics.under_edge(edge)
                    } else {
                        metrics.descent
                    },
                )
            });
            let (over, under) = boxes.across;
            out.rare_mut().kept_boxes.boxes.push_bounded(
                (node, Extent::new(ascent + over, descent + under)),
                "no more kept boxes than nodes, which a NodeId names",
            );
        }
    }
}

/// Returns a box's fixed shift relative to its parent's baseline.
/// Lengths and percentages truncate onto the grid; super/sub use the
/// configured size ratio or the parent's font offsets. Edge alignment
/// waits for line layout. Dominant baselines additionally raise non-atomic
/// boxes that are not placed by their edges.
#[allow(clippy::too_many_arguments)]
fn baseline_shift(
    input: &MeasureInput<'_>,
    metrics: &Table<TextFactsId, TextMetrics>,
    own_text: TextFactsId,
    parent: TextFactsId,
    align: VerticalAlign,
    atomic: bool,
    dominant: bool,
    own_metrics: Option<&FontLineMetrics>,
    parent_metrics: Option<&FontLineMetrics>,
) -> LayoutUnit {
    let text = own_text;
    let facts = &input.content.facts;
    let super_sub = input.super_sub;
    let mut shift = match align {
        VerticalAlign::Super | VerticalAlign::Sub => {
            let up = matches!(align, VerticalAlign::Super);
            match super_sub {
                SuperSubPosition::SizeRatio => {
                    let size = LayoutUnit::from_px(input.size(parent));
                    let one = LayoutUnit::from_px(1.0);
                    if up {
                        size.divided(3) + one
                    } else {
                        -(size.divided(5) + one)
                    }
                }
                // The offsets are downward from the baseline, a
                // superscript's negative.
                SuperSubPosition::FontMetrics => {
                    parent_metrics.map_or(LayoutUnit::ZERO, |metrics| {
                        if up {
                            -metrics.superscript
                        } else {
                            -metrics.subscript
                        }
                    })
                }
            }
        }
        VerticalAlign::Px(px) => LayoutUnit::from_px_truncated(px),
        VerticalAlign::Fraction(fraction) => {
            metrics.get(text).map_or(LayoutUnit::ZERO, |metrics| {
                LayoutUnit::from_px_truncated(metrics.line_height.to_px() * fraction)
            })
        }
        VerticalAlign::Baseline
        | VerticalAlign::Middle
        | VerticalAlign::TextTop
        | VerticalAlign::TextBottom
        | VerticalAlign::Top
        | VerticalAlign::Bottom => LayoutUnit::ZERO,
    };
    let placed_by_edges = matches!(
        align,
        VerticalAlign::TextTop
            | VerticalAlign::TextBottom
            | VerticalAlign::Top
            | VerticalAlign::Bottom
    );
    if dominant && !atomic && !placed_by_edges {
        let raise = |text: TextFactsId| {
            let dominant = facts.text(text).dominant;
            if text == own_text {
                own_metrics
            } else {
                parent_metrics
            }
            .map_or(LayoutUnit::ZERO, |metrics| metrics.baseline(dominant))
        };
        shift = shift + (raise(text) - raise(parent));
    }
    shift
}
