//! Where a text run's clusters stand for carets, emphasis marks and
//! selections: along the line, a ligature shared evenly among the clusters
//! it draws, and across the line in combined text.
//!
//! [`TextRun::places`](super::TextRun::places) and
//! [`TextRun::across_places`](super::TextRun::across_places) make the walks
//! here.

use core::ops::Range;

use crate::data::{Id, index_to_u32};
use crate::stages::analysis::ClusterId;
use crate::stages::fragments::ClusterWalk;
use crate::stages::shape::{ClusterGlyphs, GlyphStore, ShapedText};
use crate::unit::{self, InlineLayoutUnit, LayoutUnit};
use crate::work;

/// A text run's clusters where a caret and a mark see them, in logical
/// order: returned by
/// [`TextRun::places`](super::TextRun::places).
///
/// A cluster's extent is its own, where it draws glyphs of its own; a
/// ligature's advance, all on the cluster that starts it, is shared evenly
/// among the clusters it draws (it and the continuations after it), each
/// share counted from the ligature's logical start, as Blink divides a
/// ligature among its graphemes (`AddEmphasisMarkToBloberizer`, and
/// `ShapeResult::CaretPositionForOffset` for carets).
pub(crate) struct Places<'a> {
    walk: ClusterWalk<'a>,
    /// The run's shaping, whose glyph words say which clusters continue a
    /// ligature.
    shaped: &'a ShapedText,
    /// The run's last cluster boundary, which a ligature's clusters end at
    /// the latest.
    end: ClusterId,
    rtl: bool,
    /// The ligature being shared out: its start and end along the line, how
    /// many clusters it draws, and how many of them were handed out.
    ligature: Option<(InlineLayoutUnit, InlineLayoutUnit, usize, usize)>,
}

impl<'a> Places<'a> {
    /// Returns the places of the clusters `walk` walks, a text run's, which
    /// `shaped` shaped and which end at `end`, reading right to left where
    /// `rtl`.
    pub(super) fn new(
        walk: ClusterWalk<'a>,
        shaped: &'a ShapedText,
        end: ClusterId,
        rtl: bool,
    ) -> Self {
        Self {
            walk,
            shaped,
            end,
            rtl,
            ligature: None,
        }
    }

    /// How many clusters from `cluster` a ligature starting there draws: it
    /// and the continuations after it in the run.
    fn drawn_by(&self, cluster: ClusterId) -> usize {
        let mut count = 1;
        let mut next = cluster.get() + 1;
        while next < self.end.get()
            && self
                .shaped
                .glyphs
                .word(ClusterId::new(next))
                .is_continuation()
        {
            work::step();
            count += 1;
            next += 1;
        }
        count
    }
}

impl Iterator for Places<'_> {
    /// A cluster, and where it starts and ends along the line, from the line
    /// box's left, exactly.
    type Item = (ClusterId, InlineLayoutUnit, InlineLayoutUnit);

    fn next(&mut self) -> Option<Self::Item> {
        work::step();
        let (cluster, left, step) = self.walk.next()?;
        let (from, to) = match self.ligature {
            Some((start, end, count, done)) if done < count && step == InlineLayoutUnit::ZERO => {
                self.ligature = Some((start, end, count, done + 1));
                share(start, end, count, done, self.rtl)
            }
            _ => {
                let count = self.drawn_by(cluster);
                if count > 1 {
                    self.ligature = Some((left, left + step, count, 1));
                    share(left, left + step, count, 0, self.rtl)
                } else {
                    self.ligature = None;
                    (left, left + step)
                }
            }
        };
        Some((cluster, from, to))
    }
}

/// The `k`th of `count` even shares of `start..end`, counted from the
/// logical start: its left and right, right to left from `end`.
fn share(
    start: InlineLayoutUnit,
    end: InlineLayoutUnit,
    count: usize,
    k: usize,
    rtl: bool,
) -> (InlineLayoutUnit, InlineLayoutUnit) {
    let whole = i128::from((end - start).raw());
    let count = i128::from(index_to_u32(count.max(1)));
    // Exactly on the grid: the n-th boundary of the shares.
    let at = |n: usize| {
        let raw = whole * i128::from(index_to_u32(n)) / count;
        InlineLayoutUnit::from_raw(i64::try_from(raw).unwrap_or_default())
    };
    if rtl {
        (end - at(k + 1), end - at(k))
    } else {
        (start + at(k), start + at(k + 1))
    }
}

/// A combined run's clusters where a caret and a selection see them across
/// its line, in logical order: returned by
/// [`TextRun::across_places`](super::TextRun::across_places).
pub(crate) struct AcrossPlaces<'a> {
    /// The paragraph's glyphs, which a unit's are: no line breaks inside one.
    glyphs: &'a GlyphStore,
    /// The line's baseline, from the line box's top, which the unit's
    /// glyphs are placed about.
    baseline: f32,
    /// The run's next cluster, and where its clusters in the unit end.
    next: ClusterId,
    end: ClusterId,
    /// Where the unit ends.
    unit_end: ClusterId,
    /// The boundary the clusters from `next` are counted from: a cluster
    /// with glyphs, or the unit's start, and where it stands.
    from: (ClusterId, f32),
    /// The next such boundary after it: the next cluster with glyphs, or
    /// the unit's end.
    to: (ClusterId, f32),
    /// Where the unit ends across the line.
    last: f32,
}

impl<'a> AcrossPlaces<'a> {
    /// Returns the places across the line of `clusters`, a combined run's,
    /// in the unit `unit`, whose glyphs are among `glyphs`.
    ///
    /// The line's baseline and the unit's middle are `baseline` and
    /// `middle` from the line box's top.
    pub(super) fn new(
        glyphs: &'a GlyphStore,
        (baseline, middle): (f32, f32),
        clusters: Range<ClusterId>,
        unit: Range<ClusterId>,
    ) -> Self {
        let mut places = Self {
            glyphs,
            baseline,
            next: clusters.start,
            end: clusters.end.min(unit.end),
            unit_end: unit.end,
            from: (unit.start, middle),
            to: (unit.end, middle),
            last: middle,
        };
        let first = places.drawn(unit.start).unwrap_or(middle);
        places.last = 2.0 * middle - first;
        places.from = (unit.start, first);
        // The last cluster with glyphs at or before the run's start, which
        // is the unit's start but where a style boundary divides the unit.
        let mut at = clusters.start;
        while at > unit.start {
            work::step();
            if let Some(there) = places.drawn(at) {
                places.from = (at, there);
                break;
            }
            at = ClusterId::new(at.get() - 1);
        }
        places.to = places.next_drawn(places.from.0);
        places
    }

    /// Where `at`'s first glyph is drawn across the line, where it is in
    /// the unit and has glyphs of its own.
    /// On layout's grid, to the nearest 1/64, as Chrome's caret rectangles
    /// are `LayoutUnit`s.
    fn drawn(&self, at: ClusterId) -> Option<f32> {
        if at >= self.unit_end {
            return None;
        }
        match self.glyphs.glyphs(at) {
            ClusterGlyphs::Many(drawn) => drawn
                .first()
                .map(|glyph| LayoutUnit::from_px(self.baseline - glyph.y_offset.to_px()).to_px()),
            ClusterGlyphs::One(_) | ClusterGlyphs::None => None,
        }
    }

    /// The first boundary after `after` a cluster's glyphs give, or the
    /// unit's end, and where it stands.
    fn next_drawn(&self, after: ClusterId) -> (ClusterId, f32) {
        let mut at = after;
        loop {
            work::step();
            at = ClusterId::new(at.get() + 1);
            if at >= self.unit_end {
                return (self.unit_end, self.last);
            }
            if let Some(there) = self.drawn(at) {
                return (at, there);
            }
        }
    }

    /// Where the boundary before `at` stands, `at` being from `from` to
    /// `to`: theirs, or its even share of the span between them.
    fn boundary(&self, at: ClusterId) -> f32 {
        let ((from, start), (to, end)) = (self.from, self.to);
        if at <= from {
            return start;
        }
        if at >= to {
            return end;
        }
        let share = unit::whole_to_f32(index_to_u32(at.get() - from.get()))
            / unit::whole_to_f32(index_to_u32(to.get() - from.get()));
        start + (end - start) * share
    }
}

impl Iterator for AcrossPlaces<'_> {
    /// A cluster, and where it starts and ends across the line, from the
    /// line box's top.
    type Item = (ClusterId, f32, f32);

    fn next(&mut self) -> Option<Self::Item> {
        work::step();
        let at = self.next;
        if at >= self.end {
            return None;
        }
        if at >= self.to.0 {
            self.from = self.to;
            self.to = self.next_drawn(at);
        }
        let after = ClusterId::new(at.get() + 1);
        self.next = after;
        Some((at, self.boundary(at), self.boundary(after)))
    }
}
