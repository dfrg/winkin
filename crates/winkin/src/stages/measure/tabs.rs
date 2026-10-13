//! Tab stops: the one function a tab is sized by.
//!
//! A tab's width depends on where it starts on its line, so the prefix gives
//! it none. The intrinsic sizes, the breaker and line layout each size it
//! with [`tab_advance_reached`], so what the breaker fits is what line layout
//! places. A ruby column's overhang may move a tab after it ([`TabReach`]).
//!
//! The rule is Chrome's `Font::TabWidth`:
//! - Stops are multiples of `tab-size` from the block's content edge.
//! - A `tab-size` in spaces counts the block container's space: its primary
//!   font's U+0020 with its letter-spacing and word-spacing, as Chrome's
//!   `LineBreaker` sizes a tab in `InlineNode::FontForTab`. Where the primary
//!   maps no space, the first listed font that maps one gives it. The count
//!   is the tab's own style's.
//! - A tab reaches the first stop past where it starts, or the one after if
//!   that is nearer than half a space.
//! - With `tab-size: 0` a tab is as wide as its letter-spacing, which may be
//!   negative.
//!
//! Chrome snaps a tab's width up onto layout's grid (`SnappedWidth`), and so
//! does this. Nothing here panics: bad sizes and positions saturate.

use crate::stages::analysis::ClusterId;
use crate::style::TabSize;
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};

/// Where one style's tabs stop.
///
/// It holds how far apart the stops are, and the least a tab may reach:
/// half a space. A tab that would reach less goes on to the next stop. It
/// also holds how wide a tab is where the stops are zero apart.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct TabStops {
    interval: InlineLayoutUnit,
    least: InlineLayoutUnit,
    /// The block's letter-spacing, which Chrome's `Font::TabWidth` makes a
    /// tab's width where there are no stops.
    letter: InlineLayoutUnit,
}

impl TabStops {
    /// The stops of `tab_size`, in a block whose space advances `space`, with
    /// letter-spacing `letter` and word-spacing `word`.
    ///
    /// A stop of spaces counts each space with both spacings, as Blink's
    /// `TabSize::GetPixelSize` does. The least a tab reaches is half the bare
    /// space. A `tab-size` that is not a finite positive number, which no
    /// computed value is, puts the stops nothing apart, as `tab-size: 0` does.
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

    /// How far apart the stops are.
    #[cfg(test)]
    pub(super) fn interval(self) -> InlineLayoutUnit {
        self.interval
    }
}

/// How far a tab after a ruby column moves for the column's overhang, under
/// `ruby-overhang: spaces`.
///
/// Chrome sizes each item after a column where it stands, then takes what
/// it can of the column's end overhang off it (`CommitPendingEndOverhang`).
/// The prefix takes the whole overhang off the column, so a tab there stands
/// short of where Chrome sizes it by what the items after it take: `shift`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct TabReach {
    /// The tab's cluster.
    pub(super) tab: ClusterId,
    /// How far short of Chrome's place the tab stands.
    pub(super) shift: LayoutUnit,
}

/// Returns the width of a tab that starts `position` along its line, as
/// [`tab_advance`] does, sized `reach`'s shift further on where a ruby
/// column's overhang moved it.
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
/// `position` is measured from the line's start in logical order, indent
/// included. `origin` is where the line starts from the block's content edge
/// on its paragraph's start side. `stops` are the tab's text's.
///
/// The width reaches the first stop past `position`, or the one after where
/// that is nearer than half a space. It rounds up onto layout's grid, as
/// Chrome snaps a tab's width. Where the stops are nothing apart, it is the
/// block's letter-spacing, rounded up too.
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
    // The remainder is floored, so a tab starting before the content edge
    // reaches the stop past it: Chrome 155 ends a tab starting 10 px before
    // the edge at the edge, and one starting 110 px before it 100 px before.
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
