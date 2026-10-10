//! CJK line edge tests, against Chrome 153 in Yu Gothic and MS Gothic:
//! - `text-spacing-trim` trims a wrapped line's opening mark under
//!   `space-first` and `trim-start`;
//! - a closing mark that fits only trimmed ends the line trimmed;
//! - a pair a break parts gives its blank back;
//! - each line edge is reshaped once, and a line's start once however often
//!   the line is fitted;
//! - `text-autospace` room is given back where a line ends at a seam, as
//!   Chrome's `UnapplyAutoSpacing` does.
//!
//! In Test Punct at 20px every ideograph and full-width mark is 20px wide.
//! Its `halt` sets a mark in 10, an ASCII letter is 10, and its `水` makes an
//! autospace seam 2.5px.

use super::*;
use crate::config::PunctuationTrim;
use crate::style::FirstLineVariant;
use crate::style::{TextAlign, TextAutospace, TextSpacingTrim};
use crate::tests::{PLAIN_PUNCT, PUNCT, ahem_fallback};

/// Returns a fixture over Ahem, a Japanese font with `halt` and one
/// without.
fn cjk() -> Fixture {
    Fixture::new(
        &[
            TestFont::cjk("Test Punct", true),
            TestFont::cjk("Test Punct Plain", false),
        ],
        ahem_fallback(),
        StageCheck::Placed(check),
    )
}

/// Returns each line's clusters and width.
fn widths(layout: &Layout) -> Vec<(Range<usize>, f32)> {
    layout
        .line_records()
        .lines
        .iter()
        .map(|(_, line)| {
            let range = line.clusters();
            (range.start.get()..range.end.get(), line.width.to_px())
        })
        .collect()
}

/// Returns `family` at 20px, trimming as `trim` says.
fn trimmed(
    family: &'static [FontFamilyName<'static>],
    trim: TextSpacingTrim,
) -> ComputedStyle<'static> {
    let mut style = sized(family, 20.0);
    style.text.spacing_trim = trim;
    style
}

/// A line that wraps to an opening mark keeps its blank under `normal`, and
/// gives it back under `space-first` and `trim-start`.
///
/// The mark is reshaped as a start. The block's first line is not a wrapped
/// one. Chrome 153 in Yu Gothic at 160px sets `「漢字」` at 160 and at 140.
#[test]
fn a_line_wrapping_to_an_opening_mark_trims_it_under_space_first() {
    let mut fixture = cjk();
    let mut layout = Layout::new();
    let text = "漢字漢字「漢字」";
    for (trim, second) in [
        (TextSpacingTrim::Normal, 80.0),
        (TextSpacingTrim::SpaceFirst, 70.0),
        (TextSpacingTrim::TrimStart, 70.0),
        (TextSpacingTrim::SpaceAll, 80.0),
    ] {
        fixture.text(&mut layout, &trimmed(&PUNCT, trim), text);
        fixture.lay_out(&mut layout, 80.0);
        assert_eq!(widths(&layout), [(0..4, 80.0), (4..8, second)], "{trim:?}");
    }
    // Where the font has no `halt`, nothing is trimmed, as in Chrome.
    // `PunctuationTrim::Always` halves the mark instead.
    let plain = trimmed(&PLAIN_PUNCT, TextSpacingTrim::SpaceFirst);
    fixture.text(&mut layout, &plain, text);
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(widths(&layout), [(0..4, 80.0), (4..8, 80.0)]);
    let mut config = *fixture.cx.config();
    config.punctuation_trim = PunctuationTrim::Always;
    fixture.cx.set_config(config);
    fixture.text(&mut layout, &plain, text);
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(widths(&layout), [(0..4, 80.0), (4..8, 70.0)]);
}

/// A closing mark that fits only trimmed at a line's end ends the line
/// trimmed, where a line may break after it.
///
/// One that fits whole keeps its blank. One that fits neither way goes to
/// the next line, with what may not start a line without it. These match
/// Chrome 153 in Yu Gothic at 180, 200 and 179px.
#[test]
fn a_closing_mark_that_fits_only_trimmed_ends_the_line_trimmed() {
    let mut fixture = cjk();
    let mut layout = Layout::new();
    let text = "漢字漢字」漢字";
    fixture.text(&mut layout, &trimmed(&PUNCT, TextSpacingTrim::Normal), text);
    fixture.lay_out(&mut layout, 90.0);
    assert_eq!(widths(&layout), [(0..5, 90.0), (5..7, 40.0)]);
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(widths(&layout), [(0..5, 100.0), (5..7, 40.0)]);
    fixture.lay_out(&mut layout, 89.0);
    assert_eq!(widths(&layout), [(0..3, 60.0), (3..7, 80.0)]);
    fixture.text(
        &mut layout,
        &trimmed(&PUNCT, TextSpacingTrim::SpaceAll),
        text,
    );
    fixture.lay_out(&mut layout, 90.0);
    assert_eq!(widths(&layout), [(0..3, 60.0), (3..7, 80.0)]);
}

/// A pair of marks a break parts keeps the closing trim and gives the
/// opening blank back.
///
/// A closing mark kerned against the one after stays trimmed at the line's
/// end, as Chrome's reshaped end reads the text after it. An opening mark
/// kerned against the one before gets its blank back at the next line's
/// start, shaped as a start. These match Chrome 153 in Yu Gothic at 180 and
/// 160px.
#[test]
fn a_pair_a_break_parts_keeps_its_closing_trim_and_gives_its_blank_back() {
    let mut fixture = cjk();
    let mut layout = Layout::new();
    let style = trimmed(&PUNCT, TextSpacingTrim::Normal);
    fixture.text(&mut layout, &style, "漢字漢」」字漢字");
    fixture.lay_out(&mut layout, 90.0);
    assert_eq!(widths(&layout), [(0..5, 90.0), (5..8, 60.0)]);
    fixture.text(&mut layout, &style, "漢字漢字「「漢字");
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(widths(&layout), [(0..4, 80.0), (4..8, 70.0)]);
    fixture.text(&mut layout, &style, "漢字漢」「漢字漢字");
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(widths(&layout), [(0..4, 80.0), (4..8, 80.0), (8..9, 20.0)]);
}

/// A host whose area narrows to `shelf.1` from `shelf.0` down, and which
/// places each float at the left edge, as high as asked.
struct Shelf {
    width: f32,
    shelf: (f32, f32),
    /// Each float placed: its right edge, top and bottom.
    floats: Vec<(f32, f32, f32)>,
}

impl Exclusions for Shelf {
    fn band(&self, _line: usize, block: BlockExtents) -> InlineExtents {
        let (top, edge) = self.shelf;
        let right = if block.end > top { edge } else { self.width };
        let left = self
            .floats
            .iter()
            .filter(|&&(_, top, bottom)| top < block.end && block.start < bottom)
            .fold(0.0, |left: f32, &(right, ..)| left.max(right));
        InlineExtents { left, right }
    }

    fn below(&self, top: f32) -> Option<f32> {
        let bottoms = self.floats.iter().map(|&(.., bottom)| bottom);
        bottoms.filter(|&bottom| bottom > top).reduce(f32::min)
    }

    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        let (top, bottom) = (float.block_start, float.block_start + float.block_size);
        self.floats.push((float.inline_size, top, bottom));
        PlacedFloat {
            inline: InlineExtents {
                left: 0.0,
                right: float.inline_size,
            },
            block: BlockExtents {
                start: top,
                end: bottom,
            },
        }
    }

    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(self.floats.len() as u64)
    }

    fn rewind(&mut self, to: ExclusionsCheckpoint) {
        self.floats
            .truncate(usize::try_from(to.0).unwrap_or(usize::MAX));
    }
}

/// Returns how many lines of `layout` start at a cluster the font shaped
/// across, and asserts no such start's reshape is repeated, `at` naming the
/// layout.
fn starts_reshaped_once(layout: &Layout, at: &str) -> usize {
    let records = layout.line_records();
    let glyphs = &layout.shaped().text(FirstLineVariant::Standard).glyphs;
    let starts: Vec<ClusterId> = records
        .lines
        .iter()
        .map(|(_, line)| line.clusters().start)
        .filter(|&start| start.get() > 0 && glyphs.word(start).is_unsafe_to_break())
        .collect();
    let reshaped = &records.reshaped;
    for (index, one) in reshaped.iter().enumerate() {
        if starts.contains(&one.0.start) {
            assert!(
                !reshaped[..index].contains(one),
                "{one:?} reshaped twice {at}: {reshaped:?}"
            );
        }
    }
    starts.len()
}

/// A line reshapes each of its edges once.
///
/// Its start window is reshaped once, however many candidate ends it
/// measures. Each candidate starts with the window's piece from the edge
/// tables, and each end is measured once. Here every line after the first
/// starts at an opening mark kerned against the one before, so each is
/// reshaped at its start.
///
/// A line fitted again keeps its start's piece: beside a float it places,
/// and in the narrower band its own height meets.
#[test]
fn a_line_reshapes_each_edge_once() {
    let mut fixture = cjk();
    let mut layout = Layout::new();
    let style = trimmed(&PUNCT, TextSpacingTrim::Normal);
    let text = "漢字漢」「漢字漢字、「漢字」".repeat(8);
    fixture.text(&mut layout, &style, &text);
    let mut starts = 0;
    for width in [80.0, 90.0, 100.0, 130.0, 170.0] {
        fixture.lay_out(&mut layout, width);
        let reshaped = &layout.line_records().reshaped;
        starts += layout
            .line_records()
            .lines
            .iter()
            .filter(|(_, line)| {
                let start = line.clusters().start;
                start.get() > 0
                    && layout
                        .shaped()
                        .text(FirstLineVariant::Standard)
                        .glyphs
                        .word(start)
                        .is_unsafe_to_break()
            })
            .count();
        for (at, one) in reshaped.iter().enumerate() {
            assert!(
                !reshaped[..at].contains(one),
                "{one:?} reshaped twice at {width}: {reshaped:?}"
            );
        }
    }
    assert!(starts >= 10, "{starts} line starts reshaped");

    // Each unit holds a float, 10 wide, and a letter twice as tall as the
    // text, so a line refits beside its float and settles past the shelf.
    let mut large = style;
    large.font.size = 40.0;
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        for unit in 0..8 {
            let key = 10 * unit;
            b.text(NodeKey(key + 1), "漢字漢」「漢");
            b.float(
                NodeKey(key + 2),
                &style,
                FloatSide::Left,
                float_size(10.0, 15.0),
            );
            b.text(NodeKey(key + 3), "字漢字、「漢");
            b.open_box(NodeKey(key + 4), &large, None);
            b.text(NodeKey(key + 5), "字");
            b.close_box();
            b.text(NodeKey(key + 6), "」");
        }
    });
    let mut starts = 0;
    for width in [90.0, 100.0, 130.0, 170.0] {
        for shelf in [30.0, 70.0, 110.0] {
            let mut host = Shelf {
                width,
                shelf: (shelf, width - 20.0),
                floats: Vec::new(),
            };
            fixture.lay_out_with(&mut layout, Area::new(width), &mut host);
            assert!(!host.floats.is_empty(), "floats placed at {width}");
            starts += starts_reshaped_once(&layout, &format!("at {width}, {shelf}"));
        }
    }
    assert!(starts >= 10, "{starts} line starts reshaped beside floats");
}

/// A line ending at an autospace seam gives the seam's room back.
///
/// The line fits without it, and its text ends at its width when aligned.
/// Chrome 153 sets `漢字漢字ABCD` in MS Gothic at 160 and 400px this way.
#[test]
fn a_line_ending_at_a_seam_gives_its_room_back() {
    let mut fixture = cjk();
    let mut layout = Layout::new();
    let mut style = sized(&PUNCT, 20.0);
    style.text.autospace = TextAutospace::NORMAL;
    fixture.text(&mut layout, &style, "漢字漢字ABCD");
    // The whole paragraph takes the room once: 80, 2.5 and 40.
    fixture.lay_out(&mut layout, 200.0);
    assert_eq!(widths(&layout), [(0..8, 122.5)]);
    fixture.lay_out(&mut layout, 80.0);
    assert_eq!(widths(&layout), [(0..4, 80.0), (4..8, 40.0)]);
    // The first line's text item is as wide as the line.
    let line = layout.line(0).expect("a line");
    let text: f32 = line
        .items()
        .filter_map(|item| match item {
            crate::Item::Text(run) => Some(run.advance()),
            _ => None,
        })
        .sum();
    assert_eq!(text, 80.0);
    // Right-aligned in 100, it ends at the band's end.
    let block = ComputedBlockStyle {
        text_align: TextAlign::Right,
        ..ComputedBlockStyle::new(&style)
    };
    fixture.build(&mut layout, &block, |b| {
        b.text(NodeKey(1), "漢字漢字ABCD");
    });
    fixture.lay_out(&mut layout, 100.0);
    let line = layout.line(0).expect("a line");
    let origin = line.metrics().left;
    let (left, right) = line
        .items()
        .filter_map(|item| match item {
            crate::Item::Text(run) => Some(run.inline()),
            _ => None,
        })
        .fold((f32::MAX, f32::MIN), |(l, r), run| {
            (l.min(run.left), r.max(run.right))
        });
    assert_eq!((origin + left, origin + right), (20.0, 100.0));
    // Min-content does not pay it where an unbreakable part ends at the
    // seam: `漢` alone is 20, and with the room it would be 22.5.
    fixture.text(&mut layout, &style, "漢A");
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (20.0, 32.5));
}
