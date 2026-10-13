//! Stored generated measurements.

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

/// What the build's font selection hands measurement, which the context
/// keeps until the next build: the generated texts' fonts and the hyphen
/// strings they say, and the space a block whose primary font maps none
/// counts its tabs in.
#[derive(Copy, Clone)]
pub(crate) struct GeneratedInput<'a> {
    pub(crate) fonts: &'a GeneratedFonts,
    pub(crate) strings: &'a HyphenStrings,
    /// The advance of U+0020 in the font a space falls back to among the
    /// block's listed fonts, where its primary font maps none.
    pub(crate) block_space: Option<TextUnit>,
}

/// The shaped items of one generated text, one per used font, from the left
/// as drawn.
///
/// The items live in [`GeneratedTexts`]' table. Nearly always there is one.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct GeneratedItems {
    start: ShapedGeneratedId,
    end: ShapedGeneratedId,
}

impl GeneratedItems {
    /// Its items, from the left.
    pub(crate) fn ids(self) -> impl Iterator<Item = ShapedGeneratedId> {
        (self.start..self.end).ids()
    }

    /// How many items it has, each drawn as a fragment item.
    pub(crate) fn count(self) -> usize {
        self.end.get().saturating_sub(self.start.get())
    }
}

/// The hyphens and ellipses a line may draw, shaped once per style that can
/// need them.
///
/// An ellipsis is shaped once per direction its lines read in. The texts
/// sort by style, then kind, then direction, with one item per font from
/// the left as drawn. All items' glyphs share one table of sidecar glyphs,
/// so a rebuild allocates nothing.
///
/// The first line's styles have their own text facts rows, so its hyphens
/// and ellipsis are in the same table.
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

    /// Shapes every generated text that font selection chose, in
    /// `generated`, for the content of `input`.
    ///
    /// It keeps their order, by style and then kind, and uses the shaping
    /// caches in `shaping`. Each text is shaped once however many lines
    /// draw it, and an ellipsis once for each direction its lines read in.
    pub(super) fn fill<'f>(
        &mut self,
        input: &MeasureInput<'f>,
        shaping: &mut ShapeSession<'_, 'f>,
        generated: GeneratedInput<'_>,
    ) {
        let (content, fonts) = (input.content, input.fonts);
        let strings = generated.strings;
        // A hyphen is shaped in its style's direction, as Blink's
        // `HyphenResult::Shape` shapes it. An ellipsis is shaped in its
        // line's base direction, as Blink's `LineTruncator` shapes it. That
        // is the paragraph's direction: the block's, unless each paragraph
        // reads its own way (`direction: auto`, or `unicode-bidi:
        // plaintext`). The paragraphs' directions are read only where there
        // is an ellipsis.
        let mut reads = None;
        let generated = generated.fonts;
        'texts: for (facts, kind, text) in generated.iter() {
            work::step();
            let said = text.string.text(strings);
            let runs = generated.runs(text);
            // It stands as the text it ends stands, as Blink shapes a hyphen
            // in its style's font orientation.
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
                // Each run a piece, shaped in its font; from the left as
                // drawn, so from the text's last run where it reads right to
                // left.
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
                        // Pieces hardly outnumber text facts rows. Past what
                        // an id can name, the rest are not drawn, this
                        // text's pieces included, and their lines pay
                        // nothing for them.
                        self.texts.truncate(first_text);
                        self.glyphs.truncate(first_glyph);
                        break 'texts;
                    }
                }
            }
        }
    }

    /// Generated text `id`, or `None` past the last.
    pub(crate) fn get(&self, id: ShapedGeneratedId) -> Option<&ShapedGenerated> {
        self.texts.get(id)
    }

    /// The hyphen of text with the text facts `text`, by halving, or `None`
    /// where it needs none.
    pub(crate) fn hyphen(&self, text: TextFactsId) -> Option<GeneratedItems> {
        self.find(text, 0)
    }

    /// Returns the ellipsis of the block's text facts `text`, by halving.
    ///
    /// `rtl` picks the one a right-to-left line draws; otherwise it is the
    /// left-to-right one. Returns `None` where the text needs none, or no
    /// paragraph reads that way.
    pub(crate) fn ellipsis(&self, text: TextFactsId, rtl: bool) -> Option<GeneratedItems> {
        self.find(text, 1 + u8::from(rtl))
    }

    /// The items of the generated text of `text` at `rank` in
    /// [`ShapedGenerated::rank`]'s order, by halving.
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

    /// How far `text` reaches along a line, exactly: its items' advances.
    pub(crate) fn advance(&self, text: GeneratedItems) -> InlineLayoutUnit {
        text.ids()
            .filter_map(|id| self.texts.get(id))
            .fold(InlineLayoutUnit::ZERO, |sum, piece| sum + piece.advance)
    }

    /// `text`'s width on layout's grid, rounded up, as Chrome snaps a
    /// hyphen's and an ellipsis's width (`ShapeResult::SnappedWidth`, over
    /// every run of the result): what a line ending in it pays, and what an
    /// ellipsis needs room for.
    pub(crate) fn snapped(&self, text: GeneratedItems) -> LayoutUnit {
        self.advance(text)
            .ceil_to_grid()
            .to_layout()
            .max(LayoutUnit::ZERO)
    }

    /// The glyphs of `text`, as drawn: empty where they are not the table's.
    pub(crate) fn glyphs(&self, text: &ShapedGenerated) -> &[SidecarGlyph] {
        self.glyphs
            .get_slice(text.glyphs.clone())
            .unwrap_or_default()
    }

    /// Whether there is none, nor any glyph, which the tests check of
    /// content that needs none.
    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.texts.is_empty() && self.glyphs.is_empty()
    }

    /// Every generated text, in order, which the tests check.
    #[cfg(test)]
    pub(super) fn iter(&self) -> impl ExactSizeIterator<Item = &ShapedGenerated> {
        self.texts.as_slice().iter()
    }
}

/// One piece of a generated text, shaped once per text facts row that can
/// need it.
///
/// A piece is one run in the used font that font selection chose for it.
/// Nearly always it is the whole text in the primary font.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) struct ShapedGenerated {
    /// The text facts it was made for.
    pub(super) text: TextFactsId,
    /// Which it is.
    pub(crate) kind: Generated,
    /// Whether it was shaped right to left: a hyphen in its text's
    /// direction, an ellipsis in that of the lines it ends.
    rtl: bool,
    /// How it stands: as the text it ends does.
    pub(crate) orientation: RunOrientation,
    /// The used font it is set in.
    pub(crate) font: UsedFontId,
    /// Its width along the line, exactly: where the line has it, what it
    /// reaches.
    pub(crate) advance: InlineLayoutUnit,
    /// Its glyphs in [`GeneratedTexts`]' glyph table, as drawn.
    glyphs: Range<SidecarGlyphId>,
}

impl ShapedGenerated {
    /// Where it sorts among its text facts row's: the hyphen, then the
    /// ellipsis left to right, then right to left.
    fn rank(&self) -> u8 {
        match self.kind {
            Generated::Hyphen => 0,
            Generated::Ellipsis => 1 + u8::from(self.rtl),
        }
    }
}
