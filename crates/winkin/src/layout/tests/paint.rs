//! Paint tests. They pin:
//! - a line's paint order, item by item, as Chrome 155 paints it;
//! - decorations of culled boxes, as kept ones draw them;
//! - decorations, one bar a run;
//! - the font's own decoration lines a bar hands out for `from-font`.

use super::*;
use crate::paint::FontDecorationLine;

/// Returns what `layout`'s line `at` paints, one string per operation,
/// with `decorates` saying which lines each key draws.
fn painted(layout: &Layout, at: usize, decorates: &dyn Fn(NodeKey) -> Decorates) -> Vec<String> {
    let Some(line) = layout.line(at) else {
        return Vec::new();
    };
    line.paints(decorates)
        .map(|item| match item {
            Paint::Background(background) => {
                format!("background {:?}", ends(background.inline()))
            }
            Paint::Box(piece) => format!("box {} {:?}", piece.key().0, ends(piece.inline())),
            Paint::DecorationBeforeText(bar) => {
                format!("under {} {:?}", bar.key().0, ends(bar.inline()))
            }
            Paint::Annotation(run) => format!("annotation at {}", run.inline().left),
            Paint::Text(run) => format!("text at {}", run.inline().left),
            Paint::Emphasis(mark) => format!("mark at {}", mark.x),
            Paint::Atomic(atomic) => format!("atomic at {}", atomic.inline().left),
            Paint::Generated(run) => format!("generated at {}", run.inline().left),
            Paint::DecorationAfterText(bar) => {
                format!("through {} {:?}", bar.key().0, ends(bar.inline()))
            }
        })
        .collect()
}

/// Returns that every key draws every line.
fn everything(_: NodeKey) -> Decorates {
    Decorates::Both
}

/// A line paints item by item, as Chrome paints a line's fragment items:
/// each text run with its underlines and overlines before it, the block's
/// first and then each box's outermost first, and its lines through after
/// it; a box's background where the box starts. So a later box's background
/// covers the text before it and that text's decorations. Chrome 155, 40px
/// Ahem: in `AA<span style="margin-left:-50px;background:red">B</span>`
/// the red covers the second A, its underline, its overline and its line
/// through; an outer box's 6px underline lies under an inner box's 2px one.
#[test]
fn a_line_paints_its_items_one_by_one_in_chromes_order() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let span = ComputedStyle {
        paints: true,
        decorates: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX");
        b.open_box(NodeKey(2), &span, None);
        b.text(NodeKey(3), "YY");
        b.close_box();
        b.text(NodeKey(4), "ZZ");
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        painted(&layout, 0, &everything),
        [
            "background (0.0, 120.0)",
            "under 0 (0.0, 40.0)",
            "text at 0",
            "through 0 (0.0, 40.0)",
            "box 2 (40.0, 80.0)",
            "under 0 (40.0, 80.0)",
            "under 2 (40.0, 80.0)",
            "text at 40",
            "through 0 (40.0, 80.0)",
            "through 2 (40.0, 80.0)",
            "under 0 (80.0, 120.0)",
            "text at 80",
            "through 0 (80.0, 120.0)",
        ]
    );
    // Only the keys `decorates` marks are decorated.
    let underlined = |key: NodeKey| {
        if key == NodeKey(2) {
            Decorates::BeforeText
        } else {
            Decorates::None
        }
    };
    assert_eq!(
        painted(&layout, 0, &underlined),
        [
            "background (0.0, 120.0)",
            "text at 0",
            "box 2 (40.0, 80.0)",
            "under 2 (40.0, 80.0)",
            "text at 40",
            "text at 80",
        ]
    );
    // A box that does not set `decorates` is not asked, kept or not.
    let quiet = ComputedStyle {
        paints: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &quiet, None);
        b.text(NodeKey(2), "XX");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let skip_block = |key: NodeKey| {
        if key == NodeKey(0) {
            Decorates::None
        } else {
            Decorates::Both
        }
    };
    assert_eq!(
        painted(&layout, 0, &skip_block),
        ["background (0.0, 40.0)", "box 1 (0.0, 40.0)", "text at 0"]
    );
}

/// An atomic inline paints where it stands among the items, so a later
/// box's background covers it and it covers the text before it. Chrome
/// 155: a span's background pulled back over an inline-block before it
/// covers it, also where the inline-block sits in a box of its own; an
/// inline-block pulled back over text covers the text.
#[test]
fn an_atomic_inline_paints_where_it_stands() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let span = ComputedStyle {
        paints: true,
        ..root
    };
    let size = BoxSize {
        inline: 20.0,
        block: 20.0,
        baseline: None,
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.atomic(NodeKey(1), &root, None, size);
        b.open_box(NodeKey(2), &span, None);
        b.text(NodeKey(3), "X");
        b.atomic(NodeKey(4), &root, None, size);
        b.close_box();
        b.text(NodeKey(5), "Y");
        b.open_box(NodeKey(6), &span, None);
        b.text(NodeKey(7), "Z");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        painted(&layout, 0, &|_| Decorates::None),
        [
            "background (0.0, 100.0)",
            "atomic at 0",
            "box 2 (20.0, 60.0)",
            "text at 20",
            "atomic at 40",
            "text at 60",
            "box 6 (80.0, 100.0)",
            "text at 80",
        ]
    );
}

/// A run's emphasis marks follow its text and come before its lines
/// through, as Chrome paints the marks with the text and the lines through
/// after both. Chrome 155: an 80px line through covers the run's own marks,
/// and a later span's background covers them.
#[test]
fn a_runs_marks_come_between_its_text_and_its_lines_through() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let marked = ComputedStyle {
        decorates: true,
        ..marked(20.0, EmphasisSkip::INITIAL)
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &marked, None);
        b.text(NodeKey(2), "XX");
        b.close_box();
        b.text(NodeKey(3), "Y");
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let box_only = |key: NodeKey| {
        if key == NodeKey(1) {
            Decorates::Both
        } else {
            Decorates::None
        }
    };
    assert_eq!(
        painted(&layout, 0, &box_only),
        [
            "background (0.0, 60.0)",
            "under 1 (0.0, 40.0)",
            "text at 0",
            "mark at 10",
            "mark at 30",
            "through 1 (0.0, 40.0)",
            "text at 40",
        ]
    );
}

/// A reordered line paints left to right, each box part before what it
/// holds, as Chrome paints its fragment items in visual order. Chrome 155,
/// in a left-to-right and a right-to-left block alike: of two spans
/// reordered right to left, the one first in the content, which lies to the
/// right, paints over the other where a negative margin pulls it across.
#[test]
fn a_reordered_line_paints_left_to_right() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let paints = ComputedStyle {
        paints: true,
        ..root
    };
    let overriding = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::BidiOverride,
        },
        paints: true,
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &paints, None);
        b.text(NodeKey(2), "A");
        b.open_box(NodeKey(3), &paints, None);
        b.text(NodeKey(4), "B");
        b.open_box(NodeKey(5), &overriding, None);
        b.text(NodeKey(6), "CD");
        b.close_box();
        b.close_box();
        b.open_box(NodeKey(7), &overriding, None);
        b.text(NodeKey(8), "EF");
        b.close_box();
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        painted(&layout, 0, &|_| Decorates::None),
        [
            "background (0.0, 120.0)",
            "box 1 (0.0, 120.0)",
            "text at 0",
            "box 3 (20.0, 40.0)",
            "text at 20",
            "box 7 (40.0, 80.0)",
            "text at 40",
            "box 3 (80.0, 120.0)",
            "box 5 (80.0, 120.0)",
            "text at 80",
        ]
    );
}

/// The first line paints in the same order as any other, its background
/// first. Chrome 155: a `::first-line` background lies under a span's, and
/// a `::first-line` underline under a later span's background.
#[test]
fn the_first_line_paints_in_the_same_order() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let first_line = sized(&AHEM_FAMILY, 20.0);
    let span = ComputedStyle {
        paints: true,
        ..root
    };
    let block = ComputedBlockStyle {
        first_line: Some(&first_line),
        ..ComputedBlockStyle::new(&root)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "X ");
        b.open_box(NodeKey(2), &span, None);
        b.text(NodeKey(3), "Y");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(20.0), &mut NoExclusions);
    assert_eq!(layout.lines().len(), 2);
    let block_only = |key: NodeKey| {
        if key == NodeKey(0) {
            Decorates::BeforeText
        } else {
            Decorates::None
        }
    };
    assert_eq!(
        painted(&layout, 0, &block_only),
        ["background (0.0, 20.0)", "under 0 (0.0, 20.0)", "text at 0"]
    );
    assert_eq!(
        painted(&layout, 1, &block_only),
        [
            "background (0.0, 20.0)",
            "box 2 (0.0, 20.0)",
            "under 0 (0.0, 20.0)",
            "text at 0",
        ]
    );
}

/// A box that only decorates its text keeps no fragment, as Chrome culls
/// one, and decorates it all the same: each line's bars are the ones it
/// draws where it keeps a fragment, every bar's run, baseline and font
/// alike -- across lines, around and inside boxes that keep theirs for
/// their edges or their font, at a hanging space, and in text reordered
/// right to left -- the culled boxes' painted after the kept ones'.
#[test]
fn a_culled_box_decorates_as_one_kept_would() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 10.0,
                right: 10.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let larger = sized(&AHEM_FAMILY, 30.0);
    let rtl = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::BidiOverride,
        },
        ..root
    };
    let decorates = |key: NodeKey| {
        if key.0.is_multiple_of(3) {
            Decorates::Both
        } else if key.0 % 3 == 1 {
            Decorates::BeforeText
        } else {
            Decorates::AfterText
        }
    };
    // Every line's bars, each with its box, by line and then by box and
    // where it starts.
    let bars = |layout: &Layout| -> Vec<(usize, bool, u64, String)> {
        let mut bars = Vec::new();
        for (at, line) in layout.lines().enumerate() {
            for paint in line.paints(decorates) {
                let (after, bar) = match paint {
                    Paint::DecorationBeforeText(bar) => (false, bar),
                    Paint::DecorationAfterText(bar) => (true, bar),
                    _ => continue,
                };
                bars.push((at, after, bar.key().0, format!("{bar:?}")));
            }
        }
        bars.sort();
        bars
    };
    let mut both = Vec::new();
    for paints in [true, false] {
        let span = |style: &ComputedStyle<'static>| ComputedStyle {
            paints,
            decorates: true,
            ..*style
        };
        let (plain, padded, larger, rtl) = (span(&root), span(&padded), span(&larger), span(&rtl));
        let mut found = Vec::new();
        for width in [60.0, 130.0, 400.0] {
            build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
                b.text(NodeKey(1), "XX ");
                b.open_box(NodeKey(3), &plain, None);
                b.text(NodeKey(4), "XX X ");
                b.open_box(NodeKey(6), &padded, None);
                b.text(NodeKey(7), "XX ");
                b.open_box(NodeKey(9), &plain, None);
                b.text(NodeKey(10), "X");
                b.close_box();
                b.close_box();
                b.open_box(NodeKey(12), &larger, None);
                b.text(NodeKey(13), "XXX ");
                b.close_box();
                b.text(NodeKey(14), "XX ");
                b.close_box();
                b.open_box(NodeKey(15), &rtl, None);
                b.open_box(NodeKey(18), &plain, None);
                b.text(NodeKey(19), "XX X");
                b.close_box();
                b.text(NodeKey(20), " X ");
                b.close_box();
                b.text(NodeKey(21), "XX");
            });
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            found.push(bars(&layout));
        }
        let kept: Vec<_> = (1..22)
            .map(|key| {
                layout
                    .box_fragments(NodeKey(key))
                    .any(|piece| !piece.is_culled())
            })
            .collect();
        if paints {
            assert!(kept.iter().filter(|kept| **kept).count() >= 6, "{kept:?}");
        } else {
            // Only the padded and the larger box keep theirs.
            let keeps: Vec<u64> = (1..22u64).filter(|&key| kept[key as usize - 1]).collect();
            assert_eq!(keeps, [6, 12]);
        }
        assert!(!found.iter().all(Vec::is_empty), "something is decorated");
        both.push(found);
    }
    assert_eq!(both.first(), both.last());
}

/// A decoration is drawn under its box's text only, one bar a run of it,
/// as Chrome decorates each text fragment: a box's edges and an object lie
/// between bars, and what hangs has none, at 40 px. Each bar knows its
/// baseline and its font, and Chrome's underline thickness and gap.
#[test]
fn a_decoration_is_cut_by_what_takes_room() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 40.0);
    let bars = |layout: &Layout, key: u64| -> Vec<InlineExtents> {
        layout
            .line(0)
            .expect("a line")
            .paints(|at| {
                if at == NodeKey(key) {
                    Decorates::BeforeText
                } else {
                    Decorates::None
                }
            })
            .filter_map(|item| match item {
                Paint::DecorationBeforeText(bar) => Some(bar.inline()),
                _ => None,
            })
            .collect()
    };
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 20.0,
                right: 20.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let painted = ComputedStyle {
        paints: true,
        ..root
    };
    for (style, want) in [
        (
            &padded,
            &[along(0.0, 80.0), along(100.0, 180.0), along(200.0, 280.0)][..],
        ),
        (
            &painted,
            &[along(0.0, 80.0), along(80.0, 160.0), along(160.0, 240.0)][..],
        ),
    ] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "XX");
            b.open_box(NodeKey(2), style, None);
            b.text(NodeKey(3), "XX");
            b.close_box();
            b.text(NodeKey(4), "XX");
        });
        layout.break_lines(&mut cx, Area::new(600.0), &mut NoExclusions);
        assert_eq!(bars(&layout, 0), want);
    }
    // The padded box's own bar is inside its own sides.
    let decorated = ComputedStyle {
        decorates: true,
        ..padded
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &decorated, None);
        b.text(NodeKey(2), "XXXX");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(600.0), &mut NoExclusions);
    assert_eq!(bars(&layout, 1), [along(20.0, 180.0)]);
    // An object cuts it, and what hangs is not decorated.
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX");
        b.atomic(
            NodeKey(2),
            &root,
            None,
            BoxSize {
                inline: 80.0,
                block: 20.0,
                baseline: None,
            },
        );
        b.text(NodeKey(3), "XX XX");
    });
    layout.break_lines(&mut cx, Area::new(270.0), &mut NoExclusions);
    assert_eq!(bars(&layout, 0), [along(0.0, 80.0), along(160.0, 240.0)]);
    let bar = layout
        .line(0)
        .expect("a line")
        .paints(|_| Decorates::BeforeText)
        .find_map(|item| match item {
            Paint::DecorationBeforeText(bar) => Some(bar),
            _ => None,
        })
        .expect("a bar");
    assert_eq!((bar.font_size(), bar.baseline()), (40.0, 32.0));
    assert_eq!((bar.ascent(), bar.descent()), (32.0, 8.0));
    assert_eq!((bar.underline_thickness(), bar.underline_gap()), (4.0, 2.0));
}

/// A bar hands out its font's own underline and line-through, for
/// `from-font`, beside `auto`'s: Test Latin's `post` underline is 0.1 em
/// down and 0.05 em thick, and its `OS/2` strikeout 0.25 em up and 0.05 em
/// thick. `from-font` takes the underline's, as Chrome's
/// `TextDecorationThickness::Resolve` and `TextDecorationOffset` do.
#[test]
fn a_decoration_gives_its_fonts_own_lines() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&LATIN, 40.0);
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "abc");
    });
    layout.break_lines(&mut cx, Area::new(600.0), &mut NoExclusions);
    let bar = layout
        .line(0)
        .expect("a line")
        .paints(|_| Decorates::AfterText)
        .find_map(|item| match item {
            Paint::DecorationAfterText(bar) => Some(bar),
            _ => None,
        })
        .expect("a bar");
    assert_eq!(
        bar.font_underline(),
        Some(FontDecorationLine {
            offset: 4.0,
            thickness: 2.0
        })
    );
    assert_eq!(
        bar.font_line_through(),
        Some(FontDecorationLine {
            offset: -10.0,
            thickness: 2.0
        })
    );
    assert_eq!((bar.underline_thickness(), bar.underline_gap()), (4.0, 2.0));
    assert_eq!(
        (
            bar.underline_thickness_from_font(),
            bar.underline_gap_from_font()
        ),
        (2.0, 4.0)
    );
}

/// A bar's font size is the computed one, which `font-size-adjust` leaves,
/// as Chrome's `ComputeDecorationThickness` reads `ComputedFontSize`
/// (`text-decoration-thickness-percent-001`). Ahem at 20 px with its
/// x-height held at 0.25 is set at 6.25 px, and its `auto` underline stays
/// 2 px thick.
#[test]
fn a_decoration_is_as_thick_as_the_computed_size_gives() {
    use crate::style::{AdjustMetric, FontSizeAdjust};
    let mut cx = context();
    let mut layout = Layout::new();
    let mut root = sized(&AHEM_FAMILY, 20.0);
    root.font.size_adjust = FontSizeAdjust::Hold {
        metric: AdjustMetric::ExHeight,
        value: 0.25,
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "X X");
    });
    layout.break_lines(&mut cx, Area::new(600.0), &mut NoExclusions);
    let line = layout.line(0).expect("a line");
    assert_eq!(line.metrics().width, 18.75, "set at the adjusted size");
    let bar = line
        .paints(|_| Decorates::BeforeText)
        .find_map(|item| match item {
            Paint::DecorationBeforeText(bar) => Some(bar),
            _ => None,
        })
        .expect("a bar");
    assert_eq!((bar.font_size(), bar.underline_thickness()), (20.0, 2.0));
}

/// A line's background spans the block's content area, not its line box, as
/// Chrome paints a `::first-line` background: on 60 px lines, Ahem at 20 px
/// is painted from 20 to 40, and at 40 px in the first line's font from 10 to
/// 50, as in `text-box-trim-first-line-pseudo-003`.
#[test]
fn a_lines_background_spans_the_blocks_content_area() {
    let mut cx = context();
    let mut layout = Layout::new();
    let lined = |size| ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(60.0),
            ..LineGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, size)
    };
    let (root, first_line) = (lined(20.0), lined(40.0));
    for (first, expected) in [(None, (20.0, 40.0)), (Some(&first_line), (10.0, 50.0))] {
        let block = ComputedBlockStyle {
            first_line: first,
            ..ComputedBlockStyle::new(&root)
        };
        build(&mut cx, &mut layout, &block, |b| b.text(NodeKey(1), "XX"));
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        let line = layout.line(0).expect("a line");
        let background = line
            .paints(|_| Decorates::None)
            .find_map(|item| match item {
                Paint::Background(background) => Some(background.block()),
                _ => None,
            });
        assert_eq!(background, Some(across(expected.0, expected.1)));
    }
}
