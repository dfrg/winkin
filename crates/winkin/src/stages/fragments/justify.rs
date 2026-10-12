//! Justifies a line: its opportunities' shares of the spare room, and the
//! amounts a reader reads them back with.

use core::ops::Range;

use super::place::{LinePieces, PieceId};
use super::{FragmentItemFlags, Fragments, Piece, PieceKind, Placer};
use crate::config::TabJustification;
use crate::data::IdRange;
use crate::data::{Id, Table, define_id};
use crate::stages::analysis::ClusterId;
use crate::stages::lines::{LineBand, LineId, LineView};
use crate::stages::measure::{JustifyOpportunities, JustifySummary};
use crate::unit::{InlineLayoutUnit, LayoutUnit};

/// The line's spare room per justification opportunity, in 48.16: 32 bytes.
///
/// The room divides truncated, as in Chrome's `ShapeResultSpacing`
/// (`expansion / count`). The remainder goes to the opportunity Chrome fills
/// last, as its `NextExpansion` does, so the shares add up exactly. That is
/// the last one of the logically last item, or its first if that item is
/// right-to-left.
///
/// Each item's `inline` and advance already include the room. A reader adds
/// a cluster's room ([`room`](Self::room)) to its advance and moves its
/// glyphs by the room before it. Nothing is stored per cluster.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct LineJustification {
    pub(super) summary: JustifySummary,
    /// The room each opportunity gets, the last one aside.
    pub(super) per: InlineLayoutUnit,
    /// The cluster whose opportunity is filled last: its after one where it
    /// has one, else its before one.
    pub(super) last: ClusterId,
    /// What the last takes beyond `per`, in 1/65536 px. It is less than the
    /// line's count of opportunities, so it fits a `u32`.
    pub(super) leftover: u32,
}

impl LineJustification {
    /// Spreads `extra` over `count` opportunities, the last of them
    /// `last`'s. Returns `None` when `count` is zero.
    fn new(
        extra: InlineLayoutUnit,
        count: u32,
        last: ClusterId,
        summary: JustifySummary,
    ) -> Option<Self> {
        if count == 0 {
            return None;
        }
        let count = i64::from(count);
        let per = extra.raw() / count;
        // Less than `count`, and not negative for room that is not.
        let leftover = extra.raw() - per.saturating_mul(count);
        Some(Self {
            per: InlineLayoutUnit::from_raw(per),
            summary,
            last,
            leftover: u32::try_from(leftover).unwrap_or(0),
        })
    }

    /// Returns what the last opportunity takes beyond `per`.
    pub(super) fn leftover(&self) -> InlineLayoutUnit {
        InlineLayoutUnit::from_raw(i64::from(self.leftover))
    }

    /// Returns what `count` opportunities take together, with the leftover
    /// when `last` says the last one is among them.
    ///
    /// This is how much a piece of the line widens.
    fn share(&self, count: u32, last: bool) -> InlineLayoutUnit {
        let each = InlineLayoutUnit::from_raw(self.per.raw().saturating_mul(i64::from(count)));
        if last { each + self.leftover() } else { each }
    }

    /// Returns the room `cluster` takes before and after it, given which
    /// opportunities it has.
    #[inline]
    fn room(
        &self,
        cluster: ClusterId,
        before: bool,
        after: bool,
    ) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let last = cluster == self.last;
        let share = |takes: bool, rest: bool| match (takes, rest) {
            (false, _) => InlineLayoutUnit::ZERO,
            (true, false) => self.per,
            (true, true) => self.per + self.leftover(),
        };
        // Chrome takes a cluster's before opportunity first.
        (share(before, last && !after), share(after, last))
    }
}

/// A justified range's amounts and which of its clusters have opportunities.
///
/// The range is a line, or a ruby base or annotation spread as a line of its
/// own. Line layout builds one for the line it places, and a reader for the
/// item it reads. Both count opportunities by the same rule.
#[derive(Clone, Debug)]
pub(super) struct Justified<'a> {
    pub(super) justification: LineJustification,
    pub(super) opportunities: JustifyOpportunities<'a>,
}

impl<'a> Justified<'a> {
    /// Spreads `room` over the opportunities of `clusters`, justified as a
    /// line of their own, which `opportunities` finds.
    ///
    /// This serves a ruby base or an annotation. Returns `None` where there
    /// is no room, before looking for opportunities, or no opportunity.
    pub(super) fn new(
        opportunities: impl FnOnce() -> Option<JustifyOpportunities<'a>>,
        clusters: Range<ClusterId>,
        room: InlineLayoutUnit,
    ) -> Option<Self> {
        if room <= InlineLayoutUnit::ZERO {
            return None;
        }
        let opportunities = opportunities()?;
        let count = opportunities.count_range(clusters.clone());
        let last = opportunities.last(clusters)?;
        Some(Self {
            justification: LineJustification::new(room, count, last, opportunities.summary())?,
            opportunities,
        })
    }

    /// Returns the room `cluster` takes before and after it; none outside
    /// the justified range.
    ///
    /// On an ASCII line justified at spaces, the cluster's class decides
    /// (`JustifyOpportunities::is_simple`).
    #[inline]
    pub(super) fn room(&self, cluster: ClusterId) -> (InlineLayoutUnit, InlineLayoutUnit) {
        let (before, after) = if self.opportunities.is_simple() {
            (false, self.opportunities.simple_after(cluster))
        } else {
            self.opportunities.at(cluster)
        };
        self.justification.room(cluster, before, after)
    }
}

define_id! {
    /// Names a justified line's row in a layout's table of
    /// [`JustifiedLine`]s, which its line item holds.
    pub(super) struct JustifiedLineId(u32);
}

/// Facts kept only for a justified line: 40 bytes.
///
/// `content_end` fits in the padding between the line and the
/// justification.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) struct JustifiedLine {
    pub(super) line: LineId,
    pub(super) content_end: ClusterId,
    pub(super) justification: LineJustification,
}

impl<'a> Placer<'a> {
    /// Justifies `line`, whose alignment is `justify`, and records it in
    /// `out`.
    ///
    /// Writes what justification gave the line to `justified`, and returns
    /// its record's row, which the line's item names, or `None` where it is
    /// not justified. Out of line, so that a line set another way holds none
    /// of its code; the justification is written in place, as it is large.
    #[inline(never)]
    pub(super) fn justify_into(
        &self,
        pieces: &mut LinePieces,
        line: &LineView<'_>,
        id: LineId,
        band: LineBand,
        out: &mut Fragments,
        justified: &mut Option<Justified<'a>>,
    ) -> Option<JustifiedLineId> {
        let made = justified.insert(self.justify(pieces, line, band)?);
        let row = out.rare_mut().justified.push(super::JustifiedLine {
            line: id,
            content_end: line.content_end,
            justification: made.justification,
        });
        // A line has one row at most, and a JustifiedLineId names as many
        // as a LineId.
        debug_assert!(row.is_some(), "no more justified lines than lines");
        if row.is_none() {
            *justified = None;
        }
        row
    }

    /// Justifies `line` in `band`, giving each of `pieces` its share of the
    /// spare room.
    ///
    /// Returns the amounts and which clusters have opportunities. Returns
    /// `None` where the line has no room or no opportunity; it is then set at
    /// its start, as in Chrome.
    ///
    /// The shares add up to the room exactly, as in Chrome. A tab's width
    /// with its share rounds up onto the grid, as Chrome snaps a tab, so its
    /// size stays its advance. Tabs take no share where the config keeps them
    /// at their stops.
    fn justify(
        &self,
        pieces: &mut LinePieces,
        line: &LineView<'_>,
        band: LineBand,
    ) -> Option<Justified<'a>> {
        let extra = band.width() - (line.indent + line.width);
        if extra <= LayoutUnit::ZERO {
            return None;
        }
        let opportunities = JustifyOpportunities::from_line(
            &self.stages,
            line.clusters().start..line.content_end,
            None,
        )?;
        let justify_tabs = self.gates.justification == TabJustification::Stretch;
        let mut count = 0u32;
        for piece in pieces.logical.as_mut_slice() {
            // A tab that doesn't hang takes its opportunity's room only where
            // the config widens tabs.
            let tab = piece.kind == PieceKind::Text
                && piece.flags.contains(FragmentItemFlags::TAB)
                && !piece.flags.contains(FragmentItemFlags::HANGS);
            piece.opportunities = if piece.is_text() {
                opportunities.count_range(piece.start..piece.end)
            } else if tab && justify_tabs {
                opportunities.count(piece.start)
            } else {
                0
            };
            count = count.saturating_add(piece.opportunities);
        }
        let (holder, last) = last_opportunity(&pieces.logical, &opportunities)?;
        let justification = LineJustification::new(
            InlineLayoutUnit::from_layout(extra),
            count,
            last,
            opportunities.summary(),
        )?;
        let ids = (PieceId::new(0)..pieces.logical.next_id()).ids();
        for (at, piece) in ids.zip(pieces.logical.as_mut_slice()) {
            if piece.opportunities == 0 {
                continue;
            }
            piece.advance += justification.share(piece.opportunities, at == holder);
            if piece.flags.contains(FragmentItemFlags::TAB) {
                piece.advance = piece.advance.ceil_to_grid();
            }
        }
        Some(Justified {
            justification,
            opportunities,
        })
    }
}

/// Returns the justification opportunity Chrome fills last, which takes the
/// leftover: its piece and its cluster. `None` where no piece has one.
///
/// Chrome spends the room item by item in logical order (`JustifyResults`).
/// Within an item it goes in glyph order
/// (`ShapeResult::ApplySpacingOrExpansion`), which is visual.
///
/// So the last is in the logically last piece with an opportunity. Left to
/// right, it is that piece's last. Right to left, it is the first of the
/// whole item: Chrome's shape result spans pieces of the same node and level
/// that only a font change divides. A tab is an item of its own, as in
/// Chrome.
fn last_opportunity(
    pieces: &Table<PieceId, Piece>,
    opportunities: &JustifyOpportunities<'_>,
) -> Option<(PieceId, ClusterId)> {
    let (holder, piece) = pieces
        .iter()
        .rev()
        .find(|(_, piece)| piece.opportunities > 0)?;
    if !piece.is_text() || !piece.level.is_rtl() {
        let last = opportunities.last(piece.start..piece.end)?;
        return Some((holder, last));
    }
    let mut first = holder;
    while let Some(at) = first.get().checked_sub(1).map(PieceId::new)
        && let (Some(before), Some(after)) = (pieces.get(at), pieces.get(first))
        && before.is_text()
        && before.node == piece.node
        && before.level == piece.level
        && before.end == after.start
    {
        first = at;
    }
    let (at, piece) = (first..PieceId::new(holder.get() + 1))
        .ids()
        .filter_map(|at| pieces.get(at).map(|piece| (at, piece)))
        .find(|(_, piece)| piece.opportunities > 0)?;
    let last = opportunities.first(piece.start..piece.end)?;
    Some((at, last))
}
