//! Shaping tests, over Ahem and fonts built in memory, never the machine's.
//!
//! This file holds the shared fonts, fixtures and [`check`], and the record
//! sizes. The children pin:
//! - `glyphs`: advances and glyph storage;
//! - `runs`: where runs split, and what the shaping plan gets;
//! - `reshape`: unsafe bits, reshaping a sub-range, and the context's caches;
//! - `full`: variable fonts, synthesized small capitals, kerning and `trak`;
//! - `spacing`: `text-spacing-trim`;
//! - `vertical`: vertical metrics and text on its side;
//! - `plain`: the fast path against the cluster-by-cluster one.
//!
//! Every layout here is built through [`Fixture::build`], which runs
//! [`check`] each time.

use alloc::borrow::Cow;

use alloc::string::String;

use alloc::vec::Vec;

use alloc::{format, vec};

use core::mem::size_of;
use core::slice;

use fontwich::Collection;

use harfrust::Script;

use super::walk::paragraph_text;

use super::*;

use crate::config::Config;
use crate::work;

use crate::config::PunctuationTrim;
use crate::stages::content::{ItemId, ItemKind};

use crate::data::Id;

use crate::style::{
    AdjustMetric, BaseDirection, BidiGroup, ComputedStyle, Direction, EdgesGroup, FirstLineVariant,
    FontFamilyName, FontGroup, FontKerning, FontLanguageOverride, FontSizeAdjust, FontVariantCaps,
    FontWeight, Language, Sides, Tag, TextGroup, TextOrientation, TextSpacingTrim, UnicodeBidi,
    WhiteSpaceCollapse, WritingMode,
};

use crate::tests::{
    AHEM_FAMILY, ARABIC, BEH, Fixture, Form, MEEM, PLAIN_PUNCT, PUNCT, SEEN, StageCheck, TEH,
    TestAxis, TestFont, TestVertical, arabic, families_style, han, han_fallback, sized,
};

use crate::{BuildOptions, ComputedBlockStyle, Context, Layout, NodeKey};

// Fonts --------------------------------------------------------------------

/// ASCII, the soft hyphen, the combining diacritics and Hebrew, with `fi`,
/// `fl` and `ffi` ligatures, `Q` drawn as two glyphs, `R` drawn raised,
/// `AV` kerned, and a Turkish `i` of its own; and for right-to-left text, an
/// alef-lamed ligature and bet drawn as two glyphs.
fn latin_hebrew() -> TestFont {
    let mut font = TestFont::new(
        "Test Latin Hebrew",
        &[(0x20, 0x7E), (0xAD, 0xAD), (0x300, 0x36F), (0x5D0, 0x5EA)],
    );
    font.ligatures = vec![
        vec!['f', 'f', 'i'],
        vec!['f', 'i'],
        vec!['f', 'l'],
        vec![ALEF, LAMED],
    ];
    font.splits = vec!['Q', BET];
    font.raised = vec![('R', 200)];
    font.kerning = vec![('A', 'V', -100)];
    font.localized = Some((*b"TRK ", 'i'));
    font
}

/// The family of [`latin_hebrew`].
const LATIN_HEBREW: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Latin Hebrew"))];

const ALEF: char = '\u{5D0}';

const BET: char = '\u{5D1}';

const GIMEL: char = '\u{5D2}';

const LAMED: char = '\u{5DC}';

// Fixtures -----------------------------------------------------------------

/// Returns a fixture over an application layer of Ahem and `fonts`.
///
/// Its fallback puts Ahem at the head of every chain and Test Han among
/// Han's script fonts. It runs [`check`] on each build.
fn fixture_with(fonts: &[TestFont]) -> Fixture {
    Fixture::new(
        fonts,
        han_fallback("Test Han"),
        StageCheck::BuiltWithContext(check),
    )
}

/// A fixture over Ahem and the fonts above.
fn fixture() -> Fixture {
    fixture_with(&[latin_hebrew(), arabic(), han()])
}

impl Fixture {
    /// Each cluster's advance as the last build's shaping handed it on
    /// (see [`advances`]).
    fn advances(&self) -> Vec<i32> {
        advances(&self.cx)
    }

    /// Shapes `range` of `layout`'s run `run` again, as the breaker will,
    /// into tables of its own.
    fn reshape(&mut self, layout: &Layout, run: ShapedRunId, range: Range<ClusterId>) -> Piece {
        let shaped = layout.shaped().text(FirstLineVariant::Standard);
        let run = shaped.runs.get(run).expect("a run");
        let key = ShapingKey::new(
            run,
            layout.content(),
            layout.analysis(),
            layout.fonts(),
            false,
        );
        let source = ShapingSource::new(
            layout.content(),
            layout.analysis(),
            FirstLineVariant::Standard,
        );
        let analysis = layout.analysis();
        let paragraph = analysis
            .paragraphs
            .containing(range.start)
            .expect("a paragraph");
        let paragraph = paragraph_text(analysis, paragraph);
        let mut piece = Piece::default();
        let cx = self.cx.shaping();
        let mut sink = GlyphSink::new(&mut piece.words, &mut piece.sidecar, &mut piece.advances);
        let edges = ShapingEdges::default();
        piece.start = shape_range(
            &mut ShapeSession::new(cx, None),
            &source,
            paragraph,
            range,
            &key,
            edges,
            NeighbourFonts::default(),
            &mut sink,
        );
        piece
    }
}

/// A range shaped on its own, as a reshaped line edge is, its advances in
/// 16.16 as a line keeps them.
#[derive(Default)]
struct Piece {
    words: Table<ClusterId, GlyphWord>,
    sidecar: Table<SidecarGlyphId, SidecarGlyph>,
    advances: Vec<TextUnit>,
    start: bool,
}

/// Each cluster's advance along the line as the last build in `cx` handed it
/// on, in 1/65536 px, which a test's clusters fit an `i32` in.
fn advances(cx: &Context) -> Vec<i32> {
    cx.shaped_advances()
        .iter()
        .map(|advance| i32::try_from(advance.raw()).expect("a test's cluster fits 16.16"))
        .collect()
}

/// One glyph as drawn: its id, its offset across and up, and its advance,
/// all in 16.16.
type Drawn = (u32, i32, i32, i32);

/// A cluster's glyphs as drawn, whichever way its word stores them: a
/// compact glyph's advance is the cluster's.
fn drawn(
    word: GlyphWord,
    sidecar: &Table<SidecarGlyphId, SidecarGlyph>,
    advance: i32,
) -> Vec<Drawn> {
    match word.glyphs(sidecar) {
        ClusterGlyphs::None => Vec::new(),
        ClusterGlyphs::One(id) => vec![(id, 0, 0, advance)],
        ClusterGlyphs::Many(glyphs) => glyphs
            .iter()
            .map(|glyph| {
                let (x, y) = (glyph.x_offset, glyph.y_offset);
                (glyph.id(), x.raw(), y.raw(), glyph.advance.raw())
            })
            .collect(),
    }
}

/// The glyph ids of every cluster of `layout`, in cluster order.
fn ids(layout: &Layout) -> Vec<Vec<u32>> {
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    shaped
        .glyphs
        .iter()
        .map(|(_, &word)| {
            drawn(word, &shaped.glyphs.sidecar, 0)
                .iter()
                .map(|g| g.0)
                .collect()
        })
        .collect()
}

/// Checks the shaped text's invariants.
///
/// - Each cluster has a word and an advance.
/// - Runs tile the clusters from the first. Each lies in the analysis run
///   holding its start and names a used font the layout has. Each is all
///   shaped or all not.
/// - A cluster that isn't shaped has no glyphs and no advance, and nor does
///   a continuation.
/// - An expanded word points at a whole group of the sidecar, and the words
///   cover the sidecar exactly.
/// - The flags say what is there.
/// - There is no combined text and no first line's table, since the content
///   checked here never asks for them.
fn check(layout: &Layout, cx: &Context) {
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let clusters = &layout.analysis().clusters;
    let count = clusters.len();
    let advances = advances(cx);
    assert_eq!(shaped.glyphs.len(), count, "a word per cluster");
    assert_eq!(advances.len(), count, "an advance per cluster");
    let runs: Vec<(&ShapedRun, Range<ClusterId>)> = shaped
        .runs
        .iter()
        .map(|(_, run, clusters)| (run, clusters))
        .collect();
    assert_eq!(
        runs.is_empty(),
        count == 0,
        "runs exactly where clusters are"
    );
    if let Some(first) = runs.first() {
        assert_eq!(
            first.1.start.get(),
            0,
            "the runs start at the first cluster"
        );
    }
    let script_runs: Vec<_> = layout.analysis().runs.iter().map(|(_, run)| run).collect();
    for (at, (run, span)) in runs.iter().enumerate() {
        let end = runs.get(at + 1).map_or(count, |next| next.1.start.get());
        assert!(span.start.get() < end, "runs in order, none empty");
        let holding =
            script_runs.partition_point(|script_run| script_run.start() <= span.start) - 1;
        assert_eq!(run.script_run.get(), holding, "the analysis run holding it");
        let script_end = script_runs
            .get(holding + 1)
            .map_or(count, |next| next.start().get());
        assert!(
            end <= script_end,
            "a shaping run never crosses an analysis run"
        );
        assert!(run.font.get() < layout.fonts().used.len(), "a font it has");
        let classes: Vec<bool> = (span.start.get()..end)
            .map(|c| {
                clusters
                    .attrs(ClusterId::new(c))
                    .unwrap()
                    .class()
                    .is_shaped()
            })
            .collect();
        assert!(
            classes.iter().all(|&s| s == classes[0]),
            "a run is shaped or not throughout"
        );
        // It links the shaping facts of the item holding its start, which
        // every text item of a shaped run has.
        let content = layout.content();
        let shaping_at = |cluster: usize| {
            // The last item starting at or before it, those holding none
            // at its boundary coming before the one holding it.
            let items = &layout.analysis().item_clusters;
            let id = (0..items.len())
                .map(ItemId::new)
                .rfind(|&id| items.start(id) <= ClusterId::new(cluster))
                .unwrap();
            let item = content.items.get(id).unwrap();
            let text = content
                .nodes
                .text_facts(item.node, FirstLineVariant::Standard);
            (item.kind, content.facts.text(text).shaping)
        };
        assert_eq!(run.shaping, shaping_at(span.start.get()).1, "its link");
        if classes[0] {
            for cluster in span.start.get()..end {
                let (kind, shaping) = shaping_at(cluster);
                assert!(
                    kind != ItemKind::Text || shaping == run.shaping,
                    "{cluster} shapes as its run does"
                );
            }
        }
    }
    // How many clusters draw each entry of the sidecar, and each cluster's
    // range of it.
    let sidecar = shaped.glyphs.sidecar.as_slice();
    let mut drawn = vec![0usize; sidecar.len()];
    let mut groups = Vec::new();
    let mut any_unsafe = false;
    for (cluster, &word) in shaped.glyphs.iter() {
        let advance = advances[cluster.get()];
        any_unsafe |= word.is_unsafe_to_break();
        if !clusters.attrs(cluster).unwrap().class().is_shaped() {
            assert_eq!(word, GlyphWord::EMPTY, "{cluster:?} is not shaped");
            assert_eq!(advance, 0, "{cluster:?} is not shaped");
        }
        if word.is_continuation() {
            assert_eq!(word.glyphs(&shaped.glyphs.sidecar), ClusterGlyphs::None);
            assert_eq!(advance, 0, "a continuation's advance is its ligature's");
        }
        if let ClusterGlyphs::Many(glyphs) = word.glyphs(&shaped.glyphs.sidecar) {
            assert!(!glyphs.is_empty() && glyphs.last().unwrap().is_last());
            assert!(glyphs[..glyphs.len() - 1].iter().all(|g| !g.is_last()));
            let sum: i32 = glyphs.iter().map(|g| g.advance.raw()).sum();
            assert_eq!(sum, advance, "an expanded cluster's advance is its glyphs'");
            let start =
                (glyphs.as_ptr() as usize - sidecar.as_ptr() as usize) / size_of::<SidecarGlyph>();
            for count in &mut drawn[start..start + glyphs.len()] {
                *count += 1;
            }
            groups.push(start..start + glyphs.len());
        }
    }
    // Every glyph in the sidecar is some cluster's; a cluster of one glyph
    // may share its entry with others drawing the same glyph alike, and a
    // cluster of several has its own.
    assert!(
        drawn.iter().all(|&count| count > 0),
        "every glyph in the sidecar is drawn"
    );
    for group in groups.into_iter().filter(|group| group.len() > 1) {
        assert!(
            drawn[group].iter().all(|&count| count == 1),
            "a cluster of several glyphs has them to itself"
        );
    }
    // The run holding each cluster, counted from the runs' starts, is the
    // last run starting at or before it, and none past the last.
    for at in 0..=count {
        let cluster = ClusterId::new(at);
        let searched = runs
            .iter()
            .rposition(|(_, clusters)| clusters.start <= cluster)
            .filter(|_| at < count);
        assert_eq!(
            shaped.runs.containing(cluster).map(|run| run.get()),
            searched,
            "the run holding {cluster:?}"
        );
    }
    let flags = layout.shaped().flags;
    assert_eq!(flags.contains(ShapedFlags::HAS_UNSAFE), any_unsafe);
    assert!(shaped.combined.is_empty());
    assert!(layout.shaped().first_line().is_none());
}

/// Scales `units` in an em of `upem` to `scale` as harfrust does.
///
/// The sum is `(units × ((scale << 16) / upem) + 2^15) >> 16`, HarfBuzz's
/// `em_mult`.
fn scaled(units: i64, upem: i64, scale: i64) -> i32 {
    i32::try_from((units * ((scale << 16) / upem) + 32768) >> 16).unwrap()
}

/// `n` pixels in 16.16.
fn px16(n: i32) -> i32 {
    n << 16
}

// The records --------------------------------------------------------------

/// A cluster's word takes 4 bytes, a sidecar glyph 16 and a run 12.
#[test]
fn records_keep_their_sizes() {
    assert_eq!(size_of::<GlyphWord>(), 4);
    assert_eq!(size_of::<SidecarGlyph>(), 16);
    assert_eq!(size_of::<ShapedRun>(), 12);
}

/// A word refuses what it cannot hold rather than wrapping it.
#[test]
fn a_word_holds_what_fits_and_refuses_the_rest() {
    let empty = Table::new();
    assert_eq!(
        GlyphWord::from_glyph(7).map(|w| w.glyphs(&empty)),
        Some(ClusterGlyphs::One(7))
    );
    assert_eq!(GlyphWord::from_glyph(GlyphWord::MAX_PAYLOAD + 1), None);
    let at = SidecarGlyphId::new(usize::try_from(GlyphWord::MAX_PAYLOAD).unwrap());
    assert!(GlyphWord::from_sidecar(at).is_some());
    let past = SidecarGlyphId::new(usize::try_from(GlyphWord::MAX_PAYLOAD).unwrap() + 1);
    assert_eq!(GlyphWord::from_sidecar(past), None);
    // An expanded word into a sidecar cut short reads what is there.
    let word = GlyphWord::from_sidecar(SidecarGlyphId::new(3)).unwrap();
    assert_eq!(word.glyphs(&Table::new()), ClusterGlyphs::Many(&[]));
    let word = GlyphWord::EMPTY.with_unsafe(true);
    assert!(word.is_unsafe_to_break());
    assert_eq!(word.glyphs(&Table::new()), ClusterGlyphs::None);
}

mod full;
mod glyphs;
mod plain;
mod reshape;
mod runs;
mod spacing;
mod vertical;
