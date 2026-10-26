//! Tab stops, and the one function that sizes a tab.
//!
//! A tab's width depends on where it starts, so the prefix gives it none.
//! Intrinsic sizing, the breaker and line layout all call
//! [`tab_advance_reached`], so they agree.
//!
//! The rule follows Blink's `Font::TabWidth`:
//! - Stops are multiples of `tab-size` from the block's content edge.
//! - A `tab-size` in spaces counts the block's space: its primary font's
//!   U+0020 (or the first listed font's that maps one) with the block's
//!   letter- and word-spacing (`InlineNode::FontForTab`). The count comes
//!   from the tab's own style.
//! - A tab reaches the first stop past its start, or the next one if that is
//!   nearer than half a space.
//! - With `tab-size: 0` a tab is as wide as the block's letter-spacing.
//!
//! Widths round up to the 26.6 grid (`SnappedWidth`). Bad sizes and
//! positions saturate; nothing panics.

use crate::stages::analysis::ClusterId;
use crate::style::TabSize;
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};

/// The tab stops of one style, in 48.16 `InlineLayoutUnit`s: the interval,
/// and the least distance a tab reaches, half a space.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct TabStops {
    interval: InlineLayoutUnit,
    least: InlineLayoutUnit,
    /// The block's letter-spacing: a tab's width where the interval is zero.
    letter: InlineLayoutUnit,
}

impl TabStops {
    /// Returns the stops of `tab_size` in a block whose space advance is
    /// `space`, with letter-spacing `letter` and word-spacing `word`.
    ///
    /// Each space counts both spacings (Blink's `TabSize::GetPixelSize`); the
    /// least distance is half the bare space. A `tab-size` that is not finite
    /// and positive gives a zero interval.
    pub(super) fn new(
        tab_size: TabSize,
        space: InlineLayoutUnit,
        letter: TextUnit,
        word: TextUnit,
    ) -> Self {
        let spaced =
            space + InlineLayoutUnit::from_text(letter) + InlineLayoutUnit::from_text(word);
        let interval = match tab_size {
            TabSize::Spaces(count) if count.is_finite() && count > 0.0 => spaced.times(count),
            TabSize::Px(px) if px.is_finite() && px > 0.0 => InlineLayoutUnit::from_px(px),
            TabSize::Spaces(_) | TabSize::Px(_) => InlineLayoutUnit::ZERO,
        };
        Self {
            interval: interval.max(InlineLayoutUnit::ZERO),
            least: space.half().max(InlineLayoutUnit::ZERO),
            letter: InlineLayoutUnit::from_text(letter),
        }
    }

    /// Returns the interval between stops.
    #[cfg(test)]
    pub(super) fn interval(self) -> InlineLayoutUnit {
        self.interval
    }
}

/// The shift of a tab after a ruby column under `ruby-overhang: spaces`.
///
/// The prefix takes the column's whole end overhang off the column, but
/// the tab is sized before the overhang comes off (Blink's
/// `CommitPendingEndOverhang`), so the tab starts `shift` short of where it
/// is sized.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct TabReach {
    /// The tab's cluster.
    pub(super) tab: ClusterId,
    /// How far short of its sized position the tab starts.
    pub(super) shift: LayoutUnit,
}

/// Returns [`tab_advance`] at `position` plus `reach`'s shift, if any.
pub(crate) fn tab_advance_reached(
    origin: LayoutUnit,
    position: InlineLayoutUnit,
    stops: TabStops,
    reach: Option<TabReach>,
) -> LayoutUnit {
    let shift = reach.map_or(LayoutUnit::ZERO, |reach| reach.shift);
    tab_advance(
        origin,
        position + InlineLayoutUnit::from_layout(shift),
        stops,
    )
}

/// Returns the width of a tab that starts `position` along its line.
///
/// `position` is measured from the line's start, indent included; `origin`
/// is the line's start from the block's content edge on the paragraph's
/// start side. The width is never negative, except where the interval is
/// zero and the letter-spacing is.
pub(super) fn tab_advance(
    origin: LayoutUnit,
    position: InlineLayoutUnit,
    stops: TabStops,
) -> LayoutUnit {
    let interval = stops.interval;
    if interval <= InlineLayoutUnit::ZERO {
        return stops.letter.to_layout();
    }
    let at = InlineLayoutUnit::from_layout(origin) + position;
    // Floored remainder: a tab starting 10 px before the content edge ends
    // at the edge, and one starting 110 px before at 100 px before.
    let mut rem = at % interval;
    if rem < InlineLayoutUnit::ZERO {
        rem += interval;
    }
    let mut distance = interval - rem;
    if distance < stops.least {
        distance += interval;
    }
    distance.to_layout().max(LayoutUnit::ZERO)
}
