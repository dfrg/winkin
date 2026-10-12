//! `text-wrap-style: balance` and `pretty` tests, in Ahem at 10 px, with
//! every case Chrome 153 decides measured in it:
//! - balance evens a paragraph's slack as Chrome's scorer does, weighing a
//!   picture's opportunity past the space after it;
//! - balanced lines sit in their band under any `text-align` and
//!   `text-align-last`;
//! - the first line's indent and a leading float shrink their lines' rooms;
//! - few opportunities, a restyled first line and a clamp each balance as
//!   Chrome does;
//! - beyond Chrome, every paragraph balances, as do more than six lines,
//!   a float inside the paragraph and an initial letter;
//! - `pretty` gives a lone last word company over the last four lines, and
//!   leaves a last line of a third or of two words alone;
//! - `Pretty::Even` (beyond Chrome) evens a short ending of two words;
//! - relayout equals a fresh layout, any width gives valid lines, and the
//!   work stays bounded.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::*;
use crate::config::{Config, Pretty};
use crate::style::{InitialLetter, LineClamp, OverflowWrap, TextAlignLast, TextWrapStyle};
// Read only by the step-count tests, which run in debug builds.
#[cfg(debug_assertions)]
use crate::work;

/// Nine words of Ahem that break greedily at 180 px into lines of 17, 15
/// and 3 characters.
const NINE: &str = "a bb cccccc d eee fffff gg hhhhhh iii";

/// Returns a block whose `text-wrap-style` is `style`.
fn wrapping(style: TextWrapStyle) -> ComputedBlockStyle<'static> {
    ComputedBlockStyle {
        text_wrap_style: style,
        ..ComputedBlockStyle::default()
    }
}

/// Builds `text` in Ahem at 10 px into `layout`, in `block`, with a forced
/// break at each `\n`.
fn build(fixture: &mut Fixture, layout: &mut Layout, block: &ComputedBlockStyle, text: &str) {
    let style = ahem(10.0);
    fixture.build(
        layout,
        &ComputedBlockStyle {
            style: &style,
            ..*block
        },
        |b| {
            for (at, paragraph) in text.split('\n').enumerate() {
                if at > 0 {
                    b.line_break(NodeKey(100 + at as u64));
                }
                b.text(NodeKey(1 + at as u64), paragraph);
            }
        },
    );
}

/// Returns each line's text without its trailing white space, as Chrome's
/// rows read it.
fn rows(layout: &Layout) -> Vec<String> {
    texts(layout)
        .into_iter()
        .map(|text| String::from(text.trim_end()))
        .collect()
}

/// Returns each line's text for `text` in `block` broken `width` wide.
fn broken(
    fixture: &mut Fixture,
    block: &ComputedBlockStyle,
    text: &str,
    width: f32,
) -> Vec<String> {
    let mut layout = Layout::new();
    build(fixture, &mut layout, block, text);
    fixture.lay_out(&mut layout, width);
    rows(&layout)
}

/// A host placing each float at its side's edge of the band, as high as
/// asked.
///
/// It keeps every float and rewinds to a checkpoint.
struct Floats {
    width: f32,
    placed: Vec<(FloatSide, f32, f32, f32, f32)>,
}

impl Floats {
    fn new(width: f32) -> Self {
        Self {
            width,
            placed: Vec::new(),
        }
    }

    /// Returns what the floats leave of the area from `top` to `bottom`.
    fn room(&self, top: f32, bottom: f32) -> (f32, f32) {
        let (mut left, mut right) = (0.0f32, self.width);
        for &(side, from, to, start, end) in &self.placed {
            if start < bottom && top < end {
                match side {
                    FloatSide::Left => left = left.max(to),
                    FloatSide::Right => right = right.min(from),
                }
            }
        }
        (left, right.max(left))
    }
}

impl Exclusions for Floats {
    fn band(&self, _line: usize, block: BlockExtents) -> InlineExtents {
        let (left, right) = self.room(block.start, block.end);
        InlineExtents { left, right }
    }

    fn below(&self, top: f32) -> Option<f32> {
        self.placed
            .iter()
            .map(|placed| placed.4)
            .filter(|&bottom| bottom > top)
            .reduce(f32::min)
    }

    fn place(&mut self, float: FloatRequest) -> PlacedFloat {
        let top = float.block_start;
        let (left, right) = self.room(top, top + float.block_size);
        let from = match float.side {
            FloatSide::Left => left,
            FloatSide::Right => right - float.inline_size,
        };
        let to = from + float.inline_size;
        let bottom = top + float.block_size;
        self.placed.push((float.side, from, to, top, bottom));
        PlacedFloat {
            inline: InlineExtents {
                left: from,
                right: to,
            },
            block: BlockExtents {
                start: top,
                end: bottom,
            },
        }
    }

    fn checkpoint(&self) -> ExclusionsCheckpoint {
        ExclusionsCheckpoint(self.placed.len() as u64)
    }

    fn rewind(&mut self, to: ExclusionsCheckpoint) {
        self.placed
            .truncate(usize::try_from(to.0).unwrap_or(usize::MAX));
    }
}

// Balance ------------------------------------------------------------------

/// Balance evens the paragraph's slack over the greedy count of lines, as
/// Blink's `ScoreLineBreaker::BalanceBreakPoints` does for six lines or
/// fewer.
///
/// - It minimizes the sum of each line's room left squared. The narrowest
///   room keeping three lines would end them at `d`, `gg` and the end
///   instead. Measured in Chrome 153 at 180 px.
/// - A 60 px picture in place of `hhhhhh` breaks where the word did, as
///   Chrome's scorer takes atomic inlines.
/// - The lines sit in the whole band, so a centred block's lines are
///   centred in it, 35, 35 and 25 px in, as in Chrome.
#[test]
fn balance_evens_the_slack_as_chromes_scorer_does() {
    let mut fixture = fixture();
    assert_eq!(
        broken(&mut fixture, &ComputedBlockStyle::default(), NINE, 180.0),
        ["a bb cccccc d eee", "fffff gg hhhhhh", "iii"]
    );
    let balance = wrapping(TextWrapStyle::Balance);
    assert_eq!(
        broken(&mut fixture, &balance, NINE, 180.0),
        ["a bb cccccc", "d eee fffff", "gg hhhhhh iii"]
    );

    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..balance
        },
        |b| {
            b.text(NodeKey(1), "a bb cccccc d eee fffff gg ");
            b.atomic(NodeKey(2), &style, None, float_size(60.0, 10.0));
            b.text(NodeKey(3), " iii");
        },
    );
    fixture.lay_out(&mut layout, 180.0);
    assert_eq!(widths(&layout), [110.0, 110.0, 130.0]);

    let centred = ComputedBlockStyle {
        text_align: TextAlign::Center,
        ..balance
    };
    let mut layout = Layout::new();
    build(&mut fixture, &mut layout, &centred, NINE);
    fixture.lay_out(&mut layout, 180.0);
    let lefts: Vec<f32> = layout.lines().map(|line| line.metrics().left).collect();
    assert_eq!(lefts, [35.0, 35.0, 25.0]);
    assert!(bands(&layout).iter().all(|&band| band == (0.0, 180.0)));

    // Justified, the text breaks at the same places, as in Chrome 153. The
    // lines before the last span the band, and `text-align-last` sets the
    // last. The scorer reads `text-align` alone, as Chrome's does.
    let justified = ComputedBlockStyle {
        text_align: TextAlign::Justify,
        text_align_last: TextAlignLast::Center,
        ..balance
    };
    let mut layout = Layout::new();
    build(&mut fixture, &mut layout, &justified, NINE);
    fixture.lay_out(&mut layout, 180.0);
    assert_eq!(
        rows(&layout),
        ["a bb cccccc", "d eee fffff", "gg hhhhhh iii"]
    );
    let lefts: Vec<f32> = layout.lines().map(|line| line.metrics().left).collect();
    assert_eq!(lefts, [0.0, 0.0, 25.0]);

    // Six lines, the most Chrome scores, measured at 120 px.
    let six = "aa bbbbbb c dd e fff gggg hhhhhhh ii jjjj kkkkk l mmmmm nn";
    assert_eq!(
        broken(&mut fixture, &balance, six, 120.0),
        [
            "aa bbbbbb",
            "c dd e",
            "fff gggg",
            "hhhhhhh ii",
            "jjjj kkkkk",
            "l mmmmm nn"
        ]
    );
}

/// The scorer weighs an atomic inline's opportunity past the space after
/// it.
///
/// CSS Text 3 places the opportunity before the space too, but the space
/// hangs at a line's end and the next line starts past it. Blink's
/// `AppendTrailingSpaces` likewise moves its candidate past the space.
/// Measured in Chrome 153, Ahem at 10 px in 60:
/// - `a ￼ a bb ￼ a a` with 20 px pictures balances to `a ￼`, `a bb` and
///   `￼ a a`. Greedy sets `a ￼ a`, `bb ￼` and `a a`.
/// - With 60 px pictures, `a ￼ a dddd ￼ a a` balances to the greedy
///   lines, each picture keeping the space after it.
///
/// Chrome's own lines put that space on the next line, skipped there as
/// leading white space, since `LineBreaker::HandleText` finds the case too
/// complex. Nothing shows it, and it is not copied.
#[test]
fn balance_weighs_a_pictures_opportunity_past_the_space_after_it() {
    let mut fixture = fixture();
    let style = ahem(10.0);
    let lines = |fixture: &mut Fixture, wrap: TextWrapStyle, picture: f32, word: &str| {
        let mut layout = Layout::new();
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..wrapping(wrap)
            },
            |b| {
                b.text(NodeKey(1), "a ");
                b.atomic(NodeKey(2), &style, None, float_size(picture, 10.0));
                b.text(NodeKey(3), &format!(" a {word} "));
                b.atomic(NodeKey(4), &style, None, float_size(picture, 10.0));
                b.text(NodeKey(5), " a a");
            },
        );
        fixture.lay_out(&mut layout, 60.0);
        texts(&layout)
    };
    assert_eq!(
        lines(&mut fixture, TextWrapStyle::Auto, 20.0, "bb"),
        ["a \u{FFFC} a ", "bb \u{FFFC} ", "a a"]
    );
    assert_eq!(
        lines(&mut fixture, TextWrapStyle::Balance, 20.0, "bb"),
        ["a \u{FFFC} ", "a bb ", "\u{FFFC} a a"]
    );
    let greedy = lines(&mut fixture, TextWrapStyle::Auto, 60.0, "dddd");
    assert_eq!(greedy, ["a ", "\u{FFFC} ", "a dddd ", "\u{FFFC} ", "a a"]);
    assert_eq!(
        lines(&mut fixture, TextWrapStyle::Balance, 60.0, "dddd"),
        greedy
    );
}

/// The first line's room is its band less its indent, as Chrome's
/// `AvailableWidth` has it.
///
/// WPT `text-wrap-balance-text-indent-001` balances a 100 px block indented
/// 50 into lines 70 px long each, the first with its indent. Measured in
/// Chrome 153.
#[test]
fn balance_takes_the_indent_off_the_first_lines_room() {
    let mut fixture = fixture();
    let indented = ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage {
                px: 50.0,
                fraction: 0.0,
            },
            hanging: false,
            each_line: false,
        },
        ..wrapping(TextWrapStyle::Balance)
    };
    let mut layout = Layout::new();
    build(&mut fixture, &mut layout, &indented, "01 34 6 89 12 3 56");
    fixture.lay_out(&mut layout, 100.0);
    assert_eq!(rows(&layout), ["01", "34 6 89", "12 3 56"]);
    let indents: Vec<f32> = records(&layout)
        .iter()
        .map(|line| line.indent.to_px())
        .collect();
    assert_eq!(indents, [50.0, 0.0, 0.0]);
    assert_eq!(widths(&layout), [20.0, 70.0, 70.0]);
}

/// A paragraph with fewer than three opportunities is balanced by bisecting
/// the room, as Blink's `ParagraphLineBreaker` does.
///
/// The scorer has nothing to choose between. The room shrinks to the
/// narrowest that keeps the line count, so the longest line is shortest.
/// Measured in Chrome 153 at 100 px.
#[test]
fn few_opportunities_are_balanced_by_halving_the_room() {
    let mut fixture = fixture();
    let balance = wrapping(TextWrapStyle::Balance);
    assert_eq!(
        broken(&mut fixture, &balance, "aaaa bbbb cc", 100.0),
        ["aaaa", "bbbb cc"]
    );
    assert_eq!(
        broken(&mut fixture, &balance, "aaaaaaa bb cc", 100.0),
        ["aaaaaaa", "bb cc"]
    );
    // One line has nothing to even.
    assert_eq!(
        broken(&mut fixture, &balance, "aaaa bbbb cc", 200.0),
        ["aaaa bbbb cc"]
    );
}

/// Balancing takes no room that needs an `overflow-wrap` emergency break.
///
/// Chrome's breaker retries a line that overflows with the emergency
/// breaks, and that disables its bisection (`RetryAfterOverflow`). So a
/// trial room that breaks inside `CONTROLLING` is too narrow, and the
/// greedy lines stay.
#[test]
fn balance_takes_no_room_that_needs_an_emergency_break() {
    let mut fixture = fixture();
    let mut style = ahem(10.0);
    style.text.overflow_wrap = OverflowWrap::BreakWord;
    let balance = ComputedBlockStyle {
        style: &style,
        ..wrapping(TextWrapStyle::Balance)
    };
    for (text, expected) in [
        ("CONTROLLING YOUR BU", ["CONTROLLING", "YOUR BU"]),
        ("CONTROLLING YOUR BU BU", ["CONTROLLING", "YOUR BU BU"]),
    ] {
        let mut layout = Layout::new();
        fixture.build(&mut layout, &balance, |b| b.text(NodeKey(1), text));
        fixture.lay_out(&mut layout, 150.0);
        assert_eq!(rows(&layout), expected, "{text}");
    }
}

/// Each line beside a leading float is scored in its own room, as Chrome's
/// scorer reads its `LineWidths`.
///
/// In Chrome 153 a float 40 wide and 20 tall beside 10 px lines in 180
/// gives the first two lines 140 each. The paragraph balances by the slack
/// in those rooms.
#[test]
fn a_leading_floats_lines_are_balanced_in_their_own_rooms() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let style = ahem(10.0);
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..wrapping(TextWrapStyle::Balance)
        },
        |b| {
            b.float(NodeKey(2), &style, FloatSide::Left, float_size(40.0, 20.0));
            b.text(NodeKey(1), NINE);
        },
    );
    fixture.lay_out_with(&mut layout, Area::new(180.0), &mut Floats::new(180.0));
    assert_eq!(
        rows(&layout),
        ["a bb cccccc", "d eee fffff", "gg hhhhhh iii"]
    );
    assert_eq!(bands(&layout), [(40.0, 140.0), (40.0, 140.0), (0.0, 180.0)]);
    assert_eq!(layout.floats().count(), 1);
}

/// `line-clamp` applies first, and the lines it keeps are balanced.
///
/// CSS Text 4 says so, and Chrome 153 does it under
/// `-webkit-line-clamp: 2`. Greedily the two kept lines end at `c` and
/// `fff`. Balanced, they end at `bbbbbb` and `fff`, the second cut for its
/// ellipsis.
#[test]
fn a_clamps_kept_lines_are_balanced() {
    let mut fixture = fixture();
    let text = "aa bbbbbb c dd e fff gggg hhhhhhh ii jjjj kkkkk l mmmmm nn";
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(2),
        ..ComputedBlockStyle::default()
    };
    assert_eq!(
        broken(&mut fixture, &clamped, text, 120.0),
        ["aa bbbbbb c", "dd e fff"]
    );
    let balanced = ComputedBlockStyle {
        text_wrap_style: TextWrapStyle::Balance,
        ..clamped
    };
    let mut layout = Layout::new();
    build(&mut fixture, &mut layout, &balanced, text);
    fixture.lay_out(&mut layout, 120.0);
    assert_eq!(rows(&layout), ["aa bbbbbb", "c dd e fff"]);
    assert!(layout.line(1).expect("a line").has_ellipsis());
}

/// A first line in its own styles is balanced by bisecting the room, as in
/// Chrome, whose scorer takes no `::first-line`.
///
/// With a 12 px first line and 10 px for the rest, in 180 px, Chrome 153
/// ends the lines at `cccccc` and `fffff`.
#[test]
fn a_restyled_first_line_is_balanced_by_halving_the_room() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let first = ahem(12.0);
    let mut b = layout.builder(
        NodeKey(0),
        &ComputedBlockStyle {
            style: &ahem(10.0),
            first_line: Some(&first),
            ..wrapping(TextWrapStyle::Balance)
        },
        BuildOptions::default(),
    );
    b.text(NodeKey(1), NINE);
    b.finish(&mut fixture.cx);
    fixture.lay_out(&mut layout, 180.0);
    assert_eq!(
        rows(&layout),
        ["a bb cccccc", "d eee fffff", "gg hhhhhh iii"]
    );
    assert_eq!(widths(&layout), [132.0, 110.0, 130.0]);
    assert!(
        records(&layout)
            .first()
            .is_some_and(|line| line.flags.contains(LineFlags::FIRST_LINE))
    );
}

/// Beyond Chrome, every paragraph balances, and so do paragraphs of more
/// than six lines.
///
/// CSS Text 4 balances each group of lines between forced breaks
/// separately. Chrome 153 balances only the first. A paragraph of more
/// than six lines is balanced by bisecting the room, where Chrome leaves it
/// greedy. Here the room comes to 150⅓ px, the narrowest that keeps its
/// seven lines.
#[test]
fn every_paragraph_and_more_than_six_lines_are_balanced() {
    let mut fixture = fixture();
    let balance = wrapping(TextWrapStyle::Balance);
    let two = format!("{NINE}\n{NINE}");
    assert_eq!(
        broken(&mut fixture, &balance, &two, 180.0),
        [
            "a bb cccccc",
            "d eee fffff",
            "gg hhhhhh iii",
            "a bb cccccc",
            "d eee fffff",
            "gg hhhhhh iii"
        ]
    );

    let seven = "aaaaaaa b ccccc dddd eeeee f ggggggg h iiii jjj kkkkk lllll mmmmm nnnnn oo \
                 pppppp qqq rrrr sssss ttttt";
    assert_eq!(
        broken(&mut fixture, &ComputedBlockStyle::default(), seven, 180.0),
        [
            "aaaaaaa b ccccc",
            "dddd eeeee f",
            "ggggggg h iiii jjj",
            "kkkkk lllll mmmmm",
            "nnnnn oo pppppp",
            "qqq rrrr sssss",
            "ttttt"
        ]
    );
    assert_eq!(
        broken(&mut fixture, &balance, seven, 180.0),
        [
            "aaaaaaa b ccccc",
            "dddd eeeee f",
            "ggggggg h iiii",
            "jjj kkkkk lllll",
            "mmmmm nnnnn oo",
            "pppppp qqq rrrr",
            "sssss ttttt"
        ]
    );
}

/// Beyond Chrome, a paragraph holding a float is balanced by bisecting the
/// room.
///
/// Every line's band narrows alike, so the room each line leaves comes out
/// even. CSS Text 4 balances "the remaining space ... after placing
/// floats". Chrome 153 leaves it greedy. The float is placed once, the line
/// count holds, and no line overflows.
#[test]
fn a_paragraph_holding_a_float_is_balanced_beside_it() {
    let mut fixture = fixture();
    let style = ahem(10.0);
    let build_with_float =
        |fixture: &mut Fixture, layout: &mut Layout, block: &ComputedBlockStyle| {
            fixture.build(
                layout,
                &ComputedBlockStyle {
                    style: &style,
                    ..*block
                },
                |b| {
                    b.text(NodeKey(1), "a bb cccccc d eee");
                    b.float(NodeKey(2), &style, FloatSide::Left, float_size(40.0, 20.0));
                    b.text(NodeKey(3), " fffff gg hhhhhh iii");
                },
            );
        };
    let mut greedy = Layout::new();
    build_with_float(&mut fixture, &mut greedy, &ComputedBlockStyle::default());
    fixture.lay_out_with(&mut greedy, Area::new(180.0), &mut Floats::new(180.0));
    assert_eq!(
        rows(&greedy),
        ["a bb cccccc d eee", "fffff gg", "hhhhhh iii"]
    );
    let mut layout = Layout::new();
    build_with_float(&mut fixture, &mut layout, &wrapping(TextWrapStyle::Balance));
    fixture.lay_out_with(&mut layout, Area::new(180.0), &mut Floats::new(180.0));
    assert_eq!(layout.lines().len(), 3);
    assert_eq!(layout.floats().count(), 1);
    assert!(
        layout
            .lines()
            .all(|line| line.metrics().width <= line.metrics().band.size())
    );
    // The slack each line leaves is more even than the greedy lines'.
    let spread = |layout: &Layout| {
        let slack: Vec<f32> = layout
            .lines()
            .map(|line| line.metrics().band.size() - line.metrics().width)
            .collect();
        let most = slack.iter().copied().fold(f32::MIN, f32::max);
        let least = slack.iter().copied().fold(f32::MAX, f32::min);
        most - least
    };
    assert!(spread(&layout) < spread(&greedy), "{:?}", rows(&layout));
    // The float goes with its anchor to the second line. Beside it the
    // lines leave 50, 20 and 40, where the greedy ones leave 10, 60 and 40.
    assert_eq!(
        rows(&layout),
        ["a bb cccccc d", "eee fffff gg", "hhhhhh iii"]
    );
    assert_eq!(bands(&layout), [(0.0, 180.0), (40.0, 140.0), (40.0, 140.0)]);
}

/// Beyond Chrome, lines beside an initial letter are balanced in the rooms
/// it leaves.
///
/// Chrome balances nothing with an initial letter.
#[test]
fn an_initial_letters_paragraph_is_balanced() {
    let mut fixture = fixture();
    fixture.cx.set_config(Config::chrome_windows());
    let style = ahem(10.0);
    let mut letter = style;
    letter.line.initial_letter = InitialLetter {
        size: 2.0,
        sink: 2,
        ..InitialLetter::NONE
    };
    let mut layout = Layout::new();
    fixture.build(
        &mut layout,
        &ComputedBlockStyle {
            style: &style,
            ..wrapping(TextWrapStyle::Balance)
        },
        |b| {
            b.open_box(NodeKey(7), &letter, None);
            b.text(NodeKey(8), "X");
            b.close_box();
            b.text(NodeKey(1), " a bb cccccc d eee fffff gg hhhhhh iii jj kkkk");
        },
    );
    let mut greedy = Layout::new();
    fixture.build(&mut greedy, &ComputedBlockStyle::new(&style), |b| {
        b.open_box(NodeKey(7), &letter, None);
        b.text(NodeKey(8), "X");
        b.close_box();
        b.text(NodeKey(1), " a bb cccccc d eee fffff gg hhhhhh iii jj kkkk");
    });
    fixture.lay_out_with(&mut layout, Area::new(180.0), &mut Floats::new(180.0));
    fixture.lay_out_with(&mut greedy, Area::new(180.0), &mut Floats::new(180.0));
    assert_eq!(
        rows(&greedy),
        ["X a bb cccccc d", "eee fffff gg", "hhhhhh iii jj", "kkkk"]
    );
    // Lines 1 and 2 beside the letter, 157 long, the others 180.
    assert_eq!(
        rows(&layout),
        ["X a bb cccccc", "d eee fffff", "gg hhhhhh", "iii jj kkkk"]
    );
    let lengths: Vec<f32> = bands(&layout).iter().map(|band| band.1).collect();
    assert_eq!(lengths, [180.0, 157.0, 157.0, 180.0]);
}

// Pretty -------------------------------------------------------------------

/// Chrome's `pretty` gives a lone last word company over the last four
/// lines, keeping the line count.
///
/// It minimizes the squared room the lines before the last leave. A line
/// ending at the last opportunity pays Blink's orphan penalty. Measured in
/// Chrome 153 at 180 px:
/// - the greedy `iii` alone becomes `hhhhhh iii`;
/// - a paragraph of seven lines keeps its first three and scores its last
///   four;
/// - each paragraph of a block is scored.
#[test]
fn pretty_gives_a_lone_last_word_company_as_chrome_does() {
    let mut fixture = fixture();
    let pretty = wrapping(TextWrapStyle::Pretty);
    assert_eq!(
        broken(&mut fixture, &pretty, NINE, 180.0),
        ["a bb cccccc d", "eee fffff gg", "hhhhhh iii"]
    );
    let seven = "aaaaaaa b ccccc dddd eeeee f ggggggg h iiii jjj kkkkk lllll mmmmm nnnnn oo \
                 pppppp qqq rrrr sssss ttttt";
    assert_eq!(
        broken(&mut fixture, &pretty, seven, 180.0),
        [
            "aaaaaaa b ccccc",
            "dddd eeeee f",
            "ggggggg h iiii jjj",
            "kkkkk lllll",
            "mmmmm nnnnn oo",
            "pppppp qqq rrrr",
            "sssss ttttt"
        ]
    );
    let two = format!("{NINE}\n{NINE}");
    assert_eq!(
        broken(&mut fixture, &pretty, &two, 180.0),
        [
            "a bb cccccc d",
            "eee fffff gg",
            "hhhhhh iii",
            "a bb cccccc d",
            "eee fffff gg",
            "hhhhhh iii"
        ]
    );
    // The first line keeps its indent, and pretty keeps its lines, as in
    // Chrome 153.
    let indented = ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage {
                px: 30.0,
                fraction: 0.0,
            },
            hanging: false,
            each_line: false,
        },
        ..pretty
    };
    assert_eq!(
        broken(
            &mut fixture,
            &indented,
            "a bb cccccc d eee fffff gg hhhhhh iii jj",
            180.0
        ),
        ["a bb cccccc d", "eee fffff gg", "hhhhhh iii jj"]
    );
}

/// Chrome's `pretty` acts only where the last line is one word under a
/// third of its room, as `ShouldOptimize` decides.
///
/// `iiiiii`, 60 of 180, is a third and stays, and so do two short words,
/// `i j`. Measured in Chrome 153.
#[test]
fn pretty_leaves_a_third_and_two_words_alone() {
    let mut fixture = fixture();
    let pretty = wrapping(TextWrapStyle::Pretty);
    for (text, last) in [
        ("a bb cccccc d eee fffff gg hhhhhh iiiiii", "iiiiii"),
        ("a bb cccccc d eee fffff gg hhhhhh i j", "i j"),
    ] {
        let lines = broken(&mut fixture, &pretty, text, 180.0);
        assert_eq!(
            lines,
            ["a bb cccccc d eee", "fffff gg hhhhhh", last],
            "{text}"
        );
    }
}

/// `Config::pretty = Pretty::Even` (beyond Chrome) also evens an ending
/// Chrome leaves alone.
///
/// A last line under a fifth of its room costs more than any choice for the
/// six lines before it. So `i j`, 30 of 180, takes `hhhhhh` down with it,
/// and the lines before even out. Where Chrome's rule acts, so does
/// `Pretty::Even`.
#[test]
fn pretty_even_evens_endings_chrome_leaves_alone() {
    let mut fixture = fixture();
    fixture.cx.set_config(Config {
        pretty: Pretty::Even,
        ..Config::chrome_windows()
    });
    let pretty = wrapping(TextWrapStyle::Pretty);
    assert_eq!(
        broken(
            &mut fixture,
            &pretty,
            "a bb cccccc d eee fffff gg hhhhhh i j",
            180.0
        ),
        ["a bb cccccc d", "eee fffff gg", "hhhhhh i j"]
    );
    assert_eq!(
        broken(&mut fixture, &pretty, NINE, 180.0),
        ["a bb cccccc d", "eee fffff gg", "hhhhhh iii"]
    );
    // A last line of a fifth or more is left alone.
    assert_eq!(
        broken(
            &mut fixture,
            &pretty,
            "a bb cccccc d eee fffff gg hhhhhh ii jj",
            180.0
        ),
        ["a bb cccccc d eee", "fffff gg hhhhhh", "ii jj"]
    );
}

// Every layout -------------------------------------------------------------

/// A relayout of balanced and pretty text equals a fresh layout, record
/// for record.
///
/// The text has floats, a first line in its own styles, an initial letter,
/// ruby and a tab. The widths make each path run.
#[test]
fn a_relayout_of_balanced_and_pretty_text_equals_a_fresh_layout() {
    let mut fixture = fixture();
    let style = ahem(10.0);
    let first = ahem(12.0);
    let mut letter = style;
    letter.line.initial_letter = InitialLetter {
        size: 2.0,
        sink: 2,
        ..InitialLetter::NONE
    };
    let small = ahem(5.0);
    let mut tabbed = style;
    tabbed.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    for wrap in [TextWrapStyle::Balance, TextWrapStyle::Pretty] {
        for (letters, first_line, ruby) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let block = wrapping(wrap);
            let build = |fixture: &mut Fixture, layout: &mut Layout| {
                let mut b = layout.builder(
                    NodeKey(0),
                    &ComputedBlockStyle {
                        style: &style,
                        first_line: first_line.then_some(&first),
                        ..block
                    },
                    BuildOptions::default(),
                );
                if letters {
                    b.open_box(NodeKey(7), &letter, None);
                    b.text(NodeKey(8), "X");
                    b.close_box();
                }
                b.float(NodeKey(2), &style, FloatSide::Left, float_size(30.0, 25.0));
                b.text(NodeKey(1), " a bb cccccc d eee ");
                b.float(NodeKey(3), &style, FloatSide::Right, float_size(20.0, 15.0));
                b.text(NodeKey(4), "fffff gg hhhhhh iii jj kkkk lllll m nn o");
                b.line_break(NodeKey(5));
                if ruby {
                    // Ruby over a paragraph, and a tab, which is walked.
                    b.open_ruby(NodeKey(10), &style, None);
                    b.text(NodeKey(11), "rrr");
                    b.open_annotation(NodeKey(12), &small, None);
                    b.text(NodeKey(13), "annotation");
                    b.close_annotation();
                    b.close_ruby();
                    b.text(NodeKey(14), " a bb cccccc d eee fffff");
                    b.line_break(NodeKey(15));
                    b.open_box(NodeKey(16), &tabbed, None);
                    b.text(NodeKey(17), "a bb\tcccccc d eee fffff gg hhhhhh iii");
                    b.close_box();
                    b.line_break(NodeKey(18));
                }
                b.text(NodeKey(6), NINE);
                b.finish(&mut fixture.cx);
            };
            let mut layout = Layout::new();
            build(&mut fixture, &mut layout);
            for width in [180.0, 120.0, 45.0, 300.0, 10.0, 180.0] {
                fixture.lay_out_with(&mut layout, Area::new(width), &mut Floats::new(width));
                let mut fresh = Layout::new();
                build(&mut fixture, &mut fresh);
                fixture.lay_out_with(&mut fresh, Area::new(width), &mut Floats::new(width));
                let (a, b) = (layout.line_records(), fresh.line_records());
                let at = (wrap, letters, first_line, ruby, width);
                assert_eq!(a.lines.as_slice(), b.lines.as_slice(), "{at:?}");
                assert_eq!(a.floats(), b.floats(), "{at:?}");
                assert_eq!(a.block, b.block, "{at:?}");
            }
        }
    }
}

/// Balanced and pretty text breaks into valid lines at any width, under
/// either rule.
///
/// The widths include zero, negative, not a number and past the grid.
#[test]
fn any_width_gives_valid_lines() {
    let mut fixture = fixture();
    for pretty in [Pretty::Limited, Pretty::Even] {
        fixture.cx.set_config(Config {
            pretty,
            ..Config::chrome_windows()
        });
        for wrap in [TextWrapStyle::Balance, TextWrapStyle::Pretty] {
            let mut layout = Layout::new();
            build(
                &mut fixture,
                &mut layout,
                &wrapping(wrap),
                &format!("{NINE}\n{NINE} {NINE}"),
            );
            for width in [0.0, -50.0, f32::NAN, f32::INFINITY, 1e9, 5.0, 35.0, 180.0] {
                fixture.lay_out_with(&mut layout, Area::new(width), &mut Floats::new(width));
            }
        }
    }
}

/// Balancing's work, counted in `crate::work` steps, stays bounded.
///
/// - Bisecting the room of a paragraph past six lines lays it out once a
///   step, at most 25 times. So a paragraph four times as long takes about
///   four times the steps.
/// - The scorer weighs each candidate against those a line's length back.
///   So a six-line window costs the same however long the paragraph before
///   it.
///
/// Steps are counted only in debug builds.
#[cfg(debug_assertions)]
#[test]
fn balancing_takes_steps_in_line_with_the_paragraph() {
    let mut fixture = fixture();
    let steps = |fixture: &mut Fixture, wrap: TextWrapStyle, words: usize| {
        let text: Vec<&str> = NINE.split(' ').cycle().take(words).collect();
        let text = text.join(" ");
        let mut layout = Layout::new();
        build(fixture, &mut layout, &wrapping(wrap), &text);
        work::take();
        fixture.lay_out(&mut layout, 180.0);
        let taken = work::take();
        (taken as f64, layout.lines().len() as f64)
    };
    for wrap in [TextWrapStyle::Balance, TextWrapStyle::Pretty] {
        let (short, short_lines) = steps(&mut fixture, wrap, 400);
        let (long, long_lines) = steps(&mut fixture, wrap, 1600);
        let (short, long) = (short / short_lines, long / long_lines);
        assert!(
            long <= 1.5 * short + 1.0,
            "{wrap:?}: {short:.1} steps a line of 400 words, {long:.1} of 1600"
        );
    }
}
