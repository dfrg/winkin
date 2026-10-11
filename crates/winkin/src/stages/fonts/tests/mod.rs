//! Font selection tests, over Ahem and fonts built in memory, never the
//! machine's.
//!
//! This file holds the shared fonts, fixtures and [`check`], and the record
//! sizes. The children pin:
//! - `fallback`: the primary font, the used size, and fallback per cluster;
//! - `presentation`: emoji presentation;
//! - `covers`: coverage across charset pages and normalization forms;
//! - `normalized`: coverage against harfrust's normalizer;
//! - `sequences`: variation sequences, against Chrome's measured cases;
//! - `line_metrics`: the line metrics each [`Config`] source reads;
//! - `robustness`: inputs that must not panic;
//! - `caches`: web fonts and what the context keeps;
//! - `full`: variable fonts, synthesis, features, small capitals and
//!   generated text;
//! - `positions`: `font-variant-position`;
//! - `vertical`: line metrics in vertical lines;
//! - `walk`: choosing for many clusters at once against asking each one.
//! - `shared`: fonts shared between contexts.
//!
//! Every layout here is built through [`Fixture::build`], which runs
//! [`check`] each time.

use alloc::string::String;

use alloc::sync::Arc;

use alloc::vec;

use alloc::vec::Vec;

use core::ptr;

use fontwich::{FallbackKey, FallbackOverride, Family, GenericBucket};

use fontwich::{Charset, Collection, FaceDescriptors, LayerBuilder, Role, Script};

use super::{coverage, select, *};

use crate::stages::analysis::{ClusterClass, ParagraphFlags};

use crate::config::{Config, LineMetricsSource};

use crate::config::{PlatformFontVariations, PositionSynthesis, SuperSubPosition};

use crate::stages::content::NodeId;

use crate::data::Id;

use crate::style::{
    AdjustMetric, ComputedStyle,
    FirstLineVariant::{self, Standard},
    FontFamilyName, FontKerning, FontOpticalSizing, FontSizeAdjust, FontStyle, FontVariantCaps,
    FontVariantEmoji, FontVariantPosition, FontVariants, FontVariation, FontWeight, GenericFamily,
    Language, TextOrientation, TextSpacingTrim, WhiteSpaceCollapse, WritingMode,
};

use crate::tests::{
    AHEM, Fixture, StageCheck, TestAxis, TestFont, ahem_fallback, collection, families_style,
    han_fallback, sized,
};

use crate::unicode;

use crate::layout::primary_id;
use crate::{BoxSize, BuildOptions, ComputedBlockStyle, Context, Layout, LayoutBuilder, NodeKey};

// Fonts --------------------------------------------------------------------

/// A Han font: CJK ideographs and punctuation, and digits, spaces and a
/// combining acute besides, which it must not take from the fonts before it.
fn han() -> TestFont {
    let mut font = TestFont::new(
        "Test Han Acute",
        &[
            (0x20, 0x20),
            (0x30, 0x39),
            (0x301, 0x301),
            (0x3000, 0x303F),
            (0x4E00, 0x9FFF),
        ],
    );
    font.win = (880, 120);
    font.hhea = (880, -120, 0);
    font
}

/// ASCII, the breaking hyphen, and nothing else: not a no-break space, an em
/// space, a non-breaking hyphen or a joiner.
fn plain() -> TestFont {
    TestFont::new("Test Plain Hyphen", &[(0x20, 0x7E), (0x2010, 0x2010)])
}

/// A space, `e`, `a` and the combining acute, and no precomposed letter.
fn marks() -> TestFont {
    TestFont::new(
        "Test Acute Marks",
        &[(0x20, 0x20), (0x61, 0x61), (0x65, 0x65), (0x301, 0x301)],
    )
}

/// Combining marks and nothing to put them on.
fn only_marks() -> TestFont {
    TestFont::new("Test Only Marks", &[(0x301, 0x302)])
}

/// A text font with the heart suit and ASCII.
fn symbols() -> TestFont {
    TestFont::new("Test Symbols", &[(0x20, 0x7E), (0x2665, 0x2665)])
}

/// A color font with the heart suit and a grinning face.
fn emoji() -> TestFont {
    let mut font = TestFont::new("Test Emoji", &[(0x2665, 0x2665), (0x1F600, 0x1F600)]);
    font.color = true;
    font
}

/// A color font with the keycap bases and ❤ U+2764, as an emoji font has
/// them: `#`, `*`, the digits.
fn keycaps() -> TestFont {
    let mut font = TestFont::new(
        "Test Keycaps",
        &[(0x23, 0x23), (0x2A, 0x2A), (0x30, 0x39), (0x2764, 0x2764)],
    );
    font.color = true;
    font
}

/// A font on three charset pages: capitals, Latin Extended-A, and the first
/// page of ideographs.
fn pages() -> TestFont {
    TestFont::new(
        "Test Pages",
        &[(0x20, 0x20), (0x41, 0x5A), (0x100, 0x17F), (0x4E00, 0x4EFF)],
    )
}

/// A variable font over ASCII: a weight axis from 100 to 900 about 400, a
/// width axis from 50 to 100, and an optical size axis from 6 to 72 about
/// 12, each moving every advance.
fn variable() -> TestFont {
    let mut font = TestFont::new("Test Three Axes", &[(0x20, 0x7E)]);
    font.axes = vec![
        TestAxis {
            tag: *b"wght",
            min: 100.0,
            default: 400.0,
            max: 900.0,
            delta: 200,
        },
        TestAxis {
            tag: *b"wdth",
            min: 50.0,
            default: 100.0,
            max: 100.0,
            delta: 0,
        },
        TestAxis {
            tag: *b"opsz",
            min: 6.0,
            default: 12.0,
            max: 72.0,
            delta: 50,
        },
    ];
    font
}

/// ASCII with small capitals, `smcp`, for the lowercase letters, a quarter
/// of an em narrower than the letters; `c2sc` for the capitals; and
/// superscript figures, `sups`.
fn capitals() -> TestFont {
    let mut font = TestFont::new("Test Capitals", &[(0x20, 0x7E), (0xDF, 0xDF)]);
    font.alternates = vec![
        (*b"smcp", ('a'..='z').map(|ch| (ch, 250)).collect()),
        (*b"c2sc", ('A'..='Z').map(|ch| (ch, 250)).collect()),
        (*b"sups", ('0'..='9').map(|ch| (ch, 300)).collect()),
    ];
    font
}

/// ASCII and U+00B2 ², with superscript and subscript figures of their own,
/// `sups` and `subs`, and nothing else of either: the shape of most fonts
/// that have the features.
fn superscripts() -> TestFont {
    let mut font = TestFont::new("Test Superscripts", &[(0x20, 0x7E), (0xB2, 0xB2)]);
    font.alternates = vec![
        (*b"subs", ('0'..='9').map(|ch| (ch, 300)).collect()),
        (*b"sups", ('0'..='9').map(|ch| (ch, 300)).collect()),
    ];
    font
}

/// As [`superscripts`], but its superscript figures reached only through a
/// contextual lookup, whose context is the figure alone.
fn contextual_superscripts() -> TestFont {
    let mut font = TestFont::new("Test Contextual Superscripts", &[(0x20, 0x7E)]);
    font.contextual = vec![(*b"sups", ('0'..='9').map(|ch| (ch, 300)).collect())];
    font
}

/// [`capitals`] with [`variable`]'s weight axis.
fn capitals_variable() -> TestFont {
    let mut font = capitals();
    font.family = "Test Capitals Variable".into();
    font.axes = variable().axes[..1].to_vec();
    font
}

// Fixtures -----------------------------------------------------------------

/// Returns a fixture over an application layer of Ahem and then `fonts`.
///
/// Its fallback puts Ahem at the head of every chain and Test Han Acute among
/// Han's script fonts. It runs [`check`] on each build.
fn fixture_with(fonts: &[TestFont]) -> Fixture {
    Fixture::new(
        fonts,
        han_fallback("Test Han Acute"),
        StageCheck::Built(check),
    )
}

/// Returns a fixture over `collection` that knows `families` by name.
///
/// It runs [`check`] on each build.
fn fixture_over(collection: Collection, families: &[String]) -> Fixture {
    Fixture::from_collection(collection, families, StageCheck::Built(check))
}

impl Fixture {
    /// The family `font` is an instance of, or `"none"` for a used font with
    /// no instance.
    fn family(&self, layout: &Layout, font: UsedFontId) -> &str {
        let used = used_font(layout, font);
        let Some(drawn) = used.instance.as_ref() else {
            return "none";
        };
        self.family_name(drawn.key()).unwrap_or("unknown")
    }

    /// The family each cluster is set in.
    fn families(&self, layout: &Layout) -> Vec<&str> {
        (0..layout.analysis().clusters.len())
            .map(|at| {
                let font = cluster_font(layout, at).expect("a run");
                self.family(layout, font)
            })
            .collect()
    }

    /// The primary font of the block's own text.
    fn primary(&self, layout: &Layout) -> UsedFontId {
        let block = layout.content().nodes.text_facts(NodeId::BLOCK, Standard);
        primary_id(layout, block).expect("a font request")
    }
}

/// The text's font runs: every line's but a first one restyled.
fn text_runs(layout: &Layout) -> &FontRuns {
    layout.fonts().runs(Standard)
}

/// The used font the text's `cluster` is set in, by search, or `None` where
/// no run reaches it.
fn cluster_font(layout: &Layout, cluster: usize) -> Option<UsedFontId> {
    let runs = text_runs(layout);
    runs.get(runs.containing(ClusterId::new(cluster))?)
        .map(|run| run.font)
}

/// The text's font runs, in order, for a test to index and pair.
fn ordered_runs(layout: &Layout) -> Vec<FontRun> {
    text_runs(layout).iter().map(|(_, run)| *run).collect()
}

/// The used font `font` of `layout`, which it has.
fn used_font(layout: &Layout, font: UsedFontId) -> &UsedFont {
    layout.fonts().used.get(font).expect("a used font")
}

/// The coordinates the used font of `cluster` is at, as 2.14 bits.
fn cluster_coords(layout: &Layout, cluster: usize) -> Vec<i16> {
    let font = cluster_font(layout, cluster).expect("a run");
    let drawn = used_font(layout, font)
        .instance
        .as_ref()
        .expect("an instance");
    drawn.coords.to_vec()
}

/// The features the used font of `cluster` shapes with, as tags and values,
/// but the `chws` every style asks for under `text-spacing-trim: normal`,
/// which [`chws_is_on_unless_trimming_is_off_or_the_settings_say`] pins.
fn cluster_features(layout: &Layout, cluster: usize) -> Vec<(&'static str, u16)> {
    cluster_all_features(layout, cluster)
        .into_iter()
        .filter(|&feature| feature != ("chws", 1))
        .collect()
}

/// The features the used font of `cluster` shapes with, every one.
fn cluster_all_features(layout: &Layout, cluster: usize) -> Vec<(&'static str, u16)> {
    let font = cluster_font(layout, cluster).expect("a run");
    let drawn = used_font(layout, font)
        .instance
        .as_ref()
        .expect("an instance");
    drawn
        .features
        .iter()
        .map(|feature| {
            let tag = feature.tag.to_bytes();
            let name = match &tag {
                b"calt" => "calt",
                b"chws" => "chws",
                b"clig" => "clig",
                b"halt" => "halt",
                b"palt" => "palt",
                b"c2sc" => "c2sc",
                b"dlig" => "dlig",
                b"hist" => "hist",
                b"kern" => "kern",
                b"liga" => "liga",
                b"onum" => "onum",
                b"smcp" => "smcp",
                b"ss01" => "ss01",
                b"subs" => "subs",
                b"sups" => "sups",
                b"tnum" => "tnum",
                b"vkrn" => "vkrn",
                _ => "other",
            };
            (name, feature.value)
        })
        .collect()
}

/// The used font `cluster` is set in.
fn cluster_used_font(layout: &Layout, cluster: usize) -> &UsedFont {
    let font = cluster_font(layout, cluster).expect("a run");
    used_font(layout, font)
}

/// A normalized coordinate for `value` on an axis from `default` to `max`,
/// as 2.14 bits, within a unit of how `fvar` rounds it.
fn near(bits: i16, fraction: f32) {
    let expected = (fraction * 16384.0) as i32;
    assert!(
        (i32::from(bits) - expected).abs() <= 1,
        "{bits} is not {fraction} of the axis"
    );
}

/// Checks the font runs' invariants.
///
/// - Every text facts row has a primary font the layout has.
/// - The runs tile the clusters from the first, name fonts the layout has,
///   and differ from their neighbours.
/// - There are no first-line runs, since the content checked here never asks
///   for them.
/// - A hyphen or an ellipsis is generated only where the content may draw
///   one.
fn check(layout: &Layout) {
    let fonts = layout.fonts();
    let used = fonts.used.len();
    for text in layout.content().facts.text_ids() {
        let primary = primary_id(layout, text).expect("all text has fonts");
        assert!(primary.get() < used, "a primary the layout has");
    }
    let runs = ordered_runs(layout);
    let clusters = layout.analysis().clusters.len();
    assert_eq!(
        runs.is_empty(),
        clusters == 0,
        "runs exactly where clusters are"
    );
    if let Some(first) = runs.first() {
        assert_eq!(
            first.start().get(),
            0,
            "the runs start at the first cluster"
        );
    }
    for pair in runs.windows(2) {
        assert!(pair[0].start() < pair[1].start(), "runs in order");
        assert_ne!(pair[0].font, pair[1].font, "neighbours differ");
    }
    for run in &runs {
        assert!(run.start().get() < clusters, "a run starts at a cluster");
        assert!(run.font.get() < used, "a font the layout has");
    }
    assert!(!fonts.has_first_line_runs());
    // Generated text appears only where the content may need it, in style
    // order. Each text's runs reach its end one after the other.
    let soft_hyphens = layout
        .analysis()
        .flags
        .contains(ParagraphFlags::HAS_SOFT_HYPHEN);
    let cuts = layout.content().block.may_cut_lines();
    let mut last = None;
    for (style, kind, text) in fonts.generated().iter() {
        match kind {
            Generated::Hyphen => assert!(soft_hyphens, "a hyphen with no soft hyphen"),
            Generated::Ellipsis => assert!(cuts, "an ellipsis with no cut"),
        }
        let runs = fonts.generated().runs(text);
        let said = text.string.text(layout.content().lists.hyphen_strings());
        assert_eq!(runs.last().map(|run| run.end()), Some(said.len()));
        for pair in runs.windows(2) {
            assert!(pair[0].end() < pair[1].end(), "each run takes some of it");
            assert_ne!(pair[0].font, pair[1].font, "neighbours differ");
        }
        for run in runs {
            assert!(run.font.get() < used, "a font the layout has");
        }
        assert!(last <= Some(style), "in style order");
        last = Some(style);
    }
}

/// Font selection keeps nothing per cluster and 8 bytes a run.
///
/// A font request's resolution takes 12 bytes. A used font is mostly its
/// line metrics and how it is shaped and drawn. The optional shaping part
/// is no larger for being optional. A feature it shapes with takes 6 bytes.
#[test]
fn records_keep_their_sizes() {
    use core::mem::size_of;
    assert_eq!(size_of::<FontRun>(), 8);
    assert_eq!(size_of::<UsedFontId>(), 2);
    assert_eq!(size_of::<FontLineMetrics>(), 80);
    assert_eq!(size_of::<FontFeature>(), 6);
    assert_eq!(size_of::<Option<Arc<UsedInstance>>>(), 8);
    assert_eq!(size_of::<UsedFont>(), 104);
    assert_eq!(size_of::<FontResolution>(), 12);
}

mod caches;
mod covers;
mod fallback;
mod full;
mod line_metrics;
mod normalized;
mod positions;
mod presentation;
mod robustness;
mod sequences;
#[cfg(feature = "std")]
mod shared;
mod vertical;
mod walk;
