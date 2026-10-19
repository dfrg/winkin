//! Lengths that may be any `calc()`, and the host resolver for them.

use core::hash::{Hash, Hasher};
use core::mem;

use super::LengthPercentage;
#[cfg(doc)]
use crate::Context;
use crate::style::same::{Same, eq_and_hash_by_bits};

/// A host handle for a `calc()` value that is not linear in its basis.
///
/// The host assigns handles and resolves them through the [`CalcResolver`]
/// set with [`Context::set_calc_resolver`]. Two values with the same handle
/// must resolve alike. A handle may hold a pointer to the host's own value,
/// which must then outlive every layout built with it.
///
/// ```
/// use winkin::style::CalcHandle;
///
/// assert_eq!(CalcHandle::new(7).get(), 7);
/// ```
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct CalcHandle {
    /// The value's low and high halves. Two halves keep the alignment at
    /// four bytes, so a [`LengthPercentageOrCalc`] takes 12 bytes, not 16.
    halves: [u32; 2],
}

impl CalcHandle {
    /// Returns the handle with value `value`.
    pub const fn new(value: u64) -> Self {
        Self {
            halves: [value as u32, (value >> 32) as u32],
        }
    }

    /// Returns the value the handle was created with.
    pub const fn get(self) -> u64 {
        self.halves[0] as u64 | (self.halves[1] as u64) << 32
    }
}

/// Resolves the `calc()` values that styles hold as [`CalcHandle`]s.
///
/// Layout calls it as a basis becomes known: as a build or
/// [`Layout::measure`](crate::Layout::measure) resolves margins and padding,
/// and as line breaking resolves `text-indent`. The intrinsic sizes resolve
/// `text-indent` against a basis of zero. A value whose percentages
/// are linear in their basis, such as `calc(10px + 5%)`, needs no
/// resolver: give it as a [`LengthPercentage`].
pub trait CalcResolver: Send + Sync {
    /// Returns the value of `calc` in CSS pixels, its percentages taken of
    /// `basis` pixels.
    ///
    /// Clamp the result as the property requires before returning it.
    /// Layout clamps padding at zero itself. A NaN result counts as zero.
    fn resolve(&self, calc: CalcHandle, basis: f32) -> f32;
}

/// A `<length-percentage>`: a length plus a percentage, or any `calc()`.
///
/// Converts from a [`LengthPercentage`] with `into`.
#[derive(Copy, Clone, Debug)]
pub enum LengthPercentageOrCalc {
    /// A length plus a percentage of the basis.
    LengthPercentage(LengthPercentage),
    /// A `calc()` that is not linear in its basis, such as `max(20px, 10%)`.
    ///
    /// The [`CalcResolver`] set on the context resolves it. Without one, it
    /// resolves to zero.
    Calc(CalcHandle),
}

impl LengthPercentageOrCalc {
    /// Zero.
    pub const ZERO: Self = Self::LengthPercentage(LengthPercentage::ZERO);

    /// Returns a length of `px` pixels, with no percentage.
    pub const fn from_px(px: f32) -> Self {
        Self::LengthPercentage(LengthPercentage::from_px(px))
    }

    /// Returns whether it is zero at any basis.
    ///
    /// A `calc()` counts as nonzero.
    pub(crate) fn is_zero(self) -> bool {
        match self {
            Self::LengthPercentage(value) => value.is_zero(),
            Self::Calc(_) => false,
        }
    }

    /// Returns whether its length depends on the basis.
    pub(crate) fn has_percentage(self) -> bool {
        match self {
            Self::LengthPercentage(value) => value.fraction != 0.0,
            Self::Calc(_) => true,
        }
    }

    /// Returns the length in pixels, its percentages taken of `basis`
    /// pixels.
    ///
    /// `calc` resolves a `calc()`; without it, a `calc()` is zero. A NaN in
    /// a part or in `basis` gives a NaN, which the layout grid takes as
    /// zero.
    #[inline]
    pub(crate) fn resolve(self, basis: f32, calc: Option<&dyn CalcResolver>) -> f32 {
        match self {
            Self::LengthPercentage(value) => value.resolve(basis),
            Self::Calc(handle) => resolve_calc(handle, basis, calc),
        }
    }
}

/// Returns `handle` resolved by `calc` against `basis` pixels, or zero
/// without a resolver.
#[cold]
#[inline(never)]
fn resolve_calc(handle: CalcHandle, basis: f32, calc: Option<&dyn CalcResolver>) -> f32 {
    calc.map_or(0.0, |calc| calc.resolve(handle, basis))
}

impl Default for LengthPercentageOrCalc {
    fn default() -> Self {
        Self::ZERO
    }
}

impl From<LengthPercentage> for LengthPercentageOrCalc {
    fn from(value: LengthPercentage) -> Self {
        Self::LengthPercentage(value)
    }
}

impl From<CalcHandle> for LengthPercentageOrCalc {
    fn from(handle: CalcHandle) -> Self {
        Self::Calc(handle)
    }
}

impl Same for LengthPercentageOrCalc {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::LengthPercentage(a), Self::LengthPercentage(b)) => a.same(b),
            (Self::Calc(a), Self::Calc(b)) => a == b,
            _ => false,
        }
    }

    fn feed<H: Hasher>(&self, state: &mut H) {
        mem::discriminant(self).hash(state);
        match self {
            Self::LengthPercentage(value) => value.feed(state),
            Self::Calc(handle) => handle.hash(state),
        }
    }
}

eq_and_hash_by_bits!(LengthPercentageOrCalc);
