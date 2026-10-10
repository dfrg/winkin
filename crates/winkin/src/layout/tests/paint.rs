//! Paint tests. They pin:
//! - a line's paint phases in Chrome's order;
//! - decorations of culled boxes, as kept ones draw them;
//! - decorations cut by what takes room;
//! - the font's own decoration lines a bar hands out for `from-font`.

use super::*;
use crate::paint::FontDecorationLine;

/// A line paints in Chrome's phases: its
/// background, its boxes outermost first in tree order whatever the reordering,
/// the decorations under the text, the block's first, then the text, then the
/// lines through it. At 20 px, `XX<span>YY</span>` has its
/// background 0 to 80, and a box inside another after it.
#[test]
fn a_line_paints_in_chromes_order() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let paints = ComputedStyle {
        paints: true,
        ..root
    };
    let describe = |layout: &Layout, decorates: &dyn Fn(NodeKey) -> Decorates| {
        let line = layout.line(0).expect("a line");
        line.paints(decorates)
            .map(|item| match item {
                Paint::Background(background) => {
                    alloc::format!("background {:?}", ends(background.inline()))
                }
                Paint::Box(piece) => {
                    alloc::format!("box {} {:?}", piece.key().0, ends(piece.inline()))
                }
                Paint::DecorationBeforeText(bar) => {
                    alloc::format!("under {} {:?}", bar.key().0, ends(bar.inline()))
                }
                Paint::Annotation(run) => alloc::format!("annotation at {}", run.inline().left),
                Paint::Text(run) => alloc::format!("text at {}", run.inline().left),
                Paint::Emphasis(mark) => alloc::format!("mark at {}", mark.x),
                Paint::Atomic(atomic) => alloc::format!("atomic at {}", atomic.inline().left),
                Paint::Generated(run) => alloc::format!("generated at {}", run.inline().left),
                Paint::DecorationAfterText(bar) => {
                    alloc::format!("through {} {:?}", bar.key().0, ends(bar.inline()))
                }
            })
            .collect::<Vec<_>>()
    };
    let everything = |_: NodeKey| Decorates::Both;
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XX");
        b.open_box(NodeKey(2), &paints, None);
        b.text(NodeKey(3), "YY");
        b.close_box();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        describe(&layout, &everything),
        [
            "background (0.0, 80.0)",
            "box 2 (40.0, 80.0)",
            "under 0 (0.0, 80.0)",
            "under 2 (40.0, 80.0)",
            "text at 0",
            "text at 40",
            "through 0 (0.0, 80.0)",
            "through 2 (40.0, 80.0)",
        ]
    );
    // Only what the caller says decorates is decorated.
    let underlined = |key: NodeKey| {
        if key == NodeKey(2) {
            Decorates::BeforeText
        } else {
            Decorates::None
        }
    };
    assert_eq!(
        describe(&layout, &underlined),
        [
            "background (0.0, 80.0)",
            "box 2 (40.0, 80.0)",
            "under 2 (40.0, 80.0)",
            "text at 0",
            "text at 40",
        ]
    );
    // Outermost first, and a box reordering parts painted whole before the
    // next, in tree order.
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
    let boxes: Vec<String> = describe(&layout, &|_| Decorates::None)
        .into_iter()
        .filter(|item| item.starts_with("box"))
        .collect();
    assert_eq!(
        boxes,
        [
            "box 1 (0.0, 120.0)",
            "box 3 (20.0, 40.0)",
            "box 3 (80.0, 120.0)",
            "box 5 (80.0, 120.0)",
            "box 7 (40.0, 80.0)",
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

/// A decoration is drawn under its box's text only, one bar a run of it:
/// cut by an edge that takes room, an object, and what hangs, and not by a
/// box that only paints, at 40 px. Each bar knows its baseline and its
/// font, and Chrome's underline thickness and gap.
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
        (&painted, &[along(0.0, 240.0)][..]),
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
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_box(NodeKey(1), &padded, None);
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
