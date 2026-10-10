//! Style fact tests, over random content. They pin:
//! - every record the builder hands a node as its projection of the style the
//!   caller gave, whether lowered then or found in the builder's memo;
//! - the content's flags and the block's facts as the styles say them, the
//!   check every debug build runs (`content::facts_check::check`), here in
//!   release too;
//! - what the writer gives a node beside its styles, as the content says
//!   (`content::check`, at the build's finish).
//!
//! The content is drawn as `fast.rs` and `segments.rs` draw it, in styles that
//! set what the builder lowers: every writing mode and orientation, both
//! directions and every `unicode-bidi`, edges and `vertical-align` of every
//! kind, line heights, spacing, emphasis, `hyphenate-character`,
//! `text-combine-upright`, ruby, `::first-letter` with and without
//! `initial-letter`, and `::first-line` styles.

use alloc::vec;
use alloc::vec::Vec;

use super::fast::{RARE, Rng, WORDS, latin_1, roots};
use super::{Fixture, StageCheck, han, han_fallback};
use crate::stages::content::{ContentFlags, check};
use crate::style::{
    BoxDecorationBreak, ComputedStyle, Direction, DominantBaseline, EmphasisPosition, EmphasisSide,
    EmphasisSkip, EmphasisVerticalSide, FontGroup, FontVariantPosition, HangEnd,
    HangingPunctuation, InitialLetter, InitialLetterAlign, Language, LengthPercentage, LineHeight,
    RubyPosition, TextAutospace, TextBoxTrim, TextCase, TextCombineUpright, TextEmphasis,
    TextOrientation, TextSpacingTrim, UnicodeBidi, VerticalAlign, WritingMode,
};
use crate::{
    BoxSize, ComputedBlockStyle, FloatSide, Layout, LayoutBuilder, NodeKey, OriginalDisplay,
};

/// Styles that set, each, something the builder lowers.
fn styles(root: &ComputedStyle<'static>) -> Vec<ComputedStyle<'static>> {
    let mut out = vec![*root];
    let mut rtl = *root;
    rtl.bidi.direction = Direction::Rtl;
    out.push(rtl);
    for unicode_bidi in [
        UnicodeBidi::Embed,
        UnicodeBidi::Isolate,
        UnicodeBidi::BidiOverride,
        UnicodeBidi::IsolateOverride,
        UnicodeBidi::Plaintext,
    ] {
        let mut style = rtl;
        style.bidi.unicode_bidi = unicode_bidi;
        out.push(style);
    }
    let mut edged = rtl;
    edged.edges.margin.left = LengthPercentage::from_px(3.7);
    edged.edges.border.top = 1.0;
    edged.edges.padding.right = LengthPercentage::from_px(2.33);
    edged.edges.padding.bottom = LengthPercentage::from_px(1.5);
    edged.edges.decoration_break = BoxDecorationBreak::Clone;
    edged.decorates = true;
    out.push(edged);
    let mut painted = *root;
    painted.paints = true;
    painted.line.text_box_trim = TextBoxTrim::TrimBoth;
    out.push(painted);
    for align in [
        VerticalAlign::Super,
        VerticalAlign::Middle,
        VerticalAlign::Px(3.3),
        VerticalAlign::Fraction(0.25),
    ] {
        let mut style = *root;
        style.line.vertical_align = align;
        out.push(style);
    }
    for height in [
        LineHeight::Factor(1.37),
        LineHeight::Px(19.3),
        LineHeight::Px(f32::INFINITY),
    ] {
        let mut style = *root;
        style.line.height = height;
        out.push(style);
    }
    let mut spaced = *root;
    spaced.text.letter_spacing = 0.000_001;
    spaced.text.word_spacing = LengthPercentage {
        px: 1.1,
        fraction: 0.13,
    };
    spaced.text.spacing_trim = TextSpacingTrim::SpaceAll;
    out.push(spaced);
    let mut marked = *root;
    marked.text.emphasis = TextEmphasis {
        marks: true,
        position: EmphasisPosition {
            side: EmphasisSide::Under,
            vertical_side: EmphasisVerticalSide::Left,
        },
        skip: EmphasisSkip::SPACES,
    };
    marked.line.dominant_baseline = DominantBaseline::Central;
    marked.line.padding = 2.9;
    marked.text.hanging_punctuation = HangingPunctuation {
        first: true,
        last: false,
        end: HangEnd::Force,
    };
    out.push(marked);
    let mut upright = *root;
    upright.orientation.text_orientation = TextOrientation::Upright;
    upright.text.autospace = TextAutospace::NORMAL;
    out.push(upright);
    let mut combined = *root;
    combined.orientation.text_combine_upright = TextCombineUpright::Digits(9);
    out.push(combined);
    let mut all = *root;
    all.orientation.text_combine_upright = TextCombineUpright::All;
    all.orientation.text_orientation = TextOrientation::Sideways;
    out.push(all);
    let mut chinese = *root;
    chinese.text.language = Language::parse("zh-Hant").ok();
    chinese.text.hyphenate_character = Some("=");
    chinese.font.variant_position = FontVariantPosition::Super;
    out.push(chinese);
    let mut under = *root;
    under.ruby.position = RubyPosition::Under;
    out.push(under);
    let mut larger = *root;
    larger.font = FontGroup {
        size: 23.7,
        ..root.font
    };
    out.push(larger);
    out
}

/// The block styles: a writing mode each, now and then a first line
/// restyled, and the root's own bidi.
fn block<'a>(
    root: &'a ComputedStyle<'a>,
    first: &'a ComputedStyle<'a>,
    rng: &mut Rng,
) -> ComputedBlockStyle<'a> {
    let mut block = super::fast::blocks(root, first, rng);
    block.writing_mode = [
        WritingMode::HorizontalTb,
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
        WritingMode::SidewaysRl,
        WritingMode::SidewaysLr,
    ][rng.below(5)];
    if rng.chance(30) {
        block.first_line = Some(first);
    }
    block
}

/// One case's content: words, spans given first-line styles now and then,
/// atomic inlines, floats, ruby, forced breaks and a first letter.
fn content(rng: &mut Rng, b: &mut LayoutBuilder<'_>, styles: &[ComputedStyle<'_>]) {
    let mut n = 1u64;
    let mut next = || {
        n += 1;
        NodeKey(n)
    };
    let pick = |rng: &mut Rng| &styles[rng.below(styles.len())];
    if rng.chance(30) {
        let mut letter = *pick(rng);
        if rng.chance(50) {
            letter.line.initial_letter = InitialLetter {
                size: 2.0,
                sink: 2,
                align: InitialLetterAlign::Alphabetic,
            };
        }
        let first = rng.chance(50).then(|| *pick(rng));
        b.set_first_letter(next(), &letter, first.as_ref());
        if rng.chance(30) {
            b.text(next(), "“");
        }
    }
    let mut open = 0;
    for _ in 0..3 + rng.below(30) {
        let word = if rng.chance(25) {
            RARE[rng.below(RARE.len())]
        } else {
            WORDS[rng.below(WORDS.len())]
        };
        match rng.below(100) {
            0..50 => {
                b.text(next(), word);
                b.text(next(), " ");
            }
            50..62 if open < 3 => {
                let first = rng.chance(40).then(|| *pick(rng));
                b.open_box(next(), pick(rng), first.as_ref());
                open += 1;
            }
            62..70 if open > 0 => {
                b.close_box();
                open -= 1;
            }
            70..74 => b.line_break(next()),
            74..80 => b.atomic(
                next(),
                pick(rng),
                None,
                BoxSize {
                    inline: 12.3,
                    block: 10.0,
                    baseline: None,
                },
            ),
            80..82 => b.absolute(next(), OriginalDisplay::Inline),
            82..83 => b.float(
                next(),
                pick(rng),
                FloatSide::Right,
                BoxSize {
                    inline: 20.6,
                    block: 8.1,
                    baseline: None,
                },
            ),
            _ => {
                b.open_ruby(next(), pick(rng), None);
                b.text(next(), word);
                b.open_annotation(next(), pick(rng), None);
                b.text(next(), word);
                b.close_annotation();
                b.close_ruby();
            }
        }
    }
    for _ in 0..open {
        b.close_box();
    }
}

/// Every record a node names is its projection of the node's style, over
/// random content: the check holds each in the builder as the build runs,
/// and what the writer gives each node at its finish.
#[test]
fn every_record_is_its_projection_of_the_nodes_style() {
    let mut fixture = Fixture::new(
        &[latin_1(), han()],
        han_fallback("Test Han"),
        StageCheck::Built(|_, layout| check(layout.content())),
    );
    let roots = roots();
    let mut layout = Layout::new();
    for case in 0..300u64 {
        let mut rng = Rng(0x51_7cc1_b727_220a ^ (case + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let mut root = roots[rng.below(roots.len())];
        if rng.chance(30) {
            root.bidi.direction = Direction::Rtl;
            root.bidi.unicode_bidi = [
                UnicodeBidi::Normal,
                UnicodeBidi::Plaintext,
                UnicodeBidi::BidiOverride,
                UnicodeBidi::IsolateOverride,
            ][rng.below(4)];
        }
        if rng.chance(20) {
            root.text.language = Language::parse("ja").ok();
            root.orientation.text_orientation = TextOrientation::Upright;
        }
        let styles = styles(&root);
        let first = styles[rng.below(styles.len())];
        let block = block(&root, &first, &mut rng);
        fixture.build(&mut layout, &block, |b| content(&mut rng, b, &styles));
        // Built again into the same layout: the tables cleared and kept.
        check(layout.content());
    }
}

/// Styles that differ only in what a box is share their text facts, and a
/// first-line style that differs only in its transform shapes as its own.
#[test]
fn text_facts_are_shared_where_only_a_box_differs() {
    let mut fixture = Fixture::new(
        &[latin_1()],
        han_fallback("Test Han"),
        StageCheck::Placed(|_| {}),
    );
    let roots = roots();
    let root = roots[0];
    let mut edged = root;
    edged.edges.padding.left = LengthPercentage::from_px(4.0);
    edged.decorates = true;
    let mut transformed = root;
    transformed.text.transform.case = TextCase::Uppercase;
    let block = ComputedBlockStyle {
        first_line: Some(&transformed),
        ..ComputedBlockStyle::new(&root)
    };
    let mut layout = Layout::new();
    fixture.build(&mut layout, &block, |b| {
        b.text(NodeKey(1), "a ");
        b.open_box(NodeKey(2), &edged, None);
        b.text(NodeKey(3), "b");
        b.close_box();
    });
    let content = layout.content();
    assert_eq!(content.facts.text_count(), 1, "one text facts row");
    assert_eq!(content.facts.box_count(), 2, "the initial box's, the box's");
    let flags = content.flags;
    assert!(flags.contains(ContentFlags::FIRST_LINE_RESTYLE));
    assert!(!flags.contains(ContentFlags::FIRST_LINE_RESHAPES));
    check(content);
}
