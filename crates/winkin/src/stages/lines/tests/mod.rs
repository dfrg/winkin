//! Line breaking tests.
//!
//! This file holds the fonts, the fixture, [`check`] and the shared views.
//! It pins the record sizes, and areas of every degenerate width. The
//! children pin:
//! - `fitting`: exact fitting on Chrome's 1/64 grid, the search against a
//!   plain greedy breaker, and a line's cost;
//! - `overflow`: overflow, `overflow-wrap` and `word-break: break-all`;
//! - `hanging`: forced breaks, control characters and hanging white space;
//! - `tabs`: tabs measured where they land;
//! - `reshape`: unsafe edges reshaped for a kern, a ligature and Arabic
//!   joining;
//! - `heights`: line heights, bands and `vertical-align`;
//! - `relayout`: relayout equal to a fresh layout, in any context;
//! - `spacing`: indents, spacing and hanging punctuation;
//! - `floats`, `first_line`, `ruby`, `cjk` and `wrap`: floats, the first
//!   line, ruby room, CJK line edges, and `balance` and `pretty`.
//!
//! Every layout here is broken through [`Fixture::lay_out`], which checks
//! the lines' invariants each time (see [`check`]).

use alloc::borrow::Cow;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::ops::Range;

use fontwich::{Collection, LayerBuilder, Role};

use super::*;
use crate::build::FloatSide;
use crate::data::IdRange;
use crate::data::{Id, Keyed, Table};
use crate::layout::primary;
use crate::stages::analysis::{ClusterAttrs, ClusterClass, ClusterId, ParagraphFlags, ParagraphId};
use crate::stages::content::{NodeId, NodeKind};
use crate::stages::measure::{self, MeasureFlags};
use crate::stages::shape::{
    ClusterGlyphs, GlyphSink, GlyphWord, NeighbourFonts, ShapeSession, ShapingEdges, ShapingKey,
    ShapingSource, shape_range,
};
use crate::style::{
    BaseDirection, BoxDecorationBreak, ComputedStyle, EdgesGroup, FirstLineVariant, FontFamilyName,
    LengthPercentage, OverflowWrap, Sides, TabSize, TextAlign, TextGroup, TextIndent, TextOverflow,
    TextWrapMode, VerticalAlign, WhiteSpaceCollapse, WordBreak,
};
use crate::tests::{
    ARABIC, BEH, Fixture, Form, LATIN, MEEM, NARROW, SEEN, StageCheck, TEH, TestFont, ahem, at,
    han_fallback, sized, tabbed, texts,
};
use crate::unit::{InlineLayoutUnit, LayoutUnit, TextUnit};
use crate::{
    BoxSize, BuildOptions, ComputedBlockStyle, Context, Layout, LayoutBuilder, LineMetrics, NodeKey,
};

// Fonts --------------------------------------------------------------------

/// ASCII with `fi`, `fl` and `ffi` ligatures three quarters of an em, and
/// `AV` kerned by a tenth of an em.
///
/// Every other glyph is half an em.
fn latin() -> TestFont {
    let mut font = TestFont::new("Test Latin", &[(0x20, 0x7E), (0xAD, 0xAD)]);
    font.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    font.kerning = vec![('A', 'V', -100)];
    font
}

/// Arabic letters and a space.
///
/// The dual-joining letters' initial, medial and final forms are half an em
/// wide. Their isolated forms are wider, 0.6 of an em.
fn arabic() -> TestFont {
    let mut font = TestFont::new("Test Arabic", &[(0x20, 0x20), (0x621, 0x64A)]);
    font.joining = vec![BEH, TEH, SEEN, MEEM];
    font.advances = font.joining.iter().map(|&ch| (ch, 600)).collect();
    font
}

/// ASCII a third of an em wide, and `i` a tenth, so running sums fall
/// between the 1/64 grid at 16 px.
fn narrow() -> TestFont {
    let mut font = TestFont::new("Test Narrow", &[(0x20, 0x7E)]);
    font.advances = (0x20u8..=0x7E).map(|b| (char::from(b), 333)).collect();
    font.advances.push(('i', 100));
    font
}

/// Han, kana and a space, which Han falls back to from Ahem.
fn han() -> TestFont {
    TestFont::new(
        "Test Han",
        &[(0x20, 0x20), (0x3000, 0x30FF), (0x4E00, 0x9FFF)],
    )
}

// Fixtures -----------------------------------------------------------------

/// Returns a fixture over Ahem and the fonts above, checking the lines on
/// every break (see [`check`]).
///
/// Fallback puts Ahem at the head of every chain and Test Han among Han's.
fn fixture() -> Fixture {
    Fixture::new(
        &[latin(), arabic(), narrow(), han()],
        han_fallback("Test Han"),
        StageCheck::Placed(check),
    )
}

/// Checks the lines' invariants:
/// - lines tile every paragraph with clusters, in order, each holding one
///   at least;
/// - reshaped pieces sit inside their lines, in order, a word and an
///   advance a cluster, pointing into the edge glyphs where expanded;
/// - line boxes, at the tops line layout placed them at, stack in order,
///   lower where floats moved them, and the block ends after the last;
/// - the overflow flag agrees with the widths;
/// - every float is placed once, in reading order;
/// - each line's settled shifts follow the last line's, sorted by node, each
///   of a box or atomic inline whose `vertical-align` only a line settles.
fn check(layout: &Layout) {
    let lines = layout.line_records();
    let analysis = layout.analysis();
    let paragraphs = &analysis.paragraphs;
    let mut expected = paragraphs
        .iter()
        .filter(|&(id, _)| !paragraphs.clusters(id).is_empty());
    let mut paragraph = expected.next();
    let mut at = paragraph.map(|(id, _)| paragraphs.clusters(id).start);
    let mut top = lines
        .lines
        .as_slice()
        .first()
        .map_or(untrimmed_end(&lines.block), |_| {
            LayoutUnit::from_px(layout.line(0).expect("a line").metrics().top)
                + lines.block.trim_start
        });
    let words = &lines.edges().words;
    assert_eq!(
        words.len(),
        lines.edges().advances.len(),
        "an advance a word"
    );
    let mut floats = 0;
    let mut shifts = LineShiftId::new(0);
    for (id, line) in lines.lines.iter() {
        let (pid, p) = paragraph.expect("a line is in a paragraph");
        let range = line.clusters();
        assert_eq!(Some(range.start), at, "{id:?} starts where the last ended");
        assert!(range.start < range.end, "{id:?} holds a cluster");
        assert!(
            range.end <= paragraphs.clusters(pid).end,
            "{id:?} stays in its paragraph"
        );
        assert_eq!(line.paragraph, pid);
        assert_eq!(line.level(paragraphs), p.level);
        let mut before = range.start;
        let runs = &layout.shaped().text(line.variant()).runs;
        for shape in lines.edges().shapes.slice(line.shapes.clone()) {
            let piece = shape.clusters();
            assert!(before <= piece.start && piece.start < piece.end && piece.end <= range.end);
            // Shaped with its run's key, a piece stays in its run.
            let run = runs
                .containing(piece.start)
                .map(|run| runs.run_clusters(run));
            assert!(
                run.as_ref().is_some_and(|run| piece.end <= run.end),
                "{id:?}'s piece {piece:?} stays in its run {run:?}"
            );
            let count = piece.end.get() - piece.start.get();
            assert!(shape.first.get() + count <= words.len());
            for k in 0..count {
                let word = words[EdgeClusterId::new(shape.first.get() + k)];
                if let ClusterGlyphs::Many(glyphs) = word.glyphs(&lines.edges().glyphs) {
                    assert!(!glyphs.is_empty());
                }
            }
            before = piece.end;
        }
        // A line stacks on the last, or below it where floats moved it.
        let block_start = layout.fragments().items[layout.fragments().line_heads[id]].block
            + lines.block.trim_start;
        assert!(block_start >= top, "{id:?} is not above the last");
        top = block_start + line.extent.box_height();
        // Its floats follow the last line's.
        let start = lines.floats().partition_point(|float| float.line < id);
        assert_eq!(start, floats, "{id:?}'s floats follow");
        floats = start + lines.line_floats(id).len();
        let overflows = line.indent + line.width > line.band.width() + LayoutUnit::EPSILON;
        assert_eq!(
            line.flags.contains(LineFlags::OVERFLOWS),
            overflows,
            "{id:?} of {:?} in {:?}",
            line.width,
            line.band
        );
        // A clamp ends the block with a line that has text after it, cut for
        // an ellipsis. `text-overflow: ellipsis` cuts a line that overflows.
        let clamped = end_kind(layout, id) == EndKind::Clamped;
        let cut_on_overflow = layout.content().block.text_overflow == TextOverflow::Ellipsis
            && line.flags.contains(LineFlags::OVERFLOWS);
        assert_eq!(
            line.flags.contains(LineFlags::ELLIPSIS),
            clamped || cut_on_overflow,
            "{id:?}"
        );
        assert_eq!(line.shifts.start, shifts, "{id:?}'s shifts follow");
        assert!(line.shifts.start <= line.shifts.end);
        shifts = line.shifts.end;
        let settled = lines.line_shifts(line);
        assert!(settled.windows(2).all(|w| w[0].node < w[1].node));
        for shift in settled {
            let nodes = &layout.content().nodes;
            assert!(matches!(
                nodes.kind(shift.node),
                Some(NodeKind::Box | NodeKind::Atomic)
            ));
            let facts = &layout.content().facts;
            let box_ = facts.box_facts(nodes.box_facts(shift.node, FirstLineVariant::Standard));
            assert!(matches!(
                box_.align,
                VerticalAlign::Middle
                    | VerticalAlign::TextTop
                    | VerticalAlign::TextBottom
                    | VerticalAlign::Top
                    | VerticalAlign::Bottom
            ));
            assert_ne!(shift.shift, LayoutUnit::ZERO);
        }
        // The indent is the block's first line's. Under `each-line` it is
        // also each line's after a forced break, and `hanging` inverts it.
        let indent = layout.content().block.text_indent;
        let first = id.get() == 0;
        let after_break = !first && range.start == paragraphs.clusters(pid).start;
        if (first || (indent.each_line && after_break)) == indent.hanging {
            assert_eq!(line.indent, LayoutUnit::ZERO, "{id:?}");
        }
        at = Some(range.end);
        if range.end == paragraphs.clusters(pid).end {
            paragraph = expected.next();
            at = paragraph.map(|(id, _)| paragraphs.clusters(id).start);
        }
    }
    // A block a clamp ended has no lines past it, and places no float
    // past it.
    let clamped = lines
        .lines
        .last_id()
        .is_some_and(|last| end_kind(layout, last) == EndKind::Clamped);
    assert!(
        paragraph.is_none() || clamped,
        "every paragraph with clusters has its lines"
    );
    // A clearing break may hold the end lower, below the floats it clears.
    if layout.content().clearances().is_empty() {
        assert_eq!(
            untrimmed_end(&lines.block),
            top,
            "the block ends after its last line"
        );
    } else {
        assert!(
            untrimmed_end(&lines.block) >= top,
            "the block ends after its last line"
        );
    }
    assert_eq!(lines.shift_end(), shifts, "every shift is a line's");
    // Every float is placed once, in reading order. Each line's come in the
    // order its text reaches them, beside before below. Those no line
    // reached come last, and every float of the content is there.
    let placed = lines.floats();
    assert!(placed.windows(2).all(|w| w[0].float < w[1].float));
    if clamped {
        assert!(placed.len() <= layout.content().floats().len());
    } else {
        assert_eq!(placed.len(), layout.content().floats().len());
    }
}

/// Returns where the block ends before `text-box-trim` trims it.
fn untrimmed_end(block: &BlockResult) -> LayoutUnit {
    block.block_end + block.trim_start + block.trim_end
}

/// Returns `style` breaking words everywhere.
fn break_all<'a>(style: &ComputedStyle<'a>) -> ComputedStyle<'a> {
    let mut style = *style;
    style.text.word_break = WordBreak::BreakAll;
    style
}

/// Returns each line's content width, in pixels.
fn widths(layout: &Layout) -> Vec<f32> {
    layout.lines().map(|line| line.metrics().width).collect()
}

fn records(layout: &Layout) -> &[LineRecord] {
    layout.line_records().lines.as_slice()
}

/// What ended a line, as [`end_kind`] finds it; the line record does not
/// keep it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum EndKind {
    /// A soft wrap opportunity, the last at which the line fits.
    Soft,
    /// A forced break: the paragraph's separator, which is on the line.
    Forced,
    /// An `overflow-wrap` break inside a word that fits nowhere else, at
    /// `EMERGENCY_AFTER`.
    Emergency,
    /// The first opportunity after nothing fit: the line overflows its band.
    Overflow,
    /// The text's end.
    TextEnd,
    /// `line-clamp`: the last line the block keeps, with text left after it.
    Clamped,
}

/// Returns what ended `layout`'s line `id`, from its record and the
/// analysis.
///
/// - The block's last line with text after it was clamped.
/// - A line ending with its paragraph ended at its separator or the text's
///   end.
/// - Any other ended at a soft wrap opportunity, overflowing or not, or at
///   an `overflow-wrap` break where there is none.
fn end_kind(layout: &Layout, id: LineId) -> EndKind {
    let lines = &layout.line_records().lines;
    let clusters = &layout.analysis().clusters;
    let line = &lines[id];
    let end = line.clusters().end;
    if lines.last_id() == Some(id) && end < clusters.end_id() {
        return EndKind::Clamped;
    }
    let last = ClusterId::new(end.get() - 1);
    let paragraphs = &layout.analysis().paragraphs;
    if end == paragraphs.clusters(line.paragraph).end {
        return if clusters
            .class(last)
            .is_some_and(ClusterClass::is_forced_break)
        {
            EndKind::Forced
        } else {
            EndKind::TextEnd
        };
    }
    if !clusters
        .attrs(last)
        .is_some_and(|attrs| attrs.has(ClusterAttrs::BREAK_AFTER))
    {
        EndKind::Emergency
    } else if line.flags.contains(LineFlags::OVERFLOWS) {
        EndKind::Overflow
    } else {
        EndKind::Soft
    }
}

/// Returns what ended each of `layout`'s lines, in order (see
/// [`end_kind`]).
fn end_kinds(layout: &Layout) -> Vec<EndKind> {
    layout
        .line_records()
        .lines
        .iter()
        .map(|(id, _)| end_kind(layout, id))
        .collect()
}

/// Returns every cluster's word in `layout`'s shaped text, in cluster order.
///
/// Breaking must leave these as it found them.
fn glyph_words(layout: &Layout) -> Vec<GlyphWord> {
    layout
        .shaped()
        .text(FirstLineVariant::Standard)
        .glyphs
        .iter()
        .map(|(_, &word)| word)
        .collect()
}

/// Returns a float's border box, `inline` by `block` pixels.
fn float_size(inline: f32, block: f32) -> BoxSize {
    BoxSize {
        inline,
        block,
        baseline: None,
    }
}

/// Returns where each line's band starts, and how long it is.
fn bands(layout: &Layout) -> Vec<(f32, f32)> {
    layout
        .lines()
        .map(|line| {
            let band = line.metrics().band;
            (band.left, band.size())
        })
        .collect()
}

/// Returns a block whose `text-indent` is `px` plus `fraction` of the area.
fn indented(px: f32, fraction: f32, hanging: bool, each_line: bool) -> ComputedBlockStyle<'static> {
    ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage { px, fraction },
            hanging,
            each_line,
        },
        ..ComputedBlockStyle::default()
    }
}

/// Returns the width of `layout`'s line `line` with its text shaped whole.
///
/// One shaping call covers the line, with the paragraph around it as
/// context, and each cluster spaced as its style says. The reshaped pieces
/// must add up to it. The width is on the grid, as fitting reads it.
fn shaped_whole(fixture: &mut Fixture, layout: &Layout, line: &LineRecord) -> LayoutUnit {
    let analysis = layout.analysis();
    let paragraph = analysis.paragraphs.clusters(line.paragraph);
    let range = line.clusters();
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let run = shaped.runs.run_containing(range.start).expect("a run");
    let key = ShapingKey::new(run, layout.content(), analysis, layout.fonts(), false);
    let clusters = &analysis.clusters;
    let source = ShapingSource::new(layout.content(), analysis, FirstLineVariant::Standard);
    let text = clusters.start(paragraph.start)..clusters.start(paragraph.end);
    let mut words = Table::<ClusterId, GlyphWord>::new();
    let mut sidecar = Table::new();
    let mut advances: Vec<TextUnit> = Vec::new();
    let cx = fixture.cx.shaping();
    let mut sink = GlyphSink::new(&mut words, &mut sidecar, &mut advances);
    let edges = ShapingEdges {
        line_start: range.start > paragraph.start,
        ..ShapingEdges::default()
    };
    shape_range(
        &mut ShapeSession::new(cx, None),
        &source,
        text,
        range.clone(),
        &key,
        edges,
        NeighbourFonts::default(),
        &mut sink,
    );
    let measured = layout.measured().text(FirstLineVariant::Standard);
    let origin = measured.prefix.get(paragraph.start);
    let start = measured.prefix.get(range.start) - origin;
    let spacing = range.clone().ids().zip(words.as_slice()).fold(
        InlineLayoutUnit::ZERO,
        |sum, (cluster, word)| {
            // The line may cross runs, so each cluster uses its own.
            let shaping = shaped.runs.run_containing(cluster).expect("a run").shaping;
            let spacing = measure::LetterWordSpacing::from_shaping(
                &layout.content().facts,
                shaping,
                layout.measured().word_spacing_rule,
            )
            .after_cluster(
                analysis,
                layout.content().text(FirstLineVariant::Standard),
                cluster,
                word.is_continuation(),
            );
            sum + InlineLayoutUnit::from_text(spacing)
        },
    );
    let sum = advances
        .iter()
        .fold(spacing, |sum, &a| sum + InlineLayoutUnit::from_text(a));
    ((start + sum).ceil_to_grid() - start.ceil_to_grid()).to_layout()
}

/// A band that a float on one side pushes in by `.0` pixels, down the whole
/// block.
struct Pushed(f32, FloatSide);

impl Exclusions for Pushed {
    fn band(&self, _line: usize, _block: BlockExtents) -> InlineExtents {
        match self.1 {
            FloatSide::Left => InlineExtents {
                left: self.0,
                right: f32::INFINITY,
            },
            FloatSide::Right => InlineExtents {
                left: f32::NEG_INFINITY,
                right: 200.0 - self.0,
            },
        }
    }
    fn below(&self, _top: f32) -> Option<f32> {
        None
    }
    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        NoExclusions.place(float)
    }
    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(0)
    }
    fn rewind(&mut self, _to: ExclusionsCheckpoint) {}
}

// The records --------------------------------------------------------------

/// The records keep their sizes: a line 76 bytes, a reshaped edge 12, what
/// hangs 16, a band 8.
#[test]
fn records_keep_their_sizes() {
    assert_eq!(size_of::<LineRecord>(), 76);
    assert_eq!(size_of::<PlacementFacts>(), 8);
    assert_eq!(size_of::<EdgeShape>(), 12);
    assert_eq!(size_of::<LineHang>(), 16);
    assert_eq!(size_of::<LineBand>(), 8);
}

// What must not panic --------------------------------------------------------

/// Any area gives valid lines.
///
/// The widths are zero, negative, not a number, infinite and narrower than
/// a cluster, from any block start. The text breaks everywhere, reshapes,
/// may break anywhere, preserves its spaces, or has none at all. A layout
/// never built makes no lines.
#[test]
fn any_area_gives_valid_lines() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(layout.lines().len(), 0);
    let mut anywhere = sized(&LATIN, 20.0);
    anywhere.text.overflow_wrap = OverflowWrap::Anywhere;
    let mut pre = ahem(20.0);
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let arabic: String = [BEH, TEH, ' ', SEEN, MEEM, BEH].iter().collect();
    let cases: [(ComputedStyle<'_>, &str); 6] = [
        (ahem(20.0), "XX XX XX"),
        (break_all(&sized(&LATIN, 20.0)), "office AVAVA waffle"),
        (anywhere, "officially AVAVAVAV"),
        (pre, "  XX   XX  \t "),
        (break_all(&sized(&ARABIC, 20.0)), &arabic),
        (ahem(20.0), ""),
    ];
    let widths = [
        0.0,
        -10.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        f32::MIN_POSITIVE,
        0.001,
        3.0,
    ];
    for (style, text) in &cases {
        fixture.text(&mut layout, style, text);
        for width in widths {
            for block_start in [0.0, f32::NAN, f32::INFINITY, -1e30] {
                let area = Area {
                    block_start,
                    ..Area::new(width)
                };
                fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
            }
            // Ends the wrong way round leave no room.
            let area = Area {
                inline: InlineExtents {
                    left: 50.0,
                    right: -50.0,
                },
                block_start: 0.0,
                block_end: None,
                room_above: 0.0,
            };
            fixture.lay_out_with(&mut layout, area, &mut NoExclusions);
        }
    }
}

// Floats, CJK, the first line, heights, ruby and wrapping, in files of
// their own.

mod cjk;
mod first_line;
mod fitting;
mod floats;
mod hanging;
mod heights;
mod overflow;
mod relayout;
mod reshape;
mod ruby;
#[cfg(debug_assertions)]
mod ruby_costs;
mod seeks;
mod spacing;
mod tabs;
mod wrap;
