//! Typed ids and the casts between ids and indexes.

use core::fmt;
use core::ops::Range;

/// A typed index into a [`Table`](super::Table).
///
/// Implemented only through [`define_id!`], which keeps every conversion
/// between an id and a `usize` in this module.
pub(crate) trait Id: Copy + Eq + Ord + fmt::Debug {
    /// The largest index this id can name.
    const MAX: usize;

    /// The id naming `index`, or `None` if it is past [`MAX`](Self::MAX).
    ///
    /// Ids made from input go through this, so they are refused rather than
    /// truncated.
    fn try_new(index: usize) -> Option<Self>;

    /// The id naming `index`, which must be at most [`MAX`](Self::MAX).
    ///
    /// `index` is an id or length of a [`Table`](super::Table), or a count it
    /// bounds. A table holds at most `MAX` items, so its length is always an
    /// id.
    ///
    /// Never panics. Debug builds check the range; a failure is a bug in the
    /// crate. In release a value past `MAX` wraps. Values from input go
    /// through [`try_new`](Self::try_new).
    fn new(index: usize) -> Self;

    /// The index this id names.
    fn get(self) -> usize;
}

/// Defines an id newtype over an unsigned integer and implements [`Id`] for it.
///
/// ```ignore
/// define_id! {
///     /// Names a cluster.
///     pub(crate) struct ClusterId(u32);
/// }
/// ```
macro_rules! define_id {
    ($(#[$meta:meta])* $vis:vis struct $name:ident($repr:ty);) => {
        $(#[$meta])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        #[repr(transparent)]
        $vis struct $name($repr);

        impl $crate::data::Id for $name {
            const MAX: usize = <$repr>::MAX as usize;

            #[inline]
            fn try_new(index: usize) -> Option<Self> {
                (index <= Self::MAX).then(|| Self(index as $repr))
            }

            #[inline]
            fn new(index: usize) -> Self {
                debug_assert!(
                    index <= Self::MAX,
                    concat!(stringify!($name), " out of range")
                );
                Self(index as $repr)
            }

            #[inline]
            fn get(self) -> usize {
                self.0 as usize
            }
        }

        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0)
            }
        }
    };
}
pub(crate) use define_id;

/// Iteration over a range of ids.
///
/// `Range<I>` is not an iterator, since that needs the unstable `Step` trait.
pub(crate) trait IdRange<I> {
    /// Returns the ids in the range, in order.
    fn ids(self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator;
}

impl<I: Id> IdRange<I> for Range<I> {
    #[inline]
    fn ids(self) -> impl DoubleEndedIterator<Item = I> + ExactSizeIterator {
        (self.start.get()..self.end.get()).map(I::new)
    }
}

/// Packs an index or a count into a `u32`, saturating at `u32::MAX`.
///
/// No id is wider than `u32`, so only a count can saturate.
#[inline]
pub(crate) fn index_to_u32(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

/// Unpacks an index or a count packed by [`index_to_u32`].
///
/// Saturates where `usize` is narrower than `u32`; such a value is past the
/// end of any table.
#[inline]
pub(crate) fn u32_to_index(packed: u32) -> usize {
    usize::try_from(packed).unwrap_or(usize::MAX)
}
