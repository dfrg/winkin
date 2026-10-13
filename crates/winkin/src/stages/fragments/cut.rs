//! Cuts a line for an ellipsis, and writes generated text and divided
//! pieces.
//!
//! Truncation keeps what fits before the ellipsis, as Chrome's
//! `LineTruncator` does. The rest keeps its place, hidden. This module also
//! writes the items of a hyphen, of a piece the cut divides, and of the
//! ellipsis.

use super::justify::Justified;
use super::place::{LineCut, LineOffset, LinePieces, PieceSplit, VisualId};
use super::{FragmentItem, FragmentItemFlags, FragmentItemKind, Fragments, Piece, Placer};
use crate::config::EllipsisSpace;
use crate::data::Id;
use crate::data::IdRange;
use crate::stages::analysis::{BidiLevel, ClusterId};
use crate::stages::content::NodeId;
use crate::stages::lines::{LineBand, LineView};
use crate::stages::measure::GeneratedItems;
use crate::unit::{InlineLayoutUnit, LayoutUnit};
use crate::work;

/// What truncation keeps of a piece that fits in whole or in part, as
/// Chrome's `LineTruncator` keeps the part of its item that fits.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum PieceCut {
    /// The whole piece.
    Whole,
    /// The clusters on the line's start side of the boundary `at`, `kept`
    /// wide. The rest is hidden.
    At {
        at: ClusterId,
        kept: InlineLayoutUnit,
    },
}

/// Where generated text's items go: node, level, boundary, the first part's
/// start from the line box's left, baseline, flags and scale.
#[derive(Copy, Clone, Debug)]
struct GeneratedAt {
    node: NodeId,
    level: BidiLevel,
    at: ClusterId,
    inline: InlineLayoutUnit,
    baseline: LayoutUnit,
    flags: FragmentItemFlags,
    /// The scale `text-fit` sets the text in: the line's for a hyphen, 1
    /// for an ellipsis.
    scale: f32,
}

/// The ellipsis that ends a cut line: its start from the line box's left,
/// the cut's boundary, and its shaped text.
#[derive(Copy, Clone, Debug)]
pub(super) struct LineEllipsis {
    inline: InlineLayoutUnit,
    at: ClusterId,
    text: GeneratedItems,
}

impl LineEllipsis {
    /// Returns how many items it writes, one per font of its text.
    pub(super) fn items(self) -> usize {
        self.text.count()
    }
}

impl<'a> Placer<'a> {
    /// Writes the items of the hyphen `piece`, flagged `flags`, on
    /// `baseline`: one item per font of the line's hyphen.
    ///
    /// Kept out of line, since only a line or two a paragraph needs it.
    #[inline(never)]
    pub(super) fn emit_hyphen(
        &self,
        out: &mut Fragments,
        piece: &Piece,
        flags: FragmentItemFlags,
        baseline: LayoutUnit,
        scale: f32,
    ) {
        let Some(text) = self.line_state.hyphen else {
            return;
        };
        let at = GeneratedAt {
            node: piece.node,
            level: piece.level,
            at: piece.start,
            inline: piece.inline,
            baseline: baseline - piece.shift,
            flags,
            scale,
        };
        self.emit_generated(out, text, at);
    }

    /// Writes one item per font of generated `text`, left to right from
    /// where `at` puts the first.
    fn emit_generated(&self, out: &mut Fragments, text: GeneratedItems, at: GeneratedAt) {
        let generated = self.input.stages.measured.generated();
        let mut inline = at.inline;
        for id in text.ids() {
            work::step();
            let Some(piece) = generated.get(id) else {
                continue;
            };
            let advance = if at.scale == 1.0 {
                piece.advance
            } else {
                piece.advance.times(at.scale)
            };
            let item = FragmentItem::from_generated(
                at.node,
                at.level,
                at.at,
                id,
                inline,
                advance,
                at.baseline,
                at.flags,
            );
            out.items
                .push_bounded(item, "no more items than a FragmentItemId names");
            inline += advance;
        }
    }

    /// Writes the two items of a `piece` the cut divides as `split` says.
    ///
    /// The kept part is on the line's start side; the other is hidden. The
    /// part on the left is written first. Both are marked `spread` where
    /// `ruby-align` spread room into the piece.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    pub(super) fn emit_split(
        &self,
        out: &mut Fragments,
        piece: &Piece,
        kind: FragmentItemKind,
        flags: FragmentItemFlags,
        split: PieceSplit,
        baseline: LayoutUnit,
        spread: bool,
    ) {
        let hidden = flags.union(FragmentItemFlags::HIDDEN);
        let keeps_first = piece.keeps_first(self.line_state.level);
        let rest = piece.advance - split.kept;
        let (head, tail) = (piece.start..split.at, split.at..piece.end);
        let (kept, dropped) = if keeps_first {
            ((head, split.kept, flags), (tail, rest, hidden))
        } else {
            ((tail, split.kept, flags), (head, rest, hidden))
        };
        let kept_left = keeps_first != piece.level.is_rtl();
        let (left, right) = if kept_left {
            (kept, dropped)
        } else {
            (dropped, kept)
        };
        let mut inline = piece.inline;
        for (clusters, advance, flags) in [left, right] {
            let part = Piece {
                start: clusters.start,
                end: clusters.end,
                inline,
                advance,
                ..*piece
            };
            out.items.push_bounded(
                part.fragment_item(kind, flags, baseline, spread),
                "no more items than a FragmentItemId names",
            );
            inline += advance;
        }
    }

    /// Writes `ellipsis` to `out` as the block's generated text, at the
    /// line's level, on `baseline`.
    ///
    /// Chrome likewise sets an ellipsis on the line box's baseline in the
    /// line's style.
    #[inline(never)]
    pub(super) fn emit_ellipsis(
        &self,
        out: &mut Fragments,
        ellipsis: LineEllipsis,
        baseline: LayoutUnit,
    ) {
        let at = GeneratedAt {
            node: NodeId::BLOCK,
            level: self.line_state.level,
            at: ellipsis.at,
            inline: ellipsis.inline,
            baseline,
            flags: FragmentItemFlags::NONE,
            scale: 1.0,
        };
        self.emit_generated(out, ellipsis.text, at);
    }

    /// Cuts a `line` flagged for an ellipsis, as Chrome's
    /// `LineTruncator::TruncateLine` does, and returns where the ellipsis
    /// goes.
    ///
    /// Returns `None` where the block's style has no ellipsis or no piece is
    /// an item.
    ///
    /// The walk visits items from the line's end side in visual order:
    /// - An item past the end edge is hidden.
    /// - An item that fits with the ellipsis after it is kept whole, and the
    ///   ellipsis follows it.
    /// - Text that fits in part keeps its fitting clusters on the start
    ///   side; the rest is hidden.
    /// - The line's first item keeps at least a cluster, or stays whole if
    ///   it is not text. CSS asks this: "the first character or atomic
    ///   inline-level element on a line must be clipped rather than
    ///   ellipsed".
    /// - The block's initial letter stays whole, as Chrome keeps its atomic
    ///   initial letter box.
    ///
    /// A box is never cut. Its parts keep what they hold, as Chrome's box
    /// fragments keep their laid-out size ("ellipsing only affects
    /// rendering").
    ///
    /// The room runs from each item to the band's end edge as placed. Chrome
    /// measures before `text-align` moves the line, so its centered or
    /// end-aligned clamped lines put the ellipsis past the band. That is not
    /// copied.
    #[inline(never)]
    pub(super) fn truncate(
        &self,
        pieces: &mut LinePieces,
        line: &LineView<'_>,
        band: LineBand,
        offset: &LineOffset,
        justified: Option<&Justified<'_>>,
    ) -> Option<LineCut> {
        let generated = self.input.stages.measured.generated();
        let facts = self.stages.text_facts(NodeId::BLOCK);
        // The ellipsis is shaped in the line's direction, as Chrome's
        // `LineTruncator` shapes it.
        let rtl = self.line_state.level.is_rtl();
        let text = generated.ellipsis(facts, rtl)?;
        let width = InlineLayoutUnit::from_layout(generated.snapped(text));
        // The line's end edge from the line box's left: the band's right
        // for left to right, its left for right to left.
        let edge = if rtl {
            band.left
        } else {
            band.left + band.width()
        };
        let edge = InlineLayoutUnit::from_layout(edge - offset.left);
        let count = pieces.order.len();
        let is_leaf = |at: &VisualId| {
            pieces
                .order
                .get(*at)
                .and_then(|&logical| pieces.logical.get(logical))
                .is_some_and(|piece| piece.kind.is_leaf())
        };
        // The item at the line's start, which always keeps something.
        let mut visual = (VisualId::new(0)..pieces.order.next_id()).ids();
        let first = if rtl {
            visual.rev().find(is_leaf)
        } else {
            visual.find(is_leaf)
        };
        for step in 0..count {
            work::step();
            let at = VisualId::new(if rtl { step } else { count - 1 - step });
            let Some(&logical) = pieces.order.get(at) else {
                continue;
            };
            let Some(&piece) = pieces.logical.get(logical) else {
                continue;
            };
            if !piece.kind.is_leaf() {
                continue;
            }
            let is_first = first == Some(at);
            // How far it may reach from its start-side edge.
            let room = if rtl {
                piece.inline + piece.advance - edge
            } else {
                edge - piece.inline
            };
            let mut kept = None;
            if self.within_initial_letter(piece.node) {
                // The initial letter is one object, the line's first, as
                // Chrome's initial letter box is an atomic item its truncator
                // keeps whole. Once reached it stays whole, however far the
                // ellipsis after it overflows.
                kept = Some(PieceCut::Whole);
            } else if room > InlineLayoutUnit::ZERO {
                let room = room - width;
                // Text goes cluster by cluster where it may end with white
                // space the config hides.
                let spaces = self.gates.ellipsis_space == EllipsisSpace::Hidden && piece.is_text();
                if room >= piece.advance && !spaces {
                    kept = Some(PieceCut::Whole);
                } else if room > InlineLayoutUnit::ZERO || is_first {
                    kept = self.cut_to(&piece, line, room, is_first, justified);
                }
            }
            let Some(cut) = kept else {
                if !is_first && let Some(hidden) = pieces.logical.get_mut(logical) {
                    hidden.flags.insert(FragmentItemFlags::HIDDEN);
                }
                continue;
            };
            // The ellipsis goes right after what is kept, on its end side.
            let (kept_advance, boundary, split) = match cut {
                PieceCut::At { at, kept } => (
                    kept,
                    at,
                    Some(PieceSplit {
                        piece: logical,
                        at,
                        kept,
                    }),
                ),
                PieceCut::Whole if piece.keeps_first(self.line_state.level) => {
                    (piece.advance, piece.end, None)
                }
                PieceCut::Whole => (piece.advance, piece.start, None),
            };
            let inline = if rtl {
                piece.inline + piece.advance - kept_advance - width
            } else {
                piece.inline + kept_advance
            };
            let ellipsis = LineEllipsis {
                inline,
                at: boundary,
                text,
            };
            return Some(LineCut { ellipsis, split });
        }
        None
    }

    /// Whether `node` is the block's initial letter's box or inside it.
    fn within_initial_letter(&self, node: NodeId) -> bool {
        self.stages
            .measured
            .initial_letter()
            .is_some_and(|letter| self.stages.content.nodes.contains(letter.node, node))
    }

    /// Returns the part of `piece` that fits in `room` from its start-side
    /// edge, as Chrome's `ShapeResult::OffsetToFit` finds it.
    ///
    /// It keeps whole clusters from the line's start side, each as wide as
    /// drawn: reshaped edges and justification room included. Trailing white
    /// space drops where the config hides it (`EllipsisSpace::Hidden`).
    ///
    /// Returns `None` where nothing fits, or a non-text piece doesn't fit
    /// whole. The line's `first` item keeps a cluster anyway, or stays whole
    /// if it is not text.
    #[inline(never)]
    fn cut_to(
        &self,
        piece: &Piece,
        line: &LineView<'_>,
        room: InlineLayoutUnit,
        first: bool,
        justified: Option<&Justified<'_>>,
    ) -> Option<PieceCut> {
        let total = piece.end.get().saturating_sub(piece.start.get());
        if !piece.is_text() || total == 0 {
            return (first || room >= piece.advance).then_some(PieceCut::Whole);
        }
        let keeps_first = piece.keeps_first(self.line_state.level);
        let shapes = self.lines.edges().line_edges(line);
        let edges = self.lines.edges();
        // The items at the boundaries the cut walks, where boxes have edges.
        let mut near = None;
        let mut step = |cluster: ClusterId| {
            let next = ClusterId::new(cluster.get() + 1);
            let own = match shapes.iter().find(|shape| shape.entry(cluster).is_some()) {
                Some(shape) => edges.advance_sum(shape.entries(cluster..next)),
                None => self.stages.cluster_advance_near(cluster, &mut near),
            };
            let room = justified.map_or(InlineLayoutUnit::ZERO, |justified| {
                let (before, after) = justified.room(cluster);
                before + after
            });
            own + room
        };
        // The clusters from the start side, in the order a cut keeps them.
        let nth = |n: usize| {
            let at = if keeps_first {
                piece.start.get() + n
            } else {
                piece.end.get() - 1 - n
            };
            ClusterId::new(at)
        };
        let mut fits = 0;
        let mut sum = InlineLayoutUnit::ZERO;
        while fits < total {
            work::step();
            let next = sum + step(nth(fits));
            if next > room {
                break;
            }
            sum = next;
            fits += 1;
        }
        // White space the kept clusters end with goes with the hidden tail,
        // where the config says. Chrome keeps it.
        if self.gates.ellipsis_space == EllipsisSpace::Hidden {
            while fits > 0 && self.clusters.is_breaking_space(nth(fits - 1)) {
                work::step();
                fits -= 1;
                sum = sum - step(nth(fits));
            }
        }
        if fits == 0 {
            if !first {
                return None;
            }
            fits = 1;
            sum = step(nth(0));
        }
        if fits == total {
            return Some(PieceCut::Whole);
        }
        let at = if keeps_first {
            ClusterId::new(piece.start.get() + fits)
        } else {
            ClusterId::new(piece.end.get() - fits)
        };
        Some(PieceCut::At { at, kept: sum })
    }
}
