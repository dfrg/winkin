//! The oracle for the facts: it checks every fact row the writer hands a
//! node against an independent projection of the node's style.
//!
//! - As the writer takes each style, [`lowered`] derives each field again
//!   from the style, as a stage that read the style would. It checks the
//!   row whether the writer lowered it now or found it in the builder's
//!   memo. It also checks that the content's flags hold what the style
//!   says.
//! - When the build finishes, [`check`] holds what the writer gives a node
//!   beside its own styles to the content. A text node's and a break's
//!   facts are their container's. The block's initial letter is the one
//!   box that keeps `initial-letter`.
//!
//! It runs in debug builds, and the tests run it in release too, over
//! random content (`src/tests/facts.rs`). A stand-in, taken where a fact
//! table is full, is no projection, so it is not compared.

use super::facts::{
    BlockFacts, BoxFacts, BoxFactsId, BoxFlags, TextFacts, TextFactsId, TextFlags, TextLineHeight,
    TextSetting,
};
use super::memo::StyleKey;
use super::{Content, ContentFlags, NodeId, NodeKind};
use crate::data::{Id, IdRange};
use crate::style::{
    BaseDirection, BoxDecorationBreak, ComputedStyle, Direction, DominantBaseline,
    FirstLineVariant, FontVariantPosition, HangingPunctuation, Hyphens, LengthPercentage,
    LineHeight, OverflowWrap, Sides, TextBoxTrim, TextCombineUpright, TextJustify, TextOrientation,
    TextWrapMode, UnicodeBidi, VerticalAlign, WritingMode,
};
use crate::unit::{LayoutUnit, TextUnit};

/// Checks the facts the writer lowered `style` into for a node of `kind`.
///
/// `text` and `box_` are `None` where they are a stand-in. It checks them
/// against the style's projection (see the module documentation), and the
/// content's flags against what the style says. It panics on a mismatch,
/// which is a bug in the writer.
pub(crate) fn lowered(
    content: &Content,
    style: &StyleKey,
    kind: NodeKind,
    text: Option<TextFactsId>,
    box_: Option<BoxFactsId>,
) {
    let facts = &content.facts;
    let writing_mode = content.block.writing_mode;
    let mut wanted = ContentFlags::NONE;
    if let Some(text) = text {
        text_is(content, facts.text(text), style, writing_mode, kind);
        noted_text(&mut wanted, style);
    }
    if let Some(box_) = box_ {
        box_is(facts.box_facts(box_), style, kind, writing_mode);
        noted_box(&mut wanted, style, kind);
    }
    // Checks the flags the style says the content holds. `LINE_PADDING`,
    // `NONZERO_SPACING` and `AUTOSPACE` are left out: a row sets them only
    // where its value is not zero on its grid or reaches a seam, so they may
    // be missing. The others never are.
    let flags = content.flags;
    for flag in [
        ContentFlags::BOXES_WITH_EDGES,
        ContentFlags::CLONE_BOXES,
        ContentFlags::DECORATED_BOXES,
        ContentFlags::VERTICAL_ALIGN,
        ContentFlags::EMPHASIS,
        ContentFlags::INITIAL_LETTER,
        ContentFlags::HANGING_PUNCTUATION,
        ContentFlags::DOMINANT_BASELINE,
        ContentFlags::NOWRAP,
        ContentFlags::TRIMS_WRAPPED_START,
        ContentFlags::TRIMS_PUNCTUATION,
        ContentFlags::VARIANT_POSITION,
        ContentFlags::TEXT_JUSTIFY,
    ] {
        assert!(
            !wanted.contains(flag) || flags.contains(flag),
            "the content's flags {flags:?} lack {flag:?}"
        );
    }
}

/// Checks what the writer gave each node of `content` beside its own
/// styles (see the module documentation).
///
/// It panics on a mismatch, which is a bug in the writer.
pub(crate) fn check(content: &Content) {
    let facts = &content.facts;
    let nodes = &content.nodes;
    let mut reshapes = false;
    for (node, row) in nodes.nodes.iter() {
        // A text node's, a break's and an absolutely positioned box's text
        // are their container's.
        if matches!(
            row.kind,
            NodeKind::Text | NodeKind::LineBreak | NodeKind::Absolute
        ) {
            for variant in [FirstLineVariant::Standard, FirstLineVariant::FirstLine] {
                assert_eq!(
                    nodes.text_facts(row.parent, variant),
                    nodes.text_facts(node, variant),
                    "{node:?}'s text in {variant:?}"
                );
                assert_eq!(
                    nodes.box_facts(node, variant),
                    BoxFactsId::INITIAL,
                    "{node:?} is no box"
                );
            }
        }
        reshapes |= facts.text(row.text).shaping != facts.text(row.first_line_text).shaping;
    }
    let flags = content.flags;
    if reshapes && flags.contains(ContentFlags::FIRST_LINE_RESTYLE) {
        assert!(
            flags.contains(ContentFlags::FIRST_LINE_RESHAPES),
            "a node's first line shapes otherwise, unflagged"
        );
    }
    // The block's initial letter is the one inline box that keeps
    // `initial-letter`.
    let letter = (NodeId::new(1)..NodeId::new(nodes.len()))
        .ids()
        .find(|&node| {
            nodes.kind(node).is_some_and(NodeKind::is_inline_box)
                && facts
                    .box_facts(nodes.box_facts(node, FirstLineVariant::Standard))
                    .has(BoxFlags::INITIAL_LETTER)
        });
    assert_eq!(
        content.block.initial_letter, letter,
        "the block's initial letter"
    );
}

/// Notes in `flags` what a node of `kind` set in `style` says the content
/// holds as a box, read from the style directly.
fn noted_box(flags: &mut ContentFlags, style: &StyleKey, kind: NodeKind) {
    if kind.is_container() {
        if style.edges.any() || style.line.initial_letter.is_set() {
            flags.insert(ContentFlags::BOXES_WITH_EDGES);
        }
        if style.edges.decoration_break == BoxDecorationBreak::Clone {
            flags.insert(ContentFlags::CLONE_BOXES);
        }
        if style.decorates {
            flags.insert(ContentFlags::DECORATED_BOXES);
        }
    }
    if kind.reads_vertical_align() && !matches!(style.line.vertical_align, VerticalAlign::Baseline)
    {
        flags.insert(ContentFlags::VERTICAL_ALIGN);
    }
}

/// Notes in `flags` what text set in `style` says the content holds.
fn noted_text(flags: &mut ContentFlags, style: &StyleKey) {
    let mut set = |flag, on: bool| {
        if on {
            flags.insert(flag);
        }
    };
    set(ContentFlags::EMPHASIS, style.text.emphasis.marks);
    set(
        ContentFlags::INITIAL_LETTER,
        style.line.initial_letter.is_set(),
    );
    set(ContentFlags::LINE_PADDING, style.line.padding != 0.0);
    set(
        ContentFlags::HANGING_PUNCTUATION,
        style.text.hanging_punctuation != HangingPunctuation::NONE,
    );
    set(
        ContentFlags::NONZERO_SPACING,
        style.text.letter_spacing != 0.0 || style.text.word_spacing != LengthPercentage::ZERO,
    );
    set(
        ContentFlags::DOMINANT_BASELINE,
        style.line.dominant_baseline != DominantBaseline::Auto,
    );
    set(
        ContentFlags::NOWRAP,
        style.text.wrap_mode == TextWrapMode::NoWrap,
    );
    set(
        ContentFlags::TRIMS_WRAPPED_START,
        style.text.spacing_trim.trims_wrapped_start(),
    );
    set(
        ContentFlags::TRIMS_PUNCTUATION,
        style.text.spacing_trim.trims_punctuation(),
    );
    set(
        ContentFlags::TEXT_JUSTIFY,
        style.text.justify != TextJustify::Auto,
    );
    let autospace = style.text.autospace;
    set(
        ContentFlags::AUTOSPACE,
        autospace.ideograph_alpha || autospace.ideograph_numeric,
    );
    set(
        ContentFlags::VARIANT_POSITION,
        style.font.variant_position != FontVariantPosition::Normal,
    );
}

/// Checks `text`, and the shaping facts and font request it names, against
/// `style`'s projection, for a node of `at`.
fn text_is(
    content: &Content,
    text: &TextFacts,
    style: &StyleKey,
    writing_mode: WritingMode,
    at: NodeKind,
) {
    let facts = &content.facts;
    let shaping = facts.shaping(text.shaping);
    let request = facts.request(shaping.font);
    let text_group = &style.text;
    // Letter spacing on glyph geometry's grid.
    let letter = TextUnit::from_px(text_group.letter_spacing);
    // What font selection reads (`stages/fonts/mod.rs`, `features.rs`,
    // `lists.rs`).
    let upright = writing_mode.is_vertical_typographic()
        && style.orientation.text_orientation != TextOrientation::Sideways;
    assert!(
        request.font == style.font
            && request.font.families == style.font.families
            && request.font.features == style.font.features
            && request.font.variations == style.font.variations
            && request.language == style.text.language
            && request.initial_letter == style.line.initial_letter
            && request.upright == upright
            && request.spaced == (letter != TextUnit::from_raw(0))
            && request.trims_punctuation == text_group.spacing_trim.trims_punctuation(),
        "{at:?}: {request:?}"
    );
    // The shaping properties besides the request, which the shaping walk
    // compares by id, and the measurements' spacing
    // (`LetterWordSpacing::new`).
    assert!(
        shaping.letter == letter
            && shaping.word
                == TextUnit::from_px(text_group.word_spacing.resolve(style.font.computed_size()))
            && shaping.trim == text_group.spacing_trim
            && shaping.orientation == style.orientation,
        "{at:?}: {shaping:?}"
    );
    // What the analysis reads: an item's facts, how text stands, and the
    // bidi levels' upright test.
    let wraps = text_group.wrap_mode == TextWrapMode::Wrap;
    let vertical = matches!(
        writing_mode,
        WritingMode::VerticalRl | WritingMode::VerticalLr
    );
    let setting = match writing_mode {
        WritingMode::HorizontalTb => TextSetting::Horizontal,
        WritingMode::SidewaysRl | WritingMode::SidewaysLr => TextSetting::Sideways,
        WritingMode::VerticalRl | WritingMode::VerticalLr => {
            match style.orientation.text_orientation {
                TextOrientation::Mixed => TextSetting::Mixed,
                TextOrientation::Upright => TextSetting::Upright,
                TextOrientation::Sideways => TextSetting::Sideways,
            }
        }
    };
    let combine = if vertical {
        match style.orientation.text_combine_upright {
            TextCombineUpright::Digits(most) => TextCombineUpright::Digits(most.clamp(2, 4)),
            combine => combine,
        }
    } else {
        TextCombineUpright::None
    };
    let language = content.lists.languages.get(style.text.language);
    let flag = |flag: TextFlags| text.has(flag);
    assert!(
        text.line_break == text_group.line_break
            && text.word_break == text_group.word_break
            && text.collapse == text_group.white_space_collapse
            && text.setting == setting
            && text.combine == combine
            && flag(TextFlags::WRAPS) == wraps
            && flag(TextFlags::WHITE_SPACE_HANGS) == text_group.white_space_hangs()
            && flag(TextFlags::HYPHENS_NONE) == (text_group.hyphens == Hyphens::None)
            && flag(TextFlags::EMERGENCY)
                == (wraps && text_group.overflow_wrap != OverflowWrap::Normal)
            && flag(TextFlags::CJK_LINE_BREAKS) == matches!(language.language(), "ja" | "zh")
            && flag(TextFlags::UPRIGHT_BIDI)
                == (vertical
                    && (style.orientation.text_orientation == TextOrientation::Upright
                        || style.orientation.text_combine_upright == TextCombineUpright::All)),
        "{at:?}: the analysis facts of {text:?}"
    );
    // What font selection's generated text reads, and what the
    // measurements, the breaker, line layout and the readers read.
    let upright_in_vertical =
        vertical && style.orientation.text_orientation == TextOrientation::Upright;
    // `line-padding` truncated onto the grid.
    let padding = if style.line.padding.is_finite() && style.line.padding > 0.0 {
        LayoutUnit::from_px_truncated(style.line.padding)
    } else {
        LayoutUnit::ZERO
    };
    assert!(
        text.hyphen == style.text.hyphenate_character
            && text.padding == padding
            && text.hanging == text_group.hanging_punctuation
            && flag(TextFlags::HANGS_CONDITIONALLY) == text_group.white_space_hangs_conditionally()
            && flag(TextFlags::ANYWHERE) == (text_group.overflow_wrap == OverflowWrap::Anywhere)
            && flag(TextFlags::RTL) == (style.bidi.direction == Direction::Rtl)
            && flag(TextFlags::EMPHASIS) == text_group.emphasis.marks
            && text.emphasis_side == text_group.emphasis.position.side(writing_mode)
            && text.emphasis_skip == text_group.emphasis.skip
            && flag(TextFlags::AUTOSPACE_ALPHA)
                == (!upright_in_vertical && text_group.autospace.ideograph_alpha)
            && flag(TextFlags::AUTOSPACE_NUMERIC)
                == (!upright_in_vertical && text_group.autospace.ideograph_numeric)
            && text.justify == text_group.justify
            && text.dominant == style.line.dominant_baseline
            && same_tab_size(text, style),
        "{at:?}: the measure facts of {text:?}"
    );
    line_height_is(text.line_height, style, at);
}

/// Whether `text`'s `tab-size` is `style`'s, by its bits.
fn same_tab_size(text: &TextFacts, style: &StyleKey) -> bool {
    text.tab_size == style.text.tab_size
}

/// Checks `height` against `style`'s line height, for a node of `at`.
///
/// It compares two readings. The measurements round a length onto the grid
/// (`heights.rs`' `line_height`). The initial letter reads the height
/// through `TextLineHeight::px`, as Chrome's `ComputedLineHeight` gives it:
/// a length as it is, and `normal` as the primary font's line spacing.
fn line_height_is(height: TextLineHeight, style: &StyleKey, at: NodeKind) {
    let measured = match style.line.height {
        LineHeight::Normal => None,
        LineHeight::Factor(factor) => {
            let size = LayoutUnit::from_px_truncated(style.font.computed_size());
            Some(LayoutUnit::from_px_truncated(size.to_px() * factor))
        }
        LineHeight::Px(px) => Some(LayoutUnit::from_px(px)),
    };
    let lowered = match height {
        TextLineHeight::Normal => None,
        TextLineHeight::Factor(height) => Some(height),
        TextLineHeight::Length(px) => Some(LayoutUnit::from_px(px)),
    };
    let normal = LayoutUnit::from_px(18.5);
    assert!(
        measured == lowered
            && height.px(normal).to_bits() == computed_line_height(style, normal).to_bits(),
        "{at:?}: {height:?}"
    );
}

/// Returns `style`'s line height in pixels, read from the style directly.
///
/// `normal` gives `normal`, and a factor is taken of the computed size
/// truncated onto the grid. A length stays as it is, or zero where it is
/// not finite.
fn computed_line_height(style: &StyleKey, normal: LayoutUnit) -> f32 {
    match style.line.height {
        LineHeight::Normal => normal.to_px(),
        LineHeight::Factor(factor) => {
            let size = LayoutUnit::from_px_truncated(style.font.computed_size());
            LayoutUnit::from_px_truncated(size.to_px() * factor).to_px()
        }
        LineHeight::Px(px) if px.is_finite() => px,
        LineHeight::Px(_) => 0.0,
    }
}

/// Checks `facts` against what a node of `kind` set in `style` is as a box.
fn box_is(facts: &BoxFacts, style: &StyleKey, kind: NodeKind, writing_mode: WritingMode) {
    let at = kind;
    let align = if matches!(
        kind,
        NodeKind::Box | NodeKind::Atomic | NodeKind::FirstLetter
    ) {
        style.line.vertical_align
    } else {
        VerticalAlign::Baseline
    };
    if !matches!(
        kind,
        NodeKind::Box | NodeKind::Ruby | NodeKind::Annotation | NodeKind::FirstLetter
    ) {
        assert!(
            *facts
                == BoxFacts {
                    align,
                    ..BoxFacts::INITIAL
                },
            "{at:?}: {kind:?} is no container, yet {facts:?}"
        );
        return;
    }
    let edges = &style.edges;
    let across = |sides: Sides<f32>| sides.across_line_on_grid(writing_mode);
    let (padding, border) = (across(edges.padding), across(edges.border));
    let flag = |flag: BoxFlags| facts.has(flag);
    assert!(
        // `logical_edges`, `rooms`, `BoxFragment::edges`.
        facts.room == edges.inline(writing_mode)
            // `rooms` and the culled box's reach.
            && facts.margin_line == edges.margin.along_line_on_grid(writing_mode)
            // The initial letter's room.
            && facts.margin_across == edges.margin.across_line_on_grid(writing_mode)
            // `KeptBoxes`.
            && facts.across == (padding.0 + border.0, padding.1 + border.1)
            && facts.align == align
            && facts.bidi == style.bidi.unicode_bidi
            && facts.ruby == style.ruby
            && flag(BoxFlags::PAINTS) == style.paints
            && flag(BoxFlags::DECORATES) == style.decorates
            && flag(BoxFlags::HAS_EDGES) == edges.any()
            && flag(BoxFlags::CLONES) == (edges.decoration_break == BoxDecorationBreak::Clone)
            && flag(BoxFlags::RTL) == (style.bidi.direction == Direction::Rtl)
            && flag(BoxFlags::TRIMS_TEXT_BOX) == (style.line.text_box_trim != TextBoxTrim::None)
            && flag(BoxFlags::INITIAL_LETTER) == style.line.initial_letter.is_set()
            && facts.breaks_shaping(true) == edge_breaks_shaping(style, writing_mode, true)
            && facts.breaks_shaping(false) == edge_breaks_shaping(style, writing_mode, false),
        "{at:?}: {facts:?}"
    );
}

/// Whether a box's opening (`opens`) or closing edge stops shaping, read
/// from the style directly.
fn edge_breaks_shaping(style: &StyleKey, writing_mode: WritingMode, opens: bool) -> bool {
    if !matches!(style.line.vertical_align, VerticalAlign::Baseline)
        || style.bidi.unicode_bidi != UnicodeBidi::Normal
    {
        return true;
    }
    let side = |sides: Sides<f32>| {
        let (left, right) = sides.along_line(writing_mode);
        let (start, end) = style.bidi.direction.line_order(left, right);
        if opens { start } else { end }
    };
    let edges = &style.edges;
    side(edges.margin) != 0.0 || side(edges.border) != 0.0 || side(edges.padding) != 0.0
}

/// Checks `block`, the block's facts, against `root`, its own style.
///
/// It checks the base direction and override that bidi reads, the baseline
/// that font selection reads, and the Chinese rule of `text-autospace`. The
/// initial letter is checked when the build finishes (see [`check`]).
pub(super) fn block_is(block: &BlockFacts, root: &ComputedStyle<'_>) {
    if root.bidi.unicode_bidi == UnicodeBidi::Plaintext {
        assert_eq!(block.direction, BaseDirection::Auto);
    }
    let overrides = matches!(
        root.bidi.unicode_bidi,
        UnicodeBidi::BidiOverride | UnicodeBidi::IsolateOverride
    )
    .then_some(root.bidi.direction);
    assert_eq!(block.overrides, overrides, "the block's override");
    let central = block.writing_mode.is_vertical_typographic()
        && root.orientation.text_orientation != TextOrientation::Sideways;
    assert_eq!(block.central_baseline, central, "the block's baseline");
    let chinese = root
        .text
        .language
        .as_ref()
        .is_some_and(|language| super::facts::is_chinese(language.language()));
    assert_eq!(block.chinese, chinese, "the block's language");
}
