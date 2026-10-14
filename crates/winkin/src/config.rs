//! Configuration for behavior that CSS properties do not determine.
//!
//! [`Config`] selects font line metrics, superscript offsets and other
//! implementation choices. Hit testing takes [`PastLines`] per call, and
//! word motion takes [`Motion`](crate::selection::Motion).
//!
//! A context starts with the preset for its platform. The presets are
//! modelled on Chrome on each platform, except that they follow CSS for
//! platform font variations and position synthesis; fields reproduce
//! Chrome there. [`Config::spec`] follows CSS wherever it differs.

use parlance::Language;

/// Layout configuration beyond CSS properties.
///
/// Set with [`Context::set_config`](crate::Context::set_config). Start
/// from a preset and override fields:
///
/// ```
/// use winkin::config::{Config, Pretty};
///
/// let mut config = Config::chrome_windows();
/// config.pretty = Pretty::Even;
/// ```
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub struct Config {
    /// The font metrics used for line ascent, descent and line gap.
    pub line_metrics: LineMetricsSource,
    /// The language for font selection and shaping where no style sets one.
    ///
    /// Controls generic-family resolution, fallback, regional Han forms,
    /// localized glyphs and synthesized small capitals. The Chrome presets
    /// use `en`, as for an English UI; [`Config::spec`] uses `und`.
    ///
    /// `und` selects fonts by the script of each run, including Han fonts
    /// for spaces and punctuation within Han runs.
    ///
    /// Not used for line breaking, hyphenation, `text-transform` or
    /// `text-autospace`. Changes take effect on rebuild; reshaping at line
    /// edges keeps the language of the build.
    pub default_language: Language,
    /// The offset for `vertical-align: super` and `sub`.
    ///
    /// Synthesized `font-variant-position` uses font metrics instead.
    pub super_sub: SuperSubPosition,
    /// Whether `dominant-baseline` affects inline alignment.
    ///
    /// The Chrome presets ignore the property, as Chrome does outside SVG.
    /// Changes take effect on rebuild.
    pub dominant_baseline: DominantBaselines,
    /// Whether to synthesize missing `font-variant-position` glyphs.
    ///
    /// CSS requires synthesis where `sups` or `subs` does not cover the run;
    /// Chrome applies only the feature. Changes take effect on rebuild.
    pub position_synthesis: PositionSynthesis,
    /// The variation settings applied to platform fonts.
    ///
    /// Applies every CSS variation, or only the matching axes as Chrome on
    /// Windows and Linux does. Changes take effect on rebuild.
    pub platform_font_variations: PlatformFontVariations,
    /// Whether small kana may start a line under `line-break: normal`.
    ///
    /// Changes take effect on rebuild.
    pub small_kana: SmallKana,
    /// The punctuation-trimming rule for `text-spacing-trim`.
    ///
    /// Trims through the font's `halt` feature, as Chrome does, or always.
    /// Changes take effect on rebuild.
    pub punctuation_trim: PunctuationTrim,
    /// The fitting rule for `text-wrap-style: pretty`.
    ///
    /// Changes take effect at the next [`break_lines`](crate::Layout::break_lines) call.
    pub pretty: Pretty,
    /// Whether justification expands tabs.
    ///
    /// Changes take effect at the next [`break_lines`](crate::Layout::break_lines) call.
    pub tab_justification: TabJustification,
    /// Whether white space just before an ellipsis is kept.
    ///
    /// Changes take effect at the next [`break_lines`](crate::Layout::break_lines) call.
    pub ellipsis_space: EllipsisSpace,
    /// The characters `word-spacing` applies to, leading spaces included.
    ///
    /// Changes take effect on rebuild.
    pub word_spacing: WordSpacing,
    /// The ruby overhang rule.
    ///
    /// Changes take effect on rebuild.
    pub ruby_overhang: RubyOverhangRule,
    /// Whether a ruby base may break where an annotation cannot.
    ///
    /// Either keeps an unbreakable annotation on the first piece, as Chrome
    /// does, or requires a break opportunity in every nonempty annotation,
    /// as CSS Ruby does. Changes apply to lines on relayout and to intrinsic
    /// sizes on rebuild.
    pub ruby_break_within: RubyBreakWithin,
    /// The space reserved for emphasis marks.
    ///
    /// Either uses the room left below the preceding line, as Chrome does,
    /// or expands each marked line equally. Changes take effect on relayout.
    pub emphasis_room: EmphasisRoom,
}

impl Config {
    /// Returns the preset modelled on Chrome on Windows.
    ///
    /// Uses DirectWrite line metrics and English font selection; see
    /// [`default_language`](Self::default_language).
    ///
    /// Every preset applies CSS variations to platform fonts and synthesizes
    /// missing position variants. [`PlatformFontVariations`] and
    /// [`PositionSynthesis`] reproduce Chrome there.
    pub fn chrome_windows() -> Self {
        Self {
            line_metrics: LineMetricsSource::Win,
            // "en" always parses; `und` is a fallback that never applies.
            default_language: Language::parse("en").unwrap_or(Language::UND),
            super_sub: SuperSubPosition::SizeRatio,
            dominant_baseline: DominantBaselines::Ignored,
            position_synthesis: PositionSynthesis::SynthesizeMissing,
            platform_font_variations: PlatformFontVariations::All,
            small_kana: SmallKana::MayStartLine,
            punctuation_trim: PunctuationTrim::FontFeature,
            pretty: Pretty::Limited,
            tab_justification: TabJustification::Stretch,
            ellipsis_space: EllipsisSpace::Kept,
            word_spacing: WordSpacing::SpaceAndNoBreakSpace,
            ruby_overhang: RubyOverhangRule::AdjacentText,
            ruby_break_within: RubyBreakWithin::BaseOpportunities,
            emphasis_room: EmphasisRoom::Shared,
        }
    }

    /// Returns the preset modelled on Chrome on macOS.
    ///
    /// Uses `hhea` metrics whatever `USE_TYPO_METRICS` says, as Core Text does.
    pub fn chrome_mac() -> Self {
        Self {
            line_metrics: LineMetricsSource::Hhea,
            ..Self::chrome_windows()
        }
    }

    /// Returns the preset modelled on Chrome on Linux and BSD.
    ///
    /// Uses typographic metrics where `USE_TYPO_METRICS` is set, else
    /// `hhea`, as FreeType does.
    pub fn chrome_linux() -> Self {
        Self {
            line_metrics: LineMetricsSource::TypoOrHhea,
            ..Self::chrome_windows()
        }
    }

    /// Returns a configuration that follows the CSS specifications where
    /// they differ from Chrome.
    ///
    /// - `dominant_baseline`: [`Applied`](DominantBaselines::Applied), per
    ///   CSS Inline 3, which aligns dominant baselines.
    /// - `super_sub`: [`FontMetrics`](SuperSubPosition::FontMetrics), per
    ///   CSS Inline 3, which takes the `super` and `sub` offsets from the
    ///   parent font.
    /// - `small_kana`: [`Held`](SmallKana::Held), per CSS Text 4, which
    ///   forbids a break before a small kana under `line-break: normal`.
    /// - `punctuation_trim`: [`Always`](PunctuationTrim::Always), per CSS
    ///   Text 4, which trims whether or not the font has `halt`.
    /// - `tab_justification`: [`KeepStops`](TabJustification::KeepStops), per
    ///   CSS Text 4, which keeps tab stops aligned in justified text.
    /// - `word_spacing`: [`WordSeparators`](WordSpacing::WordSeparators), per
    ///   the word separators defined by CSS Text.
    /// - `ruby_break_within`: [`AllLevels`](RubyBreakWithin::AllLevels), per
    ///   CSS Ruby, which requires a break opportunity in every nonempty
    ///   annotation.
    /// - `emphasis_room`: [`Uniform`](EmphasisRoom::Uniform), per CSS Ruby,
    ///   which gives each line the leading its own annotations need; emphasis
    ///   marks reserve space as ruby does.
    /// - `position_synthesis` and `platform_font_variations`: the CSS
    ///   behavior, as in every preset.
    ///
    /// Where CSS leaves the choice open, it takes a portable value:
    /// - `line_metrics`: [`TypoOrHhea`](LineMetricsSource::TypoOrHhea), the
    ///   closest to the CSS Inline 3 advice to use typographic metrics, else
    ///   `hhea`.
    /// - `default_language`: `und`, which CSS treats as an unknown language.
    /// - `pretty`, `ellipsis_space` and `ruby_overhang`: the Chrome values,
    ///   which CSS allows.
    pub fn spec() -> Self {
        Self {
            line_metrics: LineMetricsSource::TypoOrHhea,
            default_language: Language::UND,
            super_sub: SuperSubPosition::FontMetrics,
            dominant_baseline: DominantBaselines::Applied,
            position_synthesis: PositionSynthesis::SynthesizeMissing,
            platform_font_variations: PlatformFontVariations::All,
            small_kana: SmallKana::Held,
            punctuation_trim: PunctuationTrim::Always,
            pretty: Pretty::Limited,
            tab_justification: TabJustification::KeepStops,
            ellipsis_space: EllipsisSpace::Kept,
            word_spacing: WordSpacing::WordSeparators,
            ruby_overhang: RubyOverhangRule::AdjacentText,
            ruby_break_within: RubyBreakWithin::AllLevels,
            emphasis_room: EmphasisRoom::Uniform,
        }
    }

    /// Returns the preset for the target platform.
    ///
    /// The Windows preset on Windows, the macOS preset on Apple platforms,
    /// and the Linux preset elsewhere, WebAssembly and embedded targets
    /// included.
    pub fn platform() -> Self {
        if cfg!(windows) {
            Self::chrome_windows()
        } else if cfg!(target_vendor = "apple") {
            Self::chrome_mac()
        } else {
            Self::chrome_linux()
        }
    }
}

impl Default for Config {
    /// Returns [`Config::platform`].
    fn default() -> Self {
        Self::platform()
    }
}

/// The source of font ascent, descent and line-gap metrics.
///
/// Determines `line-height: normal`. The sets can differ widely: Consolas
/// at 30 px has a Windows ascent of 28 px and a typographic ascent of
/// 22 px. Where a table is missing, `hhea` stands in for `OS/2`, and the
/// Windows metrics in `OS/2` for `hhea`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum LineMetricsSource {
    /// The `OS/2` Windows ascent and descent, as DirectWrite reads them.
    ///
    /// The line gap is what remains of the `hhea` line height, not the
    /// `hhea` line gap. Yu Gothic, for example, sets 1024 units of gap, and
    /// DirectWrite reports 645 once the Windows extents are taken out.
    Win,
    /// The `hhea` ascent, descent and line gap, as Core Text reads them.
    Hhea,
    /// The `OS/2` typographic metrics where `USE_TYPO_METRICS` is set, else
    /// [`Hhea`](Self::Hhea), as FreeType reads them.
    TypoOrHhea,
}

/// How far `vertical-align: super` and `sub` move a box.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SuperSubPosition {
    /// A ratio of the parent font size, as in Chrome.
    ///
    /// Superscripts move up by a third of the parent font size plus 1 px,
    /// and subscripts down by a fifth plus 1 px. Selected by the Chrome
    /// presets.
    SizeRatio,
    /// The parent font's superscript and subscript offsets from `OS/2`.
    ///
    /// Selected by [`Config::spec`], following CSS Inline 3.
    FontMetrics,
}

/// Whether `dominant-baseline` affects inline alignment.
///
/// Either aligns boxes on the parent's alphabetic baseline, as Chrome does
/// outside SVG, or aligns dominant baselines as CSS Inline 3 specifies,
/// which allows ideographic alignment for CJK within Latin text. The Chrome
/// presets select `Ignored`; [`Config::spec`] selects `Applied`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum DominantBaselines {
    /// Aligns on the parent's alphabetic baseline, ignoring `dominant-baseline`.
    Ignored,
    /// Aligns the dominant baselines of the box and its parent.
    ///
    /// Shifts by the difference between their primary fonts' baseline
    /// offsets, so the same baseline and font give no shift. The block's
    /// baseline is the line's.
    Applied,
}

/// Synthesis of missing superscript or subscript glyphs.
///
/// CSS Fonts 4, section 6.5, requires synthesis for a whole run where any
/// character lacks the variant. Chrome applies `sups` or `subs` without
/// synthesis and leaves the other characters unchanged
/// (<https://issues.chromium.org/issues/352218916>).
///
/// Every preset selects `SynthesizeMissing`. `FeaturesOnly` reproduces
/// Chrome.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PositionSynthesis {
    /// Synthesizes missing position variants, as CSS Fonts 4 requires.
    ///
    /// A run the feature fully covers uses it. Otherwise, where
    /// `font-synthesis-position` allows, the whole run is set in smaller,
    /// shifted glyphs sized by the font's `OS/2` metrics. Line metrics do
    /// not change. Selected by every preset.
    SynthesizeMissing,
    /// Applies `sups` or `subs` without synthesis.
    ///
    /// Characters the feature does not cover stay unchanged, as in Chrome.
    FeaturesOnly,
}

/// Variation support for system fonts.
///
/// Every preset selects `All`. `MatchingOnly` reproduces Chrome on Windows
/// and Linux, where a system font uses its matched named instance and other
/// CSS variations apply only to web fonts. Chrome on macOS applies every
/// variation.
///
/// Chrome 153 on Windows, for example, ignores `'wdth' 75` for system
/// Bahnschrift and `'wght' 700` for system Segoe UI Variable, but applies
/// them to the same files loaded as web fonts.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PlatformFontVariations {
    /// Only the matching axes and the named instance's coordinates.
    ///
    /// Weight, width and style apply; `font-variation-settings` and
    /// `font-optical-sizing` do not. Chrome on Windows and Linux sets system
    /// fonts so.
    MatchingOnly,
    /// Every CSS variation, in CSS order.
    ///
    /// Selected by every preset. Chrome on macOS and Firefox set system
    /// fonts so.
    All,
}

/// Whether a small kana may start a line under `line-break: normal`.
///
/// A small kana has Line_Break class CJ. Under `loose` it always may, and
/// under `strict` only after a space or U+200B.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SmallKana {
    /// Small kana may start a line, as in Chrome and ICU.
    ///
    /// ICU treats CJ as ideographic under `normal` and `loose`. Selected by
    /// the Chrome presets.
    MayStartLine,
    /// No break before a small kana, as under `strict`.
    ///
    /// Selected by [`Config::spec`], following CSS Text (csswg-drafts#10363).
    Held,
}

/// Trimming of the blank half of full-width punctuation.
///
/// CSS Text 4 requires trimming whatever the font supports. Chrome trims
/// only through the font's `halt` feature: at 40 px in Chrome 153, `漢「「漢`
/// measures 140 px in Yu Gothic, which has `halt`, and 160 px in MS Gothic,
/// which does not.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PunctuationTrim {
    /// Trims only through the font's `halt` feature. Selected by the Chrome
    /// presets.
    FontFeature,
    /// Trims even where the font has no `halt` feature.
    ///
    /// Without `halt`, a mark loses half its advance and an opening mark's
    /// glyph shifts into the half kept. MS Gothic `漢「「漢` then measures
    /// 140 px at 40 px. Selected by [`Config::spec`].
    Always,
}

impl PunctuationTrim {
    /// Returns whether a mark in a font with no `halt` is trimmed by halving
    /// its own advance, as under [`Always`](Self::Always).
    pub(crate) fn halves_advances(self) -> bool {
        self == Self::Always
    }
}

/// The fitting rule for `text-wrap-style: pretty`.
///
/// Both rules minimize the squared room left in the final lines and keep
/// the greedy line count, as Chrome does. They differ in when they apply,
/// how many lines they reconsider, and what they ask of the last line.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Pretty {
    /// Balances the last four lines, as Chrome does.
    ///
    /// Applies where the last line is one unbreakable word taking less than
    /// a third of the line, or where the second and third lines from the end
    /// both end in hyphens. Penalizes the last break opportunity most.
    /// Selected by every preset.
    Limited,
    /// Balances the last six lines, with a stronger last-line penalty.
    ///
    /// Also applies where the last line fills less than a fifth of its
    /// width. The penalty favors moving words onto that line over evening
    /// out the lines before it.
    Even,
}

/// Whether justification expands tabs or preserves tab stops.
///
/// Expanding tabs as spaces, as Chrome does, shifts tab stops: 80, 120 and
/// 160 may become 81, 121 and 162. Keeping tab widths keeps columns
/// aligned.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum TabJustification {
    /// Tabs expand as justification opportunities, as in Chrome. Selected by
    /// the Chrome presets.
    Stretch,
    /// Tabs keep their widths, and the other opportunities take the space.
    ///
    /// Selected by [`Config::spec`], following CSS Text 4.
    KeepStops,
}

/// Whether white space just before an ellipsis is kept.
///
/// Chrome keeps the part of an item that fits, trailing spaces included,
/// and hides an item of which nothing fits. In Chrome 153,
/// `XXXXXXXX XXXXXXXXXX` cut after the space draws `XXXXXXXX …`; with the
/// space in a text node of its own it draws `XXXXXXXX…`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum EllipsisSpace {
    /// White space that fits stays before the ellipsis, as in Chrome.
    /// Selected by every preset.
    Kept,
    /// Trailing white space is hidden, so the ellipsis follows the last word.
    Hidden,
}

/// The characters `word-spacing` applies to.
///
/// Either U+0020 and U+00A0 only, as in Chrome, or every word separator
/// kept, as CSS Text 3 specifies. Chrome skips a leading U+0020 unless the
/// block preserves white space; CSS Text 3 includes it, and
/// script-specific separators. An inline that preserves spaces in
/// a block that does not shows the difference:
/// `<p><span style="white-space: pre"> a</span></p>`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum WordSpacing {
    /// U+0020 and U+00A0 only.
    ///
    /// A leading U+0020 takes spacing only where the block preserves spaces
    /// (`preserve`, `preserve-spaces` or `break-spaces`). Selected by the
    /// Chrome presets.
    SpaceAndNoBreakSpace,
    /// Every CSS word separator, leading ones included.
    ///
    /// The separators are U+0020, U+00A0, Ethiopic U+1361, Aegean U+10100
    /// and U+10101, Ugaritic U+1039F and Phoenician U+1091F. Selected by
    /// [`Config::spec`].
    WordSeparators,
}

/// The permitted ruby annotation overhang.
///
/// `ruby-overhang: none` turns overhang off under either rule. In Chrome:
/// - `auto` allows the smaller of the base inset and half the annotation
///   font size, over text no larger than the ruby, up to half that text's
///   width;
/// - `spaces` allows overhang over the blank space beside the base, space
///   separators and the blank halves of full-width punctuation included,
///   within the base inset;
/// - an annotation never overhangs another column.
///
/// JLREQ, section 3.3.7, allows overhang only over kana, to avoid
/// ambiguous readings.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum RubyOverhangRule {
    /// Overhang over the text beside the base, within size limits, as in
    /// Chrome. Selected by every preset.
    AdjacentText,
    /// `auto` overhang over kana only, following JLREQ.
    ///
    /// Limited to one annotation em and half the excess width. `spaces`
    /// allows overhang over blank space only, avoiding ambiguous readings
    /// over kanji.
    KanaOnly,
}

/// Whether ruby splitting requires an opportunity in every annotation.
///
/// Annotations are cut in proportion to the base. Either the base breaks
/// alone and an unbreakable annotation stays on the first piece, as in
/// Chrome, or every nonempty annotation needs a break opportunity, as CSS
/// Ruby specifies.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum RubyBreakWithin {
    /// The base's break opportunities, whatever the annotations allow.
    ///
    /// An unbreakable annotation stays on the first piece. Selected by the
    /// Chrome presets.
    #[default]
    BaseOpportunities,
    /// Breaks only where every nonempty annotation has an opportunity.
    ///
    /// Selected by [`Config::spec`], following CSS Ruby.
    AllLevels,
}

/// The space reserved above or below text for emphasis marks.
///
/// Either uses the room left below the preceding line, or above the first
/// line, as Chrome does, or expands every marked line equally.
///
/// Shared room can space lines unevenly. In Chrome 153, four marked lines
/// at 40 px and `line-height: 1.5` take 250 px with no room above the
/// block: only the first line grows, by 10 px. Ruby annotations take room
/// as Chrome does under either setting.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum EmphasisRoom {
    /// Takes the room the preceding line leaves before growing the line.
    ///
    /// The first line takes the room above the block. Selected by the
    /// Chrome presets.
    Shared,
    /// Grows each marked line by the room its marks need.
    ///
    /// The growth goes above and below as the marks need. Nothing is
    /// borrowed from the preceding line, but the first line may take the
    /// room above the block. Selected by [`Config::spec`].
    Uniform,
}

/// Hit-testing behavior for a point outside every line box.
///
/// A point above a line or in a gap hits the next line, and a point below
/// every line hits the last. This sets where on that line it lands.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PastLines {
    /// The point's column on that line, as on Windows.
    Column,
    /// The line's start, or the last line's end below every line, as on
    /// macOS and Linux.
    LineEnds,
}

impl PastLines {
    /// Returns the hit-testing behavior for the target platform.
    ///
    /// [`Column`](Self::Column) on Windows and [`LineEnds`](Self::LineEnds)
    /// elsewhere.
    pub const fn platform() -> Self {
        if cfg!(windows) {
            Self::Column
        } else {
            Self::LineEnds
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presets share every choice except the source of line metrics.
    #[test]
    fn each_preset_reads_its_platforms_metrics() {
        let windows = Config::chrome_windows();
        let mac = Config::chrome_mac();
        let linux = Config::chrome_linux();
        assert_eq!(windows.line_metrics, LineMetricsSource::Win);
        assert_eq!(mac.line_metrics, LineMetricsSource::Hhea);
        assert_eq!(linux.line_metrics, LineMetricsSource::TypoOrHhea);
        for config in [windows, mac, linux] {
            assert_eq!(config.default_language.as_str(), "en");
            assert_eq!(config.super_sub, SuperSubPosition::SizeRatio);
            assert_eq!(config.dominant_baseline, DominantBaselines::Ignored);
            assert_eq!(
                config.position_synthesis,
                PositionSynthesis::SynthesizeMissing
            );
            assert_eq!(config.platform_font_variations, PlatformFontVariations::All);
            assert_eq!(config.small_kana, SmallKana::MayStartLine);
            assert_eq!(config.punctuation_trim, PunctuationTrim::FontFeature);
            assert_eq!(config.pretty, Pretty::Limited);
            assert_eq!(config.tab_justification, TabJustification::Stretch);
            assert_eq!(config.ellipsis_space, EllipsisSpace::Kept);
            assert_eq!(config.word_spacing, WordSpacing::SpaceAndNoBreakSpace);
            assert_eq!(config.ruby_overhang, RubyOverhangRule::AdjacentText);
            assert_eq!(config.emphasis_room, EmphasisRoom::Shared);
        }
    }

    /// The spec configuration takes CSS's choice wherever it differs from
    /// Chrome, and a portable one where CSS leaves it open.
    #[test]
    fn the_spec_config_follows_css() {
        let spec = Config::spec();
        let expected = Config {
            line_metrics: LineMetricsSource::TypoOrHhea,
            default_language: Language::UND,
            super_sub: SuperSubPosition::FontMetrics,
            dominant_baseline: DominantBaselines::Applied,
            position_synthesis: PositionSynthesis::SynthesizeMissing,
            platform_font_variations: PlatformFontVariations::All,
            small_kana: SmallKana::Held,
            punctuation_trim: PunctuationTrim::Always,
            pretty: Pretty::Limited,
            tab_justification: TabJustification::KeepStops,
            ellipsis_space: EllipsisSpace::Kept,
            word_spacing: WordSpacing::WordSeparators,
            ruby_overhang: RubyOverhangRule::AdjacentText,
            ruby_break_within: RubyBreakWithin::AllLevels,
            emphasis_room: EmphasisRoom::Uniform,
        };
        assert_eq!(spec, expected);
        assert_eq!(spec.default_language.as_str(), "und");
    }

    /// The default is the preset for the platform the crate is built for.
    #[test]
    fn the_default_is_the_platforms_preset() {
        let expected = if cfg!(windows) {
            Config::chrome_windows()
        } else if cfg!(target_vendor = "apple") {
            Config::chrome_mac()
        } else {
            Config::chrome_linux()
        };
        assert_eq!(Config::default(), expected);
        assert_eq!(Config::platform(), expected);
    }
}
