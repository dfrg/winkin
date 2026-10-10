//! Fitting tests. They pin:
//! - Ahem fitting exactly on Chrome's 1/64 grid, with its one 1/64 of room;
//! - widths rounded up from the paragraph's start;
//! - the search agreeing with a plain greedy breaker over many widths;
//! - a line's cost on a long paragraph.

use super::*;
use crate::style::FirstLineVariant;

/// In Ahem every glyph is an em, so a line holds what arithmetic says.
///
/// At 20 px, `XX XX` fills 100 px exactly, the space after it hanging past
/// the end. One 1/64 less still fits, as Chrome's `AvailableWidthToFit`
/// allows; two less does not.
#[test]
fn ahem_fits_what_arithmetic_says_with_one_64th_to_spare() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.text(&mut layout, &ahem(20.0), "XX XX XX");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["XX XX ", "XX"]);
    assert_eq!(widths(&layout), [100.0, 40.0]);
    let metrics = layout.line(0).expect("a line").metrics();
    assert_eq!((metrics.hang, metrics.band.size()), (20.0, 100.0));
    fixture.lay_out(&mut layout, 100.0 - 1.0 / 64.0);
    assert_eq!(texts(&layout), ["XX XX ", "XX"], "within the epsilon");
    fixture.lay_out(&mut layout, 100.0 - 2.0 / 64.0);
    assert_eq!(texts(&layout), ["XX ", "XX ", "XX"], "past it");
    for (width, count) in [
        (40.0, 3),
        (59.0, 3),
        (60.0, 3),
        (99.0, 3),
        (140.0, 2),
        (159.0, 2),
        (160.0, 1),
        (1000.0, 1),
    ] {
        fixture.lay_out(&mut layout, width);
        assert_eq!(layout.lines().len(), count, "at {width}");
    }
}

/// A width is read from its paragraph's start, each end rounded up to
/// 1/64, as Chrome reads its per-item positions.
///
/// `i` is 1.6 px at 16 px in the narrow font, 102.4 of 1/64. So a
/// paragraph of one `i` is 103/64 wide wherever the paragraph before it
/// ended. Measured from the text's start, the second would be 102/64.
#[test]
fn a_width_is_rounded_up_from_its_paragraphs_start() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = sized(&NARROW, 16.0);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "i");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "i");
    });
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(texts(&layout), ["i\n", "i"]);
    for line in records(&layout) {
        assert_eq!(line.width, LayoutUnit::from_px(103.0 / 64.0));
    }
}

/// The searched breaker makes the same lines as a plain greedy breaker.
///
/// Each line ends at the last opportunity where it fits its band and its
/// 1/64, hanging content left out, or else at the first. The text is Latin
/// with ligatures and a kern, in boxes with edges, with preserved spaces
/// and forced breaks, at a thousand widths.
#[test]
fn the_search_agrees_with_a_straightforward_breaker() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let root = sized(&LATIN, 16.0);
    let boxed = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(3.0),
            border: Sides::all(1.5),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let cloned = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 2.0,
                right: 5.25,
                ..Sides::ZERO
            }
            .into(),
            decoration_break: BoxDecorationBreak::Clone,
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let mut pre = sized(&NARROW, 13.0);
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let words = [
        "office",
        "AVAIL",
        "flat",
        "fit",
        "a",
        "quick",
        "brown",
        "fox",
        "jumps",
        "over",
        "the",
        "lazy",
        "dog",
        "WAVE",
        "ffi",
        "officially",
    ];
    fixture.build(&mut layout, &ComputedBlockStyle::new(&root), |b| {
        let mut key = 0;
        let mut next = || {
            key += 1;
            NodeKey(key)
        };
        for n in 0..240 {
            let word = words[n * 7 % words.len()];
            match n % 23 {
                5 => {
                    b.open_box(next(), &boxed, None);
                    b.text(next(), word);
                    b.text(next(), " ");
                    b.close_box();
                }
                11 => {
                    b.open_box(next(), &pre, None);
                    b.text(next(), word);
                    b.text(next(), " 	 ");
                    b.close_box();
                }
                17 => {
                    b.text(next(), word);
                    b.line_break(next());
                }
                // A cloned box across several words: every line it reaches
                // pays both its edges.
                20 => {
                    b.open_box(next(), &cloned, None);
                    b.text(next(), word);
                    b.text(next(), " quick brown fox ");
                    b.close_box();
                }
                _ => {
                    b.text(next(), word);
                    b.text(next(), " ");
                }
            }
        }
    });
    let advances = layout.shaped_advances(&mut fixture.cx);
    let analysis = layout.analysis();
    // The paragraphs with tabs are walked, and the rest are searched.
    let walked = analysis
        .paragraphs
        .iter()
        .filter(|(_, p)| p.flags.contains(ParagraphFlags::HAS_TABS))
        .count();
    assert!(walked > 0 && walked < analysis.paragraphs.len());
    // Every tab is in the preserved box. The stops count in the block's
    // space.
    let block = layout
        .content()
        .nodes
        .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
    let primary = layout.primary(block).expect("a font");
    let space = InlineLayoutUnit::from_text(primary.metrics.space);
    assert!(space > InlineLayoutUnit::ZERO);
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let (mut soft, mut overflowing) = (0, 0);
    for _ in 0..1000 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let width = (seed % 60_000) as f32 / 100.0;
        fixture.lay_out(&mut layout, width);
        let found: Vec<Range<ClusterId>> =
            records(&layout).iter().map(LineRecord::clusters).collect();
        assert_eq!(
            found,
            reference(&layout, &advances, space, width),
            "at {width}"
        );
        soft += end_kinds(&layout)
            .iter()
            .filter(|&&kind| kind == EndKind::Soft)
            .count();
        for line in records(&layout) {
            overflowing += usize::from(line.flags.contains(LineFlags::OVERFLOWS));
        }
    }
    assert!(
        soft > 10_000 && overflowing > 100,
        "{soft} soft, {overflowing} over"
    );
}

/// Returns the lines a plain greedy breaker makes of `layout` at `width`.
///
/// It tries every opportunity from the paragraph's end back. A line is
/// measured from its start by the clusters' shaped `advances`, rounded up
/// from the paragraph's start, with what hangs taken off.
/// - A tab reaches from where it lands to the next multiple of eight
///   `space`s from the line's start. Where that is nearer than half a
///   space, it reaches the one after. It rounds up to 1/64, as Chrome sizes
///   a tab of the initial `tab-size`.
/// - A line starting or ending inside a cloned box pays the line-edge costs
///   there.
///
/// It handles text broken only where safe, with no `overflow-wrap` and no
/// box edge before a tab.
fn reference(
    layout: &Layout,
    advances: &[InlineLayoutUnit],
    space: InlineLayoutUnit,
    width: f32,
) -> Vec<Range<ClusterId>> {
    let analysis = layout.analysis();
    let clusters = &analysis.clusters;
    let measured = layout.measured().text(FirstLineVariant::Standard);
    let room = InlineLayoutUnit::from_layout(
        LayoutUnit::from_px(width).max(LayoutUnit::ZERO) + LayoutUnit::EPSILON,
    );
    let attrs = |c: usize| clusters.attrs(at(c)).expect("a cluster");
    let interval = space.times(8.0);
    let tab = |position: InlineLayoutUnit| {
        let mut distance = interval - position % interval;
        if distance < space.half() {
            distance += interval;
        }
        InlineLayoutUnit::from_layout(distance.to_layout())
    };
    let mut lines = Vec::new();
    for (id, _) in analysis.paragraphs.iter() {
        let range = analysis.paragraphs.clusters(id);
        let (first, end) = (range.start.get(), range.end.get());
        let origin = measured.prefix.get(range.start);
        let fit = |b: usize, tail: InlineLayoutUnit| {
            (measured.prefix.get(at(b)) - tail - origin).ceil_to_grid()
        };
        let mut start = first;
        while start < end {
            let opportunities: Vec<usize> = (start + 1..=end)
                .filter(|&e| e == end || attrs(e - 1).has(ClusterAttrs::BREAK_AFTER))
                .collect();
            // Each tab's width where it lands on a line from `start`.
            let mut tabs = vec![InlineLayoutUnit::ZERO; end - start];
            let mut before = InlineLayoutUnit::ZERO;
            for c in start..end {
                if attrs(c).class() == ClusterClass::Tab {
                    let position =
                        measured.prefix.get(at(c)) - measured.prefix.get(at(start)) + before;
                    tabs[c - start] = tab(position);
                    before += tabs[c - start];
                }
            }
            let fits = |e: usize| {
                let mut tail = InlineLayoutUnit::ZERO;
                let mut c = e;
                while c > start && attrs(c - 1).has(ClusterAttrs::HANGS) {
                    tail += advances[c - 1] + tabs[c - 1 - start];
                    c -= 1;
                }
                let reach = tabs[..e - start]
                    .iter()
                    .fold(InlineLayoutUnit::ZERO, |sum, &width| sum + width);
                let cost = |b: usize, starts: bool| {
                    measured
                        .edge_costs()
                        .iter()
                        .find(|edge| edge.key() == at(b))
                        .map_or(InlineLayoutUnit::ZERO, |edge| {
                            InlineLayoutUnit::from_layout(if starts {
                                edge.start
                            } else {
                                edge.end
                            })
                        })
                };
                fit(e, tail - reach) - fit(start, InlineLayoutUnit::ZERO)
                    + cost(start, true)
                    + cost(e, false)
                    <= room
            };
            // The last that fits, or the first. Spaces after it join the
            // line where nothing but spaces follows.
            let only_spaces = |e: usize| {
                (e..end).all(|c| {
                    matches!(
                        attrs(c).class(),
                        ClusterClass::Space
                            | ClusterClass::Tab
                            | ClusterClass::OtherSpace
                            | ClusterClass::Separator
                    )
                })
            };
            let chosen = match opportunities.iter().rev().copied().find(|&e| fits(e)) {
                Some(e) => e,
                None => {
                    let e = opportunities.first().copied().expect("the end at least");
                    if only_spaces(e) { end } else { e }
                }
            };
            lines.push(at(start)..at(chosen));
            start = chosen;
        }
    }
    lines
}

/// Fitting searches the prefix rather than walking it.
///
/// On a long paragraph a line costs a few comparisons for each doubling of
/// its length, not one a cluster.
#[test]
fn a_line_costs_a_logarithm_of_its_length() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let words = [
        "lorem",
        "ipsum",
        "dolor",
        "sit",
        "amet",
        "consectetur",
        "adipiscing",
    ];
    let text: String = (0..4000)
        .map(|n| words[n * 3 % words.len()])
        .collect::<Vec<_>>()
        .join(" ");
    fixture.text(&mut layout, &sized(&LATIN, 16.0), &text);
    for width in [200.0, 600.0, 2000.0, 8000.0] {
        fixture.lay_out(&mut layout, width);
        let lines = layout.lines().len();
        let clusters = layout.analysis().clusters.len();
        let per_line = layout.line_records().probes as f32 / lines as f32;
        let length = clusters as f32 / lines as f32;
        // A gallop and a bisection: two comparisons for each doubling of
        // the line's length, and a few over.
        let bound = 2.0 * libm_log2(length) + 4.0;
        assert!(
            per_line <= bound,
            "{per_line} comparisons a line of {length} clusters at {width}"
        );
    }
}

/// Returns `log2` without std.
fn libm_log2(x: f32) -> f32 {
    let mut n = 0.0;
    let mut x = x;
    while x >= 2.0 {
        x /= 2.0;
        n += 1.0;
    }
    n + (x - 1.0)
}
