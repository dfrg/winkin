//! The one position type: a byte in the content's text.

use core::fmt;

/// A byte position in the content's text.
///
/// Not an id: it indexes bytes, not a table. The builder caps the text at
/// [`MAX`](Self::MAX) bytes, so the sum of two positions never overflows
/// `u32`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct TextOffset(u32);

impl TextOffset {
    /// The most bytes the text may hold: 2^30 − 1.
    ///
    /// An offset plus a length from the text stays inside `u32`, and a
    /// per-byte cluster table still fits in memory.
    pub(crate) const MAX: usize = (1 << 30) - 1;

    /// The position `at` bytes into the text.
    ///
    /// `at` is at most the text length. The builder refuses text past
    /// [`MAX`](Self::MAX), so debug builds assert it as a crate bug. Release
    /// builds clamp to `MAX`, which keeps offsets in order.
    #[inline]
    pub(crate) fn new(at: usize) -> Self {
        debug_assert!(at <= Self::MAX, "TextOffset out of range");
        Self(at.min(Self::MAX) as u32)
    }

    /// The position as an index into the text.
    #[inline]
    pub(crate) fn get(self) -> usize {
        self.0 as usize
    }

    /// The position as a shaper cluster label.
    #[inline]
    pub(crate) fn to_u32(self) -> u32 {
        self.0
    }

    /// The position a shaper cluster label names, clamped to
    /// [`MAX`](Self::MAX).
    ///
    /// The shaper returns only labels it was given; the clamp guards against
    /// any other.
    #[inline]
    pub(crate) fn from_u32(raw: u32) -> Self {
        Self(raw.min(Self::MAX as u32))
    }
}

impl fmt::Debug for TextOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.0)
    }
}
