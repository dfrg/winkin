//! Measurement tests.
//!
//! This file holds the fonts, the fixture, [`check`] and the shared views.
//! It pins the record sizes, the bytes plain Latin costs a cluster, and
//! inputs that must not panic. The children pin:
//! - `prefix`: the prefix against shaping's advances, and where it goes back;
//! - `edges`: the pen, box edges and atomic inlines;
//! - `heights`: item extents, the strut and em boxes;
//! - `intrinsic`: intrinsic sizes and tabs;
//! - `spacing`: spacing, indent, line padding and hanging punctuation;
//! - `shifts`: `vertical-align` shifts measured before any line;
//! - `autospace`: `text-autospace` seams;
//! - `ruby`: ruby columns' overhang and split columns' sizes.
//!
//! Every layout here is built through [`Fixture::build`], which checks the
//! stage's invariants each time (see [`check`]).

use crate::stages::fonts::Generated;
use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::iter;
use core::ops::Range;

use fontwich::Collection;

use super::spacing::LetterWordSpacing;
use super::tabs::TabStops;
use super::*;
use crate::config::RubyOverhangRule;
use crate::config::{Config, DominantBaselines, SuperSubPosition, WordSpacing};
use crate::data::IdRange;
use crate::data::{Id, Keyed};
use crate::stages::LineStages;
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ParagraphFlags, ParagraphId};
use crate::stages::content::{
    ContentFlags, ItemFlags, ItemId, ItemKind, NodeId, NodeKind, TextFlags,
};
use crate::stages::measure::{RubyColumn, RubyLevel};
use crate::style::{
    BaseDirection, BidiGroup, BoxDecorationBreak, ComputedStyle, Direction, DominantBaseline,
    EdgesGroup, FontFamilyName, FontGroup, HangEnd, HangingPunctuation,
    InitialLetter as InitialLetterStyle, Language, LengthPercentage, LineGroup, LineHeight,
    OverflowWrap, RubyOverhang, Sides, TabSize, TextAutospace, TextIndent, TextOrientation,
    TextWrapMode, UnicodeBidi, WhiteSpaceCollapse, WordBreak, WritingMode,
};
use crate::tests::{
    AHEM_FAMILY, Fixture, LATIN, NARROW, PUNCT, StageCheck, TestFont, at, families_style,
    han_fallback, px, sized, vertically,
};
use crate::unit::TextUnit;
use crate::{
    BoxSize, BuildOptions, ComputedBlockStyle, Context, FloatSide, Layout, LayoutBuilder, NodeKey,
};

// Fonts --------------------------------------------------------------------

/// ASCII in a font whose three sets of line metrics disagree, as Consolas's
/// and Yu Gothic's do: a win pair of 900 and 300, `hhea` of 800, 200 and a
/// 100 gap, and typographic metrics of 700, 300 and a 200 gap that it asks
/// to be used by. At 20 px: 18, 6 and no gap under `Win`; 16, 4 and 2 under
/// `Hhea`; 14, 6 and 4 under `TypoOrHhea`.
fn metrics() -> TestFont {
    let mut font = TestFont::new("Test Metrics", &[(0x20, 0x7E)]);
    font.win = (900, 300);
    font.hhea = (800, -200, 100);
    font.typo = (700, -300, 200);
    font.use_typo = true;
    font
}

/// ASCII a third of an em wide, so running sums fall between layout's 1/64
/// grid at 16 px; and a kerning pair that makes `A` go back before `V`.
fn narrow() -> TestFont {
    let mut font = TestFont::new("Test Narrow", &[(0x20, 0x7E)]);
    font.advances = (0x20u8..=0x7E).map(|b| (char::from(b), 333)).collect();
    font.kerning = vec![('A', 'V', -600)];
    font
}

/// Latin with ligatures, a character drawn as two glyphs, a raised one and a
/// kern, as the shaping stage's tests have it.
fn latin() -> TestFont {
    let mut font = TestFont::new("Test Latin", &[(0x20, 0x7E), (0xAD, 0xAD)]);
    font.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    font.splits = vec!['Q'];
    font.raised = vec![('R', 200)];
    font.kerning = vec![('A', 'V', -100)];
    font
}

/// A Han font that Han falls back to from Ahem, deeper below the baseline
/// than Ahem and with a line gap: 14, 8 and 4 at 20 px.
fn han() -> TestFont {
    let mut font = TestFont::new(
        "Test Han",
        &[(0x20, 0x20), (0x3000, 0x30FF), (0x4E00, 0x9FFF)],
    );
    font.win = (700, 400);
    font.hhea = (700, -400, 200);
    font
}

const METRICS: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Metrics"))];

const ASCII_ONLY: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test ASCII"))];

// Fixtures -----------------------------------------------------------------

/// A fixture over an application layer of Ahem and the fonts above, whose
/// fallback puts Ahem at the head of every chain and Test Han among Han's,
/// checking the stage each build (see [`check`]).
fn fixture() -> Fixture {
    Fixture::new(
        &[metrics(), narrow(), latin(), han()],
        han_fallback("Test Han"),
        StageCheck::Built(|_, layout| check(layout)),
    )
}

/// Checks the stage's invariants:
/// - a prefix entry per boundary, the text's end included;
/// - a flag byte per paragraph, and an extent per item;
/// - stored em boxes that agree with the fragment rule;
/// - line-edge costs sorted, one a boundary, each where a line may start
///   or end, and paragraphs flagged exactly where one falls;
/// - a paragraph marked `NONMONOTONE` exactly where its prefix goes back;
/// - shifts and kept boxes in node order;
/// - intrinsic sizes that are neither negative nor out of order.
fn check(layout: &Layout) {
    let measured = layout.measured();
    let text = measured.text(FirstLineVariant::Standard);
    let clusters = layout.analysis().clusters.len();
    assert_eq!(
        text.prefix.sums.len(),
        clusters + 1,
        "an entry per boundary"
    );
    assert_eq!(text.prefix.sums.first(), Some(&InlineLayoutUnit::ZERO));
    let paragraphs = &layout.analysis().paragraphs;
    assert_eq!(text.paragraphs.len(), paragraphs.len(), "flags a paragraph");
    assert_eq!(
        text.extents.len(),
        layout.content().items.len(),
        "an extent an item"
    );
    // Stored whole-item em boxes must agree with the fragment rule, including
    // first-line items whose fonts extend past the measurement reach.
    if layout.content().flags.contains(ContentFlags::RUBY)
        || layout.content().flags.contains(ContentFlags::EMPHASIS)
    {
        for variant in [FirstLineVariant::Standard, FirstLineVariant::FirstLine] {
            if variant == FirstLineVariant::FirstLine && measured.first_line().is_none() {
                continue;
            }
            let content = layout.content();
            let fonts = layout.fonts();
            let analysis = layout.analysis();
            let reach = if variant == FirstLineVariant::Standard {
                analysis.clusters.end_id()
            } else {
                analysis.first_line_reach(content).unwrap().end
            };
            let boxes = measured.text(variant).em_boxes();
            for (id, item) in content.items.iter() {
                let range = analysis.item_clusters.range(id);
                let expected = if range.start <= reach
                    && !range.is_empty()
                    && item.kind == ItemKind::Text
                    && !item.flags.contains(ItemFlags::ANNOTATION)
                {
                    let text = content.nodes.text_facts(item.node, variant);
                    let request = content.facts.text_request(text);
                    fonts
                        .runs(variant)
                        .containing(range.start)
                        .and_then(|first| {
                            let (em, uniform) =
                                super::em_box(fonts, request, range, variant, first);
                            uniform.then_some(em).filter(|em| !em.is_none())
                        })
                } else {
                    None
                };
                assert_eq!(
                    boxes.get(id),
                    expected,
                    "{variant:?} {id:?} whole-item em box"
                );
            }
        }
    }
    // The line-edge costs, which cloned boxes, line padding, hanging
    // punctuation and ruby overhang make, are sorted, one a boundary, each
    // where a line may start or end, and a paragraph is flagged exactly
    // where one falls at its start, inside it or at its end.
    let edges = text.edge_costs();
    assert!(
        edges
            .as_slice()
            .windows(2)
            .all(|pair| pair[0].key() < pair[1].key())
    );
    let flags = layout.content().flags;
    let hanging = flags.contains(ContentFlags::HANGING_PUNCTUATION);
    let soft_hyphens = layout
        .analysis()
        .flags
        .contains(ParagraphFlags::HAS_SOFT_HYPHEN);
    if !flags.contains(ContentFlags::CLONE_BOXES)
        && !flags.contains(ContentFlags::LINE_PADDING)
        && !flags.contains(ContentFlags::RUBY)
        && !hanging
        && !soft_hyphens
    {
        assert!(edges.is_empty());
    }
    let clusters_attrs = &layout.analysis().clusters;
    for edge in edges.iter() {
        let before = edge.key().get().checked_sub(1).map(ClusterId::new);
        let breaks = before
            .and_then(|before| clusters_attrs.attrs(before))
            .is_some_and(|attrs| {
                attrs.has(ClusterAttrs::BREAK_AFTER)
                    || attrs.has(ClusterAttrs::EMERGENCY_AFTER)
                    || attrs.class().is_forced_break()
            });
        let starts = paragraphs.iter().any(|(id, _)| {
            let range = paragraphs.clusters(id);
            range.start == edge.key() && !range.is_empty()
        });
        let ends_text = edge.key().get() == clusters;
        assert!(
            breaks || starts || ends_text,
            "{:?} is no place a line starts or ends",
            edge.key()
        );
        // A hyphen is owed after a soft hyphen a line may break after, and
        // nowhere else.
        let after_soft_hyphen = before
            .and_then(|before| clusters_attrs.attrs(before))
            .is_some_and(|attrs| {
                attrs.class() == ClusterClass::SoftHyphen && attrs.has(ClusterAttrs::BREAK_AFTER)
            });
        if edge.flags.contains(LineEdgeFlags::HYPHEN) {
            assert!(
                after_soft_hyphen && !ends_text,
                "{:?} owes no hyphen",
                edge.key()
            );
        }
        if !hanging {
            assert!(
                edge.flex == LayoutUnit::ZERO
                    && (edge.flags == LineEdgeFlags::NONE || edge.flags == LineEdgeFlags::HYPHEN)
            );
        }
        assert!(edge.flex >= LayoutUnit::ZERO);
        if edge.flags.contains(LineEdgeFlags::FIRST_LINE_ONLY) {
            assert_eq!(
                edge.key(),
                ClusterId::new(0),
                "only the first line starts there"
            );
        }
    }
    for (id, _) in paragraphs.iter() {
        let range = paragraphs.clusters(id);
        let monotone = (range.start..range.end)
            .ids()
            .all(|c| text.prefix.get(c) <= text.prefix.get(ClusterId::new(c.get() + 1)));
        assert_eq!(
            text.paragraph(id).contains(MeasureFlags::NONMONOTONE),
            !monotone,
            "{id:?} is marked exactly where its prefix goes back"
        );
        let costs = edges
            .iter()
            .any(|edge| range.start <= edge.key() && edge.key() <= range.end);
        assert_eq!(
            text.paragraph(id).contains(MeasureFlags::HAS_EDGE_COSTS),
            costs,
            "{id:?}'s costs"
        );
    }
    // A shift a box or an atomic inline has, in node order, where some
    // style sets `vertical-align` or `dominant-baseline`; and a shifted box
    // keeps its fragment.
    let shifts = text.shifts();
    assert!(
        shifts
            .as_slice()
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0)
    );
    let content_flags = layout.content().flags;
    if !content_flags.contains(ContentFlags::VERTICAL_ALIGN)
        && !content_flags.contains(ContentFlags::DOMINANT_BASELINE)
    {
        assert!(shifts.is_empty());
    }
    for &(node, shift) in shifts.iter() {
        assert_ne!(shift, LayoutUnit::ZERO);
        match layout.content().nodes.kind(node) {
            Some(NodeKind::Box) => assert!(text.kept_boxes.get(node).is_some()),
            Some(NodeKind::Atomic | NodeKind::FirstLetter) => {}
            other => panic!("{node:?} is shifted and is {other:?}"),
        }
        assert_eq!(text.shift(node), shift);
    }
    // A kept box's entry names the node of an opening item, in node order.
    let content = layout.content();
    let kept = text.kept_boxes.boxes.as_slice();
    assert!(kept.windows(2).all(|pair| pair[0].0 < pair[1].0));
    for (node, extent) in text.kept_boxes.iter() {
        assert!(
            content.items.as_slice().iter().any(|item| item.node == node
                && matches!(
                    item.kind,
                    ItemKind::Open | ItemKind::RubyOpen | ItemKind::AnnotationOpen
                )),
            "{node:?} is a box"
        );
        assert!(!extent.is_none());
    }
    // The text metrics: a row for each text's facts, the block's strut the
    // lines', and a mark's em box exactly where the text sets marks.
    let block = content
        .nodes
        .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
    assert_eq!(
        measured.text_metrics(block).map(|metrics| metrics.strut),
        Some(text.extents.strut),
        "the strut is the block's text's"
    );
    for id in content.facts.text_ids() {
        let metrics = measured.text_metrics(id).expect("a row a text's facts");
        let marks = content.facts.text(id).has(TextFlags::EMPHASIS);
        assert_eq!(!metrics.mark.is_none(), marks, "{id:?}'s marks");
    }
    // The ruby columns in text order, none where there is no ruby, each
    // over its own items and clusters, its levels its own, in order, and no
    // wider than it; its room inside it.
    let rubies = text.ruby_columns();
    if !content_flags.contains(ContentFlags::RUBY) {
        assert!(rubies.is_empty() && rubies.level_count() == 0);
    }
    let mut seen_levels = vec![false; rubies.level_count()];
    for (at, column) in rubies.iter() {
        let base = column.base.clone();
        assert!(base.start <= base.end);
        assert!(column.open <= column.base_end && column.base_end <= column.close);
        for id in column.levels.clone().ids() {
            assert!(
                !seen_levels[id.get()],
                "{at:?}'s levels overlap another column"
            );
            seen_levels[id.get()] = true;
        }
        if let Some(parent) = column.parent {
            assert!(parent < at);
            let parent = rubies.get(parent).unwrap();
            assert!(parent.base.start <= base.start && base.end <= parent.base.end);
            assert!(column.close <= parent.base_end);
            assert_eq!(column.overhang, (LayoutUnit::ZERO, LayoutUnit::ZERO));
        }
        for level in rubies.levels(column) {
            assert!(level.width <= column.width);
            assert!(base.end <= level.clusters.start);
        }
        assert!(column.room_before >= LayoutUnit::ZERO);
        assert!(column.room_inside >= LayoutUnit::ZERO);
        assert!(column.room_before + column.room_inside <= column.width);
        let (start, end) = column.overhang;
        assert!(start >= LayoutUnit::ZERO && end >= LayoutUnit::ZERO);
        if let Some(next) = rubies.get(RubyColumnId::new(at.get() + 1)) {
            assert!(base.start <= next.base.start);
            if column.close > next.open {
                assert_eq!(next.parent, Some(at));
            }
        }
    }
    assert!(seen_levels.into_iter().all(|seen| seen));
    // The generated texts: each the font stage chose, shaped a piece for
    // each of its runs, in the run's font, from the left as drawn.
    let generated = measured.generated();
    let fonts = layout.fonts().generated();
    let chosen: Vec<_> = fonts.iter().collect();
    for &(facts, kind, text) in &chosen {
        let runs: Vec<_> = fonts.runs(text).iter().map(|run| run.font).collect();
        let (pieces, rtl) = match kind {
            Generated::Hyphen => {
                let rtl = layout.content().facts.text(facts).has(TextFlags::RTL);
                (generated.hyphen(facts), rtl)
            }
            Generated::Ellipsis => match generated.ellipsis(facts, false) {
                Some(pieces) => (Some(pieces), false),
                None => (generated.ellipsis(facts, true), true),
            },
        };
        let pieces = pieces.expect("shaped");
        let mut shaped: Vec<_> = pieces
            .ids()
            .filter_map(|id| generated.get(id))
            .inspect(|piece| assert_eq!((piece.text, piece.kind), (facts, kind)))
            .map(|piece| piece.font)
            .collect();
        if rtl {
            shaped.reverse();
        }
        assert_eq!(shaped, runs);
        assert!(generated.snapped(pieces) >= LayoutUnit::ZERO);
    }
    assert!(generated.iter().len() >= chosen.len());
    if chosen.is_empty() {
        assert!(generated.is_empty());
    }
    assert_eq!(
        measured.first_line().is_some(),
        layout
            .analysis()
            .first_line_reach(layout.content())
            .is_some()
    );
    let intrinsic = measured.intrinsic;
    assert!(LayoutUnit::ZERO <= intrinsic.min && intrinsic.min <= intrinsic.max);
}

/// `style` with its left and right edges, each `[margin, border, padding]`.
fn edged<'a>(style: &ComputedStyle<'a>, left: [f32; 3], right: [f32; 3]) -> ComputedStyle<'a> {
    let sides = |at: usize| Sides {
        left: left[at],
        right: right[at],
        ..Sides::ZERO
    };
    ComputedStyle {
        edges: EdgesGroup {
            margin: sides(0).into(),
            border: sides(1),
            padding: sides(2).into(),
            ..EdgesGroup::INITIAL
        },
        ..*style
    }
}

fn lu(n: f32) -> LayoutUnit {
    LayoutUnit::from_px(n)
}

/// Every boundary's position, in pixels.
fn positions(layout: &Layout) -> Vec<f32> {
    let text = layout.measured().text(FirstLineVariant::Standard);
    (0..=layout.analysis().clusters.len())
        .map(|b| text.prefix.get(at(b)).to_px())
        .collect()
}

/// Where the pen stands to draw `cluster`, in `stages`, the items at its
/// boundary sought (`LineStages::pen`).
fn pen(stages: &LineStages<'_>, cluster: ClusterId) -> InlineLayoutUnit {
    stages.pen(
        cluster,
        stages
            .analysis
            .item_clusters
            .cursor_containing(cluster)
            .id(),
    )
}

/// The em box of `part` of the text of `node` in `layout`, its font runs
/// sought where it starts (`em_box`).
fn node_em_box(layout: &Layout, node: NodeId, part: Range<ClusterId>) -> Extent {
    let (content, fonts) = (layout.content(), layout.fonts());
    let variant = FirstLineVariant::Standard;
    let text = content.nodes.text_facts(node, variant);
    let request = content.facts.text_request(text);
    let first = fonts.runs(variant).containing(part.start).expect("a run");
    super::em_box(fonts, request, part, variant, first).0
}

// The records --------------------------------------------------------------

/// The records keep their sizes: a prefix entry 8 bytes, an extent 8, a
/// paragraph's flags 1, a line-edge cost 20, a ruby column 68 and an
/// annotation level 36.
#[test]
fn records_keep_their_sizes() {
    assert_eq!(size_of::<InlineLayoutUnit>(), 8);
    assert_eq!(size_of::<Extent>(), 8);
    assert_eq!(size_of::<MeasureFlags>(), 1);
    assert_eq!(size_of::<LineEdgeCost>(), 20);
    assert_eq!(size_of::<RubyColumn>(), 72);
    assert_eq!(size_of::<RubyLevel>(), 44);
}

// Nothing panics -----------------------------------------------------------

/// Whatever a caller gives -- sizes and edges that are not finite, huge, or
/// negative, line heights of every kind -- measures into a valid stage.
#[test]
fn any_input_gives_valid_measurements() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for bad in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        -1e30,
        0.0,
    ] {
        let style = ComputedStyle {
            line: LineGroup {
                height: LineHeight::Factor(bad),
                ..LineGroup::INITIAL
            },
            ..sized(&AHEM_FAMILY, bad)
        };
        let wild = edged(&style, [bad, bad, bad], [bad, -bad, bad]);
        let lined = ComputedStyle {
            line: LineGroup {
                height: LineHeight::Px(bad),
                ..LineGroup::INITIAL
            },
            ..wild
        };
        fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
            b.text(NodeKey(1), "a b");
            b.open_box(NodeKey(2), &wild, None);
            b.text(NodeKey(3), "c");
            b.open_box(NodeKey(4), &lined, None);
            b.atomic(
                NodeKey(5),
                &wild,
                None,
                BoxSize {
                    inline: bad,
                    block: bad,
                    baseline: Some(bad),
                },
            );
            b.close_box();
            b.close_box();
            b.line_break(NodeKey(6));
        });
        let _ = layout.intrinsic_sizes();
    }
}

// Memory -------------------------------------------------------------------

/// Plain Latin costs at most 19 bytes a cluster of layout state from the
/// content to the measurements, the text aside: clusters
/// 5.125 (their ends, classes and stops), glyphs 4, the prefix 8, and runs,
/// items and nodes that are a few a paragraph. Every table counted by its
/// length, the fixed cost of the style and its font included.
#[test]
fn plain_latin_keeps_a_few_bytes_a_cluster() {
    use crate::stages::analysis::{ClusterAttrs, ClusterEnd, Paragraph, ScriptRun};
    use crate::stages::content::{
        BoxFactsId, FontRequest, Item, NodeKind, ShapingFacts, TextFacts, TextFactsId,
    };
    use crate::stages::fonts::{FontResolution, FontRun, UsedFont};
    use crate::stages::shape::{GlyphWord, ShapedRun};
    let prose = "The quick brown fox jumps over the lazy dog, and then it runs away \
                 into the woods, where nobody sees it again until the evening comes. ";
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.build(
        &mut layout,
        &ComputedBlockStyle::new(&families_style(&AHEM_FAMILY)),
        |b| {
            for n in 0..12 {
                b.text(NodeKey(2 * n + 1), &prose.repeat(2));
                b.line_break(NodeKey(2 * n + 2));
            }
        },
    );
    let (content, analysis, fonts) = (layout.content(), layout.analysis(), layout.fonts());
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let text = layout.measured().text(FirstLineVariant::Standard);
    let clusters = analysis.clusters.len();
    let node = size_of::<NodeKind>()
        + 2 * size_of::<TextFactsId>()
        + 2 * size_of::<BoxFactsId>()
        + 2 * size_of::<ItemId>()
        + size_of::<NodeId>()
        + size_of::<NodeKey>();
    let stages = [
        (
            "content",
            content.items.len() * size_of::<Item>()
                + content.nodes.len() * node
                + content.facts.text_count()
                    * (size_of::<TextFacts>()
                        + size_of::<ShapingFacts>()
                        + size_of::<FontRequest>()),
        ),
        (
            "analysis",
            clusters * (size_of::<ClusterEnd>() + size_of::<ClusterAttrs>())
                + analysis.clusters.stop_bytes()
                + analysis.item_clusters.len() * size_of::<ClusterId>()
                + analysis.paragraphs.len() * size_of::<Paragraph>()
                + analysis.runs.len() * size_of::<ScriptRun>(),
        ),
        (
            "fonts",
            fonts.runs(FirstLineVariant::Standard).len() * size_of::<FontRun>()
                + fonts.used.len() * size_of::<UsedFont>()
                + content.facts.request_count() * size_of::<FontResolution>(),
        ),
        (
            "shaped",
            shaped.glyphs.len() * size_of::<GlyphWord>()
                + shaped.glyphs.sidecar.len() * 16
                + shaped.runs.len() * size_of::<ShapedRun>(),
        ),
        (
            "measured",
            text.prefix.sums.len() * size_of::<InlineLayoutUnit>()
                + text.paragraphs.len() * size_of::<MeasureFlags>()
                + text.extents.len() * size_of::<Extent>(),
        ),
    ];
    let per = |bytes: usize| bytes as f64 / clusters as f64;
    let total: usize = stages.iter().map(|(_, bytes)| bytes).sum();
    let shown: Vec<(&str, f64)> = stages
        .iter()
        .map(|&(stage, bytes)| (stage, per(bytes)))
        .collect();
    assert!(clusters > 3_000, "{clusters} clusters");
    assert!(
        per(total) <= 19.0,
        "{:.3} B a cluster: {shown:?}",
        per(total)
    );
    // The prefix is 8 bytes a cluster and one entry more.
    assert_eq!(text.prefix.sums.len(), clusters + 1);
    // What the figure is, pinned so that it moves only on purpose: 17.81,
    // of which the content is 0.38, the analysis 5.25, the fonts 0.03, the
    // shaping 4.09 and the measurements 8.07.
    assert!(
        (17.7..17.9).contains(&per(total)),
        "{:.3} B a cluster: {shown:?}",
        per(total)
    );
}

mod autospace;
mod edges;
mod heights;
mod intrinsic;
mod prefix;
mod ruby;
mod shifts;
mod spacing;
