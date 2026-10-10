//! Fast path tests: each fast path held to the general path it stands in for.
//!
//! A fast path skips work where a fact its gate reads proves the work comes to
//! nothing. The same content is laid out with the fast paths open, and again
//! inside [`work::general_paths`], which closes them all on this thread. They
//! pin:
//! - each stage's tables, as far as its fast path writes them;
//! - the lines, their items and glyphs, and what they paint at several widths.
//!
//! The content is drawn at random. A seeded generator mixes plain Latin text
//! with what each gate looks for: Latin-1 letters and pictographs, marks, soft
//! hyphens, tabs, bidi and CJK text, boxes with edges and with none, decorated
//! and spaced spans, and block styles that set what the scans skip.

use alloc::borrow::Cow;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

use super::{AHEM_FAMILY, Fixture, StageCheck, TestFont, han, han_fallback};
use crate::data::Id;
use crate::paint::Decorates;
use crate::stages::analysis::ClusterId;
use crate::stages::lines::{Area, NoExclusions};
use crate::style::FirstLineVariant;
use crate::style::{
    ComputedStyle, FontFamilyName, FontGroup, HangEnd, HangingPunctuation, Hyphens, Language,
    LengthPercentage, LineBreak, OverflowWrap, TabSize, TextAlign, TextAutospace, TextGroup,
    TextIndent, TextWrapMode, VerticalAlign, WhiteSpaceCollapse, WordBreak,
};
use crate::unicode::is_default_ignorable;
use crate::{ComputedBlockStyle, Item, Layout, LayoutBuilder, NodeKey, work};

/// A seeded xorshift: the same cases every run.
pub(super) struct Rng(pub(super) u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub(super) fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// A Latin font covering ASCII, Latin-1's letters and signs and the
/// general punctuation's dashes and quotes, with ligatures and a kerning
/// pair, as text fonts are.
pub(super) fn latin_1() -> TestFont {
    let mut font = TestFont::new(
        "Test Latin-1",
        &[(0x20, 0x7E), (0xA0, 0xFF), (0x2010, 0x2027)],
    );
    font.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    font.kerning = vec![('A', 'V', -100), ('T', 'o', -80)];
    font
}

const LATIN_1: [FontFamilyName<'static>; 1] =
    [FontFamilyName::Named(Cow::Borrowed("Test Latin-1"))];

/// What text is drawn from: plain words most often, and now and then one of
/// each kind a gate looks for.
pub(super) const WORDS: &[&str] = &[
    "the", "office", "AVATAR", "Tomorrow", "of", "and", "affluent", "fly", "a", "line",
];
pub(super) const RARE: &[&str] = &[
    "café",
    "Straße",
    "naïve",
    "©2026",
    "£5",
    "“quoted”",
    "it’s",
    "a—b",
    "x\u{a0}y",
    "hy\u{ad}phen",
    "e\u{301}",
    "\u{1f600}",
    "\u{1f44d}\u{1f3fd}",
    "日本語",
    "漢字かな",
    "مرحبا",
    "שלום",
    "a\u{200d}b",
    "a\u{200c}b",
    "x\u{fe0f}",
    "1/2",
    "\t",
    "  ",
    "(a)",
    "e.g.",
    "Ω",
    "ﬁ",
    "don’t",
    "über—naïve",
    "«Grüße»",
    "½·¾",
    "®\u{fe0f}",
    "a\u{200d}©",
    "x-1",
    "ab-12",
    "[é]",
    "Ǆemal",
    "‘it…’",
    "a\u{2009}b",
    "$5.00",
    "Ελλάδα",
];

/// What a word drawn from [`WORDS`] may have spliced into it, so that the
/// Latin-1 fast path is left and taken again inside a word: a
/// character past Latin-1 -- punctuation, a mark, a joiner, a variation
/// selector, a letter, CJK, RTL -- or one the fast path passes to the general
/// walk.
const SPLICED: &[char] = &[
    '’', '—', '…', '\u{301}', '\u{308}', '\u{200d}', '\u{200c}', '\u{fe0f}', 'ā', 'ŀ', '日', 'か',
    'ש', 'م', '\u{2009}', '\u{a0}', '\u{ad}', 'é', '©', '\t', '\u{200b}', '\u{2060}', '\u{85}',
    '(', ')', '-', '·',
];

/// One case's content: its words, with spans opened and closed among them
/// and forced breaks, built into `b`.
fn content(rng: &mut Rng, b: &mut LayoutBuilder<'_>, spans: &[ComputedStyle<'_>]) {
    let mut key = 1u64;
    let mut open = 0usize;
    let words = 3 + rng.below(40);
    for _ in 0..words {
        let mut text = String::new();
        if rng.chance(25) {
            text.push_str(RARE[rng.below(RARE.len())]);
        } else {
            let word = WORDS[rng.below(WORDS.len())];
            text.push_str(word);
            if rng.chance(20) {
                let at = rng.below(word.len() + 1);
                text.insert(at, SPLICED[rng.below(SPLICED.len())]);
            }
        }
        text.push(' ');
        if rng.chance(15)
            && open < 3
            && let Some(style) = spans.get(rng.below(spans.len()))
        {
            b.open_box(NodeKey(key), style, None);
            key += 1;
            open += 1;
        }
        b.text(NodeKey(key), &text);
        key += 1;
        if open > 0 && rng.chance(30) {
            b.close_box();
            open -= 1;
        }
        if rng.chance(3) {
            b.line_break(NodeKey(key));
            key += 1;
        }
    }
    for _ in 0..open {
        b.close_box();
    }
}

/// Everything the stages' fast paths write and a reader reads of `layout`,
/// laid out at `widths` in turn, as text to compare.
fn snapshot(fixture: &mut Fixture, layout: &mut Layout, widths: &[f32]) -> String {
    let mut out = String::new();
    // The analysis: every cluster's end, attributes and stop, each item's first
    // cluster, the paragraphs, the levels, the runs and the flags, as the
    // pass writes them.
    let analysis = layout.analysis();
    let ends = analysis.clusters.end_id();
    for at in 0..ends.get() {
        let (id, next) = (ClusterId::new(at), ClusterId::new(at + 1));
        let clusters = &analysis.clusters;
        let (end, attrs) = (clusters.end(id), clusters.attrs(id));
        let stop = clusters.first_opportunity(id..next);
        let _ = writeln!(out, "cluster {end:?} {attrs:?} {stop:?}");
    }
    for (id, _) in layout.content().items.iter() {
        let _ = writeln!(out, "item {:?}", analysis.item_clusters.range(id));
    }
    for (id, paragraph) in analysis.paragraphs.iter() {
        let clusters = analysis.paragraphs.clusters(id);
        let _ = writeln!(out, "paragraph {paragraph:?} {clusters:?}");
    }
    let _ = writeln!(out, "runs {:?}", analysis.runs.as_slice());
    let _ = writeln!(out, "analysis {:?}", analysis.flags);
    // The measurements: the prefix, each paragraph's flags, the line-edge costs
    // and each item's extent, as the scan writes them.
    let measured = layout.measured();
    let clusters = layout.analysis().clusters.end_id();
    for text in [
        Some(measured.text(FirstLineVariant::Standard)),
        measured.first_line(),
    ]
    .into_iter()
    .flatten()
    {
        let prefix: Vec<_> = (0..=clusters.get())
            .map(|at| text.prefix.get(ClusterId::new(at)))
            .collect();
        let _ = writeln!(out, "prefix {prefix:?}");
        for (id, _) in layout.analysis().paragraphs.iter() {
            let _ = writeln!(out, "paragraph {:?}", text.paragraph(id));
        }
        let _ = writeln!(out, "costs {:?}", text.edge_costs().as_slice());
        for (id, _) in layout.content().items.iter() {
            let _ = writeln!(out, "extent {:?}", text.extents.get(id));
        }
    }
    // The fonts: the font runs, and each text's primary font's size and
    // metrics.
    let fonts = layout.fonts();
    for variant in [FirstLineVariant::Standard, FirstLineVariant::FirstLine] {
        let _ = writeln!(out, "fonts {:?}", fonts.runs(variant).as_slice());
    }
    for id in layout.content().facts.text_ids() {
        let primary = layout.primary(id).map(|used| (used.size, used.metrics));
        let _ = writeln!(out, "primary {primary:?}");
    }
    let _ = writeln!(out, "sizes {:?}", layout.intrinsic_sizes());
    let decorates = |key: NodeKey| {
        if key == NodeKey(0) {
            Decorates::None
        } else {
            Decorates::Both
        }
    };
    for &width in widths {
        layout.break_lines(&mut fixture.cx, Area::new(width), &mut NoExclusions);
        let (metrics, below) = (layout.metrics(), layout.room_below());
        let _ = writeln!(out, "width {width}: {metrics:?} below {below}");
        for line in layout.lines() {
            let _ = writeln!(out, "line {:?} {:?}", line.text_range(), line.metrics());
            for item in line.items() {
                let _ = match item {
                    Item::Text(run) | Item::Generated(run) => {
                        let glyphs: Vec<_> = run.glyphs().collect();
                        writeln!(out, "  run {:?} {glyphs:?}", run.inline())
                    }
                    Item::Box(piece) => {
                        writeln!(out, "  box {:?} {:?}", piece.key(), piece.inline())
                    }
                    Item::Atomic(atomic) => writeln!(out, "  atomic {:?}", atomic.inline()),
                };
            }
            for paint in line.paints(decorates) {
                let _ = writeln!(out, "  paint {paint:?}");
            }
        }
    }
    out
}

/// The block styles the cases are set in, each with something the scans
/// look for, or nothing.
/// Now and then its first line restyled, `first`, as `::first-line` does.
pub(super) fn blocks<'a>(
    root: &'a ComputedStyle<'a>,
    first: &'a ComputedStyle<'a>,
    rng: &mut Rng,
) -> ComputedBlockStyle<'a> {
    let mut block = ComputedBlockStyle::new(root);
    if rng.chance(20) {
        block.first_line = Some(first);
    }
    match rng.below(4) {
        0 => block.text_align = TextAlign::Justify,
        1 => {
            block.text_indent = TextIndent {
                amount: LengthPercentage {
                    px: 12.0,
                    fraction: 0.0,
                },
                hanging: rng.chance(50),
                each_line: rng.chance(50),
            }
        }
        _ => {}
    }
    block
}

/// The root styles: plain, and each with one thing a gate reads.
pub(super) fn roots() -> Vec<ComputedStyle<'static>> {
    let plain = ComputedStyle {
        font: FontGroup {
            families: &LATIN_1,
            size: 16.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let mut preserved = plain;
    preserved.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    preserved.text.tab_size = TabSize::Spaces(4.0);
    let mut spaced = plain;
    spaced.text.letter_spacing = 1.5;
    let mut hanging = plain;
    hanging.text.hanging_punctuation = HangingPunctuation {
        first: true,
        last: true,
        end: HangEnd::Allow,
    };
    let mut padded = plain;
    padded.line.padding = 3.0;
    let mut autospaced = plain;
    autospaced.text.autospace = TextAutospace::NORMAL;
    // What the Latin-1 fast path reads of an item's style.
    let mut break_spaces = plain;
    break_spaces.text.white_space_collapse = WhiteSpaceCollapse::BreakSpaces;
    let mut anywhere = plain;
    anywhere.text.overflow_wrap = OverflowWrap::Anywhere;
    anywhere.text.line_break = LineBreak::Anywhere;
    let mut break_all = plain;
    break_all.text.word_break = WordBreak::BreakAll;
    vec![
        plain,
        plain,
        plain,
        preserved,
        spaced,
        hanging,
        padded,
        autospaced,
        break_spaces,
        anywhere,
        break_all,
    ]
}

/// The spans' styles: bare, decorated, spaced, with edges, raised, larger,
/// in Ahem.
pub(super) fn spans(root: &ComputedStyle<'static>) -> Vec<ComputedStyle<'static>> {
    let bare = *root;
    let painted = ComputedStyle {
        paints: true,
        ..*root
    };
    let mut spaced = *root;
    spaced.text.word_spacing = LengthPercentage {
        px: 2.0,
        fraction: 0.0,
    };
    let mut edged = *root;
    edged.edges.padding.left = LengthPercentage::from_px(4.0);
    edged.edges.padding.right = LengthPercentage::from_px(2.0);
    let mut raised = *root;
    raised.line.vertical_align = VerticalAlign::Super;
    let mut larger = *root;
    larger.font.size = 20.0;
    let ahem = ComputedStyle {
        font: FontGroup {
            families: &AHEM_FAMILY,
            ..root.font
        },
        ..*root
    };
    // What the Latin-1 fast path reads of an item's style: its wrapping, its
    // language and the options it breaks under.
    let mut nowrap = *root;
    nowrap.text.wrap_mode = TextWrapMode::NoWrap;
    let japanese = ComputedStyle {
        text: TextGroup {
            language: Language::parse("ja").ok(),
            ..root.text
        },
        ..*root
    };
    let mut strict = *root;
    strict.text.line_break = LineBreak::Strict;
    strict.text.hyphens = Hyphens::None;
    vec![
        bare, bare, painted, painted, spaced, edged, raised, larger, ahem, nowrap, japanese, strict,
    ]
}

/// No ASCII character is one harfrust hides: the font walk takes an ASCII
/// cluster as covered where its font maps it, and as not where it does not,
/// with no default-ignorable to pass it over (`Walker::kept_end`).
#[test]
fn no_ascii_character_is_default_ignorable() {
    for byte in 0..=0x7F_u8 {
        assert!(!is_default_ignorable(char::from(byte)), "{byte:#x}");
    }
}

/// Lays out `cases` random contents both ways and compares them.
#[test]
fn every_fast_path_comes_out_as_the_general_path() {
    let mut fixture = Fixture::new(
        &[latin_1(), han()],
        han_fallback("Test Han"),
        StageCheck::Placed(|_| {}),
    );
    let widths = [23.0, 61.5, 140.0, 333.0, 1.0e6];
    let roots = roots();
    for case in 0..400u64 {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ (case + 1).wrapping_mul(0x2545_f491_4f6c_dd1d));
        let root = roots[rng.below(roots.len())];
        let spans = spans(&root);
        let seed = rng.next();
        let first = ComputedStyle {
            font: FontGroup {
                size: 22.0,
                ..root.font
            },
            ..root
        };
        let block = blocks(&root, &first, &mut rng);
        let build = |fixture: &mut Fixture| {
            let mut layout = Layout::new();
            let mut rng = Rng(seed | 1);
            fixture.build(&mut layout, &block, |b| content(&mut rng, b, &spans));
            snapshot(fixture, &mut layout, &widths)
        };
        let fast = build(&mut fixture);
        let general = work::general_paths(|| build(&mut fixture));
        assert_eq!(fast, general, "case {case}");
    }
}
