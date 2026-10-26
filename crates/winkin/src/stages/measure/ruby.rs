//! Ruby and emphasis-mark em boxes, and how `ruby-align` spreads a column's
//! room.
//!
//! A ruby column is as wide as the widest of its base and its annotation
//! levels, each measured at its max-content, as Blink's `HandleRuby` sizes
//! it. [`RubySpread`] places the room a narrower part leaves. An emphasis
//! mark is set like an annotation of its character, at half the size (CSS
//! Text Decoration 4, "emphasis marks"). Nothing here allocates.

use core::ops::Range;

use crate::stages::analysis::ClusterId;
use crate::stages::content::FontRequestId;
use crate::stages::fonts::{FontRunId, Fonts};
use crate::style::{FirstLineVariant, RubyAlign, TextAlign};
use crate::unit::LayoutUnit;

use super::Extent;
use crate::work;

/// Returns the em box of `clusters`' text in font request `request`, around
/// its baseline, as Blink's `ComputeEmHeight` gives a text item's.
///
/// The box unites the em boxes of the fonts the clusters are drawn in, each
/// rounded up to a whole pixel, capped at the primary font's ascent and
/// descent. The `variant`'s font runs are walked from `first`, the run
/// holding the first cluster.
///
/// The `bool` says every font has the same em box. Returns
/// `(Extent::NONE, false)` for no clusters or no primary font.
pub(crate) fn em_box(
    fonts: &Fonts,
    request: FontRequestId,
    clusters: Range<ClusterId>,
    variant: FirstLineVariant,
    first: FontRunId,
) -> (Extent, bool) {
    if clusters.is_empty() {
        return (Extent::NONE, false);
    }
    let Some(cap) = fonts.primary_font(request).map(|used| &used.metrics) else {
        return (Extent::NONE, false);
    };
    let mut em = Extent::NONE;
    let mut uniform = true;
    for run in fonts.runs(variant).rest(first) {
        work::step();
        if run.start >= clusters.end {
            break;
        }
        let Some(used) = fonts.used.get(run.font) else {
            uniform = false;
            continue;
        };
        let metrics = &used.metrics;
        let own = Extent::new(metrics.em_over.ceil_px(), metrics.em_under.ceil_px());
        uniform &= em.is_none() || em == own;
        em = em.unite(own);
    }
    if em.is_none() {
        return (em, false);
    }
    let em = Extent::new(em.ascent().min(cap.ascent), em.descent().min(cap.descent));
    (em, uniform)
}

/// Returns the em box of `request`'s primary font, rounded up to whole
/// pixels and capped at its ascent and descent, for an empty ruby base: an
/// empty base line takes its container's font. [`Extent::NONE`]
/// without a primary font.
pub(super) fn strut_em(fonts: &Fonts, request: FontRequestId) -> Extent {
    let Some(metrics) = fonts.primary_font(request).map(|used| &used.metrics) else {
        return Extent::NONE;
    };
    Extent::new(
        metrics.em_over.ceil_px().min(metrics.ascent),
        metrics.em_under.ceil_px().min(metrics.descent),
    )
}

/// The placement of a ruby column's room around and inside a base or an
/// annotation, as `ruby-align` sets it. The end takes what is left.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct RubySpread {
    /// The room before the first cluster, at the logical start.
    pub(crate) start: LayoutUnit,
    /// The room the opportunities share, as Blink's `JustifyResults`
    /// spreads it. The last takes what the division leaves.
    pub(crate) inside: LayoutUnit,
}

impl RubySpread {
    /// Returns the spread of `space` beside a base with `opportunities`, as
    /// Blink's `ApplyRubyAlign` places it on a justified base line.
    ///
    /// - `space-around`: an inset each side of half what one more
    ///   opportunity would take, the rest to the opportunities.
    /// - `space-between`: all to the opportunities.
    /// - `center`, or either of those without opportunities: centred.
    /// - `start`: all after the base.
    pub(crate) fn from_base(align: RubyAlign, space: LayoutUnit, opportunities: u32) -> Self {
        if space <= LayoutUnit::ZERO {
            return Self::default();
        }
        match align {
            RubyAlign::SpaceAround if opportunities > 0 => {
                Self::from_space_around(space, opportunities, None)
            }
            RubyAlign::SpaceBetween if opportunities > 0 => Self {
                start: LayoutUnit::ZERO,
                inside: space,
            },
            RubyAlign::SpaceAround | RubyAlign::SpaceBetween | RubyAlign::Center => Self {
                start: space.half(),
                inside: LayoutUnit::ZERO,
            },
            RubyAlign::Start => Self::default(),
        }
    }

    /// Returns the spread of `space` beside an annotation, as Blink's
    /// `ApplyRubyAlign` places it on an annotation line.
    ///
    /// Only `space-around` differs from [`from_base`](Self::from_base). In a
    /// block set at its start or justified, the inset is at most one
    /// full-width character (`font_size` pixels) each side, or the room is
    /// centred without opportunities. Otherwise the block's `text-align`
    /// sets it.
    pub(crate) fn from_annotation(
        align: RubyAlign,
        text_align: TextAlign,
        space: LayoutUnit,
        opportunities: u32,
        font_size: f32,
        rtl: bool,
    ) -> Self {
        if space <= LayoutUnit::ZERO || align != RubyAlign::SpaceAround {
            return Self::from_base(align, space, opportunities);
        }
        // Puts `left` of the room on the physical left, as a logical start.
        let from_left = |left: LayoutUnit| Self {
            start: if rtl { space - left } else { left },
            inside: LayoutUnit::ZERO,
        };
        match text_align {
            TextAlign::Start | TextAlign::Justify if opportunities > 0 => {
                let cap = LayoutUnit::from_px(2.0 * font_size);
                Self::from_space_around(space, opportunities, Some(cap))
            }
            TextAlign::Start | TextAlign::Justify | TextAlign::Center => {
                Self::from_base(RubyAlign::Center, space, 0)
            }
            TextAlign::Left => from_left(LayoutUnit::ZERO),
            TextAlign::Right => from_left(space),
            TextAlign::End => Self {
                start: space,
                inside: LayoutUnit::ZERO,
            },
        }
    }

    /// Returns `space` spread under `space-around` over one or more
    /// `opportunities`, the inset capped at `cap`.
    fn from_space_around(space: LayoutUnit, opportunities: u32, cap: Option<LayoutUnit>) -> Self {
        let count = i32::try_from(opportunities.saturating_add(1)).unwrap_or(i32::MAX);
        let inset = space.divided(count);
        let inset = cap.map_or(inset, |cap| inset.min(cap));
        Self {
            start: inset.half(),
            inside: space - inset,
        }
    }
}
