//! Initial and first letter tests. They pin:
//! - an initial letter's size, place and sunk lines, as Chrome sets them;
//! - a `::first-letter` as a box of its own.

use super::*;

/// A host with no floats that keeps what it is asked to place.
#[derive(Default)]
struct Recording {
    asked: Vec<crate::FloatRequest>,
}

impl crate::Exclusions for Recording {
    fn band(&self, _line: usize, _block: crate::BlockExtents) -> InlineExtents {
        InlineExtents::EVERYTHING
    }

    fn below(&self, _top: f32) -> Option<f32> {
        None
    }

    fn place(&mut self, float: crate::FloatRequest) -> crate::PlacedFloat {
        self.asked.push(float);
        crate::PlacedFloat::default()
    }

    fn checkpoint(&self) -> crate::ExclusionsCheckpoint {
        crate::ExclusionsCheckpoint(0)
    }

    fn rewind(&mut self, _to: crate::ExclusionsCheckpoint) {}
}

/// An initial letter as Chrome 153 sets one, measured in Ahem at 10 px on
/// 10 px lines: three lines tall, at 35 px, its box fitted to
/// its `X`'s ink, the outline as designed, 35 px wide, its glyph drawn at
/// the line's start and the text after it at 35; its box from the first
/// line's top to 35 below, its baseline on the third line's, 28 down; the
/// line it opens no taller for it; and its exclusion its margin box, 35
/// wide. With 2 px of padding at its start and 5 of margin at its end, the
/// glyph is at 2 and the text after at 42. Raised, `3 1`, the first line's
/// text moves down two lines, and the letter stays. (Chrome on Windows
/// reads DirectWrite's hinted ink, a pixel past the glyph either side, so
/// its box is 37 wide, kerned a pixel left, and the text after it at 36;
/// the engine hints nothing, which is the host's to hand over.)
#[test]
fn an_initial_letter_is_set_as_chrome_sets_it() {
    let mut cx = context();
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let root = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(10.0),
            ..LineGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, 10.0)
    };
    let mut letter = root;
    letter.line.initial_letter = InitialLetter {
        size: 3.0,
        sink: 3,
        ..InitialLetter::NONE
    };
    let mut padded = letter;
    padded.edges = EdgesGroup {
        padding: Sides::<f32> {
            left: 2.0,
            ..Sides::ZERO
        }
        .into(),
        margin: Sides::<f32> {
            right: 5.0,
            ..Sides::ZERO
        }
        .into(),
        ..EdgesGroup::INITIAL
    };
    let mut raised = letter;
    raised.line.initial_letter.sink = 1;
    let block = ComputedBlockStyle::new(&root);
    // Where the letter's glyph and the text after it are on the first line,
    // its box along and across it, and the line's top, baseline and height.
    let mut set = |style: &ComputedStyle<'_>| {
        build(&mut cx, &mut layout, &block, |b| {
            b.set_first_letter(NodeKey(9), style, None);
            b.text(NodeKey(1), "X XX XX XX XX XX XX XX XX");
        });
        let mut host = Recording::default();
        layout.break_lines(&mut cx, Area::new(100.0), &mut host);
        let first = runs(&layout, 0);
        let glyph = first
            .first()
            .and_then(|run| run.glyphs().next())
            .map_or(f32::NAN, |glyph| glyph.x);
        let after = first.get(1).map_or(f32::NAN, |run| run.inline().left);
        let piece = layout
            .box_fragments(NodeKey(9))
            .next()
            .map(|piece| (piece.inline(), piece.block()));
        let line = layout.line(0).map(|line| {
            let metrics = line.metrics();
            (metrics.baseline - metrics.top, metrics.height())
        });
        let asked = host
            .asked
            .iter()
            .map(|asked| {
                (
                    asked.key,
                    asked.inline_size,
                    asked.block_size,
                    asked.block_start,
                )
            })
            .collect::<Vec<_>>();
        (glyph, after, piece, line, asked)
    };
    let (glyph, after, piece, line, asked) = set(&letter);
    assert_eq!((glyph, after), (0.0, 35.0));
    assert_eq!(piece, Some((along(0.0, 35.0), across(0.0, 35.0))));
    assert_eq!(line, Some((8.0, 10.0)));
    assert_eq!(asked, [(NodeKey(9), 35.0, 35.0, 0.0)]);
    let (glyph, after, piece, _, asked) = set(&padded);
    assert_eq!((glyph, after), (2.0, 42.0));
    assert_eq!(piece, Some((along(0.0, 37.0), across(0.0, 35.0))));
    assert_eq!(asked, [(NodeKey(9), 42.0, 35.0, 0.0)]);
    let (glyph, after, piece, line, _) = set(&raised);
    assert_eq!((glyph, after), (0.0, 35.0));
    assert_eq!(piece, Some((along(0.0, 35.0), across(0.0, 35.0))));
    assert_eq!(line, Some((28.0, 30.0)));
}

/// A `::first-letter` is an inline box of its own: its text answers to its
/// key, set in its style, and a query by its key finds its box; set larger,
/// it makes the line it opens as tall as it is, and no other.
#[test]
fn a_first_letter_is_a_box_its_text_answers_to() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let letter = ComputedStyle {
        paints: true,
        ..sized(&AHEM_FAMILY, 60.0)
    };
    let block = ComputedBlockStyle::new(&root);
    build(&mut cx, &mut layout, &block, |b| {
        b.set_first_letter(NodeKey(9), &letter, None);
        b.text(NodeKey(1), "\u{201C}Once upon a time");
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    let first = runs(&layout, 0);
    let keys: Vec<(NodeKey, f32)> = first.iter().map(|run| (run.key(), run.advance())).collect();
    assert_eq!(keys.first(), Some(&(NodeKey(9), 120.0)));
    assert_eq!(keys.get(1).map(|&(key, _)| key), Some(NodeKey(1)));
    let pieces: Vec<InlineExtents> = layout
        .box_fragments(NodeKey(9))
        .map(|piece| piece.inline())
        .collect();
    assert_eq!(pieces, [along(0.0, 120.0)]);
    let heights: Vec<f32> = layout.lines().map(|line| line.metrics().height()).collect();
    assert_eq!(heights.first(), Some(&60.0));
    assert!(
        heights.iter().skip(1).all(|&height| height == 20.0),
        "{heights:?}"
    );
}

/// An initial letter is sized by the lines it spans in every writing mode,
/// as in the `initial-letter-sunk-initial` tests: in Ahem at 20 px on 24 px
/// lines, `3 2` sets it at 80 px, its box 80 along the line and 80 across,
/// and asks for an exclusion of that size. In `vertical-*` modes, whose
/// lines sit on the central baseline, it starts 4 px before the first
/// line, as the tests' references put it.
#[test]
fn an_initial_letter_is_sized_alike_in_every_writing_mode() {
    use crate::style::WritingMode;
    let mut cx = context();
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let root = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(24.0),
            ..LineGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, 20.0)
    };
    let mut letter = root;
    letter.line.initial_letter = InitialLetter {
        size: 3.0,
        sink: 2,
        ..InitialLetter::NONE
    };
    for (mode, over) in [
        (WritingMode::HorizontalTb, 2.0),
        (WritingMode::VerticalRl, -4.0),
        (WritingMode::VerticalLr, -4.0),
        (WritingMode::SidewaysRl, 2.0),
    ] {
        let block = ComputedBlockStyle {
            writing_mode: mode,
            ..ComputedBlockStyle::new(&root)
        };
        build(&mut cx, &mut layout, &block, |b| {
            b.set_first_letter(NodeKey(9), &letter, None);
            b.text(NodeKey(1), "bc def ghi jkl mno");
        });
        let mut host = Recording::default();
        layout.break_lines(&mut cx, Area::new(230.0), &mut host);
        let piece = layout
            .box_fragments(NodeKey(9))
            .next()
            .map(|piece| (piece.inline(), piece.block()));
        assert_eq!(
            piece,
            Some((along(0.0, 80.0), across(over, over + 80.0))),
            "{mode:?}"
        );
        let asked = host
            .asked
            .first()
            .map(|asked| (asked.inline_size, asked.block_start));
        assert_eq!(asked, Some((80.0, over.min(0.0))), "{mode:?}");
    }
}

/// The paragraph style the `initial-letter` tests set: Ahem at 20 px on
/// 24 px lines, in `direction`.
fn paragraph(direction: Direction) -> ComputedStyle<'static> {
    ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(24.0),
            ..LineGroup::INITIAL
        },
        bidi: BidiGroup {
            direction,
            ..BidiGroup::INITIAL
        },
        ..sized(&AHEM_FAMILY, 20.0)
    }
}

/// `style` with a `3 drop` initial letter.
fn dropped(style: &ComputedStyle<'static>) -> ComputedStyle<'static> {
    let mut letter = *style;
    letter.line.initial_letter = InitialLetter {
        size: 3.0,
        sink: 3,
        ..InitialLetter::NONE
    };
    letter
}

/// Returns the line's text runs' extents along it, in visual order, with
/// their keys.
fn run_extents(layout: &Layout, line: usize) -> Vec<(NodeKey, (f32, f32))> {
    runs(layout, line)
        .iter()
        .map(|run| (run.key(), ends(run.inline())))
        .collect()
}

/// An initial letter stands at its line's start in right-to-left text, as
/// in `initial-letter-drop-initial-rtl`: Blink sets its box as one neutral
/// U+FFFC, so the left-to-right text after it does not carry it along. In
/// Ahem at 20 px on 24 px lines, the 80 px letter is at the line box's
/// right, `bc` beside it on its left, and its exclusion floats right.
#[test]
fn an_initial_letter_stands_at_the_start_of_right_to_left_text() {
    let mut cx = context();
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let root = paragraph(Direction::Rtl);
    let letter = dropped(&root);
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&root)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.set_first_letter(NodeKey(9), &letter, None);
        b.text(NodeKey(1), "Abc");
    });
    let mut host = Recording::default();
    layout.break_lines(&mut cx, Area::new(230.0), &mut host);
    let piece = layout
        .box_fragments(NodeKey(9))
        .next()
        .map(|piece| ends(piece.inline()));
    assert_eq!(piece, Some((40.0, 120.0)));
    assert_eq!(
        run_extents(&layout, 0),
        [(NodeKey(1), (0.0, 40.0)), (NodeKey(9), (40.0, 120.0))]
    );
    let sides: Vec<_> = host.asked.iter().map(|asked| asked.side).collect();
    assert_eq!(sides, [crate::FloatSide::Right]);
}

/// A tab kept before the letter is in its box, as in
/// `initial-letter-with-tab`: Blink counts leading spaces into the
/// `::first-letter`, and a tab's advance into the box. In Ahem at 20 px on
/// 24 px lines, the tab reaches the 160 px stop, the box is 240 px wide,
/// the text after it starts at 240, and the lines below make room for all
/// of it.
#[test]
fn a_tab_before_an_initial_letter_widens_its_box() {
    let mut cx = context();
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let mut root = paragraph(Direction::Ltr);
    root.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let letter = dropped(&root);
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.set_first_letter(NodeKey(9), &letter, None);
        b.text(NodeKey(1), "\tAbc\ndef");
    });
    let mut host = Recording::default();
    layout.break_lines(&mut cx, Area::new(800.0), &mut host);
    let piece = layout
        .box_fragments(NodeKey(9))
        .next()
        .map(|piece| ends(piece.inline()));
    assert_eq!(piece, Some((0.0, 240.0)));
    let after = run_extents(&layout, 0).last().copied();
    assert_eq!(after, Some((NodeKey(1), (240.0, 280.0))));
    let asked: Vec<_> = host.asked.iter().map(|asked| asked.inline_size).collect();
    assert_eq!(asked, [240.0]);
}

/// The first line's indent comes twice before an initial letter, as in
/// `initial-letter-indentation`: once before its box, and once inside it,
/// where Chrome's box takes its own first line's indent. With a 10 px
/// indent, in Ahem at 20 px on 24 px lines, the box is at 10 and 90 px
/// wide, its letter at 20, the text after it at 100, and the lines below
/// start at 100.
#[test]
fn an_indent_comes_before_an_initial_letter_and_inside_its_box() {
    use crate::style::{LengthPercentage, TextIndent};
    let mut cx = context();
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let root = paragraph(Direction::Ltr);
    let letter = dropped(&root);
    let block = ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage {
                px: 10.0,
                fraction: 0.0,
            },
            hanging: false,
            each_line: false,
        },
        ..ComputedBlockStyle::new(&root)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.set_first_letter(NodeKey(9), &letter, None);
        b.text(NodeKey(1), "Abc def");
    });
    let mut host = Recording::default();
    layout.break_lines(&mut cx, Area::new(230.0), &mut host);
    let piece = layout
        .box_fragments(NodeKey(9))
        .next()
        .map(|piece| ends(piece.inline()));
    assert_eq!(piece, Some((10.0, 100.0)));
    let first = run_extents(&layout, 0);
    assert_eq!(first.first(), Some(&(NodeKey(9), (20.0, 100.0))));
    assert_eq!(first.get(1), Some(&(NodeKey(1), (100.0, 220.0))));
    let asked: Vec<_> = host.asked.iter().map(|asked| asked.inline_size).collect();
    assert_eq!(asked, [100.0]);
}

/// The block's decoration skips the initial letter, which only its own
/// draws under, as in `initial-letter-layout-text-decoration-underline`:
/// Chrome's letter box is atomic, and the decorations around it do not
/// reach in. In Ahem at 20 px on 24 px lines, the block's underline on the
/// first line runs under `bc` alone, and the letter's under its 80 px.
#[test]
fn the_blocks_decoration_skips_an_initial_letter() {
    let mut cx = context();
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let root = paragraph(Direction::Ltr);
    let letter = dropped(&root);
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.set_first_letter(NodeKey(9), &letter, None);
        b.text(NodeKey(1), "Abc");
    });
    layout.break_lines(&mut cx, Area::new(230.0), &mut Recording::default());
    let bars: Vec<_> = layout
        .line(0)
        .expect("a line")
        .paints(|_| Decorates::BeforeText)
        .filter_map(|item| match item {
            Paint::DecorationBeforeText(bar) => Some((bar.key(), ends(bar.inline()))),
            _ => None,
        })
        .collect();
    assert_eq!(
        bars,
        [(NodeKey(0), (80.0, 120.0)), (NodeKey(9), (0.0, 80.0))]
    );
}

/// Ruby over the first line moves it down, and an initial letter it opens
/// with it, but the letter's own size adds nothing to the annotations'
/// room, as in `initial-letter-block-position-drop-over-ruby`. In Ahem at
/// 20 px on 24 px lines, with 10 px annotations: dropped, the line moves
/// down 8, its baseline to 26, and the letter and its exclusion with it, to
/// 10 and 90. Raised `3 1`, the line already stands 48 lower, which the
/// annotation fits within, so nothing moves.
#[test]
fn ruby_moves_an_initial_letter_with_its_line() {
    let mut cx = context();
    cx.set_config(Config::chrome_windows());
    let mut layout = Layout::new();
    let root = paragraph(Direction::Ltr);
    let small = sized(&AHEM_FAMILY, 10.0);
    let letter = dropped(&root);
    let mut raised = letter;
    raised.line.initial_letter.sink = 1;
    for (letter, baseline, over, asked) in [(letter, 26.0, 10.0, 90.0), (raised, 66.0, 2.0, 82.0)] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.set_first_letter(NodeKey(9), &letter, None);
            b.text(NodeKey(1), "Abc ");
            b.open_ruby(NodeKey(2), &root, None);
            b.text(NodeKey(3), "xyz");
            b.open_annotation(NodeKey(4), &small, None);
            b.text(NodeKey(5), "XYZ");
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(6), "\u{2028}def");
        });
        let mut host = Recording::default();
        layout.break_lines(&mut cx, Area::new(230.0), &mut host);
        let first = layout.line(0).map(|line| line.metrics().baseline);
        assert_eq!(first, Some(baseline));
        let piece = layout
            .box_fragments(NodeKey(9))
            .next()
            .map(|piece| piece.block());
        assert_eq!(piece, Some(across(over, over + 80.0)));
        let sizes: Vec<_> = host.asked.iter().map(|asked| asked.block_size).collect();
        assert_eq!(sizes, [asked]);
    }
}
