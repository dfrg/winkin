//! Fixed-point length types and the float roundings onto their grids.
//!
//! Glyph geometry is 16.16 [`TextUnit`], layout geometry is 26.6
//! [`LayoutUnit`], and running advances are 48.16 [`InlineLayoutUnit`].
//! These follow Blink's `TextRunLayoutUnit`, `LayoutUnit` and
//! `InlineLayoutUnit`, with Blink's rounding at each conversion.
//!
//! Lengths enter and leave as `f32`. Every float-to-integer conversion lives
//! here, each with the rounding its point of use takes. Index casts live in
//! `data`.

use core::fmt;
use core::ops::{Add, AddAssign, Neg, Rem, Sub};

/// Rounds `x` down to a whole number. Saturates at the ends of `i32`; NaN
/// gives zero.
///
/// `f32::floor` needs std, so this is written out.
#[inline]
pub(crate) fn floor(x: f32) -> i32 {
    let truncated = x as i32;
    if (truncated as f32) > x {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

/// Rounds `x` up to a whole count, as C's `ceil` does. Saturates; NaN and
/// negative values give zero.
#[inline]
pub(crate) fn ceil_count(x: f32) -> u32 {
    u32::try_from(floor(-x).saturating_neg()).unwrap_or(0)
}

/// Rounds `x` to the nearest whole number, halves away from zero, as C's
/// `lroundf` does.
#[inline]
fn round(x: f32) -> i32 {
    if x < 0.0 {
        floor(-x + 0.5).saturating_neg()
    } else {
        floor(x + 0.5)
    }
}

/// Floors `px` to a hundredth of a pixel, as Blink's
/// `FontDescription::EffectiveFontSize` does to key fonts by size.
/// Saturates; NaN gives zero.
#[inline]
pub(crate) fn floor_to_hundredth(px: f32) -> f32 {
    floor(px * 100.0) as f32 / 100.0
}

/// Rounds `px` to a whole pixel, halves away from zero, as C's `lroundf`
/// does. Blink sizes synthesized small capitals and emphasis marks this way.
/// Saturates; NaN gives zero.
#[inline]
pub(crate) fn round_to_whole(px: f32) -> f32 {
    round(px) as f32
}

/// Returns the length of the vector `(x, y)`, computed in `f64` and rounded
/// once to the nearest `f32`.
///
/// Infinite if a component is infinite; NaN if one is NaN. Paths measure
/// their segments with it.
///
/// `f32::sqrt` needs std, so this runs Newton's method. The first guess,
/// read off the square's exponent, is within a few per cent; four steps
/// reach `f64` precision.
#[inline]
pub(crate) fn hypot(x: f32, y: f32) -> f32 {
    let (x, y) = (f64::from(x), f64::from(y));
    let square = x * x + y * y;
    if square.is_nan() || square.is_infinite() || square <= 0.0 {
        // NaN, infinity and zero are their own roots.
        return square as f32;
    }
    // The first guess halves the exponent bits.
    let mut root = f64::from_bits((square.to_bits() >> 1) + (1023 << 51));
    for _ in 0..4 {
        root = 0.5 * (root + square / root);
    }
    root as f32
}

/// Converts a whole number of pixels, lines or font units to `f32`.
///
/// Exact below 2^24; rounds to the nearest float above it.
#[inline]
pub(crate) fn whole_to_f32(whole: impl Into<i64>) -> f32 {
    whole.into() as f32
}

/// Rounds `x` to the nearest whole number, halves away from zero. Saturates
/// at the ends of `i64`; NaN gives zero.
///
/// `f64::round` needs std, so this is written out.
#[inline]
fn round_wide(x: f64) -> i64 {
    let truncated = x as i64;
    let rest = x - truncated as f64;
    if rest >= 0.5 {
        truncated.saturating_add(1)
    } else if rest <= -0.5 {
        truncated.saturating_sub(1)
    } else {
        truncated
    }
}

/// A layout length: 26.6 fixed point, in 1/64 px.
///
/// Named and rounded as Blink's `LayoutUnit`. It differs from `Fixed`,
/// which in OpenType and read-fonts is 16.16.
///
/// Box, line and fitting widths use it. Float inputs round onto the grid
/// once, here.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct LayoutUnit(i32);

impl LayoutUnit {
    /// Zero length.
    pub(crate) const ZERO: Self = Self(0);

    /// The most negative length. Blink's `FontHeight::Empty()` fills both
    /// sides of an extent with it, as the identity for a union.
    pub(crate) const MIN: Self = Self(i32::MIN);

    /// The longest length, about 33.5 million pixels. Wider widths saturate
    /// here.
    #[cfg(test)]
    pub(crate) const MAX: Self = Self(i32::MAX);

    /// The smallest step, 1/64 px.
    ///
    /// Blink's `AddEpsilon` adds it to the available width
    /// (`AvailableWidthToFit`), so a line that rounds up past its band by at
    /// most this much still fits.
    pub(crate) const EPSILON: Self = Self(1);

    /// Rounds `px` to the nearest 1/64, halves away from zero, as
    /// `LayoutUnit::FromFloatRound` does. Saturates; NaN gives zero.
    #[inline]
    pub(crate) fn from_px(px: f32) -> Self {
        Self(round(px * 64.0))
    }

    /// Rounds `px` up to the next 1/64, as `LayoutUnit::FromFloatCeil` does.
    /// Saturates; NaN gives zero.
    #[inline]
    pub(crate) fn from_px_ceil(px: f32) -> Self {
        Self(floor(-(px * 64.0)).saturating_neg())
    }

    /// Truncates `px` toward zero onto the grid, as Blink's plain
    /// `LayoutUnit(float)` does.
    ///
    /// Blink converts these lengths this way:
    /// - a computed font size, as the length a numeric `line-height`
    ///   multiplies, and the product as a line height
    ///   (`MinimumValueForLength`);
    /// - `text-indent` and `line-padding`;
    /// - every length a box takes room by: its margin, border and padding,
    ///   and an atomic inline's or a float's size.
    ///
    /// So 10.012px is 10, 10.99px is 10.984375 and −10.012px is −10.
    /// Saturates; NaN gives zero.
    #[inline]
    pub(crate) fn from_px_truncated(px: f32) -> Self {
        Self((px * 64.0) as i32)
    }

    /// Halves the length, truncating toward zero, as Blink divides a
    /// `LayoutUnit` by two.
    #[inline]
    pub(crate) fn half(self) -> Self {
        Self(self.0 / 2)
    }

    /// Divides the length by `by`, truncating toward zero, as Blink divides
    /// a `LayoutUnit` by an integer.
    ///
    /// Superscript and subscript shifts use it. Returns zero when `by` is
    /// zero or the quotient overflows.
    #[inline]
    pub(crate) fn divided(self, by: i32) -> Self {
        Self(self.0.checked_div(by).unwrap_or(0))
    }

    /// Rounds down to a whole pixel, toward negative infinity, as Blink's
    /// `LayoutUnit::Floor()` does. A negative half pixel becomes −1 px.
    #[inline]
    pub(crate) fn floor_px(self) -> Self {
        Self((self.0 >> 6).saturating_mul(64))
    }

    /// Rounds up to a whole pixel, toward positive infinity, as Blink's
    /// `LayoutUnit::Ceil()` does.
    #[inline]
    pub(crate) fn ceil_px(self) -> Self {
        -((-self).floor_px())
    }

    /// Rounds `px` to a whole pixel, halves up, as Skia rounds font metrics
    /// (`SkScalarRoundToScalar`, `floor(x + 0.5)`). Saturates; NaN gives zero.
    #[inline]
    pub(crate) fn from_px_whole(px: f32) -> Self {
        Self(floor(px + 0.5).saturating_mul(64))
    }

    /// Multiplies the length by `count`, as Blink multiplies a `LayoutUnit`
    /// by an integer. Exact; saturates.
    #[inline]
    pub(crate) fn times_whole(self, count: u32) -> Self {
        let count = i32::try_from(count).unwrap_or(i32::MAX);
        Self(self.0.saturating_mul(count))
    }

    /// Multiplies the length by `factor` and truncates onto the grid, as
    /// Blink's `LayoutUnit(length * factor)` does. Saturates; NaN gives zero.
    #[inline]
    pub(crate) fn times_truncated(self, factor: f32) -> Self {
        Self::from_px_truncated(self.to_px() * factor)
    }

    #[inline]
    pub(crate) fn to_px(self) -> f32 {
        self.0 as f32 / 64.0
    }
}

impl Neg for LayoutUnit {
    type Output = Self;

    /// Negates the length, saturating.
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

impl Add for LayoutUnit {
    type Output = Self;

    /// Adds, saturating as Blink's `LayoutUnit` does. Input lengths can
    /// reach the ends, and must not panic.
    #[inline]
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl Sub for LayoutUnit {
    type Output = Self;

    /// Subtracts, saturating.
    #[inline]
    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl fmt::Debug for LayoutUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.to_px())
    }
}

/// A glyph-geometry length: 16.16 fixed point, in 1/65536 px.
///
/// Shaping runs at harfrust scale `size × 65536`, as Blink sets up
/// HarfBuzz. Font sizes, glyph advances and glyph offsets use it.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct TextUnit(i32);

impl TextUnit {
    /// Truncates `px` toward zero onto the grid, as Blink turns a font size
    /// into a HarfBuzz scale (`SkiaScalarToHarfBuzzPosition`, a clamped
    /// `static_cast`). Saturates; NaN gives zero.
    #[inline]
    pub(crate) fn from_px_truncated(px: f32) -> Self {
        Self((px * 65536.0) as i32)
    }

    /// Rounds `px` to the nearest 1/65536, halves away from zero, as Skia
    /// passes a glyph advance to HarfBuzz (`SkScalarToHarfBuzzPosition`).
    /// Table-read advances become glyph advances this way. Saturates; NaN
    /// gives zero.
    #[inline]
    pub(crate) fn from_px(px: f32) -> Self {
        let units = round_wide(f64::from(px) * 65536.0);
        Self(i32::try_from(units).unwrap_or(if units < 0 { i32::MIN } else { i32::MAX }))
    }

    #[inline]
    pub(crate) fn to_px(self) -> f32 {
        self.0 as f32 / 65536.0
    }

    /// Returns the count of 1/65536 px, as harfrust takes scales and gives
    /// positions.
    ///
    /// A font size's raw value is the scale that makes shaper positions come
    /// out in these units.
    #[inline]
    pub(crate) const fn raw(self) -> i32 {
        self.0
    }

    /// Wraps a shaper position in 1/65536 px.
    #[inline]
    pub(crate) const fn from_raw(raw: i32) -> Self {
        Self(raw)
    }
}

impl fmt::Debug for TextUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.to_px())
    }
}

/// A running advance: 48.16 fixed point in an `i64`, in 1/65536 px.
///
/// Blink's `InlineLayoutUnit`, for run and line advances. Glyph advances are
/// [`TextUnit`]s, so sums of them are exact. Paragraph prefix advances use
/// it, so a line's width is one exact subtraction.
///
/// No paragraph reaches the 48-bit range, but sums still saturate because
/// input lengths flow into them and must not panic.
///
/// Line fitting reads positions rounded up to the 1/64 grid, as Blink's
/// cached character positions are (`ToCeil<LayoutUnit>`); see
/// [`ceil_to_grid`](Self::ceil_to_grid).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub(crate) struct InlineLayoutUnit(i64);

impl InlineLayoutUnit {
    /// Zero length.
    pub(crate) const ZERO: Self = Self(0);

    /// Wraps a count of 1/65536 px, such as a sum of shaper positions.
    #[inline]
    pub(crate) const fn from_raw(raw: i64) -> Self {
        Self(raw)
    }

    /// Returns the count of 1/65536 px.
    #[inline]
    pub(crate) const fn raw(self) -> i64 {
        self.0
    }

    /// Converts `length` exactly; 1/64 px is 1024 units.
    #[inline]
    pub(crate) fn from_layout(length: LayoutUnit) -> Self {
        Self(i64::from(length.0) << 10)
    }

    #[inline]
    pub(crate) fn from_text(length: TextUnit) -> Self {
        Self(i64::from(length.0))
    }

    /// Rounds `px` to the nearest 1/65536, halves away from zero. Used for
    /// input lengths such as a length `tab-size`. Saturates; NaN gives zero.
    #[inline]
    pub(crate) fn from_px(px: f32) -> Self {
        Self(round_wide(f64::from(px) * 65536.0))
    }

    /// Multiplies by `factor`, rounding to the nearest 1/65536, as for a
    /// `tab-size` given in spaces. Saturates; a NaN factor gives zero.
    #[inline]
    pub(crate) fn times(self, factor: f32) -> Self {
        Self(round_wide(self.0 as f64 * f64::from(factor)))
    }

    /// Halves the length, truncating toward zero.
    #[inline]
    pub(crate) fn half(self) -> Self {
        Self(self.0 / 2)
    }

    /// Rounds up to the 1/64 grid, as Blink rounds running sums into the
    /// character positions it fits lines with (`LayoutUnit::FromFloatCeil`,
    /// `ToCeil<LayoutUnit>`).
    ///
    /// The result stays in this type, so positions beyond the `LayoutUnit`
    /// range keep their value.
    #[inline]
    pub(crate) fn ceil_to_grid(self) -> Self {
        let whole = (self.0 >> 10) + i64::from(self.0 & 1023 != 0);
        Self(whole.saturating_mul(1024))
    }

    /// Converts to a [`LayoutUnit`], rounding up to 1/64 and saturating.
    #[inline]
    pub(crate) fn to_layout(self) -> LayoutUnit {
        let whole = (self.0 >> 10) + i64::from(self.0 & 1023 != 0);
        LayoutUnit(i32::try_from(whole).unwrap_or(if whole < 0 { i32::MIN } else { i32::MAX }))
    }

    /// Returns [`to_layout`](Self::to_layout) and the amount the rounding
    /// added, in 1/65536 px.
    ///
    /// The amount is below 1/64 px, so it fits in ten bits. Outside the
    /// `LayoutUnit` range the length saturates and the split is inexact.
    #[inline]
    pub(crate) fn to_layout_and_short(self) -> (LayoutUnit, u16) {
        // The distance up to the next multiple of 1024; zero on the grid.
        let short = u16::try_from(self.0.wrapping_neg() & 1023).unwrap_or(0);
        (self.to_layout(), short)
    }

    /// Returns `length` less `short` 1/65536 px, inverting
    /// [`to_layout_and_short`](Self::to_layout_and_short).
    #[inline]
    pub(crate) fn from_layout_less(length: LayoutUnit, short: u16) -> Self {
        Self(Self::from_layout(length).0 - i64::from(short))
    }

    #[inline]
    pub(crate) fn to_px(self) -> f32 {
        (self.0 as f64 / 65536.0) as f32
    }
}

impl Add for InlineLayoutUnit {
    type Output = Self;

    /// Adds, saturating.
    #[inline]
    fn add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }
}

impl AddAssign for InlineLayoutUnit {
    #[inline]
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sub for InlineLayoutUnit {
    type Output = Self;

    /// Subtracts, saturating.
    #[inline]
    fn sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl Neg for InlineLayoutUnit {
    type Output = Self;

    /// Negates the length, saturating.
    #[inline]
    fn neg(self) -> Self {
        Self(self.0.saturating_neg())
    }
}

impl Rem for InlineLayoutUnit {
    type Output = Self;

    /// Returns the remainder, truncated toward zero like C's `fmod`, so it
    /// takes the sign of `self`.
    ///
    /// Blink finds a position's distance past a tab stop this way. Returns
    /// zero when `other` is zero or the division overflows.
    #[inline]
    fn rem(self, other: Self) -> Self {
        Self(self.0.checked_rem(other.0).unwrap_or(0))
    }
}

impl fmt::Debug for InlineLayoutUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0 as f64 / 65536.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Floats round onto each grid as Blink's conversions do; NaN and
    /// infinities never panic.
    #[test]
    fn lengths_round_onto_their_grids() {
        assert_eq!(LayoutUnit::from_px(1.0).to_px(), 1.0);
        // Halves away from zero on the 1/64 grid, as `lroundf`.
        assert_eq!(LayoutUnit::from_px(0.5 / 64.0), LayoutUnit(1));
        assert_eq!(LayoutUnit::from_px(-0.5 / 64.0), LayoutUnit(-1));
        assert_eq!(LayoutUnit::from_px(1.0 / 256.0), LayoutUnit(0));
        // Whole pixels, halves up, as Skia rounds font metrics. Times at
        // 30 px has a 34.5 px line; Chrome steps 35.
        assert_eq!(LayoutUnit::from_px_whole(34.5), LayoutUnit::from_px(35.0));
        assert_eq!(LayoutUnit::from_px_whole(34.49), LayoutUnit::from_px(34.0));
        assert_eq!(LayoutUnit::from_px_whole(-0.5), LayoutUnit::ZERO);
        assert_eq!(LayoutUnit::from_px_whole(-0.51), LayoutUnit::from_px(-1.0));
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
            let _ = (LayoutUnit::from_px(bad), LayoutUnit::from_px_whole(bad));
            let _ = TextUnit::from_px_truncated(bad);
        }
        assert_eq!(LayoutUnit::from_px(f32::NAN), LayoutUnit::ZERO);
        assert_eq!(LayoutUnit::from_px(f32::INFINITY), LayoutUnit(i32::MAX));
        // A font size truncates, as Blink's HarfBuzz scale does.
        assert_eq!(TextUnit::from_px_truncated(1.5), TextUnit(98_304));
        assert_eq!(TextUnit::from_px_truncated(-1.5), TextUnit(-98_304));
        assert_eq!(TextUnit::from_px_truncated(1.0 / 131_072.0), TextUnit(0));
        assert_eq!(TextUnit::from_px_truncated(f32::NAN), TextUnit(0));
    }

    /// Running advances stay exact at 16.16 beyond the 26.6 range, and round
    /// up onto the 1/64 grid as Blink's fitting positions do.
    #[test]
    fn running_advances_round_up_onto_the_grid() {
        let unit = InlineLayoutUnit::from_raw(1);
        assert_eq!(unit.ceil_to_grid(), InlineLayoutUnit::from_raw(1024));
        assert_eq!(unit.to_layout(), LayoutUnit(1));
        let on = InlineLayoutUnit::from_layout(LayoutUnit(-3));
        assert_eq!(on.raw(), -3 * 1024);
        assert_eq!(on.ceil_to_grid(), on, "a position on the grid stays");
        // Up is toward positive infinity: -1.5/64 px rounds to -1/64.
        let negative = InlineLayoutUnit::from_raw(-1536);
        assert_eq!(negative.ceil_to_grid().raw(), -1024);
        assert_eq!(negative.to_layout(), LayoutUnit(-1));
        // Beyond the 26.6 range the sum stays exact; only the width
        // saturates.
        let far = InlineLayoutUnit::from_text(TextUnit(i32::MAX)) + InlineLayoutUnit::from_raw(1);
        assert_eq!(far.raw(), i64::from(i32::MAX) + 1);
        let wide = InlineLayoutUnit::from_raw(i64::from(i32::MAX) << 12);
        assert_eq!(wide.to_layout(), LayoutUnit::MAX);
        assert_eq!((-wide.ceil_to_grid()).to_layout(), LayoutUnit::MIN);
        // Nothing overflows.
        let top = InlineLayoutUnit::from_raw(i64::MAX);
        assert_eq!(top + top, top);
        let _ = (top.ceil_to_grid(), top.to_layout(), top.to_px());
    }

    /// Splitting a running advance into a grid length and the rounding
    /// remainder round-trips within the 26.6 range; the ends never panic.
    #[test]
    fn a_running_advance_splits_and_comes_back() {
        for raw in [0, 1, 1023, 1024, 1025, -1, -1023, -1024, -1536, 123_456_789] {
            let advance = InlineLayoutUnit::from_raw(raw);
            let (layout, short) = advance.to_layout_and_short();
            assert_eq!(layout, advance.to_layout());
            assert!(short < 1024);
            assert_eq!(InlineLayoutUnit::from_layout_less(layout, short), advance);
        }
        for raw in [i64::MAX, i64::MIN] {
            let (layout, short) = InlineLayoutUnit::from_raw(raw).to_layout_and_short();
            let _ = InlineLayoutUnit::from_layout_less(layout, short);
        }
    }

    /// Running advance arithmetic:
    /// - pixel lengths round to the nearest 1/65536, halves away from zero;
    /// - multiples round the same way;
    /// - remainders truncate toward zero, as `fmod` does;
    /// - range ends and NaN never panic.
    #[test]
    fn running_advances_scale_and_divide_as_chromes_floats_do() {
        assert_eq!(TextUnit::from_px(7.5), TextUnit(491_520));
        assert_eq!(TextUnit::from_px(0.5 / 65536.0), TextUnit(1));
        assert_eq!(TextUnit::from_px(-0.5 / 65536.0), TextUnit(-1));
        assert_eq!(TextUnit::from_px(f32::NAN), TextUnit(0));
        assert_eq!(TextUnit::from_px(f32::INFINITY), TextUnit(i32::MAX));
        assert_eq!(TextUnit::from_px(f32::NEG_INFINITY), TextUnit(i32::MIN));
        assert_eq!(InlineLayoutUnit::from_px(60.0).raw(), 60 << 16);
        assert_eq!(InlineLayoutUnit::from_px(f32::MAX).raw(), i64::MAX);
        assert_eq!(InlineLayoutUnit::from_px(f32::NAN).raw(), 0);
        let space = InlineLayoutUnit::from_px(7.5);
        assert_eq!(space.times(8.0), InlineLayoutUnit::from_px(60.0));
        assert_eq!(space.times(1e30).raw(), i64::MAX);
        assert_eq!(space.times(f32::NAN), InlineLayoutUnit::ZERO);
        assert_eq!(space.half(), InlineLayoutUnit::from_px(3.75));
        let interval = InlineLayoutUnit::from_px(60.0);
        assert_eq!(
            InlineLayoutUnit::from_px(135.0) % interval,
            InlineLayoutUnit::from_px(15.0)
        );
        assert_eq!(
            InlineLayoutUnit::from_px(-15.0) % interval,
            InlineLayoutUnit::from_px(-15.0)
        );
        assert_eq!(interval % InlineLayoutUnit::ZERO, InlineLayoutUnit::ZERO);
        let low = InlineLayoutUnit::from_raw(i64::MIN);
        assert_eq!(low % InlineLayoutUnit::from_raw(-1), InlineLayoutUnit::ZERO);
    }

    /// Half-leading halves toward zero, then floors to a whole pixel, as Blink's
    /// `LayoutUnit` does.
    #[test]
    fn a_length_halves_and_floors_as_chromes_does() {
        assert_eq!(LayoutUnit(-3).half(), LayoutUnit(-1));
        assert_eq!(LayoutUnit(3 * 64).half().floor_px(), LayoutUnit(64));
        assert_eq!(LayoutUnit(-96).floor_px(), LayoutUnit(-128));
        assert_eq!(LayoutUnit::from_px_truncated(1.0 / 128.0), LayoutUnit(0));
        assert_eq!(LayoutUnit::from_px_truncated(-1.99 / 64.0), LayoutUnit(-1));
        assert_eq!(LayoutUnit::MAX + LayoutUnit(1), LayoutUnit::MAX);
        assert_eq!(LayoutUnit::MIN - LayoutUnit(1), LayoutUnit::MIN);
    }

    /// Vector lengths round to the nearest `f32` across the range:
    /// - Pythagorean triples are exact and diagonals match to the last bit;
    /// - vectors whose square overflows or underflows `f32` still measure;
    /// - zero is zero, and NaN and infinities never panic.
    #[test]
    fn a_vector_is_measured_to_the_nearest_float() {
        use core::f32::consts::SQRT_2;
        use core::f64::consts::SQRT_2 as SQRT_2_WIDE;
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(hypot(-5.0, 12.0), 13.0);
        assert_eq!(hypot(1.0, 1.0), SQRT_2);
        assert_eq!(hypot(0.0, 0.0), 0.0);
        assert_eq!(hypot(0.0, -7.25), 7.25);
        assert_eq!(hypot(3e30, 4e30), 5e30);
        assert_eq!(hypot(3e-30, 4e-30), 5e-30);
        assert_eq!(hypot(f32::MAX, f32::MAX), f32::INFINITY);
        assert_eq!(hypot(f32::INFINITY, 0.0), f32::INFINITY);
        assert!(hypot(f32::NAN, 1.0).is_nan());
        for n in 1..2000u16 {
            let side = f32::from(n) * 0.37;
            let exact = f64::from(side) * SQRT_2_WIDE;
            assert_eq!(hypot(side, side), exact as f32, "{side}");
        }
    }
}
