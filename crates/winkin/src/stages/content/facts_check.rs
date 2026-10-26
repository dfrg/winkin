//! The debug oracle for the facts: it checks each fact row the writer gives
//! a node against an independent projection of the node's style.
//!
//! - [`lowered`] runs as the writer takes each style. It derives each field
//!   again from the style, as a stage reading the style would, whether the
//!   row is new or from the memo. It also checks the content's flags.
//! - [`check`] runs when the build finishes. It checks what a node gets
//!   beyond its own style: a text node, break or absolutely positioned box
//!   takes its container's text facts, and the block's initial letter is
//!   the one box that keeps `initial-letter`.
//!
//! It runs in debug builds, and in release under the random-content tests
//! (`src/tests/facts.rs`). A stand-in row, used where a fact table is full,
//! is not compared.

use super::facts::{
    BlockFacts, BoxFacts, BoxFactsId, BoxFlags, TextFacts, TextFactsId, TextFlags, TextLineHeight,
    TextSetting,
};
use super::memo::StyleKey;
use super::{Content, ContentFlags, NodeId, NodeKind};
use crate::data::{Id, IdRange};
use crate::style::{
    BaseDirection, BoxDecorationBreak, CalcResolver, ComputedStyle, Direction, DominantBaseline,
    FirstLineVariant, FontVariantPosition, HangingPunctuation, Hyphens, LengthPercentage,
    LengthPercentageOrCalc, LineHeight, OverflowWrap, Sides, TextBoxEdge, TextBoxTrim,
    TextCombineUpright, TextDecorationSkipSpaces, TextEdgeOver, TextEdgeUnder, TextJustify,
    TextOrientation, TextWrapMode, UnicodeBidi, VerticalAlign, WordBreak, WritingMode,
};
use crate::unit::{LayoutUnit, TextUnit};

/// Checks the facts the writer lowered `style` into for a node of `kind`,
/// and the content's flags.
///
/// `text` and `box_` are `None` where they are stand-ins. Panics on a
/// mismatch, which is a writer bug.
pub(crate) fn lowered(
    content: &Content,
    calc: Option<&dyn CalcResolver>,
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
        box_is(
            facts.box_facts(box_),
            style,
            kind,
            writing_mode,
            content.basis(),
            calc,
        );
        noted_box(&mut wanted, style, kind);
    }
    // `LINE_PADDING`, `NONZERO_SPACING` and `AUTOSPACE` are left out: a row
    // sets them only where its value is nonzero on its grid or applies at
    // some boundary, so they may be missing. The others never are.
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
        ContentFlags::SKIPS_ALL_SPACES,
        ContentFlags::HOSTED_BREAKS,
    ] {
        assert!(
            !wanted.contains(flag) || flags.contains(flag),
            "the content's flags {flags:?} lack {flag:?}"
        );
    }
}

/// Checks what the writer gave each node of `content` beyond its own style
/// (see the module documentation). Panics on a mismatch, which is a writer
/// bug.
pub(crate) fn check(content: &Content) {
    let facts = &content.facts;
    let nodes = &content.nodes;
    let mut reshapes = false;
    for (node, row) in nodes.nodes.iter() {
        // A text node, a break and an absolutely positioned box take their
        // container's text facts.
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
            flags.contains(ContentFlags::FIRST_LINE_SHAPING),
            "a node's first line shapes otherwise, unflagged"
        );
    }
    // The block's initial letter is the only inline box that keeps
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

/// Adds to `flags` the box flags a node of `kind` in `style` implies, read
/// from the style directly.
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

/// Sets in `flags` the text flags `style` implies.
fn noted_text(flags: &mut ContentFlags, style: &StyleKey) {
    flags.set(ContentFlags::EMPHASIS, style.text.emphasis.marks);
    flags.set(
        ContentFlags::INITIAL_LETTER,
        style.line.initial_letter.is_set(),
    );
    flags.set(ContentFlags::LINE_PADDING, style.line.padding != 0.0);
    flags.set(
        ContentFlags::HANGING_PUNCTUATION,
        style.text.hanging_punctuation != HangingPunctuation::NONE,
    );
    flags.set(
        ContentFlags::NONZERO_SPACING,
        style.text.letter_spacing != LengthPercentage::ZERO
            || style.text.word_spacing != LengthPercentage::ZERO,
    );
    flags.set(
        ContentFlags::DOMINANT_BASELINE,
        style.line.dominant_baseline != DominantBaseline::Auto,
    );
    flags.set(
        ContentFlags::NOWRAP,
        style.text.wrap_mode == TextWrapMode::NoWrap,
    );
    flags.set(
        ContentFlags::TRIMS_WRAPPED_START,
        style.text.spacing_trim.trims_wrapped_start(),
    );
    flags.set(
        ContentFlags::TRIMS_PUNCTUATION,
        style.text.spacing_trim.trims_punctuation(),
    );
    flags.set(
        ContentFlags::TEXT_JUSTIFY,
        style.text.justify != TextJustify::Auto,
    );
    let autospace = style.text.autospace;
    flags.set(
        ContentFlags::AUTOSPACE,
        autospace.ideograph_alpha || autospace.ideograph_numeric,
    );
    flags.set(
        ContentFlags::SKIPS_ALL_SPACES,
        style.text.decoration_skip_spaces == TextDecorationSkipSpaces::All,
    );
    flags.set(
        ContentFlags::VARIANT_POSITION,
        style.font.variant_position != FontVariantPosition::Normal,
    );
    flags.set(
        ContentFlags::HOSTED_BREAKS,
        style.text.hyphens == Hyphens::Auto || style.text.word_break == WordBreak::AutoPhrase,
    );
}

/// Checks `text`, and the shaping facts and font request it names, against
/// `style`'s projection, for a node of kind `at`.
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
    // Letter spacing in `TextUnit`.
    let letter = TextUnit::from_px(
        text_group
            .letter_spacing
            .resolve(style.font.computed_size()),
    );
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
    // The shaping properties beyond the request, which the shaping walk
    // compares by id, and measuring's spacing (`LetterWordSpacing::new`).
    assert!(
        shaping.letter == letter
            && shaping.word
                == TextUnit::from_px(text_group.word_spacing.resolve(style.font.computed_size()))
            && shaping.trim == text_group.spacing_trim
            && shaping.orientation == style.orientation,
        "{at:?}: {shaping:?}"
    );
    // What analysis reads: an item's facts, text orientation, and the bidi
    // levels' upright test.
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
    let language = content.languages.get(style.text.language);
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
            && flag(TextFlags::HYPHENS_AUTO) == (text_group.hyphens == Hyphens::Auto)
            && flag(TextFlags::EMERGENCY)
                == (wraps && text_group.overflow_wrap != OverflowWrap::Normal)
            && flag(TextFlags::CJK_LINE_BREAKS)
                == super::facts::breaks_cj(text_group.line_break, language.language())
            && flag(TextFlags::UPRIGHT_BIDI)
                == (vertical
                    && (style.orientation.text_orientation == TextOrientation::Upright
                        || style.orientation.text_combine_upright == TextCombineUpright::All)),
        "{at:?}: the analysis facts of {text:?}"
    );
    // What font selection's generated text, measuring, the breaker, line
    // layout and the readers read.
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
            && flag(TextFlags::SKIPS_START_SPACES)
                == text_group.decoration_skip_spaces.skips_start()
            && flag(TextFlags::SKIPS_END_SPACES) == text_group.decoration_skip_spaces.skips_end()
            && flag(TextFlags::SKIPS_ALL_SPACES)
                == (text_group.decoration_skip_spaces == TextDecorationSkipSpaces::All)
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

/// Whether `text`'s `tab-size` equals `style`'s, by bits.
fn same_tab_size(text: &TextFacts, style: &StyleKey) -> bool {
    text.tab_size == style.text.tab_size
}

/// Checks `height` against `style`'s line height, for a node of kind `at`.
///
/// It compares two readings. Measuring rounds a length onto the grid
/// (`line_height` in `stages/measure/metrics.rs`). The initial letter reads
/// the height through `TextLineHeight::px`, as Blink's
/// `ComputedLineHeight` gives it: a length as is, and `normal` as the
/// primary font's line spacing.
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
/// `normal` returns the `normal` argument. A factor multiplies the computed
/// size truncated onto the grid. A length stays as is, or zero where it is
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

/// Checks `facts` against the box a node of `kind` in `style` is.
fn box_is(
    facts: &BoxFacts,
    style: &StyleKey,
    kind: NodeKind,
    writing_mode: WritingMode,
    basis: f32,
    calc: Option<&dyn CalcResolver>,
) {
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
    let edges = &style.edges.used(basis, calc);
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
            && flag(BoxFlags::OUTLINES) == style.outlines
            && flag(BoxFlags::DECORATES) == style.decorates
            && flag(BoxFlags::HAS_EDGES) == style.edges.any()
            && flag(BoxFlags::CLONES)
                == (style.edges.decoration_break == BoxDecorationBreak::Clone)
            && flag(BoxFlags::RTL) == (style.bidi.direction == Direction::Rtl)
            && trims_text_box(facts.text_box(), style.line.text_box_trim, style.line.text_box_edge)
            && flag(BoxFlags::INITIAL_LETTER) == style.line.initial_letter.is_set()
            && facts.breaks_shaping(true) == edge_breaks_shaping(style, writing_mode, true)
            && facts.breaks_shaping(false) == edge_breaks_shaping(style, writing_mode, false),
        "{at:?}: {facts:?}"
    );
}

/// Whether a box's trims and edges, `(over, under, edge)`, match `trim`
/// and `edge`, read from the style directly.
fn trims_text_box(facts: (bool, bool, TextBoxEdge), trim: TextBoxTrim, edge: TextBoxEdge) -> bool {
    let (over, under) = trim.sides();
    if !over && !under {
        return facts == (false, false, TextBoxEdge::AUTO);
    }
    let kept = TextBoxEdge {
        over: match edge.over {
            TextEdgeOver::IdeographicInk => TextEdgeOver::Ideographic,
            other => other,
        },
        under: match edge.under {
            TextEdgeUnder::IdeographicInk => TextEdgeUnder::Ideographic,
            other => other,
        },
    };
    facts == (over, under, kept)
}

/// Whether a box's opening edge (if `opens`) or closing edge stops shaping,
/// read from the style directly.
fn edge_breaks_shaping(style: &StyleKey, writing_mode: WritingMode, opens: bool) -> bool {
    if !matches!(style.line.vertical_align, VerticalAlign::Baseline)
        || style.bidi.unicode_bidi != UnicodeBidi::Normal
    {
        return true;
    }
    let side = |sides: Sides<bool>| {
        let (left, right) = sides.along_line(writing_mode);
        let (start, end) = style.bidi.direction.line_order(left, right);
        if opens { start } else { end }
    };
    let set = |sides: Sides<LengthPercentageOrCalc>| Sides {
        top: !sides.top.is_zero(),
        right: !sides.right.is_zero(),
        bottom: !sides.bottom.is_zero(),
        left: !sides.left.is_zero(),
    };
    let edges = &style.edges;
    let border = Sides {
        top: edges.border.top != 0.0,
        right: edges.border.right != 0.0,
        bottom: edges.border.bottom != 0.0,
        left: edges.border.left != 0.0,
    };
    side(set(edges.margin)) || side(border) || side(set(edges.padding))
}

/// Checks `block`, the block's facts, against `root`, its style.
///
/// It checks the base direction and override bidi reads, the baseline font
/// selection reads, and the Chinese rule of `text-autospace`. [`check`]
/// checks the initial letter when the build finishes.
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
