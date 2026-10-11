//! Ruby columns and emphasis marks: the parts that don't depend on the
//! width.
//!
//! **A ruby column** is a base and the annotations set beside it, one level
//! each. It is sized once, as Chrome's `HandleRuby` sizes a column while it
//! breaks a line. It is as wide as the widest of its base and its levels,
//! each measured as a line of its own at its max-content.
//! - A wider annotation leaves room around and inside the base.
//!   `ruby-align` places that room, as Chrome's `ApplyRubyAlign` does.
//! - Some of the room may reach over the text beside the column, as
//!   `ruby-overhang` and `Config::ruby_overhang` say.
//! - The prefix holds what the column takes along the line. Line layout
//!   spreads the room from the insets kept here. The breaker reads how far
//!   the annotations reach across the line from the em boxes kept here.
//!
//! **An emphasis mark** is set like an annotation of its character, in the
//! same font at half the size (CSS Text Decoration 4, "emphasis marks"). Its
//! em box is the font's, normalized to half the computed size, and rounded
//! as Chrome rounds the size of the font it draws marks in
//! (`SimpleFontData::EmphasisMarkFontData`). Each row of text facts that
//! sets marks has one, in `TextMetrics::mark`.
//!
//! Nothing here allocates: the columns and their levels go into the stage's
//! tables, which keep their capacity.

use core::ops::Range;

use crate::stages::analysis::ClusterId;
use crate::stages::content::FontRequestId;
use crate::stages::fonts::{FontRunId, Fonts};
use crate::style::{FirstLineVariant, RubyAlign, TextAlign};
use crate::unit::LayoutUnit;

use super::Extent;
use crate::work;

/// Returns the em box of the text of `clusters`, set for the font request
/// `request`, around its baseline, as Chrome's `ComputeEmHeight` gives a
/// text item's.
///
/// It is the union of the em boxes of the fonts its clusters are drawn in,
/// each rounded up to a whole pixel, and no taller than its primary font's
/// ascent and descent. A ruby annotation is set against it, and an emphasis
/// mark is set on it. It is [`Extent::NONE`] for no clusters.
///
/// The `variant`'s font runs are walked from `first`, the run holding the
/// first of `clusters`. The caller's walk holds it, or a reader of one
/// fragment seeks it.
///
/// Also returns whether every font the text is drawn in has that one em
/// box. It is `false` where two fonts' em boxes differ, or where a run has
/// no font.
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

/// Returns the em box of the primary font of the font request `request`,
/// rounded up to whole pixels and no taller than its ascent and descent.
///
/// An empty ruby base stands for it, as Chrome's empty base line takes its
/// ruby container's font.
pub(super) fn strut_em(fonts: &Fonts, request: FontRequestId) -> Extent {
    let Some(metrics) = fonts.primary_font(request).map(|used| &used.metrics) else {
        return Extent::NONE;
    };
    Extent::new(
        metrics.em_over.ceil_px().min(metrics.ascent),
        metrics.em_under.ceil_px().min(metrics.descent),
    )
}

/// Where a ruby column's room goes around and inside what it spreads, as
/// `ruby-align` places it.
///
/// What it spreads is a base or an annotation. The room goes at its logical
/// start, among its justification opportunities, and at its end. The end
/// gets whatever the start and the inside leave.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct RubySpread {
    /// The room before its first cluster, at its logical start.
    pub(crate) start: LayoutUnit,
    /// The room its opportunities share, as Chrome's `JustifyResults`
    /// spreads it. The last takes what the division leaves.
    pub(crate) inside: LayoutUnit,
}

impl RubySpread {
    /// Returns how `space`, the room a base narrower than its column has, is
    /// placed around and inside it.
    ///
    /// `opportunities` counts its clusters' justification opportunities.
    /// Chrome's `ApplyRubyAlign` places the room on a base line, whose
    /// `text-align` is always `justify`:
    ///
    /// - `space-around`: a base with opportunities is inset each side by half
    ///   of what one more opportunity would take. The rest goes to its
    ///   opportunities (`ApplyJustification` for `kRubyBase`). A base with
    ///   none is centred.
    /// - `space-between`: all of it to the opportunities, or centred where
    ///   there are none;
    /// - `center`: half each side;
    /// - `start`: all of it after the base.
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

    /// Returns how `space`, the room an annotation narrower than its column
    /// has, is placed around and inside it.
    ///
    /// Chrome's `ApplyRubyAlign` places it on an annotation line.
    /// `opportunities` counts its justification opportunities, `font_size`
    /// is its computed size, `text_align` is the block's, and `rtl` says it
    /// reads right to left.
    ///
    /// Under `space-around`, it is justified where the block is set at its
    /// start or justified. It is then inset by half what one more
    /// opportunity would take, and never by more than one full-width
    /// character either side (`kRubyText`). Otherwise it is set as the
    /// block sets its lines.
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
        // The spread that puts `left` of the room on the physical left,
        // measured from the logical start.
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

    /// Returns `space` spread under `space-around` over `opportunities`, one
    /// or more: an inset each side of half what one more opportunity would
    /// take, no more than `cap` in all, and the rest at the opportunities.
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
