//! Used fonts: the size a request's fonts are used at, and the interner of
//! the layout's table of them.
//!
//! - `used_size` keys a size as Chrome does.
//! - `adjusted_size` applies `font-size-adjust`.
//! - `InitialLetterGrid` sizes an initial letter to the lines it spans.
//! - `UsedFontInterner` finds or adds a used font.

use hashbrown::HashTable;

use super::instance::{FaceOverrides, InstanceId, Instances};
use super::metrics::{LineBaseline, UnscaledMetrics};
use super::{UsedFontId, UsedFonts, UsedSynthesis};
use crate::config::LineMetricsSource;
use crate::data::hash_one;
use crate::stages::content::FontRequest;
use crate::style::{FontSizeAdjust, sanitized_font_size};
use crate::unit::{self, TextUnit};
use crate::work;

/// Returns the size a used font is set at for `px` pixels.
///
/// The size is floored to a hundredth of a pixel, as Chrome's
/// `FontDescription::EffectiveFontSize` keys its fonts, then truncated to
/// 16.16 `TextUnit`, as Chrome turns a size into its HarfBuzz scale.
pub(super) fn used_size(px: f32) -> TextUnit {
    TextUnit::from_px_truncated(unit::floor_to_hundredth(sanitized_font_size(px)))
}

/// The lines an initial letter is sized to span.
///
/// It holds the block's first-line line height and its primary font's
/// metrics and size, which give the letter's alignment points.
#[derive(Copy, Clone, Debug)]
pub(super) struct InitialLetterGrid {
    /// The block's first-line line height in pixels, as Chrome's
    /// `ComputedLineHeight` gives it.
    pub(super) line_height: f32,
    /// The block's primary font's metrics, unscaled.
    pub(super) unscaled: UnscaledMetrics,
    /// The primary font's size, as its metrics scale: floored to a
    /// hundredth of a pixel, as a font is keyed.
    pub(super) px: f32,
}

impl InitialLetterGrid {
    /// The most times an initial letter's size steps down a pixel before
    /// giving up.
    ///
    /// Once or twice is enough for any font. The limit only stops a font's
    /// metrics from keeping the loop going.
    const MAX_STEPS: usize = 64;

    /// Returns the size in pixels an initial letter of `request` is used at,
    /// as Chrome's `ComputeInitialLetterFont` computes it.
    ///
    /// `computed` is the request's computed size and `unscaled` its primary
    /// font's metrics; `source` says how lines are measured. The letter's
    /// over point, above its baseline, must reach `size − 1` line heights
    /// plus the paragraph primary's over point. Under
    /// `initial-letter-align: alphabetic`, the initial value, the over point
    /// is the cap height. The size is scaled to fit, then reduced a pixel at
    /// a time while the letter's over point overshoots. `None` where no
    /// finite size above a pixel fits.
    pub(super) fn letter_size(
        &self,
        request: &FontRequest,
        computed: f32,
        unscaled: &UnscaledMetrics,
        source: LineMetricsSource,
    ) -> Option<f32> {
        let letter = request.initial_letter;
        let align = letter.align;
        let own =
            |px: f32| unscaled.initial_letter_over(align, source, unit::floor_to_hundredth(px));
        let wanted = self.line_height * (letter.size - 1.0)
            + self.unscaled.initial_letter_over(align, source, self.px);
        let at_computed = own(computed);
        if !(wanted.is_finite() && wanted > 0.0 && at_computed > 0.0) {
            return None;
        }
        let mut size = sanitized_font_size(wanted * computed / at_computed);
        for _ in 0..Self::MAX_STEPS {
            work::step();
            if size <= 1.0 {
                return None;
            }
            if own(size) <= wanted {
                return Some(size);
            }
            size -= 1.0;
        }
        None
    }
}

/// Returns the size in pixels `request`'s fonts are used at under
/// `font-size-adjust`.
///
/// Scales the computed size `computed` so that the metric's aspect, read
/// from the `unscaled` metrics, reaches the value asked for. A value of
/// zero gives a size of zero, as CSS Fonts 4 says and Blink's `FontBuilder`
/// sets it. `None` where no adjustment is asked for, the value is not a
/// finite number of zero or more, or `computed` is not positive. The result
/// is sanitized.
pub(super) fn adjusted_size(
    request: &FontRequest,
    computed: f32,
    unscaled: &UnscaledMetrics,
    overrides: &FaceOverrides,
    source: LineMetricsSource,
) -> Option<f32> {
    let FontSizeAdjust::Hold { metric, value } = request.font.size_adjust else {
        return None;
    };
    if !(value.is_finite() && value >= 0.0) || computed <= 0.0 {
        return None;
    }
    if value == 0.0 {
        return Some(0.0);
    }
    let aspect = unscaled.aspect(metric, overrides, computed, source);
    Some(sanitized_font_size(value / aspect * computed))
}

/// Interns a layout's used fonts.
///
/// It holds the layout's table, the scratch index over it, and how many
/// fonts were replaced because the table was full.
pub(super) struct UsedFontInterner<'a> {
    pub(super) used: &'a mut UsedFonts,
    /// Each used font by the context's instance it was made from. Only the
    /// scratch keeps instance ids; the layout holds none.
    pub(super) index: &'a mut HashTable<(u64, Option<InstanceId>, UsedFontId)>,
    /// Which of a font's metrics measure the layout's lines.
    pub(super) source: LineMetricsSource,
    /// The baseline the block's lines are set on.
    pub(super) baseline: LineBaseline,
    /// How many used fonts were replaced because the table was full.
    pub(super) replaced: usize,
}

impl UsedFontInterner<'_> {
    /// Returns the used font for `instance` at `size` with `synthesis`,
    /// interning it if new. Returns `None` if the table is full.
    ///
    /// The instance's id stays in the scratch, to find the used font again
    /// during this build.
    fn intern(
        &mut self,
        instance: Option<InstanceId>,
        size: TextUnit,
        synthesis: UsedSynthesis,
        instances: &Instances,
    ) -> Option<UsedFontId> {
        // An id with no instance, which never happens, draws nothing.
        let resolved = instance.and_then(|id| Some((id, instances.get(id)?)));
        let instance = resolved.map(|(id, _)| id);
        let key = (instance, size, synthesis);
        let hash = hash_one(&key);
        let used = &*self.used;
        let found = self.index.find(hash, |&(h, held, id)| {
            h == hash
                && held == instance
                && used
                    .get(id)
                    .is_some_and(|used| used.size == size && used.synthesis == synthesis)
        });
        if let Some(&(_, _, id)) = found {
            return Some(id);
        }
        let made = resolved.map(|(_, made)| made);
        let id = self
            .used
            .push(made, size, synthesis, self.source, self.baseline)?;
        // Indexed only once stored, so a refused font leaves nothing.
        self.index
            .insert_unique(hash, (hash, instance, id), |&(h, _, _)| h);
        Some(id)
    }

    /// Interns as [`intern`](Self::intern) does, returning `fallback` where
    /// the table is full and counting the replacement.
    pub(super) fn intern_or(
        &mut self,
        instance: Option<InstanceId>,
        size: TextUnit,
        synthesis: UsedSynthesis,
        instances: &Instances,
        fallback: UsedFontId,
    ) -> UsedFontId {
        match self.intern(instance, size, synthesis, instances) {
            Some(id) => id,
            None => {
                self.replaced = self.replaced.saturating_add(1);
                fallback
            }
        }
    }
}
