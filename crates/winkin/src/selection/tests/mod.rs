//! Selection tests, against Chrome 153 on the same text in Ahem at 20px.
//!
//! This file holds the context, the styles and the shared queries. The
//! children pin:
//! - `carets`: carets, strong and weak, at wraps, soft hyphens and direction
//!   changes;
//! - `hit`: hit testing in and past lines, padding, ellipses and ruby;
//! - `caret_motion`: motion by character, line and line end, logical and on
//!   screen;
//! - `word_motion`: motion by word, on each platform and in each script;
//! - `rects`: selection rectangles and copied text;
//! - `offsets`: conversion to and from the caller's nodes;
//! - `vertical`: vertical lines and combined text;
//! - `cost`: the cost of each query;
//! - `ruby`: split ruby annotations;
//! - `security`: text masked by `-webkit-text-security`;
//! - `seeks`: the searches each public call makes.
//!
//! Chrome counts offsets in UTF-16 and this crate in UTF-8 bytes. Where the
//! text holds a soft hyphen or Hebrew, the offsets here match Chrome's byte
//! for byte, not number for number.

use alloc::borrow::Cow;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::build::BoxSize;
use crate::config::PastLines;
use crate::stages::analysis::ClusterId;
use crate::stages::lines::{Area, InlineExtents, NoExclusions};
use crate::style::{
    BaseDirection, ComputedStyle, EdgesGroup, FontFamilyName, FontGroup, FontVariantCaps,
    LineHeight, Sides, TextCase, TextOverflow, TextWrapMode, WhiteSpaceCollapse, WordBreak,
};
use crate::tests::{TestFont, across, ahem_fallback, along, collection};
use crate::{BuildOptions, ComputedBlockStyle, Context, CrossExtents, Item, LayoutBuilder};

/// Returns a caret's or rectangle's extents along and across its line, as pairs to compare.
fn pairs(inline: InlineExtents, block: CrossExtents) -> ((f32, f32), (f32, f32)) {
    ((inline.left, inline.right), (block.over, block.under))
}

const FAMILIES: [FontFamilyName<'static>; 4] = [
    FontFamilyName::Named(Cow::Borrowed("Ahem")),
    FontFamilyName::Named(Cow::Borrowed("Test Hebrew")),
    FontFamilyName::Named(Cow::Borrowed("Test Arabic")),
    FontFamilyName::Named(Cow::Borrowed("Test Latin")),
];

/// Returns a context over Ahem and three test fonts.
///
/// The fonts are a Hebrew one whose letters are an em wide, an Arabic one
/// whose lam and alef form a ligature, and a Latin one with an `fi` ligature.
fn context() -> Context {
    let mut hebrew = TestFont::new("Test Hebrew", &[(0x5D0, 0x5EA)]);
    hebrew.advances = (0x5D0..=0x5EA)
        .filter_map(char::from_u32)
        .map(|ch| (ch, 1000))
        .collect();
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E)]);
    latin.ligatures = vec![vec!['f', 'i']];
    latin.advances = (0x20u8..=0x7E).map(|b| (char::from(b), 1000)).collect();
    let mut arabic = TestFont::new("Test Arabic", &[(0x621, 0x64A)]);
    arabic.joining = vec!['\u{628}'];
    arabic.ligatures = vec![vec!['\u{644}', '\u{627}']];
    Context::new(collection(&[hebrew, arabic, latin], ahem_fallback()))
}

/// Ahem at 20px, so a character is a 20px square and a line 20px tall.
fn ahem() -> ComputedStyle<'static> {
    ComputedStyle {
        font: FontGroup {
            families: &FAMILIES,
            size: 20.0,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    }
}

/// Returns the initial Ahem style with `change` made to it.
fn styled(change: impl FnOnce(&mut ComputedStyle<'static>)) -> ComputedStyle<'static> {
    let mut style = ahem();
    change(&mut style);
    style
}

/// Returns a layout of `calls` in `block`, with an offset map, broken at `width`.
fn laid_with(
    block: &ComputedBlockStyle<'_>,
    width: f32,
    calls: impl FnOnce(&mut LayoutBuilder<'_>),
) -> Layout {
    let mut cx = context();
    let mut layout = Layout::new();
    let options = BuildOptions {
        map_source: true,
        ..BuildOptions::default()
    };
    let mut builder = layout.builder(NodeKey(0), block, options);
    calls(&mut builder);
    builder.finish(&mut cx);
    layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
    layout
}

/// Returns `text` as one Ahem text node, broken at `width`.
fn laid(text: &str, width: f32) -> Layout {
    laid_with(&ComputedBlockStyle::new(&ahem()), width, |b| {
        b.text(NodeKey(1), text)
    })
}

/// Returns the downstream caret at `at`, as its line and x.
fn caret(layout: &Layout, at: usize) -> (usize, f32) {
    position_caret(layout, Position::from(at))
}

/// Returns the caret at `position`, as its line and x.
fn position_caret(layout: &Layout, position: Position) -> (usize, f32) {
    let caret = layout.caret(position).unwrap();
    let InlineExtents { left: x, right: to } = caret.inline;
    assert_eq!(x, to, "a caret across its line");
    (caret.line, x)
}

/// Returns `position` snapped to a caret stop, with its cluster.
fn snapped(layout: &Layout, position: Position) -> place::ClusteredPosition {
    place::snap(layout, place::ClusteredPosition::new(layout, position))
}

/// Returns the cluster holding byte `at`.
fn cluster(layout: &Layout, at: usize) -> ClusterId {
    place::ClusteredPosition::new(layout, Position::from(at)).cluster
}

/// Returns whether a caret can stop at byte `at`: a cluster's start that is a caret stop.
fn is_stop(layout: &Layout, at: usize) -> bool {
    let cluster = cluster(layout, at);
    layout.analysis().clusters.start(cluster).get() == at && place::is_stop(layout, cluster)
}

/// Returns where a point at `x` along line `line`'s middle hits, as on Windows.
fn hit(layout: &Layout, x: f32, line: usize) -> Position {
    let metrics = layout.line(line).unwrap().metrics();
    let y = metrics.top + metrics.height() / 2.0;
    layout
        .hit_test(metrics.left + x, y, PastLines::Column)
        .unwrap()
}

/// Returns `selection` moved by `motion`.
fn moved(layout: &Layout, selection: Selection, motion: Motion) -> Selection {
    let mut moved = selection;
    moved.modify(layout, motion);
    moved
}

/// Returns the offsets a caret visits moving `direction` by `granularity` from `from`, until it stops.
fn walk(layout: &Layout, from: usize, motion: Motion) -> Vec<usize> {
    let mut at = Selection::from(Position::from(from));
    let mut visited = vec![from];
    loop {
        let before = at.focus().offset;
        at.modify(layout, motion);
        if at.focus().offset == before {
            return visited;
        }
        visited.push(at.focus().offset);
    }
}

/// Returns the way Chrome's right or left arrow goes from `position`.
///
/// Chrome's arrows go forward or backward by the paragraph's direction, even
/// in bidi text (probe 16g).
fn chromes_arrow(layout: &Layout, position: Position, right: bool) -> MotionDirection {
    let rtl = layout.paragraph_direction(position) == Direction::Rtl;
    if right != rtl {
        MotionDirection::Forward
    } else {
        MotionDirection::Backward
    }
}

fn rects(layout: &Layout, range: Range<usize>) -> Vec<(usize, (f32, f32), SelectionRectKind)> {
    layout
        .selection_rects(range)
        .map(|rect| (rect.line, (rect.inline.left, rect.inline.right), rect.kind))
        .collect()
}

fn copied(layout: &Layout, kind: CopyKind) -> String {
    layout
        .selected_text(0..layout.text().len(), kind)
        .to_string()
}

/// "I like to eat fried rice with a fried egg every morning."
const THAI: &str = "\u{E09}\u{E31}\u{E19}\u{E0A}\u{E2D}\u{E1A}\u{E01}\u{E34}\u{E19}\u{E02}\u{E49}\u{E32}\u{E27}\u{E1C}\u{E31}\u{E14}\u{E01}\u{E31}\u{E1A}\u{E44}\u{E02}\u{E48}\u{E14}\u{E32}\u{E27} \u{E17}\u{E38}\u{E01}\u{E27}\u{E31}\u{E19}\u{E15}\u{E2D}\u{E19}\u{E40}\u{E0A}\u{E49}\u{E32}";

/// "I write Japanese text on a computer": kanji, hiragana and katakana.
const JAPANESE: &str = "\u{79C1}\u{306F}\u{30B3}\u{30F3}\u{30D4}\u{30E5}\u{30FC}\u{30BF}\u{30FC}\u{3067}\u{65E5}\u{672C}\u{8A9E}\u{306E}\u{30C6}\u{30AD}\u{30B9}\u{30C8}\u{3092}\u{66F8}\u{304D}\u{307E}\u{3059}\u{3002}";

mod caret_motion;
mod carets;
// Steps are counted only in debug builds.
#[cfg(debug_assertions)]
mod cost;
mod hit;
mod offsets;
mod rects;
mod ruby;
mod security;
// Seeks are counted only in debug builds.
#[cfg(debug_assertions)]
mod seeks;
mod vertical;
mod word_motion;
