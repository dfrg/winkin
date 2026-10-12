//! Analysis tests.
//!
//! This file holds the builders, the views, [`check`] and the record sizes.
//! The children pin:
//! - `clusters`: clusters, against UAX #29's own test file and ICU's
//!   segmenter;
//! - `paragraphs`: paragraphs and their base levels;
//! - `bidi`: bidi levels;
//! - `breaks`: line-break opportunities, each option with the answer Chrome
//!   or CSS gives;
//! - `ruby`: breaks inside and after ruby columns;
//! - `runs`: shaping boundaries and script runs;
//! - `orientation`: orientation runs in vertical lines;
//! - `whole`: rebuilds, `::first-line` and text of every kind;
//! - `stream`: the line stream, which resumes exactly over UAX #14's test
//!   strings.
//!
//! Every layout here is built through [`build_with`], which checks the analysis's
//! invariants each time:
//! - the clusters tile the text, and each lies in one item;
//! - the paragraphs tile the clusters, and the runs tile each paragraph's;
//! - the stops are the break bits;
//! - each item finds its first cluster.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

use super::breaks::{LineKey, LineStream};
use super::*;
use crate::config::SmallKana;
use crate::data::IdRange;
use crate::data::{Id, Run};
use crate::stages::content::{ItemKind, NodeKey};
use crate::style::{
    BaseDirection, ComputedBlockStyle, ComputedStyle, Direction, EdgesGroup, FontWeight, Hyphens,
    Language, LineBreak, OverflowWrap, Sides, TextCombineUpright, TextOrientation, TextWrapMode,
    UnicodeBidi, VerticalAlign, WhiteSpaceCollapse, WordBreak, WritingMode,
};
use crate::tests::{key, no_fonts, nowrap, styled, white_space};
use crate::{BoxSize, BuildOptions, FloatSide, Layout, LayoutBuilder, unicode};

// Building -----------------------------------------------------------------

fn language(tag: &str) -> ComputedStyle<'static> {
    let language = Language::parse(tag).ok();
    styled(|style| style.text.language = language)
}

fn word_break(word_break: WordBreak) -> ComputedStyle<'static> {
    styled(|style| style.text.word_break = word_break)
}

fn line_break(line_break: LineBreak) -> ComputedStyle<'static> {
    styled(|style| style.text.line_break = line_break)
}

/// A layout of `calls` in a block set in `block`, checked.
fn build_with(
    block: &ComputedBlockStyle<'_>,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) -> Layout {
    let mut layout = Layout::new();
    let mut cx = no_fonts();
    let mut builder = layout.builder(&mut cx, key(0), block, BuildOptions::default());
    calls(&mut builder);
    builder.finish();
    check(&layout);
    layout
}

/// A layout of `calls` under the initial styles.
fn build(calls: impl FnOnce(&mut LayoutBuilder<'_>)) -> Layout {
    build_with(&ComputedBlockStyle::new(&ComputedStyle::initial()), calls)
}

/// A layout of one text in `root`.
fn root_text(root: &ComputedStyle<'_>, text: &str) -> Layout {
    build_with(&ComputedBlockStyle::new(root), |b| b.text(key(1), text))
}

/// A layout of one text, as written: white space preserved.
fn pre(text: &str) -> Layout {
    root_text(&white_space(WhiteSpaceCollapse::Preserve), text)
}

/// A layout of one text under the initial style.
fn plain(text: &str) -> Layout {
    root_text(&ComputedStyle::initial(), text)
}

/// A layout of `pieces` as text nodes of their own, each in its style.
fn pieces(pieces: &[(&str, &ComputedStyle<'_>)]) -> Layout {
    build(|b| {
        for (at, (text, style)) in (1..).zip(pieces) {
            b.open_box(key(100 + at), style, None);
            b.text(key(at), text);
            b.close_box();
        }
    })
}

// Views --------------------------------------------------------------------

fn analysis(layout: &Layout) -> &Analysis {
    layout.analysis()
}

/// The text `range` of clusters covers.
fn cluster_text(layout: &Layout, range: Range<ClusterId>) -> &str {
    let clusters = &analysis(layout).clusters;
    let start = clusters.start(range.start).get();
    let end = clusters.start(range.end).get();
    let end = if range.end.get() >= clusters.len() {
        layout.content().text.len()
    } else {
        end
    };
    layout.content().text.get(start..end).unwrap_or_default()
}

/// Each cluster's text.
fn clusters(layout: &Layout) -> Vec<&str> {
    let clusters = &analysis(layout).clusters;
    clusters
        .ids()
        .map(|id| {
            let range = clusters.range(id);
            layout
                .content()
                .text
                .get(range.start.get()..range.end.get())
                .unwrap_or_default()
        })
        .collect()
}

/// Each cluster's class.
fn classes(layout: &Layout) -> Vec<ClusterClass> {
    let clusters = &analysis(layout).clusters;
    clusters
        .ids()
        .filter_map(|id| clusters.attrs(id))
        .map(ClusterAttrs::class)
        .collect()
}

/// The offsets at the ends of the clusters that have `bit`.
fn ends_with(layout: &Layout, bit: u8) -> Vec<usize> {
    let clusters = &analysis(layout).clusters;
    clusters
        .ids()
        .filter(|&id| clusters.attrs(id).is_some_and(|attrs| attrs.has(bit)))
        .filter_map(|id| clusters.end(id))
        .map(|end| end.end().get())
        .collect()
}

/// Where a line may break, as text offsets.
fn breaks(layout: &Layout) -> Vec<usize> {
    ends_with(layout, ClusterAttrs::BREAK_AFTER)
}

/// The text of each cluster that hangs.
fn hanging(layout: &Layout) -> Vec<&str> {
    let clusters = &analysis(layout).clusters;
    clusters
        .ids()
        .filter(|&id| clusters.hangs(id))
        .map(|id| cluster_text(layout, id..ClusterId::new(id.get() + 1)))
        .collect()
}

/// Where each continuation starts.
fn continuations(layout: &Layout) -> Vec<usize> {
    let clusters = &analysis(layout).clusters;
    clusters
        .ids()
        .filter(|&id| clusters.end(id).is_some_and(ClusterEnd::is_continuation))
        .map(|id| clusters.start(id).get())
        .collect()
}

/// Each paragraph's text.
fn paragraphs(layout: &Layout) -> Vec<&str> {
    let paragraphs = &analysis(layout).paragraphs;
    paragraphs
        .iter()
        .map(|(id, _)| cluster_text(layout, paragraphs.clusters(id)))
        .collect()
}

/// Each paragraph's level.
fn levels(layout: &Layout) -> Vec<u8> {
    analysis(layout)
        .paragraphs
        .iter()
        .map(|(_, paragraph)| paragraph.level.get())
        .collect()
}

/// Each run's text, script and language.
fn runs(layout: &Layout) -> Vec<(&str, String, String)> {
    let analysis = analysis(layout);
    let runs = analysis.runs.as_slice();
    runs.iter()
        .enumerate()
        .map(|(at, run)| {
            let end = runs
                .get(at + 1)
                .map_or(ClusterId::new(analysis.clusters.len()), ScriptRun::start);
            let language = layout.content().languages.get(run.language);
            (
                cluster_text(layout, run.start()..end),
                String::from(run.script.as_str()),
                String::from(language.as_str()),
            )
        })
        .collect()
}

/// Each run's text and script.
fn scripts(layout: &Layout) -> Vec<(&str, String)> {
    runs(layout)
        .into_iter()
        .map(|(text, script, _)| (text, script))
        .collect()
}

/// Everything the analysis holds, to compare two of them.
fn dump(analysis: &Analysis) -> String {
    format!(
        "{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}\n{:?}",
        analysis.clusters.ends,
        analysis.clusters.attrs,
        analysis.clusters.stops,
        analysis.item_clusters,
        analysis.paragraphs,
        analysis.runs,
        analysis.flags,
    )
}

/// Each cluster's level: its run's, which the runs, tiling the clusters,
/// say once a run.
fn cluster_levels(layout: &Layout) -> Vec<u8> {
    let analysis = analysis(layout);
    let runs = &analysis.runs;
    let end = analysis.clusters.end_id();
    runs.iter()
        .flat_map(|(id, run)| runs.span(id, end).ids().map(|_| run.level.get()))
        .collect()
}

// The invariants ------------------------------------------------------------

/// Checks everything the analysis promises about itself.
fn check(layout: &Layout) {
    let content = layout.content();
    let text = &content.text;
    let analysis = analysis(layout);
    let clusters = &analysis.clusters;

    // The clusters tile the text, none empty.
    let mut at = 0;
    for id in clusters.ids() {
        let end = clusters.end(id).map_or(0, |end| end.end().get());
        assert!(end > at, "{id:?} is empty or runs backwards");
        assert!(text.is_char_boundary(end), "{id:?} ends inside a character");
        at = end;
    }
    assert_eq!(at, text.len(), "the clusters do not reach the text's end");

    // The stops are the break bits.
    assert_eq!(clusters.stop_bytes(), clusters.len().div_ceil(64) * 8);
    for id in clusters.ids() {
        let after = ClusterId::new(id.get() + 1);
        let bit = clusters.first_opportunity(id..after) == Some(after);
        let breaks = clusters
            .attrs(id)
            .is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER));
        assert_eq!(bit, breaks, "{id:?}'s stop is not its break");
    }

    // The paragraphs tile the clusters, each ending at its separator, and a
    // separator ends one.
    let paragraphs = analysis.paragraphs.paragraphs.as_slice();
    assert!(!paragraphs.is_empty(), "there is always a paragraph");
    let mut next = ClusterId::new(0);
    let mut union = ParagraphFlags::NONE;
    for (at, paragraph) in paragraphs.iter().enumerate() {
        let range = analysis.paragraphs.clusters(ParagraphId::new(at));
        assert_eq!(range.start, next, "paragraph {at} does not follow on");
        assert!(range.start <= range.end);
        let last = at + 1 == paragraphs.len();
        if range.start == range.end {
            assert!(last, "only the last paragraph can be empty");
        }
        for id in range.clone().ids() {
            let class = clusters.attrs(id).map(ClusterAttrs::class);
            let forced = class.is_some_and(ClusterClass::is_forced_break);
            let is_last = id.get() + 1 == range.end.get();
            assert_eq!(
                forced,
                is_last && (forced || !last),
                "paragraph {at}: a separator is exactly a paragraph's end",
            );
            if is_last {
                let attrs = clusters
                    .attrs(id)
                    .unwrap_or(ClusterAttrs::new(ClusterClass::Text));
                assert!(
                    !attrs.has(ClusterAttrs::BREAK_AFTER),
                    "a paragraph's end breaks"
                );
                assert!(!attrs.has(ClusterAttrs::EMERGENCY_AFTER));
            }
        }
        let unshaped = range.clone().ids().any(|id| {
            clusters.attrs(id).is_some_and(|attrs| {
                !attrs.class().is_shaped() && attrs.class() != ClusterClass::Separator
            })
        });
        assert!(
            !unshaped || paragraph.flags.contains(ParagraphFlags::HAS_SHAPING_STOPS),
            "paragraph {at}: the shaping gate covers every unshaped cluster"
        );
        union.insert(paragraph.flags);
        next = range.end;
    }
    assert_eq!(
        next.get(),
        clusters.len(),
        "the paragraphs do not reach the end"
    );
    assert_eq!(
        union, analysis.flags,
        "the flags are the paragraphs' together"
    );

    // Nothing breaks before a continuation or a separator.
    for id in clusters.ids().skip(1) {
        let separator = clusters
            .attrs(id)
            .is_some_and(|attrs| attrs.class().is_forced_break());
        if separator || clusters.end(id).is_some_and(ClusterEnd::is_continuation) {
            let before = clusters.attrs(ClusterId::new(id.get() - 1));
            assert!(
                before.is_some_and(|attrs| !attrs.has(ClusterAttrs::BREAK_AFTER)
                    && !attrs.has(ClusterAttrs::EMERGENCY_AFTER)),
                "a break before {id:?}"
            );
        }
    }

    // The runs tile each paragraph's clusters, with its level.
    let runs = analysis.runs.as_slice();
    let mut starts = runs.iter().map(ScriptRun::start).peekable();
    let mut previous = None;
    for (id, _) in analysis.paragraphs.iter() {
        let range = analysis.paragraphs.clusters(id);
        if range.start == range.end {
            continue;
        }
        assert_eq!(
            starts.peek().copied(),
            Some(range.start),
            "a run starts each paragraph"
        );
        while let Some(start) = starts.next_if(|&start| start < range.end) {
            assert!(Some(start) > previous, "the runs are out of order");
            previous = Some(start);
        }
    }
    assert_eq!(starts.next(), None, "a run starts past the text");
    // Each run is at its paragraph's level, but in a paragraph whose
    // clusters are at other levels, which has a run at another.
    for run in runs {
        let paragraph = analysis
            .paragraphs
            .iter()
            .find(|&(id, _)| analysis.paragraphs.clusters(id).contains(&run.start()))
            .map(|(_, paragraph)| paragraph);
        let level = paragraph.map_or(BidiLevel::LTR, |paragraph| paragraph.level);
        let mixed = paragraph
            .is_some_and(|paragraph| paragraph.flags.contains(ParagraphFlags::MIXED_LEVELS));
        assert!(
            mixed || run.level == level,
            "{run:?} at its paragraph's level"
        );
        // Horizontal lines set text across; lines set as horizontal ones
        // turned set it on its side; only vertical ones set it otherwise.
        let expected = match content.block.writing_mode {
            WritingMode::HorizontalTb => Some(RunOrientation::Horizontal),
            WritingMode::SidewaysRl | WritingMode::SidewaysLr => Some(RunOrientation::Sideways),
            WritingMode::VerticalRl | WritingMode::VerticalLr => None,
        };
        if let Some(expected) = expected {
            assert_eq!(run.orientation, expected);
        } else {
            assert_ne!(run.orientation, RunOrientation::Horizontal);
        }
    }

    // Each item finds its first cluster, and each text-bearing item's
    // clusters are exactly its text.
    let items = content.items.as_slice();
    assert_eq!(analysis.item_clusters.len(), items.len());
    let firsts = analysis.item_clusters.firsts.as_slice();
    for (at, item) in items.iter().enumerate() {
        let first = firsts[at];
        let end = firsts
            .get(at + 1)
            .copied()
            .unwrap_or(ClusterId::new(clusters.len()));
        assert_eq!(
            clusters.start(first),
            item.start,
            "item {at}'s first cluster"
        );
        if item.kind.has_text() {
            let covered = (first..end).ids().map(|id| clusters.range(id)).fold(
                item.start..item.start,
                |range, cluster| {
                    assert_eq!(range.end, cluster.start);
                    range.start..cluster.end
                },
            );
            assert_eq!(covered, item.start..item.end, "item {at}'s clusters");
        } else {
            assert_eq!(first, end, "item {at} holds no clusters");
        }
        let objects = (first..end)
            .ids()
            .filter(|&id| clusters.attrs(id).map(ClusterAttrs::class) == Some(ClusterClass::Object))
            .count();
        assert_eq!(objects, usize::from(item.kind == ItemKind::Atomic));
    }
}

#[test]
fn records_keep_their_sizes() {
    assert_eq!(size_of::<ClusterEnd>(), 4);
    assert_eq!(size_of::<ClusterAttrs>(), 1);
    assert_eq!(size_of::<Paragraph>(), 8);
    assert_eq!(size_of::<ScriptRun>(), 12);
}

mod bidi;
mod bidi_allocations;
mod bidi_conformance;
mod breaks;
mod clusters;
mod orientation;
mod paragraphs;
mod ruby;
mod runs;
mod stream;
mod whole;
