//! Static positions: where line layout puts each absolutely positioned
//! box's anchor.
//!
//! An anchor is a piece that takes no room, at its own bidi level, so
//! reordering places it as Chrome places an out-of-flow line item. Chrome's
//! `PlaceOutOfFlowObjects` turns where it stands into the box's static
//! position:
//! - an inline-level box stands where its anchor is, at the line box's top;
//! - a block-level box stands at the block's start edge, whatever the floats
//!   and `text-indent`, and below the line where in-flow content comes before
//!   it in the line's direction.
//!
//! An anchor no line holds stands on an empty line after the last, as an
//! empty line box holds it in Chrome. That is an anchor after a final forced
//! break, past a clamp, or in content with no line at all.

use super::place::{LineOffset, LinePieces, PieceId, Side};
use super::{Anchor, Fragments, Piece, PieceKind, Placer};
use crate::build::OriginalDisplay;
use crate::stages::analysis::BidiLevel;
use crate::stages::content::NodeId;
use crate::stages::lines::LineView;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

impl Placer<'_> {
    /// Records the static position of each anchor among `line`'s pieces,
    /// now placed with the line box's left at `offset`.
    pub(super) fn place_anchors(
        &self,
        pieces: &LinePieces,
        out: &mut Fragments,
        line: &LineView<'_>,
        offset: LineOffset,
    ) {
        let top = self.block.top(line.block_start);
        let left = InlineLayoutUnit::from_layout(offset.left);
        let height = line.extent.zero_if_none().height();
        for (at, piece) in pieces.logical.iter() {
            work::step();
            if piece.kind != PieceKind::Absolute {
                continue;
            }
            let anchor = if self.is_block_level(piece.node) {
                let rtl = self.block_rtl();
                let below = content_precedes(self, pieces, at, piece.node);
                Anchor {
                    node: piece.node,
                    line: Some(line.id),
                    inline: self.start_edge(rtl),
                    block: if below { top + height } else { top },
                    rtl,
                }
            } else {
                Anchor {
                    node: piece.node,
                    line: Some(line.id),
                    inline: left + piece.inline,
                    block: top,
                    rtl: piece.level.is_rtl(),
                }
            };
            push(out, anchor);
        }
    }

    /// Records the static position of each anchor no line holds: those after
    /// the last line's.
    ///
    /// Each stands on an empty line at the lines' end, set as the last line
    /// of a paragraph is. Its band is the area's, since the host's floats are
    /// not asked about a line that holds nothing.
    pub(super) fn place_unheld_anchors(&self, out: &mut Fragments) {
        let content = self.stages.content;
        let last = out.anchors().last().map(|anchor| anchor.node);
        let rtl = self.block_rtl();
        let block = self.block.block_end;
        let start = self.empty_line_start(rtl);
        for absolute in content.absolutes().as_slice() {
            work::step();
            let node = content.item_node(absolute.item);
            if last.is_some_and(|last| node <= last) {
                continue;
            }
            let inline = match absolute.display {
                OriginalDisplay::Inline => start,
                OriginalDisplay::Block => self.start_edge(rtl),
            };
            push(
                out,
                Anchor {
                    node,
                    line: None,
                    inline,
                    block,
                    rtl,
                },
            );
        }
        debug_assert_eq!(
            out.anchors().len(),
            content.absolutes().len(),
            "an anchor for each absolutely positioned box"
        );
    }

    /// Returns whether the anchor of `node` is a block-level box's.
    fn is_block_level(&self, node: NodeId) -> bool {
        let content = self.stages.content;
        let item = content.nodes.items(node).start;
        content
            .item_absolute(item)
            .is_some_and(|absolute| absolute.display == OriginalDisplay::Block)
    }

    /// Returns whether the block reads right to left, as a block-level box
    /// takes it: its own direction, or the current paragraph's under `auto`.
    fn block_rtl(&self) -> bool {
        BidiLevel::from_direction(self.stages.content.block.direction)
            .unwrap_or(self.line_state.level)
            .is_rtl()
    }

    /// Returns the area's edge a line starts from, the right where `rtl`.
    fn start_edge(&self, rtl: bool) -> InlineLayoutUnit {
        InlineLayoutUnit::from_layout(if rtl {
            self.gates.area.right
        } else {
            self.gates.area.left
        })
    }

    /// Returns where the content of an empty line after the last starts,
    /// right to left where `rtl`, as the last line of a paragraph is set.
    ///
    /// It takes `text-indent` where the block's first line would, or each
    /// line after a forced break would.
    fn empty_line_start(&self, rtl: bool) -> InlineLayoutUnit {
        let indent = self.stages.content.block.text_indent;
        let first = self.lines.lines.is_empty();
        let indent = if (first || indent.each_line) != indent.hanging {
            indent.length(self.gates.area.width())
        } else {
            LayoutUnit::ZERO
        };
        let align = self.gates.text_align_last.resolve(self.gates.text_align);
        let spare = self.gates.area.width() - indent;
        let left = self.gates.area.left + Side::from_align(align, rtl).left_offset(rtl, spare);
        InlineLayoutUnit::from_layout(if rtl { left } else { left + indent })
    }
}

/// Adds `anchor` to `out`'s anchors.
fn push(out: &mut Fragments, anchor: Anchor) {
    out.rare_mut()
        .anchors
        .push_bounded(anchor, "no more anchors than an AnchorId names");
}

/// Returns whether in-flow content comes before the anchor of `node`, the
/// piece at logical place `at`, in the line's direction.
///
/// In-flow content is text, an atomic inline, a hyphen, or a box that keeps
/// its fragment and does not hold the anchor, as Chrome's
/// `HasInFlowFragment` counts a box's placeholder. The pieces are walked in
/// visual order: from the left to the anchor, or from the right where the
/// paragraph reads right to left.
fn content_precedes(placer: &Placer<'_>, pieces: &LinePieces, at: PieceId, node: NodeId) -> bool {
    let nodes = &placer.stages.content.nodes;
    let in_flow = |piece: &Piece| match piece.kind {
        PieceKind::Text | PieceKind::Atomic | PieceKind::Hyphen => true,
        PieceKind::Place => !nodes.contains(piece.node, node),
        _ => false,
    };
    let order = pieces.order.as_slice();
    let Some(visual) = order.iter().position(|&logical| logical == at) else {
        return false;
    };
    let before = if placer.line_state.level.is_rtl() {
        order.get(visual + 1..)
    } else {
        order.get(..visual)
    };
    before.unwrap_or_default().iter().any(|&logical| {
        work::step();
        pieces.logical.get(logical).is_some_and(in_flow)
    })
}
