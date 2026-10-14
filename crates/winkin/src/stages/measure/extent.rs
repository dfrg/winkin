//! [`Extent`]: how far something reaches over and under a baseline.

use crate::stages::content::Atomic;
use crate::stages::fonts::LineBaseline;
use crate::style::WritingMode;
use crate::unit::LayoutUnit;

/// A pair of distances either side of a baseline, in 26.6 `LayoutUnit`s.
///
/// The ascent reaches over the baseline and the descent under, each positive
/// away from it. Either may be negative, so a small line height makes lines
/// overlap rather than clip.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct Extent {
    ascent: LayoutUnit,
    descent: LayoutUnit,
}

impl Extent {
    /// The extent of what takes no room across the line: a closing edge, a
    /// float's anchor, an item out of the line's flow.
    ///
    /// Both sides are `LayoutUnit::MIN`, as in Chrome's `FontHeight::Empty()`,
    /// so it is the identity of [`unite`](Self::unite).
    pub(crate) const NONE: Self = Self {
        ascent: LayoutUnit::MIN,
        descent: LayoutUnit::MIN,
    };

    /// Returns an extent reaching `ascent` over the baseline and `descent`
    /// under.
    pub(crate) fn new(ascent: LayoutUnit, descent: LayoutUnit) -> Self {
        Self { ascent, descent }
    }

    /// Returns the margin box of `atomic` against its baseline, in a block
    /// of `writing_mode` whose lines are set on `baseline`.
    ///
    /// This follows Blink's `BaselineMetrics`:
    /// - An atomic inline with its own baseline sits on it.
    /// - One without sits its margin box's under edge on the line's baseline,
    ///   or on a `central` baseline is centred, the odd 1/64 over
    ///   (`SynthesizeMetrics`).
    ///
    /// An atomic's baseline is measured down from its block-start edge. In
    /// `vertical-lr`, where lines flip, that edge is the under side, so the
    /// baseline is measured up from it (Blink's `IsFlippedLines`). In
    /// `sideways-lr` it is the over side (Chrome 153).
    pub(crate) fn from_atomic(
        atomic: &Atomic,
        writing_mode: WritingMode,
        baseline: LineBaseline,
    ) -> Self {
        // Chrome truncates the margins (already so in the row) and the size.
        // The baseline is a position in the host's layout, so it rounds.
        let (over, under) = atomic.margins_across;
        let size = atomic.size;
        let block = LayoutUnit::from_px_truncated(size.block);
        match size.baseline {
            Some(baseline) => {
                let baseline = LayoutUnit::from_px(baseline);
                let (ascent, descent) = if writing_mode.flips_lines() {
                    (block - baseline, baseline)
                } else {
                    (baseline, block - baseline)
                };
                Self::new(over + ascent, descent + under)
            }
            None => {
                let size = over + block + under;
                match baseline {
                    LineBaseline::Alphabetic => Self::new(size, LayoutUnit::ZERO),
                    LineBaseline::Central => Self::new(size - size.half(), size.half()),
                }
            }
        }
    }

    /// Returns how far it reaches over the baseline.
    pub(crate) fn ascent(self) -> LayoutUnit {
        self.ascent
    }

    /// Returns how far it reaches under the baseline.
    pub(crate) fn descent(self) -> LayoutUnit {
        self.descent
    }

    /// Returns the farther of the two on each side, as a line box grows.
    pub(crate) fn unite(self, other: Self) -> Self {
        Self {
            ascent: self.ascent.max(other.ascent),
            descent: self.descent.max(other.descent),
        }
    }

    /// Returns whether it is [`NONE`](Self::NONE), taking no room.
    pub(crate) fn is_none(self) -> bool {
        self == Self::NONE
    }

    /// Returns the extent with its baseline raised by `up`.
    ///
    /// The ascent grows by `up` and the descent shrinks by it, as Blink's
    /// `FontHeight::Move` by `-up`. [`NONE`](Self::NONE) stays `NONE`.
    pub(crate) fn raised(self, up: LayoutUnit) -> Self {
        if self.is_none() {
            return self;
        }
        Self {
            ascent: self.ascent + up,
            descent: self.descent - up,
        }
    }

    /// Returns the ascent plus the descent; zero for [`NONE`](Self::NONE).
    pub(crate) fn height(self) -> LayoutUnit {
        if self.is_none() {
            LayoutUnit::ZERO
        } else {
            self.ascent + self.descent
        }
    }

    /// Returns the extent, or zero on both sides where it is
    /// [`NONE`](Self::NONE).
    ///
    /// Blink zeroes an empty `FontHeight` before aligning it, and Chrome
    /// does the same for an empty annotation line.
    #[inline]
    pub(crate) fn zero_if_none(self) -> Self {
        if self.is_none() {
            Self::new(LayoutUnit::ZERO, LayoutUnit::ZERO)
        } else {
            self
        }
    }

    /// Returns the height of a line box of this extent:
    /// [`height`](Self::height), never negative.
    #[inline]
    pub(crate) fn box_height(self) -> LayoutUnit {
        self.height().max(LayoutUnit::ZERO)
    }

    /// Returns its top and bottom around the baseline position `baseline`;
    /// both at `baseline` for [`NONE`](Self::NONE).
    #[inline]
    pub(crate) fn around(self, baseline: LayoutUnit) -> (LayoutUnit, LayoutUnit) {
        let extent = self.zero_if_none();
        (baseline - extent.ascent, baseline + extent.descent)
    }
}
