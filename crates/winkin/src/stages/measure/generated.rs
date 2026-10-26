//! The shaped hyphens and ellipses a line may draw.

use core::ops::Range;

use crate::data::IdRange;
use crate::data::{Id, Table};
use crate::stages::analysis::RunOrientation;
use crate::stages::content::{HyphenStrings, TextFactsId, TextFlags};
use crate::stages::fonts::{Generated, GeneratedFonts, UsedFontId};
use crate::stages::shape::{self, ShapeSession, ShapingKey, SidecarGlyph, SidecarGlyphId};
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::work;

use super::{MeasureInput, ShapedGeneratedId};

/// The font selection results measurement reads after a build, kept by the
/// context until the next build.
#[derive(Copy, Clone)]
pub(crate) struct GeneratedInput<'a> {
    pub(crate) fonts: &'a GeneratedFonts,
    pub(crate) strings: &'a HyphenStrings,
    /// The advance of U+0020 in the block's fallback font for a space, which
    /// tabs use; `None` where the primary font maps a space.
    pub(crate) block_space: Option<TextUnit>,
}

/// The shaped items of one generated text in [`GeneratedTexts`], one per
/// used font, from the left. Nearly always there is one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct GeneratedItems {
    start: ShapedGeneratedId,
    end: ShapedGeneratedId,
}

impl GeneratedItems {
    /// Returns its items' ids, from the left.
    pub(crate) fn ids(self) -> impl Iterator<Item = ShapedGeneratedId> {
        (self.start..self.end).ids()
    }

    /// Returns how many items it has; each draws as one fragment item.
    pub(crate) fn count(self) -> usize {
        self.end.get().saturating_sub(self.start.get())
    }
}

/// The hyphens and ellipses a line may draw, shaped once per text facts row
/// that can need them, the first line's included.
///
/// Items sort by text facts row, then [`ShapedGenerated::rank`]. All share
/// one sidecar glyph table, and both tables keep their buffers, so a warm
/// rebuild allocates nothing.
pub(crate) struct GeneratedTexts {
    pub(super) texts: Table<ShapedGeneratedId, ShapedGenerated>,
    pub(super) glyphs: Table<SidecarGlyphId, SidecarGlyph>,
}

impl GeneratedTexts {
    pub(super) const fn new() -> Self {
        Self {
            texts: Table::new(),
            glyphs: Table::new(),
        }
    }

    pub(super) fn clear(&mut self) {
        self.texts.clear();
        self.glyphs.clear();
    }

    /// Shapes every generated text in `generated`, in its order, with the
    /// caches in `shaping`. An ellipsis is shaped once for each direction
    /// some paragraph reads in.
    pub(super) fn fill<'f>(
        &mut self,
        input: &MeasureInput<'f>,
        shaping: &mut ShapeSession<'_, 'f>,
        generated: GeneratedInput<'_>,
    ) {
        let (content, fonts) = (input.content, input.fonts);
        let strings = generated.strings;
        // A hyphen takes its style's direction (Blink's `HyphenResult::Shape`),
        // an ellipsis its paragraph's (`LineTruncator`). Paragraph directions
        // are read only when an ellipsis needs them.
        let mut reads = None;
        let generated = generated.fonts;
        'texts: for (facts, kind, text) in generated.iter() {
            work::step();
            let said = text.string.text(strings);
            let runs = generated.runs(text);
            // The orientation of the text it ends.
            let orientation = content
                .facts
                .text(facts)
                .setting
                .generated_orientation(said);
            let directions = match kind {
                Generated::Hyphen => {
                    let rtl = content.facts.text(facts).has(TextFlags::RTL);
                    [Some(rtl), None]
                }
                Generated::Ellipsis => {
                    let (ltr, rtl) =
                        *reads.get_or_insert_with(|| input.analysis.paragraphs.directions());
                    [ltr.then_some(false), rtl.then_some(true)]
                }
            };
            for rtl in directions.into_iter().flatten() {
                let (first_text, first_glyph) = (self.texts.next_id(), self.glyphs.next_id());
                // One piece per run, from the left: last run first in RTL.
                for at in 0..runs.len() {
                    work::step();
                    let at = if rtl { runs.len() - 1 - at } else { at };
                    let Some(run) = runs.get(at) else {
                        continue;
                    };
                    let from = at.checked_sub(1).and_then(|before| runs.get(before));
                    let from = from.map_or(0, |before| before.end());
                    let piece = said.get(from..run.end()).unwrap_or_default();
                    let key = ShapingKey::from_generated(
                        run.font,
                        facts,
                        content,
                        fonts,
                        rtl,
                        orientation,
                    );
                    let start = self.glyphs.next_id();
                    let advance = shape::shape_generated(shaping, piece, &key, &mut self.glyphs);
                    let shaped = ShapedGenerated {
                        text: facts,
                        kind,
                        rtl,
                        orientation,
                        font: run.font,
                        advance,
                        glyphs: start..self.glyphs.next_id(),
                    };
                    if self.texts.push(shaped).is_none() {
                        // Past the id limit, drop this text and all later
                        // ones; their lines draw and pay nothing.
                        self.texts.truncate(first_text);
                        self.glyphs.truncate(first_glyph);
                        break 'texts;
                    }
                }
            }
        }
    }

    /// Returns the piece `id`, or `None` past the last.
    pub(crate) fn get(&self, id: ShapedGeneratedId) -> Option<&ShapedGenerated> {
        self.texts.get(id)
    }

    /// Returns the hyphen of text facts row `text`, or `None` where it has
    /// none.
    pub(crate) fn hyphen(&self, text: TextFactsId) -> Option<GeneratedItems> {
        self.find(text, 0)
    }

    /// Returns the ellipsis of text facts row `text` for a right-to-left line
    /// (`rtl`) or a left-to-right one.
    ///
    /// Returns `None` where the text has none, or no paragraph reads that way.
    pub(crate) fn ellipsis(&self, text: TextFactsId, rtl: bool) -> Option<GeneratedItems> {
        self.find(text, 1 + u8::from(rtl))
    }

    /// Returns the items of `text` at `rank` ([`ShapedGenerated::rank`]).
    fn find(&self, text: TextFactsId, rank: u8) -> Option<GeneratedItems> {
        work::seek();
        let texts = self.texts.as_slice();
        let want = (text, rank);
        let start = texts.partition_point(|piece| (piece.text, piece.rank()) < want);
        let end = texts.partition_point(|piece| (piece.text, piece.rank()) <= want);
        (start < end).then_some(GeneratedItems {
            start: ShapedGeneratedId::try_new(start)?,
            end: ShapedGeneratedId::try_new(end)?,
        })
    }

    /// Returns the exact width of `text`: the sum of its items' advances.
    pub(crate) fn advance(&self, text: GeneratedItems) -> InlineLayoutUnit {
        text.ids()
            .filter_map(|id| self.texts.get(id))
            .fold(InlineLayoutUnit::ZERO, |sum, piece| sum + piece.advance)
    }

    /// Returns the width of `text` rounded up to a 26.6 `LayoutUnit`, never
    /// negative, as Blink's `ShapeResult::SnappedWidth` snaps it.
    pub(crate) fn snapped(&self, text: GeneratedItems) -> LayoutUnit {
        self.advance(text)
            .ceil_to_grid()
            .to_layout()
            .max(LayoutUnit::ZERO)
    }

    /// Returns the glyphs of `text` in visual order, or an empty slice where
    /// its range is outside the glyph table.
    pub(crate) fn glyphs(&self, text: &ShapedGenerated) -> &[SidecarGlyph] {
        self.glyphs
            .get_slice(text.glyphs.clone())
            .unwrap_or_default()
    }

    /// Returns whether it holds no piece and no glyph.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.texts.is_empty() && self.glyphs.is_empty()
    }

    /// Returns every piece, in table order.
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = &ShapedGenerated> {
        self.texts.as_slice().iter()
    }
}

/// One shaped run of a generated text for one text facts row. Nearly always
/// it is the whole text in the primary font.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ShapedGenerated {
    /// The text facts row it is shaped for.
    pub(super) text: TextFactsId,
    /// Hyphen or ellipsis.
    pub(crate) kind: Generated,
    /// Whether it is shaped right to left.
    rtl: bool,
    /// The orientation, that of the text it ends.
    pub(crate) orientation: RunOrientation,
    /// The used font it is set in.
    pub(crate) font: UsedFontId,
    /// The exact width along the line.
    pub(crate) advance: InlineLayoutUnit,
    /// The glyphs in [`GeneratedTexts`]' glyph table, in visual order.
    glyphs: Range<SidecarGlyphId>,
}

impl ShapedGenerated {
    /// Returns its rank within a text facts row: hyphen, LTR ellipsis, RTL
    /// ellipsis.
    fn rank(&self) -> u8 {
        match self.kind {
            Generated::Hyphen => 0,
            Generated::Ellipsis => 1 + u8::from(self.rtl),
        }
    }
}
