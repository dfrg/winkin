//! A used font's line metrics.
//!
//! Font selection computes them once per used font, when it first uses an
//! instance at a size. They come from the instance's unscaled metrics, the
//! size and the context's [`Config`](crate::config::Config), whose
//! [`LineMetricsSource`] picks which set of vertical metrics measures a line.
//! The ascent, descent and line gap are rounded to whole pixels, as Windows
//! hands them to Chrome and as Skia rounds them everywhere. No other code
//! computes metrics.
//!
//! A metric the font does not give takes a default, so that a font with
//! broken tables, and a used font with no font at all, still measure:
//!
//! | Metric | Default |
//! |---|---|
//! | ascent, descent, line gap | 0.8, 0.2 and 0 em |
//! | x-height | the top of `x`, else 0.56 × the ascent, as Blink estimates it |
//! | cap height | the top of `H`, else 0.7 em |
//! | em box | the typographic pair scaled to one em, else the line's ascent and descent so scaled, else all above |
//! | ideographic baseline | the descent, rounded: the em box's bottom |
//! | central baseline | the middle of the ascent and descent |
//! | mathematical baseline | half the x-height up |
//! | hanging baseline | 0.8 of the ascent up |
//! | underline, strikeout | none: no thickness, which the painter reads as `auto`'s |
//! | superscript, subscript | 0.34 em up, 0.22 em down |
//! | ideographic advance | 水's advance, else one em |
//!
//! A decoration line the font gives is at least a pixel thick.
//!
//! **A vertical line is centred.** Where the block's lines run down the page
//! with their text upright or mixed, the dominant baseline is the central
//! one, as Blink's `ComputedStyle::GetFontBaseline` has it ([`LineBaseline`]).
//! Every font of the layout is then measured about it:
//! - its line's height, in whole pixels, splits with the odd pixel over, as
//!   `FontMetrics::IntAscentInternal(kCentralBaseline)` splits it;
//! - its em box is the size on the grid halved, as
//!   `NormalizedTypoAscentAndDescent` halves it;
//! - every baseline and decoration line, the alphabetic one included, is an
//!   offset from the central baseline.
//!
//! This module also reads how far a glyph's ink reaches, which an initial
//! letter is fitted to ([`UsedFonts::glyph_ink`]).

use read_fonts::TableProvider;
use read_fonts::model::Font;
use read_fonts::model::metrics::{GlyphMetrics, LineBox, ScaleF32};
use read_fonts::tables::base::{BaseAxis, BaseInstance};
use read_fonts::types::{F48Dot16, GlyphId, Tag};

use super::instance::FaceOverrides;
use super::{GlyphInk, UsedFontId, UsedFonts};
use crate::FontInstance;
use crate::config::LineMetricsSource;
use crate::font::FontMetricsProvider;
use crate::stages::content::Content;
use crate::style::{
    AdjustMetric, DominantBaseline, FontVariantPosition, InitialLetterAlign, TextBoxEdge,
    TextEdgeOver, TextEdgeUnder,
};
use crate::unit::{LayoutUnit, TextUnit};

/// The line metrics a font has where it gives none: 0.8 em above, 0.2 below
/// and no gap, which make one em between them, as Ahem does.
const DEFAULT_LINE: (f32, f32, f32) = (0.8, 0.2, 0.0);

/// The baseline a font is measured about in the layout's lines.
///
/// It is the line's dominant baseline. The block's `writing-mode` and
/// `text-orientation` choose it, as Blink's `ComputedStyle::GetFontBaseline`
/// chooses the baseline of an inline formatting context from its block's
/// style.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub(crate) enum LineBaseline {
    /// The alphabetic baseline: horizontal lines, and vertical ones whose
    /// text is on its side.
    #[default]
    Alphabetic,
    /// The central baseline, the middle of the line's text: `vertical-rl`
    /// and `vertical-lr` with `text-orientation: mixed` or `upright`.
    Central,
}

impl LineBaseline {
    /// Returns the baseline `content`'s lines are set on, which its block
    /// chooses.
    ///
    /// Blink sets every box of an inline formatting context on the baseline
    /// its block's style chooses (`InlineBoxState::ComputeTextMetrics`). So
    /// a span turned on its side in a mixed column is centred as the column
    /// is. The baseline is central where the block's text stands upright in
    /// a vertical line, or is mixed.
    pub(crate) fn from_content(content: &Content) -> Self {
        if content.block.central_baseline {
            Self::Central
        } else {
            Self::Alphabetic
        }
    }
}

/// How a used font measures a line, in pixels at its size on layout's grid.
///
/// The ascent, descent and line gap are whole pixels. Heights are positive.
/// The ascent and descent are about the line's dominant baseline
/// ([`LineBaseline`]). The baselines and the decoration lines are offsets
/// from it, positive downward, as a line box's y is. The alphabetic offset is
/// zero except in a centred vertical line, where it is under the central
/// baseline. The superscript and subscript values say how far each moves a
/// baseline, not where one is: the first is negative, the second positive.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct FontLineMetrics {
    /// The ascent above the dominant baseline, in whole pixels.
    pub(crate) ascent: LayoutUnit,
    /// The descent below the dominant baseline, in whole pixels, positive.
    pub(crate) descent: LayoutUnit,
    /// How far the alphabetic baseline is under the dominant one.
    ///
    /// It is zero in a horizontal line. In a centred vertical line it is how
    /// far under the central baseline sideways text sits.
    pub(crate) alphabetic: LayoutUnit,
    /// The line gap `line-height: normal` adds, in whole pixels, which a
    /// line reads in its [`normal_line_height`](Self::normal_line_height).
    pub(super) line_gap: LayoutUnit,
    /// The height of a lowercase `x`.
    pub(crate) x_height: LayoutUnit,
    /// The height of a capital.
    pub(crate) cap_height: LayoutUnit,
    /// How far the em box reaches above the baseline.
    pub(crate) em_over: LayoutUnit,
    /// How far the em box reaches below the baseline: with `em_over`, one
    /// em.
    pub(crate) em_under: LayoutUnit,
    /// The ideographic baseline's offset: below the alphabetic one.
    pub(crate) ideographic: LayoutUnit,
    /// The central baseline's offset: above the alphabetic one, negative.
    pub(crate) central: LayoutUnit,
    /// The mathematical baseline's offset.
    mathematical: LayoutUnit,
    /// The hanging baseline's offset.
    hanging: LayoutUnit,
    /// Where `post`'s underline's top sits, in whole pixels: below the
    /// baseline, positive.
    ///
    /// The painter hands it and the three below out for `from-font`
    /// decorations (`paint::Decoration`).
    pub(crate) underline_offset: LayoutUnit,
    /// `post`'s underline thickness: at least a pixel, zero where the font
    /// gives none.
    pub(crate) underline_thickness: LayoutUnit,
    /// Where `OS/2`'s strikeout's top sits: above the baseline, negative.
    pub(crate) strikeout_offset: LayoutUnit,
    /// `OS/2`'s strikeout thickness: at least a pixel, zero where the font
    /// gives none.
    pub(crate) strikeout_thickness: LayoutUnit,
    /// Where `OS/2` puts a superscript's baseline: negative, raised.
    pub(crate) superscript: LayoutUnit,
    /// Where `OS/2` puts a subscript's baseline: positive, lowered.
    pub(crate) subscript: LayoutUnit,
    /// The advance of the font's space at the used size, 16.16.
    ///
    /// Chrome reads it as `SimpleFontData::SpaceWidth`. A `tab-size` given as
    /// a number counts in it, and a `word-spacing` percentage is of it.
    pub(crate) space: TextUnit,
    /// The ideographic advance, CSS's `ic`, at the used size, 16.16.
    ///
    /// It is the advance of U+6C34 水, or one em where the font does not map
    /// it, as Chrome's `SimpleFontData::IdeographicInlineSize` falls back to
    /// the font's size. `text-autospace` puts an eighth of it between an
    /// ideograph and a letter. It is the horizontal advance, which an upright
    /// vertical line also takes as its `ic`: nothing reads the vertical
    /// advance.
    pub(super) ideographic_advance: TextUnit,
}

impl FontLineMetrics {
    /// Returns how far `edge`'s over edge stands over the baseline.
    ///
    /// - `text`, `cap` and `ex` give the ascent, cap height and x-height, as
    ///   Chrome's `AdjustEdges` reads `FixedAscent`, `FixedCapHeight` and
    ///   `FixedXHeight`.
    /// - Beyond Chrome, which parses neither, `ideographic` gives the top of
    ///   the ideographic em box. `ideographic-ink` gives the same, since font
    ///   metrics carry no character face.
    ///
    /// Cap height and x-height stand over the alphabetic baseline. In a
    /// vertical line set on its central baseline, the alphabetic offset comes
    /// off them. Chrome 153 trims a 40 px line of 20 px Ahem to 30 at its
    /// start and 26 at its end under `cap alphabetic` in `vertical-rl`.
    pub(crate) fn over_edge(&self, edge: TextBoxEdge) -> LayoutUnit {
        let alphabetic = self.alphabetic;
        match edge.over {
            TextEdgeOver::Text => self.ascent,
            TextEdgeOver::Cap => self.cap_height - alphabetic,
            TextEdgeOver::Ex => self.x_height - alphabetic,
            // The ideographic baseline is already an offset from the lines'.
            TextEdgeOver::Ideographic | TextEdgeOver::IdeographicInk => {
                self.em_over + self.em_under - self.ideographic
            }
        }
    }

    /// Returns how far `edge`'s under edge stands under the baseline.
    ///
    /// `text` gives the descent and `alphabetic` the alphabetic baseline, as
    /// Chrome's `AdjustEdges` reads them. Beyond Chrome, `ideographic` and
    /// `ideographic-ink` give the bottom of the ideographic em box.
    pub(crate) fn under_edge(&self, edge: TextBoxEdge) -> LayoutUnit {
        match edge.under {
            TextEdgeUnder::Text => self.descent,
            TextEdgeUnder::Alphabetic => self.alphabetic,
            TextEdgeUnder::Ideographic | TextEdgeUnder::IdeographicInk => self.ideographic,
        }
    }
}

/// A font's metrics in ems at an instance's coordinates, before any size.
///
/// They are read once, when the instance is made, before any choice of which
/// set measures a line. A value the font does not give is `None`, and the
/// used font's line metrics fill it in from the defaults in the module
/// documentation. A font whose `head` cannot be read, or says its em is zero,
/// gives nothing at all. Descents are positive downward. So are the offsets
/// of the baselines and decoration lines from the alphabetic baseline: a
/// superscript's offset is negative.
#[derive(Copy, Clone, PartialEq, Debug, Default)]
pub(super) struct UnscaledMetrics {
    /// `OS/2`'s `usWinAscent` and `usWinDescent`, where they are not both zero.
    win: Option<[f32; 2]>,
    /// `hhea`'s ascender, descender and line gap, where they are not all zero.
    hhea: Option<[f32; 3]>,
    /// `OS/2`'s typographic ascender, descender and line gap.
    typo: Option<[f32; 3]>,
    /// `OS/2` sets `USE_TYPO_METRICS`.
    use_typo: bool,
    /// `OS/2`'s x-height, or the top of `x`'s outline.
    x_height: Option<f32>,
    /// `OS/2`'s cap height, or the top of `H`'s outline.
    cap_height: Option<f32>,
    /// `post`'s underline: its offset and thickness.
    underline: Option<[f32; 2]>,
    /// `OS/2`'s strikeout: its offset and thickness.
    strikeout: Option<[f32; 2]>,
    /// `OS/2`'s superscript offset.
    superscript: Option<f32>,
    /// `OS/2`'s subscript offset.
    subscript: Option<f32>,
    /// `OS/2`'s superscript and subscript sizes, as ratios of the em.
    ///
    /// A synthesized position is drawn at this size. With the two offsets
    /// above, it is all that synthesis reads.
    script_sizes: Option<[f32; 2]>,
    /// `BASE`'s ideographic baseline, `ideo`.
    ideographic: Option<f32>,
    /// `BASE`'s central baseline: `idce`, else the middle of `ideo` and
    /// `idtp` (one em above `ideo` where the font names no `idtp`), else the
    /// middle of `icfb` and `icft`.
    central: Option<f32>,
    /// `BASE`'s mathematical baseline, `math`.
    mathematical: Option<f32>,
    /// `BASE`'s hanging baseline, `hang`.
    hanging: Option<f32>,
    /// The advance of U+0020, `hmtx` moved by `HVAR` at the coordinates.
    ///
    /// A `tab-size` given as a number counts in it.
    space: Option<f32>,
    /// The advance of `0`, where the font maps it: the `ch` unit, and what
    /// `font-size-adjust: ch-width` holds.
    zero: Option<f32>,
    /// The advance of U+6C34 水, where the font maps it: the `ic` unit, and
    /// what `font-size-adjust: ic-width` holds.
    ideographic_width: Option<f32>,
    /// The vertical advance of U+6C34 水 from `vmtx`, where the font maps it
    /// and has one: what `font-size-adjust: ic-height` holds.
    ideographic_height: Option<f32>,
    /// The font's units per em, zero where it cannot be read.
    ///
    /// A metric in ems is scaled back to units by it where it must scale as
    /// a platform scales it: units times the size over the em.
    em: f32,
}

impl UnscaledMetrics {
    /// Reads the metrics of `font` at an instance's coordinates.
    ///
    /// read-fonts applies `MVAR` to the font's own metrics, `HVAR` to the
    /// advances and the instance's outlines to the glyphs' bounds.
    ///
    /// Never fails and never panics: a table that is missing or cannot be
    /// read gives `None` for what it holds.
    pub(super) fn new(font: &Font) -> Self {
        let metrics = font.metrics();
        let em = match metrics.units_per_em {
            0 => return Self::default(),
            em => f32::from(em),
        };
        // In ems, from read-fonts' design units, each with the delta its
        // instance's coordinates give it already applied.
        let ems = |units: F48Dot16| units.to_f32() / em;
        let style = font.style_metrics();
        let glyphs = font.glyph_metrics();
        let charmap = font.charmap();
        // Where the font says nothing, the line as `[over, under, gap]`,
        // descents positive downward. A zeroed `hhea` says nothing, and
        // FreeType reads past it.
        let line = |line: LineBox| [ems(line.ascender), -ems(line.descender), ems(line.line_gap)];
        let hhea = metrics
            .hhea_line
            .filter(|line| line.ascender != F48Dot16::ZERO || line.descender != F48Dot16::ZERO)
            .map(line);
        let top_em = |ch: char| glyph_top(&glyphs, charmap.map_unicode(ch)?).map(|top| top / em);
        let base = font
            .tables()
            .base()
            .ok()
            .map(|base| BaseInstance::with_coords(base, font.normalized_coords()));
        let base =
            |tag: &[u8; 4]| base_baseline(base.as_ref()?, Tag::new(tag)).map(|coord| -coord / em);
        let ideographic = base(b"ideo");
        let central = base(b"idce")
            .or_else(|| {
                // `idtp` is the em box's top, one em above its bottom where
                // the font names none; offsets here grow downward.
                ideographic.map(|bottom| (bottom + base(b"idtp").unwrap_or(bottom - 1.0)) / 2.0)
            })
            .or_else(|| Some((base(b"icfb")? + base(b"icft")?) / 2.0));
        let advance_em = |glyph: GlyphId| glyphs.h_advance(glyph) / em;
        let mapped_em = |ch: char| charmap.map_unicode(ch).map(advance_em);
        // A strictly positive height: `OS/2` gives one only from version 2,
        // and a zero says nothing.
        let height =
            |units: Option<F48Dot16>| units.filter(|units| *units > F48Dot16::ZERO).map(ems);
        Self {
            // A zeroed win pair says nothing either: DirectWrite then reports
            // the `hhea` line, which Skia hands to Chrome on Windows.
            win: metrics
                .win_line
                .filter(|win| win.ascender != F48Dot16::ZERO || win.descender != F48Dot16::ZERO)
                .map(|win| [ems(win.ascender), -ems(win.descender)]),
            hhea,
            typo: metrics.typo_line.map(line),
            use_typo: metrics.use_typo_metrics,
            x_height: height(metrics.x_height).or_else(|| top_em('x')),
            cap_height: height(metrics.cap_height).or_else(|| top_em('H')),
            // `post` measures its underline upward, and offsets here grow
            // downward.
            underline: style
                .underline
                .map(|line| [-ems(line.position), ems(line.thickness)]),
            strikeout: style
                .strikethrough
                .map(|line| [-ems(line.position), ems(line.thickness)]),
            superscript: style.superscript.map(|script| -ems(script.y_offset)),
            subscript: style.subscript.map(|script| ems(script.y_offset)),
            script_sizes: style
                .superscript
                .zip(style.subscript)
                .map(|(sup, sub)| [ems(sup.y_size), ems(sub.y_size)]),
            ideographic,
            central,
            mathematical: base(b"math"),
            hanging: base(b"hang"),
            // `.notdef`'s where the font maps no space, as Chrome's
            // `SpaceWidth` is the width of whatever glyph a space gets.
            space: Some(advance_em(
                charmap.map_unicode(' ').unwrap_or(GlyphId::NOTDEF),
            )),
            zero: mapped_em('0'),
            ideographic_width: mapped_em('\u{6C34}'),
            ideographic_height: charmap
                .map_unicode('\u{6C34}')
                .and_then(|glyph| font.tables().vmtx().ok()?.advance(glyph))
                .map(|advance| f32::from(advance) / em),
            em,
        }
    }

    /// Returns the advance of the font's space at `px`, 16.16: a quarter of
    /// an em, a common space's width, where it gives none.
    pub(super) fn space(&self, px: f32) -> TextUnit {
        TextUnit::from_px(self.space.unwrap_or(0.25) * px)
    }

    /// Returns the aspect `font-size-adjust` holds: how much of the em
    /// `metric` is.
    ///
    /// The font is used at `px` with the `@font-face` `overrides`, measuring
    /// lines as `source` says. The aspect is 1 where the font does not give
    /// it, as Chrome takes it (`FontSizeFunctions::AspectValue`, after
    /// csswg-drafts#6384). A font that maps U+6C34 but has no `vmtx` takes
    /// its line's height as its `ic-height`, as Blink's
    /// `OpenTypeVerticalData` falls back.
    pub(super) fn aspect(
        &self,
        metric: AdjustMetric,
        overrides: &FaceOverrides,
        px: f32,
        source: LineMetricsSource,
    ) -> f32 {
        let aspect = match metric {
            AdjustMetric::ExHeight => self.x_height,
            AdjustMetric::CapHeight => self.cap_height,
            AdjustMetric::ChWidth => self.zero,
            AdjustMetric::IcWidth => self.ideographic_width,
            AdjustMetric::IcHeight => self.ideographic_height.or_else(|| {
                self.ideographic_width?;
                let line =
                    FontLineMetrics::new(self, overrides, px, source, LineBaseline::Alphabetic);
                (px > 0.0).then(|| (line.ascent + line.descent).to_px() / px)
            }),
        };
        aspect.filter(|a| a.is_finite() && *a > 0.0).unwrap_or(1.0)
    }

    /// Returns how far over the alphabetic baseline an initial letter's top
    /// reaches, for `align`, in the font at `px` pixels.
    ///
    /// Lines are measured as `source` says. The top is:
    /// - the cap height for `alphabetic`, as Chrome's
    ///   `ComputeInitialLetterFont` reads it;
    /// - the hanging baseline for `hanging`;
    /// - the whole em box for `ideographic`, whose under point is its bottom.
    ///
    /// Each takes the line metrics' default where the font gives none. It
    /// scales as Skia scales a metric, its units times the size over the em.
    /// So Ahem's capitals at 13 px are 10.4 in the float Chrome compares, not
    /// a hair over.
    pub(super) fn initial_letter_over(
        &self,
        align: InitialLetterAlign,
        source: LineMetricsSource,
        px: f32,
    ) -> f32 {
        let over = match align {
            InitialLetterAlign::Alphabetic => self.cap_height.unwrap_or(0.7),
            InitialLetterAlign::Hanging => self.hanging.map_or_else(
                || 0.8 * self.line(source).unwrap_or(DEFAULT_LINE).0,
                |hanging| -hanging,
            ),
            InitialLetterAlign::Ideographic => 1.0,
        };
        if self.em > 0.0 {
            px * (over * self.em) / self.em
        } else {
            px * over
        }
    }

    /// Returns how a superscript or subscript is synthesized in the font used
    /// at `px`: the glyphs' size in pixels and how far they are raised.
    ///
    /// Returns `None` for `normal`. The font's own `OS/2` size and offset
    /// apply where the size is a third of an em to one em and the offset is
    /// within an em. Otherwise the size is two thirds, and the offset 0.34 em
    /// up or 0.22 em down. Nothing else is read: `Config::super_sub` belongs
    /// to `vertical-align`, and moves a box, not a glyph. CSS Fonts 4,
    /// section 6.5, notes the `OS/2` values "are not always accurate in
    /// practice", which is what the ranges guard against.
    pub(super) fn synthesized_position(
        &self,
        position: FontVariantPosition,
        px: f32,
    ) -> Option<(f32, LayoutUnit)> {
        let sup = match position {
            FontVariantPosition::Normal => return None,
            FontVariantPosition::Super => true,
            FontVariantPosition::Sub => false,
        };
        let scale = self
            .script_sizes
            .map(|[sup_size, sub_size]| if sup { sup_size } else { sub_size })
            .filter(|scale| (1.0 / 3.0..=1.0).contains(scale))
            .unwrap_or(2.0 / 3.0);
        // Offsets grow downward here: a superscript's is negative.
        let offset = if sup {
            self.superscript.filter(|o| (-1.0..=0.0).contains(o))
        } else {
            self.subscript.filter(|o| (0.0..=1.0).contains(o))
        }
        .unwrap_or(if sup { -0.34 } else { 0.22 });
        Some((px * scale, LayoutUnit::from_px(-offset * px)))
    }

    /// Returns the ascent, descent and line gap `source` names, in ems.
    ///
    /// Falls back to the other table where the font lacks the one it names.
    /// Returns `None` where it has neither.
    fn line(&self, source: LineMetricsSource) -> Option<(f32, f32, f32)> {
        // The line gap DirectWrite reports beside the win pair: what is left
        // of the `hhea` line to reach, which is nothing where the win pair
        // already covers it. Measured from Consolas, whose win pair is its
        // `hhea` line and whose reported gap is zero, and from Yu Gothic,
        // whose 1024-unit `hhea` gap comes back as 645.
        let win = self.win.map(|[ascent, descent]| {
            let gap = self
                .hhea
                .map_or(0.0, |[a, d, g]| (a + d + g - ascent - descent).max(0.0));
            (ascent, descent, gap)
        });
        let hhea = self.hhea.map(|[a, d, g]| (a, d, g));
        let typo = self.typo.map(|[a, d, g]| (a, d, g));
        match source {
            LineMetricsSource::Win => win.or(hhea),
            LineMetricsSource::Hhea => hhea.or(win),
            LineMetricsSource::TypoOrHhea if self.use_typo => typo.or(hhea),
            LineMetricsSource::TypoOrHhea => hhea.or(win),
        }
    }
}

impl FontLineMetrics {
    /// Scales `unscaled` to `size` pixels, measuring lines as `source` says.
    ///
    /// The `@font-face` descriptors' `overrides` replace what they override.
    /// An override replaces its metric before rounding, as Blink puts one in
    /// place of Skia's (`FontMetrics::AscentDescentWithHacks`). So an
    /// overridden line rounds as any other. An override is a ratio of the
    /// size the font is used at, `size-adjust` included.
    ///
    /// `baseline` is the line's dominant baseline. Where it is the central
    /// one, the line's height splits with the odd pixel over, and everything
    /// else is measured from it
    /// ([`with_central_baseline`](Self::with_central_baseline)).
    pub(super) fn new(
        unscaled: &UnscaledMetrics,
        overrides: &FaceOverrides,
        size: f32,
        source: LineMetricsSource,
        baseline: LineBaseline,
    ) -> Self {
        let metrics = Self::from_unscaled(unscaled, overrides, size, source);
        match baseline {
            LineBaseline::Alphabetic => metrics,
            LineBaseline::Central => metrics.with_central_baseline(size),
        }
    }

    /// Returns the same metrics about the central baseline of a vertical
    /// line, the font being used at `size` pixels.
    ///
    /// The line's height `h` (a whole pixel ascent plus descent) splits into
    /// `h − ⌊h/2⌋` over and `⌊h/2⌋` under, the odd pixel over. Blink's
    /// `FontMetrics::IntAscentInternal(kCentralBaseline)` is `Height() -
    /// Height() / 2` (`platform/fonts/font_metrics.cc`), and Chrome 153 sets
    /// 15px Ahem 8 over and 7 under. The split ignores the font's `central`
    /// baseline, whose fallback rounds the other way, and its `BASE` table,
    /// which Blink does not read here.
    ///
    /// The em box is the size split the same way on the grid, `h/2`
    /// truncated under, as `SimpleFontData::NormalizedTypoAscentAndDescent`
    /// splits it for a baseline other than the alphabetic. The baselines and
    /// decoration lines keep their places in the font, each as an offset
    /// from the central baseline. The superscript and subscript shifts are
    /// distances, and stay.
    fn with_central_baseline(self, size: f32) -> Self {
        let height = self.ascent + self.descent;
        let under = height.half().floor_px();
        let over = height - under;
        // How far the alphabetic baseline is under the central one.
        let alphabetic = self.ascent - over;
        let em = LayoutUnit::from_px(size);
        let em_under = em.half();
        Self {
            ascent: over,
            descent: under,
            alphabetic,
            em_over: em - em_under,
            em_under,
            ideographic: self.ideographic + alphabetic,
            central: LayoutUnit::ZERO,
            mathematical: self.mathematical + alphabetic,
            hanging: self.hanging + alphabetic,
            underline_offset: self.underline_offset + alphabetic,
            strikeout_offset: self.strikeout_offset + alphabetic,
            ..self
        }
    }

    /// Returns the metrics about the alphabetic baseline.
    fn from_unscaled(
        unscaled: &UnscaledMetrics,
        overrides: &FaceOverrides,
        size: f32,
        source: LineMetricsSource,
    ) -> Self {
        let (ascent, descent, gap) = unscaled.line(source).unwrap_or(DEFAULT_LINE);
        let (over_ascent, over_descent, over_gap) = overrides.line();
        let (ascent, descent, gap) = (
            over_ascent.unwrap_or(ascent),
            over_descent.unwrap_or(descent),
            over_gap.unwrap_or(gap),
        );
        let (ascent_px, descent_px) = (ascent * size, descent * size);
        let px = |em: f32| LayoutUnit::from_px(em * size);
        let x_height = unscaled.x_height.unwrap_or(0.56 * ascent);
        // The em box is the font's typographic pair scaled to one em. The font
        // puts its letters in the ideographic em box there, and ruby and
        // emphasis marks are set against it. Blink's
        // `ComputeNormalizedTypoAscentAndDescent` takes the pair where its
        // ascender is above zero. Otherwise it takes the line's ascent and
        // descent as the font reports them, in whole pixels. Either applies
        // only where the ascent is within the height and the height is above
        // zero. The ascent rounds onto layout's 1/64 grid, and the descent is
        // the rest of the em. Chrome 153 sets a ruby annotation over Yu
        // Gothic, whose box's top is 26.39px up at 30px, 27px up: the 1/64
        // value ceiled (`ComputeEmHeight`). Rounding to a whole pixel would
        // give 26. A font with neither is all above, where Blink's is empty.
        let valid = |(over, under): (f32, f32)| {
            let height = over + under;
            (height > 0.0 && over >= 0.0 && over <= height).then_some((over, height))
        };
        let pair = unscaled
            .typo
            .filter(|[a, _, _]| *a > 0.0)
            .and_then(|[a, d, _]| valid((a, d)))
            .or_else(|| {
                let (over, under) = (
                    LayoutUnit::from_px_whole(ascent_px).to_px(),
                    LayoutUnit::from_px_whole(descent_px).to_px(),
                );
                valid((over, under))
            });
        let em = LayoutUnit::from_px(size);
        let em_over = match pair {
            Some((over, height)) => LayoutUnit::from_px(over * size / height),
            None => em,
        };
        let em_under = em - em_over;
        // A line the font gives is a pixel thick at least, and one it does
        // not give has no thickness.
        let line = |line: Option<[f32; 2]>| {
            line.map_or((0.0, LayoutUnit::ZERO), |[offset, thickness]| {
                (offset, LayoutUnit::from_px((thickness * size).max(1.0)))
            })
        };
        let (underline, underline_thickness) = line(unscaled.underline);
        let (strikeout, strikeout_thickness) = line(unscaled.strikeout);
        Self {
            ascent: LayoutUnit::from_px_whole(ascent_px),
            descent: LayoutUnit::from_px_whole(descent_px),
            alphabetic: LayoutUnit::ZERO,
            line_gap: LayoutUnit::from_px_whole((gap * size).max(0.0)),
            x_height: px(x_height),
            cap_height: px(unscaled.cap_height.unwrap_or(0.7)),
            em_over,
            em_under,
            // Where the font says nothing, these fall back to the bottom of
            // the line's descent and the middle of its ascent and descent,
            // which match Chrome.
            ideographic: unscaled
                .ideographic
                .map_or(LayoutUnit::from_px_whole(descent_px), px),
            central: unscaled.central.map_or_else(
                || -LayoutUnit::from_px_whole((ascent_px - descent_px) / 2.0),
                px,
            ),
            mathematical: unscaled
                .mathematical
                .map_or_else(|| -LayoutUnit::from_px_whole(x_height * size / 2.0), px),
            hanging: unscaled.hanging.map_or_else(|| px(-0.8 * ascent), px),
            underline_offset: LayoutUnit::from_px_whole(underline * size),
            underline_thickness,
            strikeout_offset: px(strikeout),
            strikeout_thickness,
            superscript: px(unscaled.superscript.unwrap_or(-0.34)),
            subscript: px(unscaled.subscript.unwrap_or(0.22)),
            space: unscaled.space(size),
            ideographic_advance: TextUnit::from_px(
                unscaled.ideographic_width.unwrap_or(1.0) * size,
            ),
        }
    }

    /// Returns the metrics of a used font with no font at all, at `size`
    /// pixels: every default in the module's table.
    pub(super) fn from_size(size: f32, baseline: LineBaseline) -> Self {
        // Any source: there is nothing to choose between.
        Self::new(
            &UnscaledMetrics::default(),
            &FaceOverrides::NONE,
            size,
            LineMetricsSource::Win,
            baseline,
        )
    }

    /// Returns how far over the dominant baseline the middle of the ascent
    /// and descent is.
    ///
    /// That middle lies between the text-over and text-under baselines, and
    /// combined text is centred on it (CSS Writing Modes 3, section 9.1.2).
    /// It is half a pixel in a centred line of odd height, whose odd pixel is
    /// over. On the alphabetic baseline it is half their difference.
    pub(crate) fn text_middle(&self) -> LayoutUnit {
        (self.ascent - self.descent).half()
    }

    /// The font's ascent over its alphabetic baseline, whole pixels, as
    /// Skia hands it to Chrome, whichever baseline the line is centred on:
    /// what Blink's vertical metrics fall back to where a font has none.
    pub(crate) fn alphabetic_ascent(&self) -> LayoutUnit {
        self.ascent + self.alphabetic
    }

    /// The font's descent under its alphabetic baseline, likewise.
    pub(crate) fn alphabetic_descent(&self) -> LayoutUnit {
        self.descent - self.alphabetic
    }

    /// Returns the font's own line spacing, which `line-height: normal` uses.
    ///
    /// It sums the ascent, descent and line gap, each already a whole pixel.
    pub(crate) fn normal_line_height(&self) -> LayoutUnit {
        self.ascent + self.descent + self.line_gap
    }

    /// Whether `other` has this font's ascent, descent and line gap:
    /// Blink's `HasIdenticalAscentDescentAndLineGap`, which decides whether
    /// a box set in another font than its parent's changes what its
    /// fragment would report.
    pub(crate) fn same_line_metrics(&self, other: &Self) -> bool {
        self.ascent == other.ascent
            && self.descent == other.descent
            && self.line_gap == other.line_gap
    }

    /// Returns how far below the line's dominant baseline the font puts the
    /// baseline `dominant` names, positive down.
    ///
    /// It is zero for `auto`, which is the dominant one outside SVG. It is
    /// zero for the alphabetic one except in a centred vertical line.
    /// Raising a box by it sets that baseline where the dominant one was.
    pub(crate) fn baseline(&self, dominant: DominantBaseline) -> LayoutUnit {
        match dominant {
            DominantBaseline::Auto => LayoutUnit::ZERO,
            DominantBaseline::Alphabetic => self.alphabetic,
            DominantBaseline::Ideographic => self.ideographic,
            DominantBaseline::Central => self.central,
            DominantBaseline::Mathematical => self.mathematical,
        }
    }
}

/// The top of `glyph`'s outline in font units, at the instance's
/// coordinates, or `None` where its outline has no bounds or none above the
/// baseline.
///
/// What a platform reads for an x-height or a cap height the `OS/2` table
/// does not give: DirectWrite and FreeType both measure the glyph.
fn glyph_top(glyphs: &GlyphMetrics<'_>, glyph: GlyphId) -> Option<f32> {
    let top = glyphs.extents(glyph)?.y_bearing;
    (top > 0.0).then_some(top)
}

/// One baseline from `BASE`'s horizontal axis, read at the instance's
/// coordinates, in font units upward from the alphabetic baseline, or
/// `None` where the font does not name it.
///
/// A coordinate is a height in the glyphs' own design space, whose origin is
/// the alphabetic baseline. It is not an offset from the script's default
/// baseline. Only the first script record's values are read: a font that
/// distinguishes scripts names the same baselines in each.
fn base_baseline(base: &BaseInstance<'_>, wanted: Tag) -> Option<f32> {
    let axis = base.axis(BaseAxis::Horizontal)?;
    let tags = axis.base_tag_list()?.ok()?;
    let scripts = axis.base_script_list().ok()?;
    let record = scripts.base_script_records().first()?;
    let script = record.base_script(scripts.offset_data()).ok()?;
    let values = script.base_values()?.ok()?;
    let at = tags
        .baseline_tags()
        .iter()
        .position(|tag| tag.get() == wanted)?;
    let coord = values.base_coords().get(at).ok()?;
    Some(base.coord(&coord).to_f32())
}

// A glyph's ink ------------------------------------------------------------
//
// How far a glyph's ink reaches. Layout reads it to fit an initial letter's
// box.
//
// CSS Inline 3, section 7.5.5, fits the box to "the glyph outlines", and
// this reads them. It takes the outline as designed, at the size its glyphs are set at,
// rounded out to whole pixels as Skia bounds a glyph. Chrome reads its
// rasterizer's bounds (`ShapeResult::ComputeInkBounds`), which DirectWrite
// hints on Windows. The engine does no hinting of its own: hinting is costly,
// and a host may cache it or turn it off. Hinted bounds come from the host's
// `FontMetricsProvider` when one is given.

impl UsedFonts {
    /// The ink of `glyph` in the used font `font`, at the size its glyphs
    /// are set at, from its origin: its outline as designed at the
    /// instance's coordinates, as read-fonts bounds it, rounded out; or the
    /// host's, where `provider` gives it. `None` where it draws nothing, or
    /// the used font has no font to draw with.
    pub(crate) fn glyph_ink(
        &self,
        font: UsedFontId,
        glyph: u32,
        provider: Option<&dyn FontMetricsProvider>,
    ) -> Option<GlyphInk> {
        let used_font = self.get(font)?;
        let instance = used_font.instance.as_ref()?;
        let px = used_font.glyph_size().to_px();
        if let Some(provider) = provider {
            let font = FontInstance {
                bytes: &instance.bytes,
                index: instance.index,
                coords: &instance.coords,
                size: px,
                embolden: instance.embolden,
                skew: instance.skew,
            };
            if let Some(bounds) = provider.glyph_extents(font, glyph) {
                return bounds.and_then(|bounds| {
                    GlyphInk::from_outline_box(
                        TextUnit::from_raw(bounds.x_min).to_px(),
                        TextUnit::from_raw(bounds.x_max).to_px(),
                        TextUnit::from_raw(bounds.y_min).to_px(),
                        TextUnit::from_raw(bounds.y_max).to_px(),
                    )
                });
            }
        }
        let font = &instance.font;
        let ink = font
            .glyph_metrics()
            .scaled(ScaleF32::from_ppem(px, font.units_per_em()))
            .extents(GlyphId::new(glyph))?;
        GlyphInk::from_outline_box(
            ink.x_bearing,
            ink.x_bearing + ink.width,
            ink.y_bearing - ink.height,
            ink.y_bearing,
        )
    }
}

impl GlyphInk {
    /// The ink of an outline whose box reaches from `x_min` to `x_max` along
    /// the line and from `y_min` to `y_max` up, rounded out to whole pixels,
    /// as Skia bounds a glyph (`SkRect::roundOut`): `None` where it is empty
    /// or not a number. Each edge is put on layout's grid first, so that an
    /// outline scaled to land on a pixel's edge, which the float scaling can
    /// overshoot by a hair, rounds out to that edge and not past it.
    fn from_outline_box(x_min: f32, x_max: f32, y_min: f32, y_max: f32) -> Option<Self> {
        let finite = [x_min, x_max, y_min, y_max]
            .iter()
            .all(|edge| edge.is_finite());
        if !finite || x_max <= x_min || y_max <= y_min {
            return None;
        }
        Some(Self {
            left: LayoutUnit::from_px(x_min).floor_px(),
            right: LayoutUnit::from_px(x_max).ceil_px(),
            over: LayoutUnit::from_px(y_max).ceil_px(),
            under: LayoutUnit::from_px(-y_min).ceil_px(),
        })
    }
}
