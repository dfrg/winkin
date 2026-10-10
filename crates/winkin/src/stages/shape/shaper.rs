//! The one shaping function, and the only code that calls harfrust.
//!
//! The shaping pass calls [`shape_range`] once per shaping run, with a
//! [`GlyphSink`] over the shaped text's tables. The breaker calls it per
//! unsafe line edge, with a sink over the line's own. Because there is one
//! function, a reshaped edge differs from the paragraph's shaping only at
//! the break itself. The key, the context either side, the glyph-to-cluster
//! mapping and the encoding are all the same.
//!
//! **Glyphs to clusters.** harfrust returns glyphs in visual order, each
//! labelled with the byte its cluster starts at. Walked in the text's order
//! (backwards for right-to-left), the labels never decrease. So one forward
//! cursor finds each glyph's cluster, and each cluster's glyphs are one
//! contiguous slice of the output, in visual order. A cluster no glyph lands
//! on lies inside something the shaper merged, such as a ligature. If it
//! follows glyphs, it is a continuation. Where a merge ends inside a later
//! cluster, the clusters up to that one's end are merged whole, as Chrome's
//! grapheme cluster level merges them. Letter-spacing then goes once after
//! them, as in Chrome's `ShapeResult::ApplySpacing`.
//!
//! **Unsafe bits.** harfrust flags whether a break at a glyph's cluster
//! start changes the shaping. A cluster takes the flag of the glyph labelled
//! at its start. A cluster with no such glyph lies inside a merged one, and
//! is unsafe. The first cluster shaped is a start, so it keeps no bit.
//! Instead, harfrust's flag for it is returned, for the breaker's reshape
//! window.
//!
//! **Punctuation trim** (`text-spacing-trim`). For each call, the rule in
//! `trim` finds the marks that give their blanks back, from the characters
//! either side. The font's `halt` applies over each mark's own bytes, as in
//! Chrome's `HanKerning`. Where a mark is kerned against what precedes it,
//! breaking before it is unsafe, as Chrome marks it, so a line starting
//! there is reshaped as a start.
//!
//! This holds for a shaping run's first cluster too, which is kerned against
//! the run before. Its word says a break is unsafe, not that a reshape
//! cannot start there, since the runs are shaped apart. Where the font has
//! no `halt` and the config asks for it, the mark's advance is halved
//! instead, which Chrome does not do.

use super::sink::{AdvanceStore, AdvanceUnit, Flags, GlyphSink, Halved, along, sidecar_glyph};
#[cfg(test)]
use core::cell::Cell;
use core::iter::Peekable;
use core::ops::Range;
use core::{ptr, slice};

use harfrust::{Buffer, Direction, GlyphInfo, GlyphPosition};
use parlance::{Language, Script};

use super::context::{CallSettings, CallText, Features, PlanLanguage, ShapeSession};
use super::trim::{self, ShapingEdges, TrimClass, TrimMark};
use super::vertical::VerticalFallback;
use super::{CombineWidth, ShapedRun, SidecarGlyph, SidecarGlyphId};
use crate::data::{Id, Table, TextOffset};
use crate::stages::analysis::{Analysis, BidiLevel, ClusterId, Clusters, RunOrientation};
use crate::stages::content::{Content, FontKey, TextCursor, TextFactsId, VariantText};
use crate::stages::fonts::{CaseMap, Fonts, UsedFont, UsedFontId, UsedInstance};
use crate::style::{FirstLineVariant, TextSpacingTrim};
use crate::unit::{InlineLayoutUnit, TextUnit};
use crate::work;

/// The text shaping reads, and its clusters.
///
/// The text is the content's, or the first line's where `::first-line`
/// transforms some of it. Either way, clusters are found by their content
/// offsets.
///
/// Each character is fed labelled with its byte in the shaped text, so a
/// cluster starts at its content offset mapped into that text
/// ([`start`](Self::start)). A first line's `ß` in capitals is two
/// characters of one cluster. A cluster inside a character the first line's
/// text holds whole starts where that character ends. It draws nothing, like
/// a ligature's continuation.
///
/// Clusters are read through a cursor. A walk that shapes its runs in order
/// with one source steps through the first line's map and searches nothing.
pub(crate) struct ShapingSource<'a> {
    text: VariantText<'a>,
    cursor: TextCursor<'a>,
    clusters: &'a Clusters,
}

impl<'a> ShapingSource<'a> {
    /// Returns the text `variant` reads of `content`, with `analysis`'s
    /// clusters.
    pub(crate) fn new(
        content: &'a Content,
        analysis: &'a Analysis,
        variant: FirstLineVariant,
    ) -> Self {
        let text = content.text(variant);
        Self {
            text,
            cursor: text.cursor(),
            clusters: &analysis.clusters,
        }
    }

    /// The text shaped, read by content offsets.
    pub(super) fn text(&self) -> VariantText<'a> {
        self.text
    }

    /// Where `cluster` starts in the text shaped; the end, past the last.
    fn start(&self, cluster: ClusterId) -> TextOffset {
        self.cursor.offset(self.clusters.start(cluster))
    }

    /// Where `cluster` starts in the text shaped, stepped to from the
    /// cursor's place without moving it.
    ///
    /// A range's end is found this way, so the cursor stays at its start
    /// for the walk over its clusters.
    fn start_ahead(&self, cluster: ClusterId) -> TextOffset {
        self.cursor.clone().offset(self.clusters.start(cluster))
    }

    /// Where the content's `range` is in the text shaped, found without
    /// moving the cursor.
    fn range(&self, range: Range<TextOffset>) -> Range<TextOffset> {
        self.text.offset(range.start)..self.text.offset(range.end)
    }
}

/// Everything a range of clusters is shaped with: what makes two runs shape
/// differently.
///
/// It is resolved from a [`ShapedRun`] and the prepared data, and borrows
/// from them. The font is the layout's own used-font record, which holds the
/// whole font. The context's caches are keyed by that font and its
/// coordinates. So a key names nothing of the context's, and the breaker
/// reshapes with what the layout holds, whatever the context has dropped
/// since.
#[derive(Copy, Clone, Debug)]
pub(crate) struct ShapingKey<'a> {
    /// The record of the run's used font's instance, or `None` where there
    /// is no font to set in, and nothing is drawn.
    font: Option<&'a UsedInstance>,
    /// The size the glyphs are set at: the used size, or a synthesized
    /// form's glyph size.
    size: TextUnit,
    /// The point size the shaper is told, in CSS pixels, which only a `trak`
    /// table reads.
    ///
    /// It is the style's computed `font-size`, as Blink hands HarfBuzz
    /// `FontDescription::SpecifiedSize`. That is before `font-size-adjust`,
    /// a face's `size-adjust` and flooring to a hundredth. Synthesized small
    /// capitals and positions use the element's own size. Where the style
    /// gives none, it is the glyph size.
    point_size: f32,
    /// How the text is fed: as written, or in another case, for synthesized
    /// small capitals and unicase.
    case: CaseMap,
    /// The language the run's fonts were chosen in is Turkish or
    /// Azerbaijani, whose `i` and `ı` case as their own capitals, as ICU
    /// maps them for Blink's `CaseMap`.
    turkic: bool,
    level: BidiLevel,
    orientation: RunOrientation,
    script: Script,
    /// What the plan is made for: the language the run's fonts were chosen
    /// in, or the language system its `font-language-override` names.
    language: PlanLanguage,
    /// The run's `text-spacing-trim`. A run ends where it changes, since
    /// Blink's `FontDescription` holds it.
    trim: TextSpacingTrim,
    /// A mark in a font with no `halt` is trimmed by halving its advance,
    /// as the build's config says (`Config::punctuation_trim`).
    halving: bool,
    /// What the font's vertical metrics fall back to, from its line
    /// metrics. An upright run reads them with this (see `vertical`).
    vertical: VerticalFallback,
    /// The narrower form combined text is fitted with, if any. The text is
    /// shaped with that form's feature.
    narrow: Option<CombineWidth>,
}

impl<'a> ShapingKey<'a> {
    /// Returns the key `run` was shaped with, from `content`, `analysis` and
    /// `fonts`.
    ///
    /// The breaker uses it to reshape part of a run exactly as the run was
    /// shaped.
    /// - Script, level and orientation come from its script run.
    /// - Font and size come from its used font.
    /// - The language is the one its fonts were chosen in: the script
    ///   run's, or the default `fonts` keeps where the text names none.
    /// - Point size, language system override and `text-spacing-trim` come
    ///   from its shaping facts (`ShapedRun::shaping`), shared by every text
    ///   item in the run.
    ///
    /// `halving` says whether the build halves the advance of punctuation in
    /// a font with no `halt`, as [`ShapedFlags`](super::ShapedFlags) records.
    pub(crate) fn new(
        run: &ShapedRun,
        content: &Content,
        analysis: &Analysis,
        fonts: &'a Fonts,
        halving: bool,
    ) -> Self {
        let script_run = analysis.runs.get(run.script_run);
        let language = script_run.map_or(Language::UND, |run| {
            content.lists.languages.get(run.language)
        });
        let facts = &content.facts;
        let shaping = facts.shaping(run.shaping);
        let request = facts.request(shaping.font);
        Self {
            level: script_run.map_or(BidiLevel::LTR, |run| run.level),
            orientation: script_run.map_or(RunOrientation::Horizontal, |run| run.orientation),
            script: script_run.map_or(Script::UNKNOWN, |run| run.script),
            trim: shaping.trim,
            halving,
            ..Self::from_font(fonts, run.font, &request.font, language)
        }
    }

    /// Returns the key a generated text is shaped with, in the used font
    /// `font`, with the text facts `text`, right to left where `rtl`, standing
    /// as `orientation` says.
    ///
    /// It takes the font's size and synthesis, and the request's computed
    /// size and language. This matches how Blink's `HyphenResult::Shape` and
    /// `LineTruncator::SetupEllipsis` shape with the style's font. A hyphen
    /// or an ellipsis is Common text, and nothing in it is trimmed.
    pub(crate) fn from_generated(
        font: UsedFontId,
        text: TextFactsId,
        content: &Content,
        fonts: &'a Fonts,
        rtl: bool,
        orientation: RunOrientation,
    ) -> Self {
        let facts = &content.facts;
        let request = facts.request(facts.text_request(text));
        let language = content.lists.languages.get(request.language);
        Self {
            level: BidiLevel::new(u8::from(rtl)),
            orientation,
            script: Script::COMMON,
            trim: TextSpacingTrim::SpaceAll,
            ..Self::from_font(fonts, font, &request.font, language)
        }
    }

    /// Returns the part of a key that the used font `font`, the font
    /// properties `font_group` and `language` decide.
    ///
    /// It reads left to right across the line, in the unknown script, with
    /// nothing trimmed. The caller then sets its run's own direction, script
    /// and trim.
    /// - Font and size are the used font's.
    /// - Point size is the font group's computed size, as Blink's
    ///   `SpecifiedSize`, sanitized when taken. Where that is zero, it is the
    ///   glyph size, as Blink's `HarfBuzzFace::GetScaledFont` falls back to
    ///   the platform size.
    /// - The language is `language`, or the build's default where that
    ///   names none, as Chrome shapes in `LocaleOrDefault`.
    /// - The language system is the font group's
    ///   `font-language-override`, if it names one.
    fn from_font(
        fonts: &'a Fonts,
        font: UsedFontId,
        font_group: &FontKey,
        language: Language,
    ) -> Self {
        let used = fonts.used.get(font);
        let language = fonts.default_language.fonts_language(language);
        let size = used.map_or(TextUnit::default(), UsedFont::glyph_size);
        let specified = font_group.computed_size();
        let point_size = if specified > 0.0 {
            specified
        } else {
            size.to_px()
        };
        Self {
            font: used.and_then(|used| used.instance.as_deref()),
            size,
            point_size,
            case: used.map_or(CaseMap::Keep, |used| used.synthesis.case()),
            turkic: matches!(language.language(), "tr" | "az"),
            level: BidiLevel::LTR,
            orientation: RunOrientation::Horizontal,
            script: Script::UNKNOWN,
            language: PlanLanguage::new(language, font_group.language_override),
            trim: TextSpacingTrim::Normal,
            halving: false,
            vertical: used.map_or(VerticalFallback::default(), |used| {
                let metrics = used.metrics;
                VerticalFallback::new(metrics.alphabetic_ascent(), metrics.alphabetic_descent())
            }),
            narrow: None,
        }
    }

    /// Returns how a call shapes text as this key says.
    fn settings(&self) -> CallSettings {
        CallSettings {
            direction: self.direction(),
            script: harfrust::Script::from_iso15924_tag(harfrust::Tag::new(
                &self.script.to_bytes(),
            )),
            language: self.language,
            scale: self.size.raw(),
            point_size: self.point_size,
            case: self.case,
            turkic: self.turkic,
            vertical: self.is_vertical().then_some(self.vertical),
        }
    }

    /// Returns the same key, with combined text shaped in `width`'s form.
    pub(super) fn with_combine_width(self, width: CombineWidth) -> Self {
        Self {
            narrow: Some(width),
            ..self
        }
    }

    /// The run's `text-spacing-trim`.
    pub(super) fn trim(&self) -> TextSpacingTrim {
        self.trim
    }

    /// The direction the shaper sets the run in.
    ///
    /// - Upright glyphs in a vertical line go down, or up where they read
    ///   right to left, as Blink reverses `HB_DIRECTION_TTB` for
    ///   right-to-left text.
    /// - A sideways run is shaped as horizontal text, which the painter
    ///   turns.
    /// - Combined text is shaped left to right, as the box Blink sets it in
    ///   lays it out.
    /// - Otherwise the bidi level decides.
    fn direction(&self) -> Direction {
        let rtl = self.level.is_rtl();
        match self.orientation {
            RunOrientation::Upright if rtl => Direction::BottomToTop,
            RunOrientation::Upright => Direction::TopToBottom,
            RunOrientation::Combined => Direction::LeftToRight,
            RunOrientation::Horizontal | RunOrientation::Sideways if rtl => Direction::RightToLeft,
            RunOrientation::Horizontal | RunOrientation::Sideways => Direction::LeftToRight,
        }
    }

    /// Whether the run is combined text.
    pub(super) fn is_combined(&self) -> bool {
        self.orientation == RunOrientation::Combined
    }

    /// Whether the run is shaped down the line.
    fn is_vertical(&self) -> bool {
        self.orientation == RunOrientation::Upright
    }
}

/// The fonts the characters either side of a shaped range are shaped in.
///
/// `None` on a side where the character is in the range's own font, where
/// there is none, or where the caller does not trim.
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct NeighbourFonts<'a> {
    before: Option<&'a UsedInstance>,
    after: Option<&'a UsedInstance>,
}

impl<'a> NeighbourFonts<'a> {
    /// Returns the instances of the used fonts `before` and `after` in
    /// `fonts`.
    pub(crate) fn new(
        fonts: &'a Fonts,
        before: Option<UsedFontId>,
        after: Option<UsedFontId>,
    ) -> Self {
        let instance = |id: Option<UsedFontId>| {
            fonts
                .used
                .get(id?)
                .and_then(|used| used.instance.as_deref())
        };
        Self {
            before: instance(before),
            after: instance(after),
        }
    }
}

/// Shapes the clusters `range` of `source` with `key` into `out`, with
/// `paragraph` around them for context.
///
/// It writes a word and an advance for each cluster, in cluster order, and
/// the glyphs of each expanded one. It returns whether harfrust says
/// breaking before the range's start changes the shaping. The words leave
/// this out, since a shaped range starts safe. If true, the shaped glyphs
/// must not be joined there to the glyphs before them.
///
/// The range must lie in one shaping run, so that `key` is how it shapes,
/// and in `paragraph`. Where the run has no font, or its font cannot be
/// read, its clusters are written with no glyphs. `neighbours` names the
/// fonts the characters either side are shaped in, for `text-spacing-trim`.
///
/// harfrust's data is looked up in `cx` by the key's font, and built where
/// `cx` lacks it. So the result does not depend on what `cx` has kept; a
/// warm cache only saves the building.
///
/// The breaker calls this for a line edge at an unsafe break `e`:
/// - the line's end is `[the nearest earlier safe cluster, e)`;
/// - the next line's start is `[e, the next safe cluster)`.
///
/// Each goes into the line's own tables. Where the returned start is unsafe,
/// the breaker widens the window and shapes again. The context either side
/// is the paragraph's, as Chrome gives HarfBuzz the whole text around the
/// range, so a word broken across lines keeps its joining forms.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shape_range<I: Id, A: AdvanceUnit, S: AdvanceStore<A>>(
    cx: &mut ShapeSession<'_, '_>,
    source: &ShapingSource<'_>,
    paragraph: Range<TextOffset>,
    range: Range<ClusterId>,
    key: &ShapingKey<'_>,
    edges: ShapingEdges,
    neighbours: NeighbourFonts<'_>,
    out: &mut GlyphSink<'_, I, A, S>,
) -> bool {
    let count = range.end.get().saturating_sub(range.start.get());
    if count == 0 {
        return false;
    }
    let text = source.text.text();
    let start = source.start(range.start);
    let end = source.start_ahead(range.end);
    let paragraph = source.range(paragraph);
    let slice =
        |from: TextOffset, to: TextOffset| text.get(from.get()..to.get()).unwrap_or_default();
    let call = CallText {
        chars: slice(start, end),
        base: start.to_u32(),
        before: slice(paragraph.start, start),
        after: slice(end, paragraph.end),
        starts_paragraph: start == paragraph.start,
    };
    let settings = key.settings();
    let direction = settings.direction;
    // Find the marks that give their blanks back into the context's
    // scratch, which is taken for the call and given back.
    let mut found = cx.take_trim_marks();
    let mut halving = None;
    // Combined text is set as one character: nothing in it is trimmed, and
    // no mark beside it trims against it.
    if let Some(font) = &key.font
        && key.orientation != RunOrientation::Combined
        && key.trim.trims_punctuation()
        && trim::may_hold_mark(call.chars)
        && !has_exclusive_feature(&font.features)
    {
        let horizontal = !matches!(direction, Direction::TopToBottom | Direction::BottomToTop);
        // Down the line as across it, with `vhal` and the glyphs' heights
        // where the run is upright, as Blink's `HanKerning` trims
        // `!is_horizontal`.
        let trim_font = cx.trim_font(font, key.language, horizontal);
        if trim_font.has_halt() || key.halving {
            // A neighbour in another font is classed in that font, read in
            // the run's language and direction.
            let mut beside = |ch: Option<char>, theirs: Option<&UsedInstance>| {
                let theirs = theirs.filter(|theirs| !ptr::eq(*theirs, *font));
                ch.map(|ch| {
                    trim::class_beside(ch, &trim_font, || {
                        theirs.map(|theirs| cx.trim_font(theirs, key.language, horizontal))
                    })
                })
            };
            let before = beside(call.before.chars().next_back(), neighbours.before);
            let after = beside(call.after.chars().next(), neighbours.after);
            found.find(call.chars, call.base, before, after, &trim_font, edges);
            if !trim_font.has_halt() {
                // Chrome's fallback trims a full-width mark alone: one at
                // least nine tenths of an em.
                let size = i64::from(key.size.raw());
                halving = Some(i32::try_from(size * 9 / 10).unwrap_or(i32::MAX));
            }
        }
    }
    let halt: &[TrimMark] = if halving.is_none() {
        found.marks()
    } else {
        &[]
    };
    let marks = CallMarks {
        unsafe_before: found.unsafe_before(),
        halving: halving.map(|least| (found.marks(), least)),
    };
    let shaped = key.font.as_ref().and_then(|font| {
        let narrow = key.narrow.map(CombineWidth::feature);
        cx.shape_call(
            font,
            &call,
            &settings,
            Features::Font { halt, narrow },
            |output| sink_output(output, source, range.clone(), direction, &marks, out),
        )
    });
    cx.restore_trim_marks(found);
    match shaped {
        Some(start) => start,
        None => {
            out.empty(count);
            false
        }
    }
}

/// Shapes a generated hyphen or ellipsis `text` with `key`, and appends its
/// glyphs to `out` in drawing order.
///
/// Each glyph carries its advance and offsets. A generated text is shaped
/// once, however many lines draw it. Returns the glyphs' exact total advance
/// along the line.
///
/// The text is shaped alone, with no context either side, as Blink's
/// `HarfBuzzShaper` shapes a `HyphenResult` and an ellipsis. Where the key
/// has no font, the font cannot be read, or `out` cannot take every glyph,
/// nothing is appended, and the text draws nothing and takes no room.
pub(crate) fn shape_generated(
    cx: &mut ShapeSession<'_, '_>,
    text: &str,
    key: &ShapingKey<'_>,
    out: &mut Table<SidecarGlyphId, SidecarGlyph>,
) -> InlineLayoutUnit {
    let Some(font) = &key.font else {
        return InlineLayoutUnit::ZERO;
    };
    if text.is_empty() {
        return InlineLayoutUnit::ZERO;
    }
    // Across the line, or down it where it stands upright, as its key has
    // it. Offsets are kept in the line's terms, as a run's are.
    let vertical = key.is_vertical();
    let call = CallText {
        chars: text,
        base: 0,
        before: "",
        after: "",
        starts_paragraph: true,
    };
    let start = out.next_id();
    let features = Features::Font {
        halt: &[],
        narrow: None,
    };
    let shaped = cx.shape_call(font, &call, &key.settings(), features, |output| {
        let mut advance = 0i64;
        for (glyph, position) in output.glyph_infos().iter().zip(output.glyph_positions()) {
            work::step();
            out.push(sidecar_glyph(
                glyph,
                position,
                vertical,
                Halved::default(),
                false,
            ))?;
            advance += along(position, vertical);
        }
        Some(advance)
    });
    match shaped.flatten() {
        Some(advance) => InlineLayoutUnit::from_raw(advance),
        None => {
            out.truncate(start);
            InlineLayoutUnit::ZERO
        }
    }
}

/// A cursor over the clusters of a range that finds the cluster each glyph's
/// label falls in.
///
/// It only moves forward, which labels that never decrease in the text's
/// order allow.
struct ClusterCursor<'a> {
    source: &'a ShapingSource<'a>,
    at: ClusterId,
    end: ClusterId,
    /// Where `at` starts, found once as the cursor reaches it.
    start: TextOffset,
    /// Where the cluster after `at` starts, or `None` where `at` is the last.
    next: Option<TextOffset>,
}

impl<'a> ClusterCursor<'a> {
    fn new(source: &'a ShapingSource<'a>, range: &Range<ClusterId>) -> Self {
        let mut cursor = Self {
            source,
            at: range.start,
            end: range.end,
            start: source.start(range.start),
            next: None,
        };
        cursor.next = cursor.start_after(range.start);
        cursor
    }

    /// Where the cluster the cursor is at starts.
    fn start(&self) -> TextOffset {
        self.start
    }

    fn start_after(&self, cluster: ClusterId) -> Option<TextOffset> {
        let next = ClusterId::new(cluster.get() + 1);
        (next < self.end).then(|| self.source.start(next))
    }

    /// Whether a glyph labelled `label` belongs to the current cluster, that
    /// is, is not past it.
    ///
    /// A label before the cluster, which ordered output never has, stays
    /// with it rather than being lost.
    fn holds(&self, label: TextOffset) -> bool {
        self.next.is_none_or(|next| label < next)
    }

    /// Moves forward to the cluster a glyph labelled `label` is in, and
    /// returns it.
    fn locate(&mut self, label: TextOffset) -> ClusterId {
        // Past the current cluster, so there is one after it, which starts
        // at `next`.
        while let Some(next) = self.next.filter(|&next| label >= next) {
            self.at = ClusterId::new(self.at.get() + 1);
            self.start = next;
            self.next = self.start_after(self.at);
        }
        self.at
    }
}

/// Whether breaking before a cluster starting at `start`, with `glyphs`, is
/// unsafe.
///
/// The glyph labelled at `start` decides. Where there is none, the cluster
/// starts inside a merged glyph, and breaking is unsafe.
fn is_unsafe_start(glyphs: &[GlyphInfo], start: TextOffset) -> bool {
    let label = start.to_u32();
    glyphs
        .iter()
        .find(|glyph| glyph.cluster == label)
        .is_none_or(GlyphInfo::unsafe_to_break)
}

/// Whether `features` turns on one that changes glyph widths, which a mark's
/// `halt` would fight.
///
/// Blink's `HanKerning` leaves the text alone under any of them
/// (`IsExclusiveFeature`).
fn has_exclusive_feature(features: &[parlance::FontFeature]) -> bool {
    const EXCLUSIVE: [&[u8; 4]; 9] = [
        b"halt", b"hwid", b"palt", b"pwid", b"qwid", b"twid", b"valt", b"vhal", b"vpal",
    ];
    features.iter().any(|feature| {
        feature.value != 0
            && EXCLUSIVE
                .iter()
                .any(|tag| feature.tag == parlance::Tag::new(tag))
    })
}

/// The marks of a call, as [`sink_output`] reads them.
///
/// `unsafe_before` lists where breaking is unsafe because a pair meets.
/// `halving`, where marks are halved, lists them with the least advance a
/// mark needs to be halved.
struct CallMarks<'a> {
    unsafe_before: &'a [u32],
    halving: Option<(&'a [TrimMark], i32)>,
}

/// Writes harfrust's output for `range` into `out`, one cluster at a time,
/// and returns harfrust's unsafe flag for the range's start.
///
/// `marks` adds unsafe bits and halving. Most text has no kerned or halved
/// mark and comes back in the text's order along a horizontal line. That
/// text takes the plain path, [`sink_clusters`] with `MARKED` unset, which
/// skips the per-cluster mark checks.
fn sink_output<I: Id, A: AdvanceUnit, S: AdvanceStore<A>>(
    output: &Buffer,
    source: &ShapingSource<'_>,
    range: Range<ClusterId>,
    direction: Direction,
    marks: &CallMarks<'_>,
    out: &mut GlyphSink<'_, I, A, S>,
) -> bool {
    let infos = output.glyph_infos();
    let positions = output.glyph_positions();
    let count = infos.len().min(positions.len());
    let infos = infos.get(..count).unwrap_or_default();
    let positions = positions.get(..count).unwrap_or_default();
    if direction == Direction::LeftToRight
        && marks.unsafe_before.is_empty()
        && marks.halving.is_none()
        && plain_paths()
    {
        sink_clusters::<false, I, A, S>(infos, positions, source, range, direction, marks, out)
    } else {
        sink_clusters::<true, I, A, S>(infos, positions, source, range, direction, marks, out)
    }
}

/// Writes `infos` and `positions`, harfrust's output for `range` set in
/// `direction`, into `out` as [`sink_output`] says.
///
/// Each cluster's glyphs are one slice of the output. The walk goes in the
/// text's order, and one cursor finds each cluster, its start found once.
/// A [`ClusterWriter`] writes each cluster found, and the empty ones before
/// it.
///
/// One body gives two paths:
/// - `MARKED` reads glyphs backwards for right-to-left text, down the line
///   for vertical text, and checks every cluster against `marks`.
/// - The plain path is for horizontal glyphs in the text's order with empty
///   `marks`. It skips those checks, and writes a one-glyph cluster compact
///   where it can without further checks.
fn sink_clusters<const MARKED: bool, I: Id, A: AdvanceUnit, S: AdvanceStore<A>>(
    infos: &[GlyphInfo],
    positions: &[GlyphPosition],
    source: &ShapingSource<'_>,
    range: Range<ClusterId>,
    direction: Direction,
    marks: &CallMarks<'_>,
    out: &mut GlyphSink<'_, I, A, S>,
) -> bool {
    let count = infos.len();
    let backward = MARKED && matches!(direction, Direction::RightToLeft | Direction::BottomToTop);
    let vertical = MARKED && matches!(direction, Direction::TopToBottom | Direction::BottomToTop);
    // The glyph `k`th in the text's order.
    let label = |k: usize| {
        let at = if backward { count - 1 - k } else { k };
        TextOffset::from_u32(infos.get(at).map_or(0, |info| info.cluster))
    };
    let mut cursor = ClusterCursor::new(source, &range);
    let mut writer = ClusterWriter::<MARKED>::new(source, &range, vertical, marks);
    let mut k = 0;
    while k < count {
        work::step();
        let cluster = cursor.locate(label(k));
        let start = cursor.start();
        let mut e = k + 1;
        while e < count && cursor.holds(label(e)) {
            e += 1;
        }
        // Where the next glyph is labelled inside a later cluster, the
        // shaper merged characters across the clusters between, such as a
        // Bengali ya-phala joined to the syllable before it. The clusters
        // up to the end of the one holding that label are then one, as
        // HarfBuzz's grapheme cluster level, which Chrome shapes with,
        // makes them. The later ones are continuations.
        while e < count {
            let at = label(e);
            cursor.locate(at);
            if cursor.start() == at {
                break;
            }
            while e < count && cursor.holds(label(e)) {
                e += 1;
            }
        }
        // Where the cluster's glyphs lie in the output, which is visual
        // order.
        let (lo, hi) = if backward {
            (count - e, count - k)
        } else {
            (k, e)
        };
        let glyphs = infos.get(lo..hi).unwrap_or_default();
        let placed = positions.get(lo..hi).unwrap_or_default();
        writer.write(cluster, start, glyphs, placed, out);
        k = e;
    }
    writer.fill_empty(range.end, out);
    writer.first
}

/// What [`sink_clusters`] carries from one cluster it writes to the next.
///
/// It writes each cluster in the text's order, and reads the call's marks
/// once each as it passes them.
struct ClusterWriter<'a, const MARKED: bool> {
    source: &'a ShapingSource<'a>,
    /// The next cluster to write.
    next: ClusterId,
    /// The range's first cluster, which keeps no unsafe bit of its own.
    range_start: ClusterId,
    /// Whether a cluster of the range has had glyphs: an empty one after is
    /// a continuation.
    drawn: bool,
    /// Whether harfrust says breaking before the range's start is unsafe.
    first: bool,
    /// The glyphs were shaped down the line.
    vertical: bool,
    /// Where a pair makes breaking unsafe, not yet passed.
    kerned: Peekable<slice::Iter<'a, u32>>,
    /// The marks halved, not yet passed.
    halved: Peekable<slice::Iter<'a, TrimMark>>,
    /// The least advance a mark needs to be halved, or `None` where marks
    /// are not halved.
    least: Option<i32>,
}

impl<'a, const MARKED: bool> ClusterWriter<'a, MARKED> {
    fn new(
        source: &'a ShapingSource<'a>,
        range: &Range<ClusterId>,
        vertical: bool,
        marks: &CallMarks<'a>,
    ) -> Self {
        Self {
            source,
            next: range.start,
            range_start: range.start,
            drawn: false,
            first: false,
            vertical,
            kerned: marks.unsafe_before.iter().peekable(),
            halved: marks
                .halving
                .map_or(&[][..], |(marks, _)| marks)
                .iter()
                .peekable(),
            least: marks.halving.map(|(_, least)| least),
        }
    }

    /// Writes `cluster`, which starts at `start` and is drawn by `glyphs`
    /// placed at `placed`, after the empty clusters before it.
    fn write<I: Id, A: AdvanceUnit, S: AdvanceStore<A>>(
        &mut self,
        cluster: ClusterId,
        start: TextOffset,
        glyphs: &[GlyphInfo],
        placed: &[GlyphPosition],
        out: &mut GlyphSink<'_, I, A, S>,
    ) {
        self.fill_empty(cluster, out);
        let moved = self.halve(start, placed);
        let flags = self.flags(cluster, start, is_unsafe_start(glyphs, start), false);
        match (glyphs, placed) {
            ([glyph], [position]) if !MARKED => out.one(glyph, position, flags),
            _ => out.cluster(glyphs, placed, self.vertical, flags, moved),
        }
        self.drawn = true;
        self.next = ClusterId::new(cluster.get() + 1);
    }

    /// Writes the clusters from the next one up to `end` with no glyphs:
    /// unsafe, since they lie inside something merged, and continuations
    /// once a cluster of the range has drawn.
    fn fill_empty<I: Id, A: AdvanceUnit, S: AdvanceStore<A>>(
        &mut self,
        end: ClusterId,
        out: &mut GlyphSink<'_, I, A, S>,
    ) {
        while self.next < end {
            let next = self.next;
            let flags = self.flags(next, self.source.start(next), true, self.drawn);
            out.cluster(&[], &[], self.vertical, flags, Halved::default());
            self.next = ClusterId::new(next.get() + 1);
        }
    }

    /// Returns the flags `cluster`, starting at `start`, is written with.
    ///
    /// - `raw` is whether harfrust says breaking before it is unsafe.
    /// - A pair meeting before it also makes breaking unsafe.
    /// - `continuation` is whether it has no glyphs after a cluster that had
    ///   some.
    ///
    /// The range's start keeps no unsafe bit of its own. For it, `raw` is
    /// recorded in `first` instead.
    fn flags(
        &mut self,
        cluster: ClusterId,
        start: TextOffset,
        raw: bool,
        continuation: bool,
    ) -> Flags {
        let kerned = self.is_kerned(start);
        if cluster == self.range_start {
            self.first = raw;
            // Kept only where a pair meets there, since the start of the
            // shaped range is shaped as a start.
            return Flags {
                unsafe_to_break: kerned,
                continuation,
            };
        }
        Flags {
            unsafe_to_break: raw || kerned,
            continuation,
        }
    }

    /// Whether a pair makes breaking before a cluster starting at `start`
    /// unsafe: never on the plain path.
    fn is_kerned(&mut self, start: TextOffset) -> bool {
        if !MARKED {
            return false;
        }
        let start = start.to_u32();
        let mut found = false;
        while let Some(&&at) = self.kerned.peek() {
            if at > start {
                break;
            }
            found |= at == start;
            self.kerned.next();
        }
        found
    }

    /// Returns how a cluster starting at `start`, whose glyphs are `placed`,
    /// moves.
    ///
    /// It is halved where it is a found mark at least nine tenths of an em
    /// wide (Chrome's `HanKerning::ApplyKerning`), and never on the plain
    /// path.
    fn halve(&mut self, start: TextOffset, placed: &[GlyphPosition]) -> Halved {
        let Some(least) = self.least.filter(|_| MARKED) else {
            return Halved::default();
        };
        let start = start.to_u32();
        while self.halved.peek().is_some_and(|mark| mark.start() < start) {
            self.halved.next();
        }
        let Some(mark) = self.halved.peek().filter(|mark| mark.start() == start) else {
            return Halved::default();
        };
        let advance: i64 = placed
            .iter()
            .map(|position| along(position, self.vertical))
            .sum();
        if advance < i64::from(least) {
            return Halved::default();
        }
        // Half, toward zero, as Chrome halves a `TextRunLayoutUnit`.
        let half = -i32::try_from(advance / 2).unwrap_or(0);
        match mark.class() {
            TrimClass::Open => Halved {
                shift: half,
                trim: 0,
            },
            TrimClass::Close => Halved {
                shift: 0,
                trim: half,
            },
            _ => Halved::default(),
        }
    }
}

/// Whether [`sink_output`] takes its plain path where it can.
///
/// It always does, except in tests that force the marked path.
fn plain_paths() -> bool {
    #[cfg(test)]
    {
        !MARKED_ONLY.with(Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

#[cfg(test)]
std::thread_local! {
    /// When set, [`sink_output`] on this thread takes the marked path
    /// even where there are no marks, so tests can compare it with the plain
    /// path.
    pub(super) static MARKED_ONLY: Cell<bool> = const { Cell::new(false) };
}
