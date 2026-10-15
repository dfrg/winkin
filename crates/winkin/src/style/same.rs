//! Style equality and hashing, by each value's bits.
//!
//! The builder interns the facts a style lowers into, and its memo finds a
//! repeated style by its bits. A hash table needs an equality that is an
//! equivalence and a hash that agrees with it. `f32`'s `==` is neither: NaN
//! is unequal to itself, and `0.0 == -0.0` with different bits. So every
//! style value compares and hashes by its bits through [`Same`], and the
//! style groups' `PartialEq`, `Eq` and `Hash` use it. Two styles that differ
//! only in a zero's sign make two keys, which costs one more lowering.
//!
//! A borrowed list or string compares and hashes by content. The builder's
//! memo never hashes one: it keys a style by the ids its lists were
//! interned to.
//!
//! Several of parlance's types, such as weights and wrapping keywords, have
//! no `Hash` or `Eq`. [`Same`] is the crate's own trait so this file can
//! implement it for them.

use core::hash::{Hash, Hasher};
use core::mem;

use parlance::{
    BaseDirection, FontFamilyName, FontFeature, FontStyle, FontVariation, FontWeight, FontWidth,
    GenericFamily, Language, OverflowWrap, Tag, TextWrapMode,
};

/// Equality and hashing by bits, for the values a style holds.
pub(crate) trait Same {
    /// Returns whether the two hold the same bits.
    fn same(&self, other: &Self) -> bool;

    /// Feeds the bits to `state`, agreeing with [`same`](Self::same).
    fn feed<H: Hasher>(&self, state: &mut H);
}

/// Implements [`Same`] for types whose own `Eq` and `Hash` already compare
/// and hash every bit that matters: integers, flags, and enums of them.
macro_rules! same_by_value {
    ($($ty:ty),* $(,)?) => {$(
        impl $crate::style::Same for $ty {
            #[inline]
            fn same(&self, other: &Self) -> bool {
                self == other
            }

            #[inline]
            fn feed<H: core::hash::Hasher>(&self, state: &mut H) {
                core::hash::Hash::hash(self, state);
            }
        }
    )*};
}
pub(crate) use same_by_value;

/// Writes `PartialEq`, `Eq` and `Hash` for a type in terms of its [`Same`], so
/// hosts can compare a style's parts and put them in hash tables.
macro_rules! eq_and_hash_by_bits {
    ($($ty:ty),* $(,)?) => {$(
        impl PartialEq for $ty {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                $crate::style::Same::same(self, other)
            }
        }

        impl Eq for $ty {}

        impl core::hash::Hash for $ty {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                $crate::style::Same::feed(self, state);
            }
        }
    )*};
}
pub(crate) use eq_and_hash_by_bits;

/// Defines a struct whose fields are all [`Same`], and implements `Same`,
/// `PartialEq`, `Eq` and `Hash` for it field by field.
///
/// The fields are listed once, in the definition. A field added to a style
/// group is compared and hashed with no second list to keep in step. The
/// struct may take one lifetime, for the lists and strings it borrows.
macro_rules! style_struct {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident $(<$lt:lifetime>)? {
            $(
                $(#[$field_meta:meta])*
                $field_vis:vis $field:ident: $ty:ty,
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Debug)]
        $vis struct $name $(<$lt>)? {
            $(
                $(#[$field_meta])*
                $field_vis $field: $ty,
            )*
        }

        impl $(<$lt>)? $crate::style::Same for $name $(<$lt>)? {
            #[inline]
            fn same(&self, other: &Self) -> bool {
                true $(&& $crate::style::Same::same(&self.$field, &other.$field))*
            }

            #[inline]
            fn feed<H: core::hash::Hasher>(&self, state: &mut H) {
                $($crate::style::Same::feed(&self.$field, state);)*
            }
        }

        impl $(<$lt>)? PartialEq for $name $(<$lt>)? {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                $crate::style::Same::same(self, other)
            }
        }

        impl $(<$lt>)? Eq for $name $(<$lt>)? {}

        impl $(<$lt>)? core::hash::Hash for $name $(<$lt>)? {
            #[inline]
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                $crate::style::Same::feed(self, state);
            }
        }
    };
}
pub(crate) use style_struct;

impl Same for f32 {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self.to_bits() == other.to_bits()
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        state.write_u32(self.to_bits());
    }
}

same_by_value!(bool, u8, u16, u32, Tag, Language, GenericFamily);

/// Implements [`Same`] for parlance's keyword enums by their discriminant.
///
/// They have `Eq` but no `Hash`, and their discriminant is every bit they
/// hold.
macro_rules! same_by_discriminant {
    ($($ty:ty),* $(,)?) => {$(
        impl Same for $ty {
            #[inline]
            fn same(&self, other: &Self) -> bool {
                self == other
            }

            #[inline]
            fn feed<H: Hasher>(&self, state: &mut H) {
                core::mem::discriminant(self).hash(state);
            }
        }
    )*};
}

same_by_discriminant!(OverflowWrap, TextWrapMode, BaseDirection);

impl Same for FontWeight {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self.value().same(&other.value())
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        self.value().feed(state);
    }
}

impl Same for FontWidth {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self.ratio().same(&other.ratio())
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        self.ratio().feed(state);
    }
}

impl Same for FontStyle {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Normal, Self::Normal) | (Self::Italic, Self::Italic) => true,
            (Self::Oblique(a), Self::Oblique(b)) => a.same(b),
            _ => false,
        }
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        if let Self::Oblique(angle) = self {
            angle.feed(state);
        }
    }
}

impl Same for FontFeature {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self == other
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        state.write_u16(self.value);
    }
}

impl Same for FontVariation {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self.tag == other.tag && self.value.same(&other.value)
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        self.tag.hash(state);
        self.value.feed(state);
    }
}

impl<T: Same> Same for Option<T> {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Some(a), Some(b)) => a.same(b),
            (None, None) => true,
            _ => false,
        }
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        match self {
            Some(value) => {
                state.write_u8(1);
                value.feed(state);
            }
            None => state.write_u8(0),
        }
    }
}

impl<T: Same> Same for [T] {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().zip(other).all(|(a, b)| a.same(b))
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len());
        for value in self {
            value.feed(state);
        }
    }
}

impl<T: Same + ?Sized> Same for &T {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        (**self).same(*other)
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        (**self).feed(state);
    }
}

impl Same for str {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self == other
    }

    #[inline]
    fn feed<H: Hasher>(&self, state: &mut H) {
        self.hash(state);
    }
}

impl Same for FontFamilyName<'_> {
    #[inline]
    fn same(&self, other: &Self) -> bool {
        self == other
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Named(name) => {
                state.write_u8(0);
                name.hash(state);
            }
            Self::Generic(generic) => {
                state.write_u8(1);
                generic.hash(state);
            }
        }
    }
}
