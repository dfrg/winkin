//! Polyline and cubic Bézier paths without a curve-library dependency.
//!
//! Arc lengths are measured into supplied buffers, so neither path allocates.
//! Distance queries use binary search over corners or curve samples. Reuse
//! measurements with `from_measured` when drawing a path again.
//!
//! Short buffers include only the content they can hold. Nonfinite points
//! contribute no length or rotation. A zero-length path is a point with
//! a rightward tangent. Invalid input does not panic.

use super::{PagePoint, PathPoint, TextPath};
use crate::data::index_to_u32;
use crate::unit::{hypot, whole_to_f32};

/// The tangent of a zero-length path: rightward, as horizontal text runs.
const RIGHTWARD: (f32, f32) = (1.0, 0.0);

/// A path through points, straight from each to the next.
///
/// ```
/// use winkin::path::{PagePoint, Polyline, TextPath};
///
/// let points = [
///     PagePoint::new(0.0, 0.0),
///     PagePoint::new(30.0, 40.0),
///     PagePoint::new(30.0, 100.0),
/// ];
/// let mut ends = [0.0; 3];
/// let path = Polyline::new(&points, &mut ends);
/// assert_eq!(path.length(), 110.0);
/// let at = path.at(80.0);
/// assert_eq!((at.x, at.y, at.dx, at.dy), (30.0, 70.0, 0.0, 1.0));
/// ```
#[derive(Copy, Clone, Debug)]
pub struct Polyline<'a> {
    points: &'a [PagePoint],
    /// Arc length at each point: zero at the first, the path length at the
    /// last.
    ends: &'a [f32],
}

impl<'a> Polyline<'a> {
    /// Creates a polyline through `points`, storing arc lengths in `ends`.
    ///
    /// Includes only as many points as the buffer can hold.
    pub fn new(points: &'a [PagePoint], ends: &'a mut [f32]) -> Self {
        let mut total = 0.0;
        let mut previous: Option<PagePoint> = None;
        for (end, &point) in ends.iter_mut().zip(points) {
            if let Some(PagePoint { x, y }) = previous {
                total += step(hypot(point.x - x, point.y - y));
            }
            *end = total;
            previous = Some(point);
        }
        Self::from_measured(points, ends)
    }

    /// Creates a polyline from arc lengths previously computed by [`new`](Self::new).
    ///
    /// Reuses `ends` without measuring again.
    pub fn from_measured(points: &'a [PagePoint], ends: &'a [f32]) -> Self {
        let count = points.len().min(ends.len());
        Self {
            points: points.get(..count).unwrap_or_default(),
            ends: ends.get(..count).unwrap_or_default(),
        }
    }
}

impl TextPath for Polyline<'_> {
    #[inline]
    fn length(&self) -> f32 {
        self.ends.last().copied().unwrap_or(0.0)
    }

    #[inline]
    fn at(&self, distance: f32) -> PathPoint {
        let Some(segment) = reaching(self.ends, distance) else {
            let PagePoint { x, y } = self.points.first().copied().unwrap_or_default();
            return still(x, y);
        };
        let (
            Some(&PagePoint { x: x0, y: y0 }),
            Some(&PagePoint { x: x1, y: y1 }),
            Some(&from),
            Some(&to),
        ) = (
            self.points.get(segment - 1),
            self.points.get(segment),
            self.ends.get(segment - 1),
            self.ends.get(segment),
        )
        else {
            return still(0.0, 0.0);
        };
        let length = to - from;
        let t = ((distance - from) / length).clamp(0.0, 1.0);
        PathPoint {
            x: x0 + (x1 - x0) * t,
            y: y0 + (y1 - y0) * t,
            dx: (x1 - x0) / length,
            dy: (y1 - y0) / length,
        }
    }
}

/// A sequence of cubic Bézier segments.
///
/// Supply a start point followed by two control points and an endpoint per
/// segment, matching SVG `C`. Incomplete trailing segments are ignored.
/// A straight segment has its endpoints as control points. Convert quadratic
/// `p0, c, p1` to cubic controls `p0 + ⅔(c − p0)` and `p1 + ⅔(c − p1)`.
///
/// Arc length uses evenly spaced parameter samples from the supplied buffer.
/// Sixteen samples per segment approximate a quarter-circle length within
/// one thousandth; more improve accuracy. Distances interpolate between
/// samples, while positions and tangents are evaluated on the curve.
///
/// ```
/// use winkin::path::{BezierPath, PagePoint, TextPath};
///
/// // A quarter of a circle of radius 100, as one cubic.
/// let k = 55.228_475;
/// let points = [
///     PagePoint::new(100.0, 0.0),
///     PagePoint::new(100.0, k),
///     PagePoint::new(k, 100.0),
///     PagePoint::new(0.0, 100.0),
/// ];
/// let mut table = [0.0; 33];
/// let path = BezierPath::new(&points, &mut table);
/// let quarter = core::f32::consts::FRAC_PI_2 * 100.0;
/// assert!((path.length() - quarter).abs() < 0.05);
/// let middle = path.at(path.length() / 2.0);
/// assert!((middle.x - middle.y).abs() < 1e-3 && (middle.dx + middle.dy).abs() < 1e-3);
/// ```
#[derive(Copy, Clone, Debug)]
pub struct BezierPath<'a> {
    points: &'a [PagePoint],
    /// Arc length at each sample: zero at the first, the path length at the
    /// last.
    table: &'a [f32],
    /// Samples per segment, counted from its start.
    per: usize,
}

impl<'a> BezierPath<'a> {
    /// Creates a cubic Bézier path, storing sampled arc lengths in `table`.
    ///
    /// Distributes available samples across complete segments. If the buffer
    /// cannot hold one sample per segment, measures as many segments as it
    /// can using their chords.
    pub fn new(points: &'a [PagePoint], table: &'a mut [f32]) -> Self {
        let (segments, per) = sampling(points.len(), table.len());
        let path = Self {
            points,
            table: &[],
            per,
        };
        let mut total = 0.0;
        let mut previous: Option<PagePoint> = None;
        let samples = segments * per + usize::from(segments > 0);
        for (sample, entry) in table.iter_mut().take(samples).enumerate() {
            let point = path.sample(sample);
            if let Some(before) = previous {
                total += step(hypot(point.x - before.x, point.y - before.y));
            }
            *entry = total;
            previous = Some(point);
        }
        Self::from_measured(points, table)
    }

    /// Creates a path from arc lengths previously computed by [`new`](Self::new).
    ///
    /// Reuses `table` without measuring again.
    pub fn from_measured(points: &'a [PagePoint], table: &'a [f32]) -> Self {
        let (segments, per) = sampling(points.len(), table.len());
        let samples = segments * per + usize::from(segments > 0);
        Self {
            points,
            table: table.get(..samples).unwrap_or_default(),
            per,
        }
    }

    /// Returns the start, control points and end of `segment`, or `None` past
    /// the last whole segment.
    fn segment(&self, segment: usize) -> Option<[PagePoint; 4]> {
        let first = segment * 3;
        match self.points.get(first..first + 4)? {
            &[p0, c1, c2, p1] => Some([p0, c1, c2, p1]),
            _ => None,
        }
    }

    /// Returns the segment `sample` lies on and its parameter there.
    fn parameter(&self, sample: usize) -> (usize, f32) {
        let per = self.per.max(1);
        let (segment, step) = (sample / per, sample % per);
        // The last sample is the last segment's end, not the next's start.
        if step == 0 && segment > 0 && self.segment(segment).is_none() {
            return (segment - 1, 1.0);
        }
        (segment, fraction(step, per))
    }

    /// Returns the point at `sample`.
    fn sample(&self, sample: usize) -> PagePoint {
        let (segment, t) = self.parameter(sample);
        self.segment(segment)
            .map_or(PagePoint::new(0.0, 0.0), |points| cubic(points, t))
    }
}

impl TextPath for BezierPath<'_> {
    #[inline]
    fn length(&self) -> f32 {
        self.table.last().copied().unwrap_or(0.0)
    }

    #[inline]
    fn at(&self, distance: f32) -> PathPoint {
        let Some(sample) = reaching(self.table, distance) else {
            let PagePoint { x, y } = self.points.first().copied().unwrap_or_default();
            return still(x, y);
        };
        let (Some(&from), Some(&to)) = (self.table.get(sample - 1), self.table.get(sample)) else {
            return still(0.0, 0.0);
        };
        // Interpolates the parameter between the two samples.
        let (segment, t0) = self.parameter(sample - 1);
        let (_, t1) = self.parameter(sample);
        let t1 = if t1 <= t0 { 1.0 } else { t1 };
        let along = ((distance - from) / (to - from)).clamp(0.0, 1.0);
        let t = t0 + (t1 - t0) * along;
        let Some(points) = self.segment(segment) else {
            return still(0.0, 0.0);
        };
        let PagePoint { x, y } = cubic(points, t);
        // Uses the derivative, or the chord between the samples where the
        // derivative vanishes (a cusp, or a control point on an endpoint).
        let (mut dx, mut dy) = derivative(points, t);
        let mut length = hypot(dx, dy);
        if !(length > 0.0 && length.is_finite()) {
            let (start, end) = (cubic(points, t0), cubic(points, t1));
            (dx, dy, length) = (end.x - start.x, end.y - start.y, to - from);
        }
        PathPoint {
            x,
            y,
            dx: dx / length,
            dy: dy / length,
        }
    }
}

/// Returns how many whole segments `points` points make and how many
/// samples each gets from a `table`-entry table, after the start sample.
///
/// A table too short for one sample per segment measures as many segments
/// as fit, one sample (the chord) each.
fn sampling(points: usize, table: usize) -> (usize, usize) {
    let segments = points.saturating_sub(1) / 3;
    let room = table.saturating_sub(1);
    match segments.min(room) {
        0 => (0, 0),
        whole if whole < segments => (whole, 1),
        _ => (segments, room / segments),
    }
}

/// Returns the index in `ends` that ends the piece of path reaching
/// `distance`.
///
/// `ends` holds the arc length at each point. The index is the first entry
/// at or past `distance`, never 0, moved to a piece with nonzero length.
/// Returns `None` where no piece has length.
#[inline]
fn reaching(ends: &[f32], distance: f32) -> Option<usize> {
    let last = ends.len().checked_sub(1).filter(|&last| last > 0)?;
    let found = ends.partition_point(|&end| end < distance).clamp(1, last);
    let long = |at: &usize| matches!((ends.get(at - 1), ends.get(*at)), (Some(from), Some(to)) if to > from);
    // Coincident points make a piece with no length or direction: take the
    // next piece with length, else the last one before.
    (found..=last)
        .find(long)
        .or_else(|| (1..found).rev().find(long))
}

/// Returns `length`, or zero where it is not finite, so a nonfinite point
/// adds no length.
fn step(length: f32) -> f32 {
    if length.is_finite() { length } else { 0.0 }
}

/// Returns the point of a zero-length path at `(x, y)`, with a rightward
/// tangent.
#[inline]
fn still(x: f32, y: f32) -> PathPoint {
    PathPoint {
        x,
        y,
        dx: RIGHTWARD.0,
        dy: RIGHTWARD.1,
    }
}

/// Returns `step / of` as a parameter.
fn fraction(step: usize, of: usize) -> f32 {
    whole_to_f32(index_to_u32(step)) / whole_to_f32(index_to_u32(of.max(1)))
}

/// Evaluates the cubic Bézier `points` at `t`.
fn cubic([p0, c1, c2, p1]: [PagePoint; 4], t: f32) -> PagePoint {
    let s = 1.0 - t;
    let (a, b, c, d) = (s * s * s, 3.0 * s * s * t, 3.0 * s * t * t, t * t * t);
    PagePoint {
        x: a * p0.x + b * c1.x + c * c2.x + d * p1.x,
        y: a * p0.y + b * c1.y + c * c2.y + d * p1.y,
    }
}

/// Returns the derivative of the cubic Bézier `points` at `t`, unnormalized.
fn derivative([p0, c1, c2, p1]: [PagePoint; 4], t: f32) -> (f32, f32) {
    let s = 1.0 - t;
    let (a, b, c) = (3.0 * s * s, 6.0 * s * t, 3.0 * t * t);
    (
        a * (c1.x - p0.x) + b * (c2.x - c1.x) + c * (p1.x - c2.x),
        a * (c1.y - p0.y) + b * (c2.y - c1.y) + c * (p1.y - c2.y),
    )
}
