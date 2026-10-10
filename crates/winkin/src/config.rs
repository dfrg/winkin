//! Configuration for behavior not determined by CSS properties.
//!
//! [`Config`] selects font line metrics, superscript offsets and other
//! implementation choices. Hit-testing behavior is selected per call through
//! [`PastLines`], and word motion through [`Motion`](crate::selection::Motion).
//!
//! Contexts use the platform preset by default. Presets generally match Chrome,
//! but follow CSS for known Chrome limitations in platform font variations
//! and position synthesis. Individual fields can reproduce those limitations.
//! [`Config::spec`] selects specification behavior where it differs.

use parlance::Language;

/// Layout configuration beyond CSS properties.
///
/// Set with [`Context::set_config`](crate::Context::set_config).
/// Start from a preset and override individual fields:
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
    /// The font-selection and shaping language when no style language is set.
    ///
    /// Controls generic-family resolution, fallback, regional Han forms,
    /// localized glyphs and synthesized small capitals. Chrome presets use
    /// `en`, matching an English UI; [`Config::spec`] uses `und`.
    ///
    /// `und` selects fonts by the script of each run, including Han fonts
    /// for spaces and punctuation within Han runs.
    ///
    /// Does not supply a language for line breaking, hyphenation,
    /// `text-transform` or `text-autospace`. Changes take effect on rebuild;
    /// line-edge reshaping retains the language used during the build.
    pub default_language: Language,
    /// The offset for `vertical-align: super` and `sub`.
    ///
    /// Synthesized `font-variant-position` uses font metrics instead.
    pub super_sub: SuperSubPosition,
    /// Whether `dominant-baseline` affects inline alignment.
    ///
    /// The Chrome presets ignore the property, matching Chrome outside SVG.
    /// Changes take effect on rebuild.
    pub dominant_baseline: DominantBaselines,
    /// Whether to synthesize missing `font-variant-position` glyphs.
    ///
    /// CSS requires synthesis if `sups` or `subs` does not cover the run;
    /// Chrome applies only the feature. Changes take effect on rebuild.
    pub position_synthesis: PositionSynthesis,
    /// The variation settings applied to platform fonts.
    ///
    /// Selects all CSS variations or only matching variations, as on Chrome
    /// for Windows and Linux. Changes take effect on rebuild.
    pub platform_font_variations: PlatformFontVariations,
    /// Whether small kana may start a line under `line-break: normal`.
    ///
    /// Changes take effect on rebuild.
    pub small_kana: SmallKana,
    /// The punctuation-trimming rule for `text-spacing-trim`.
    ///
    /// Selects font `halt` support, as in Chrome, or unconditional trimming.
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
    /// Whether whitespace preceding an ellipsis cut is retained.
    ///
    /// Changes take effect at the next [`break_lines`](crate::Layout::break_lines) call.
    pub ellipsis_space: EllipsisSpace,
    /// The characters eligible for `word-spacing`, including leading spaces.
    ///
    /// Changes take effect on rebuild.
    pub word_spacing: WordSpacing,
    /// The ruby overhang rule.
    ///
    /// Changes take effect on rebuild.
    pub ruby_overhang: RubyOverhangRule,
    /// Whether a ruby base may break when an annotation cannot.
    ///
    /// Either keeps an unbreakable annotation on the first piece, matching
    /// Chrome, or requires a break opportunity in every nonempty annotation,
    /// as CSS Ruby does. Changes affect lines on relayout and intrinsic sizes
    /// on rebuild.
    pub ruby_break_within: RubyBreakWithin,
    /// The space reserved for emphasis marks.
    ///
    /// Either uses space remaining below the preceding line, matching Chrome,
    /// or expands each marked line equally. Changes take effect on relayout.
    pub emphasis_room: EmphasisRoom,
}

impl Config {
    /// Returns the Windows Chrome preset.
    ///
    /// Uses DirectWrite line metrics. Defaults to English font selection; see
    /// [`default_language`](Self::default_language).
    ///
    /// All presets apply CSS variations to platform fonts and synthesize
    /// missing position variants. Use [`PlatformFontVariations`] and
    /// [`PositionSynthesis`] to reproduce Chrome limitations.
    pub fn chrome_windows() -> Self {
        Self {
            line_metrics: LineMetricsSource::Win,
            // A tag that always parses: `und` could stand in only if the
            // parser changed under it.
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

    /// Returns the macOS Chrome preset.
    ///
    /// Uses `hhea` metrics regardless of `USE_TYPO_METRICS`, matching Core Text.
    pub fn chrome_mac() -> Self {
        Self {
            line_metrics: LineMetricsSource::Hhea,
            ..Self::chrome_windows()
        }
    }

    /// Returns the Linux and BSD Chrome preset.
    ///
    /// Uses typographic metrics when `USE_TYPO_METRICS` is set, otherwise
    /// `hhea`, matching FreeType.
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
    /// Where CSS leaves the choice open, a portable value is used:
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
    /// Uses Windows settings on Windows, macOS settings on Apple platforms,
    /// and Linux settings elsewhere, including WebAssembly and embedded targets.
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
    /// Returns the preset for the platform this is built for, as
    /// [`platform`](Config::platform) does.
    fn default() -> Self {
        Self::platform()
    }
}

/// The source of font ascent, descent and line-gap metrics.
///
/// Determines `line-height: normal`. Font metric sets can differ substantially:
/// Consolas at 30 px has a Windows ascent of 28 px and typographic ascent
/// of 22 px. If a required table is absent, `hhea` substitutes for `OS/2`,
/// and the Windows metrics in `OS/2` substitute for `hhea`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum LineMetricsSource {
    /// Uses `OS/2` Windows ascent and descent, matching DirectWrite.
    ///
    /// The line gap fills the remaining `hhea` line height rather than using
    /// `hhea` line gap directly. For example, Yu Gothic specifies 1024 units
    /// of gap but DirectWrite reports 645 after accounting for Windows extents.
    Win,
    /// Uses `hhea` ascent, descent and line gap, matching Core Text.
    Hhea,
    /// Uses `OS/2` typographic metrics if `USE_TYPO_METRICS` is set.
    ///
    /// Otherwise uses [`Hhea`](Self::Hhea), matching FreeType.
    TypoOrHhea,
}

/// How far `vertical-align: super` and `sub` move a box.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SuperSubPosition {
    /// Uses a font-size ratio, matching Chrome.
    ///
    /// Superscripts move up by one third of the parent font size plus one
    /// pixel; subscripts move down by one fifth plus one pixel.
    SizeRatio,
    /// Uses parent-font superscript and subscript offsets from `OS/2`.
    ///
    /// Selected by [`Config::spec`], following CSS Inline 3.
    FontMetrics,
}

/// Whether `dominant-baseline` affects inline alignment.
///
/// Either aligns boxes on the parent alphabetic baseline, matching Chrome,
/// which resets the property to `auto` outside SVG, or aligns dominant
/// baselines as CSS Inline 3 does, allowing ideographic alignment for CJK
/// within Latin text. Chrome presets ignore the property; [`Config::spec`]
/// selects `Applied`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum DominantBaselines {
    /// Uses the parent alphabetic baseline, ignoring `dominant-baseline`.
    Ignored,
    /// Aligns the box and parent dominant baselines.
    ///
    /// Adjusts by the difference between their primary-font baseline offsets.
    /// Inheriting the same baseline and font produces no shift. The block
    /// baseline defines the line baseline.
    Applied,
}

/// Synthesis of missing superscript or subscript glyphs.
///
/// CSS Fonts 4, section 6.5, requires synthesis for an entire contiguous run
/// if any character lacks the requested variant. Chrome applies `sups` or
/// `subs` without synthesis, leaving unsupported characters unchanged
/// (<https://issues.chromium.org/issues/352218916>).
///
/// Presets use `SynthesizeMissing` to follow CSS. `FeaturesOnly` reproduces
/// Chrome.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PositionSynthesis {
    /// Synthesizes missing position variants as required by CSS Fonts 4.
    ///
    /// A run fully covered by the feature uses it. Otherwise, if synthesis
    /// is permitted, the whole run uses smaller, shifted glyphs based on
    /// font `OS/2` size and offset metrics. Line metrics remain unchanged.
    /// Selected by all presets.
    SynthesizeMissing,
    /// Applies `sups` or `subs` without synthesizing missing glyphs.
    ///
    /// Unsupported characters remain unchanged, matching Chrome.
    FeaturesOnly,
}

/// Variation support for system fonts.
///
/// All presets use `All`. `MatchingOnly` reproduces Chrome on Windows and Linux,
/// where system fonts use the platform-matched named instance and additional
/// CSS variations apply only to web fonts. macOS applies all variations.
///
/// For example, Chrome 153 on Windows ignores `'wdth' 75` for system
/// Bahnschrift and `'wght' 700` for system Segoe UI Variable, while applying
/// them to the same files loaded as web fonts.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PlatformFontVariations {
    /// Uses only matching axes and pinned named-instance coordinates.
    ///
    /// Includes weight, width and style but excludes additional
    /// `font-variation-settings` and `font-optical-sizing`, matching Chrome
    /// system fonts on Windows and Linux.
    MatchingOnly,
    /// Applies all CSS variations in CSS order.
    ///
    /// Selected by all presets; matches Chrome on macOS and Firefox.
    All,
}

/// Whether a small kana may start a line under `line-break: normal`.
///
/// A small kana is Line_Break CJ. Under `loose` it always may. Under `strict`
/// it may only after a space or a U+200B.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum SmallKana {
    /// Allows small kana at line start, matching Chrome and ICU.
    ///
    /// ICU treats CJ as ideographic under `normal` and `loose`.
    /// Selected by Chrome presets.
    MayStartLine,
    /// Prevents breaks before small kana, as under `strict`.
    ///
    /// Selected by [`Config::spec`], following CSS Text (csswg-drafts#10363).
    Held,
}

/// Trimming of the blank half of full-width punctuation.
///
/// CSS Text 4 requires trimming regardless of font support. Chrome trims
/// only through the font `halt` feature. In Chrome 153 at 40 px, `漢「「漢`
/// measures 140 px in Yu Gothic with `halt` and 160 px in MS Gothic without
/// it.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PunctuationTrim {
    /// Trims only through font `halt` support. Selected by Chrome presets.
    FontFeature,
    /// Trims even if the font has no `halt` feature.
    ///
    /// Fallback removes half the mark advance and shifts opening glyphs into
    /// the retained half. MS Gothic `漢「「漢` then measures 140 px at 40 px.
    /// Selected by [`Config::spec`].
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
/// Both rules minimize squared remaining space in the final lines and retain
/// the greedy line count, matching Chrome. They differ
/// in activation conditions, the number of lines considered and final-line
/// requirements.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Pretty {
    /// Balances the last four lines, matching Chrome.
    ///
    /// Activates for a single unbreakable final word occupying less than a
    /// third of the line, or consecutive hyphens on the second and third
    /// lines from the end. Penalizes the final break opportunity most heavily.
    Limited,
    /// Balances the last six lines with a stronger final-line penalty.
    ///
    /// Also activates when the last line occupies less than a fifth of its
    /// width. The penalty favors moving words to that line over improving
    /// the preceding lines alone.
    Even,
}

/// Whether justification expands tabs or preserves tab stops.
///
/// Expanding tabs as spaces, as Chrome does, shifts tab stops: 80, 120 and
/// 160 may become 81, 121 and 162. Preserving tab widths and expanding only
/// other opportunities keeps columns aligned.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum TabJustification {
    /// Expands tabs as justification opportunities, matching Chrome.
    Stretch,
    /// Preserves tab widths and distributes space to other opportunities.
    ///
    /// Selected by [`Config::spec`], following CSS Text 4.
    KeepStops,
}

/// Whether whitespace immediately before an ellipsis cut is retained.
///
/// Retaining it matches Chrome, which keeps the fitting part of an item,
/// trailing spaces included, but hides an item of which nothing fits. In
/// Chrome 153, `XXXXXXXX XXXXXXXXXX` cut after the space draws
/// `XXXXXXXX …`. Putting the space in a separate text node instead draws
/// `XXXXXXXX…`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum EllipsisSpace {
    /// Retains fitting whitespace before the ellipsis, matching Chrome.
    Kept,
    /// Hides trailing whitespace so the ellipsis follows the last word.
    Hidden,
}

/// The characters eligible for `word-spacing`.
///
/// Either spaces U+0020 and U+00A0 only, matching Chrome, or all retained
/// word separators, as CSS Text 3 does. Chrome excludes an initial U+0020
/// unless the block preserves whitespace; CSS Text 3 includes
/// script-specific separators and initial spaces. A preserving inline inside
/// a collapsing block can expose the difference:
/// `<p><span style="white-space: pre"> a</span></p>`.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum WordSpacing {
    /// Spaces U+0020 and U+00A0 only.
    ///
    /// Initial U+0020 receives spacing only if the block preserves spaces
    /// (`preserve`, `preserve-spaces` or `break-spaces`).
    SpaceAndNoBreakSpace,
    /// Spaces all CSS word separators, including initial separators.
    ///
    /// Includes U+0020, U+00A0, Ethiopic U+1361, Aegean U+10100/U+10101,
    /// Ugaritic U+1039F and Phoenician U+1091F. Selected by [`Config::spec`].
    WordSeparators,
}

/// The permitted ruby annotation overhang.
///
/// `ruby-overhang: none` disables overhang under either rule. In Chrome:
/// - `auto` permits the smaller of the base inset and half the annotation
///   font size, over text no larger than the ruby, up to half that text width;
/// - `spaces` permits adjacent blank space, including space separators and
///   blank halves of full-width punctuation, within the base inset;
/// - annotations never overhang another column.
///
/// JLREQ, section 3.3.7, permits overhang only over kana to avoid ambiguous readings.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum RubyOverhangRule {
    /// Permits overhang over adjacent text within size limits.
    ///
    /// Matches Chrome. Selected by all presets.
    AdjacentText,
    /// Permits `auto` overhang over kana only, following JLREQ.
    ///
    /// Limits it to one annotation em and half the excess width. `spaces`
    /// permits blank-space overhang only, avoiding ambiguous readings over kanji.
    KanaOnly,
}

/// Whether ruby splitting requires an opportunity in every annotation.
///
/// Annotations are cut in proportion to the base. Either permits base-only
/// breaks and keeps an unbreakable annotation on the first piece, matching
/// Chrome, or requires a break opportunity in every nonempty annotation, as
/// CSS Ruby does.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, Default)]
pub enum RubyBreakWithin {
    /// Uses base break opportunities independently of annotations.
    ///
    /// Unbreakable annotations remain on the first piece.
    /// Selected by Chrome presets.
    #[default]
    BaseOpportunities,
    /// Requires a break opportunity in every nonempty annotation.
    ///
    /// Selected by [`Config::spec`], following CSS Ruby.
    AllLevels,
}

/// The space reserved above or below text for emphasis marks.
///
/// Either uses available space below the preceding line or above the first
/// line, matching Chrome, or expands every marked line equally.
///
/// Shared space can produce uneven line spacing. In Chrome 153, four marked
/// lines at 40 px and `line-height: 1.5` occupy 250 px without space above
/// the block; only the first line adds 10 px. Ruby annotations use Chrome
/// spacing under either setting.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum EmphasisRoom {
    /// Uses space left by the preceding line before expanding this line.
    ///
    /// The first line uses available space above the block.
    /// Selected by Chrome presets.
    Shared,
    /// Expands each marked line by the required emphasis space.
    ///
    /// Distributes expansion above and below in proportion to the marks.
    /// Does not borrow from preceding lines; the first line can use space
    /// above the block. Selected by [`Config::spec`].
    Uniform,
}

/// Hit-testing behavior outside line boxes.
///
/// Uses the next line after the point, or the last line below all lines,
/// matching Chrome.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum PastLines {
    /// Hits the selected line at the same column, matching Windows.
    Column,
    /// Hits a line start, or the final line end below all lines, matching macOS and Linux.
    LineEnds,
}

impl PastLines {
    /// Returns the hit-testing behavior for the target platform.
    ///
    /// Preserves the column on Windows and selects line ends elsewhere.
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

    /// The default is the preset for where the crate runs, so a caller that
    /// never sets a config gets Chrome's behaviour there.
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
