//! Sets of flags, one bit or more each, over an unsigned integer.

/// Defines a set of flags over an unsigned integer: its flags, each a
/// constant of one bit or more, and what every set of flags is asked, which
/// [`NONE`] and four methods answer.
///
/// ```ignore
/// define_flags! {
///     /// What the shaped runs hold, which a later stage checks first.
///     pub(crate) struct ShapedFlags(u8) {
///         /// Some cluster is unsafe to break before.
///         pub(crate) const HAS_UNSAFE = 1 << 0;
///     }
/// }
/// ```
///
/// A type's own questions of its flags go in an `impl` of its own beside
/// it. What a type does not ask of its flags is not reported unused.
///
/// [`NONE`]: #associatedconstant.NONE
macro_rules! define_flags {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident($repr:ty) {
            $(
                $(#[$flag_meta:meta])*
                $flag_vis:vis const $flag:ident = $bits:expr;
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
        $vis struct $name($repr);

        impl $name {
            $(
                $(#[$flag_meta])*
                $flag_vis const $flag: Self = Self($bits);
            )*

            /// No flags.
            #[allow(dead_code)]
            pub(crate) const NONE: Self = Self(0);

            /// Whether every flag in `other` is set.
            #[allow(dead_code)]
            #[inline]
            pub(crate) const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            /// The flags set in either.
            #[allow(dead_code)]
            #[inline]
            pub(crate) const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }

            /// Sets the flags in `other`.
            #[allow(dead_code)]
            #[inline]
            pub(crate) fn insert(&mut self, other: Self) {
                self.0 |= other.0;
            }

            /// Sets the flags in `other` where `on`, and clears them
            /// otherwise.
            #[allow(dead_code)]
            #[inline]
            pub(crate) fn set(&mut self, other: Self, on: bool) {
                if on {
                    self.0 |= other.0;
                } else {
                    self.0 &= !other.0;
                }
            }
        }
    };
}
pub(crate) use define_flags;
