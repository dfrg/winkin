//! Spacing tests. They pin:
//! - letter- and word-spacing in the prefix, per cluster as in Chrome;
//! - `NONMONOTONE` from negative spacing;
//! - `text-indent`, line padding and hanging punctuation as costs;
//! - spacing of any size measured without panicking.

use super::*;
use crate::stages::content::FixedSpacing;
use crate::style::FirstLineVariant;

/// Returns the spacing of the block's text in `layout` under rule `words`.
fn root_spacing(layout: &Layout, words: super::WordSpacingRule) -> LetterWordSpacing {
    let content = layout.content();
    let text = content
        .nodes
        .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
    let facts = &content.facts;
    LetterWordSpacing::from_shaping(facts, facts.text(text).shaping, words)
}

/// The prefix adds `letter-spacing` after every cluster that draws, the last
/// included, and `word-spacing` after each separator. Unspaced text is
/// untouched.
#[test]
fn spacing_follows_each_cluster_in_the_prefix() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&AHEM_FAMILY, 10.0);
    fixture.text(&mut layout, &style, "XX XX");
    assert_eq!(positions(&layout), [0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    style.text.letter_spacing = LengthPercentage::from_px(2.0);
    style.text.word_spacing = LengthPercentage {
        px: 3.0,
        fraction: 0.0,
    };
    fixture.text(&mut layout, &style, "XX XX");
    assert_eq!(positions(&layout), [0.0, 12.0, 24.0, 39.0, 51.0, 63.0]);
    style.text.word_spacing = LengthPercentage {
        px: 1.0,
        fraction: 0.5,
    };
    fixture.text(&mut layout, &style, "XX XX");
    assert_eq!(positions(&layout), [0.0, 12.0, 24.0, 42.0, 54.0, 66.0]);
    // A forced break, a tab and an atomic inline take none.
    let mut pre = style;
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    pre.text.word_spacing = LengthPercentage::ZERO;
    fixture.build(&mut layout, &ComputedBlockStyle::new(&pre), |b| {
        b.text(NodeKey(1), "X\tX");
        let size = BoxSize {
            inline: 7.0,
            block: 10.0,
            baseline: None,
        };
        b.atomic(NodeKey(2), &pre, None, size);
        b.line_break(NodeKey(3));
        b.text(NodeKey(4), "X");
    });
    assert_eq!(
        positions(&layout),
        [0.0, 12.0, 12.0, 24.0, 31.0, 31.0, 43.0]
    );
}

/// A `word-spacing` percentage resolves against the style's computed font
/// size, not the space's advance (CSS Text 4; Chrome 153's
/// `FontDescription::WordSpacing`): `50%` equals `0.5em`. Test Latin's
/// space is half an em, so at 20 px `50%` adds 10 px, not 5. A 40 px span
/// inheriting the percentage adds 20 px.
#[test]
fn a_word_spacing_percentage_is_of_the_font_size() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&LATIN, 20.0);
    style.text.letter_spacing = LengthPercentage::from_px(2.0);
    style.text.word_spacing = LengthPercentage {
        px: 0.0,
        fraction: 0.5,
    };
    fixture.text(&mut layout, &style, "ab cd");
    // Letters 10 + 2 px; the space 10 + 2 + 10 px.
    assert_eq!(positions(&layout), [0.0, 12.0, 24.0, 46.0, 58.0, 70.0]);
    // `calc(3px + 50%)`, and `0.5em`, resolve the percentage alike.
    for word in [
        LengthPercentage {
            px: 3.0,
            fraction: 0.5,
        },
        LengthPercentage {
            px: 10.0,
            fraction: 0.0,
        },
    ] {
        style.text.word_spacing = word;
        fixture.text(&mut layout, &style, "ab cd");
        let extra = if word.px == 3.0 { 3.0 } else { 0.0 };
        assert_eq!(
            positions(&layout),
            [0.0, 12.0, 24.0, 46.0 + extra, 58.0 + extra, 70.0 + extra],
            "{word:?}"
        );
    }
    // A span at twice the size, same percentages.
    style.text.word_spacing = LengthPercentage {
        px: 0.0,
        fraction: 0.5,
    };
    let big = ComputedStyle {
        font: FontGroup {
            size: 40.0,
            ..style.font
        },
        ..style
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "a ");
        b.open_box(NodeKey(2), &big, None);
        b.text(NodeKey(3), "b c");
        b.close_box();
    });
    // `a` 12, its space 22; `b` 20 and 2, its space 20, 2 and 20; `c`.
    assert_eq!(positions(&layout), [0.0, 12.0, 34.0, 56.0, 98.0, 120.0]);
}

/// `letter-spacing: 50%` adds half the font size: 10 px at 20 px, 20 px in
/// an inheriting 40 px span. Only a pure length is fixed for `text-fit`.
#[test]
fn a_letter_spacing_percentage_is_of_the_font_size() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&AHEM_FAMILY, 20.0);
    style.text.letter_spacing = LengthPercentage {
        px: 0.0,
        fraction: 0.5,
    };
    let big = ComputedStyle {
        font: FontGroup {
            size: 40.0,
            ..style.font
        },
        ..style
    };
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.text(NodeKey(1), "AA");
        b.open_box(NodeKey(2), &big, None);
        b.text(NodeKey(3), "A");
        b.close_box();
    });
    assert_eq!(positions(&layout), [0.0, 30.0, 60.0, 120.0]);
    let mut fixed = |letter: LengthPercentage, word: LengthPercentage| {
        style.text.letter_spacing = letter;
        style.text.word_spacing = word;
        fixture.text(&mut layout, &style, "A A");
        let content = layout.content();
        let facts = &content.facts;
        let text = content
            .nodes
            .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
        facts.shaping(facts.text(text).shaping).fixed
    };
    let px = LengthPercentage::from_px(2.0);
    let percent = LengthPercentage {
        px: 0.0,
        fraction: 0.1,
    };
    let both = LengthPercentage {
        px: 2.0,
        fraction: 0.1,
    };
    let none = LengthPercentage::ZERO;
    assert_eq!(fixed(px, none), FixedSpacing::LETTER);
    assert_eq!(fixed(none, px), FixedSpacing::WORD);
    assert_eq!(fixed(percent, both), FixedSpacing::NONE);
    assert_eq!(fixed(none, none), FixedSpacing::NONE);
}

/// [`LetterWordSpacing::after`], shared by the prefix and reshaped edges,
/// for each kind of cluster.
#[test]
fn spacing_follows_chromes_rules() {
    use crate::stages::analysis::ClusterClass as Class;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&AHEM_FAMILY, 10.0);
    style.text.letter_spacing = LengthPercentage::from_px(2.0);
    style.text.word_spacing = LengthPercentage {
        px: 3.0,
        fraction: 0.0,
    };
    fixture.text(&mut layout, &style, "X");
    let words = layout.measured().word_spacing_rule;
    assert_eq!(
        words,
        super::WordSpacingRule::CHROME,
        "the default config's"
    );
    let spacing = root_spacing(&layout, words);
    let after = |class: Class, text: &str, continuation: bool| {
        spacing.after(class, text, continuation, false).to_px()
    };
    assert_eq!(after(Class::Text, "a", false), 2.0, "a letter");
    assert_eq!(after(Class::Text, "a\u{301}", false), 2.0, "once a cluster");
    assert_eq!(
        after(Class::Text, "i", true),
        0.0,
        "a ligature's continuation"
    );
    assert_eq!(after(Class::Space, " ", false), 5.0, "a space both");
    assert_eq!(after(Class::NoBreakSpace, "\u{A0}", false), 5.0);
    assert_eq!(
        after(Class::NoBreakSpace, "\u{2007}", false),
        2.0,
        "no separator"
    );
    // Chrome's rule: other scripts' separators and a U+0020 starting a
    // collapsing block's text take no word-spacing.
    for separator in [
        "\u{1361}",
        "\u{10100}",
        "\u{10101}",
        "\u{1039F}",
        "\u{1091F}",
    ] {
        assert_eq!(after(Class::Text, separator, false), 2.0, "{separator:?}");
    }
    let first = |class: Class, text: &str| spacing.after(class, text, false, true).to_px();
    assert_eq!(first(Class::Space, " "), 2.0, "a space starting the text");
    assert_eq!(first(Class::NoBreakSpace, "\u{A0}"), 5.0, "U+00A0 anywhere");
    assert_eq!(after(Class::OtherSpace, "\u{3000}", false), 2.0);
    for (class, text) in [
        (Class::Tab, "\t"),
        (Class::Separator, "\n"),
        (Class::Object, "\u{FFFC}"),
        (Class::SoftHyphen, "\u{AD}"),
        (Class::ZeroWidthSpace, "\u{200B}"),
        (Class::Control, "\u{200D}"),
    ] {
        assert_eq!(after(class, text, false), 0.0, "{class:?}");
    }
    // Cursive clusters take none: an Arabic letter, digit or mark, the
    // tatweel, a joiner. A Latin letter with a mark does.
    for text in [
        "\u{628}",
        "\u{661}",
        "\u{64E}\u{628}",
        "\u{640}",
        "\u{200D}\u{628}",
    ] {
        assert_eq!(after(Class::Text, text, false), 0.0, "{text:?}");
    }
    for script in [
        "\u{710}",
        "\u{840}",
        "\u{1820}",
        "\u{7CA}",
        "\u{A840}",
        "\u{10D00}",
    ] {
        assert_eq!(after(Class::Text, script, false), 0.0, "{script:?}");
    }
    assert_eq!(
        after(Class::Text, "\u{5D0}", false),
        2.0,
        "Hebrew is not cursive"
    );
    assert_eq!(after(Class::Text, "\u{65E5}", false), 2.0, "nor Han");
    // `WordSpacing::WordSeparators`: CSS Text's list, the text's start
    // included.
    let css = super::WordSpacingRule::new(WordSpacing::WordSeparators, layout.content());
    let spacing = root_spacing(&layout, css);
    for separator in [
        "\u{1361}",
        "\u{10100}",
        "\u{10101}",
        "\u{1039F}",
        "\u{1091F}",
    ] {
        let after = spacing.after(Class::Text, separator, false, false).to_px();
        assert_eq!(after, 5.0, "{separator:?}");
    }
    for first in [false, true] {
        assert_eq!(spacing.after(Class::Space, " ", false, first).to_px(), 5.0);
    }
    assert_eq!(spacing.after(Class::Text, "a", false, false).to_px(), 2.0);
    assert_eq!(
        spacing
            .after(Class::NoBreakSpace, "\u{2007}", false, false)
            .to_px(),
        2.0
    );
}

/// Chrome gives a U+0020 that starts the text no word-spacing unless the
/// block keeps its spaces (WPT `word-spacing-003`); a U+00A0 takes it
/// either way. `WordSpacing::WordSeparators` spaces it, and other scripts'
/// separators. A layout keeps the rule it was built with after the config
/// changes. Ahem at 10 px, 5 px word-spacing.
#[test]
fn a_space_starting_the_text_is_spaced_as_chrome_spaces_it() {
    use crate::config::WordSpacing;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut collapse = sized(&AHEM_FAMILY, 10.0);
    collapse.text.word_spacing = LengthPercentage {
        px: 5.0,
        fraction: 0.0,
    };
    let mut pre = collapse;
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let spans = |fixture: &mut Fixture, layout: &mut Layout, text: &str| {
        fixture.build(layout, &ComputedBlockStyle::new(&collapse), |b| {
            b.open_box(NodeKey(1), &pre, None);
            b.text(NodeKey(2), text);
            b.close_box();
        });
    };
    for (word_spacing, first) in [
        (WordSpacing::SpaceAndNoBreakSpace, 10.0),
        (WordSpacing::WordSeparators, 15.0),
    ] {
        fixture.cx.set_config(Config {
            word_spacing,
            ..Config::default()
        });
        spans(&mut fixture, &mut layout, " a b");
        assert_eq!(layout.text(), " a b");
        assert_eq!(
            positions(&layout),
            [0.0, first, first + 10.0, first + 25.0, first + 35.0],
            "{word_spacing:?}"
        );
        // A block that keeps its spaces spaces the first one.
        fixture.text(&mut layout, &pre, " a");
        assert_eq!(positions(&layout), [0.0, 15.0, 25.0], "{word_spacing:?}");
        // U+00A0 is spaced wherever it is.
        spans(&mut fixture, &mut layout, "\u{A0}a");
        assert_eq!(positions(&layout), [0.0, 15.0, 25.0], "{word_spacing:?}");
        // The Ethiopic word space is a separator only to CSS.
        fixture.text(&mut layout, &collapse, "a\u{1361}a");
        let css = if word_spacing == WordSpacing::WordSeparators {
            5.0
        } else {
            0.0
        };
        let positions = positions(&layout);
        assert_eq!(
            positions[2] - positions[1],
            positions[3] - positions[2] + css
        );
    }
    // Built under CSS's rule, then configured for Chrome's: the layout
    // keeps its rule.
    fixture.cx.set_config(Config {
        word_spacing: WordSpacing::WordSeparators,
        ..Config::default()
    });
    spans(&mut fixture, &mut layout, " a b");
    fixture.cx.set_config(Config::default());
    assert_eq!(
        layout.measured().word_spacing_rule,
        super::WordSpacingRule::new(WordSpacing::WordSeparators, layout.content())
    );
}

/// A space's letter- and word-spacing goes after the marks set on it, once,
/// though the marks are a cluster of their own. Chrome's HarfBuzz cluster
/// holds the space and its marks, and `ShapeResultSpacing` spaces it once,
/// after the marks: in Chrome 155, Arial at 20 px with `letter-spacing:
/// 10px` and `word-spacing: 100px`, the `b` of `a \u{301}b` and of `a b`
/// both start at 136.67 px, and the mark stays on its space (WPT
/// `word-spacing-combining-mark-001`). A test font with a zero-width mark
/// at 10 px, 2 px letter- and 3 px word-spacing.
#[test]
fn a_spaces_spacing_goes_after_its_marks_as_chrome_spaces_them() {
    let mut font = TestFont::new("Test Space Marks", &[(0x20, 0x7E), (0x301, 0x301)]);
    font.advances = vec![('\u{301}', 0)];
    let mut fixture = Fixture::new(
        &[font],
        han_fallback("Test Space Marks"),
        StageCheck::BuiltWithContext(check),
    );
    let mut layout = Layout::new();
    let family = [FontFamilyName::named("Test Space Marks")];
    let mut style = sized(&family, 10.0);
    style.text.letter_spacing = LengthPercentage::from_px(2.0);
    style.text.word_spacing = LengthPercentage::from_px(3.0);
    fixture.text(&mut layout, &style, "X X");
    assert_eq!(positions(&layout), [0.0, 7.0, 17.0, 24.0]);
    // The mark starts where the space's advance ends, and the room follows
    // it.
    fixture.text(&mut layout, &style, "X \u{301}X");
    assert_eq!(positions(&layout), [0.0, 7.0, 12.0, 17.0, 24.0]);
    fixture.text(&mut layout, &style, "X \u{301}\u{301}X");
    assert_eq!(positions(&layout), [0.0, 7.0, 12.0, 17.0, 24.0]);
    // A joiner after a space draws nothing and takes no room, so the space
    // keeps its own (WPT `letter-spacing-control-chars-001`).
    fixture.text(&mut layout, &style, "X \u{200C}X");
    assert_eq!(positions(&layout), [0.0, 7.0, 17.0, 17.0, 24.0]);
    // A mark on a letter or a no-break space is in its cluster already.
    fixture.text(&mut layout, &style, "X\u{301} X");
    assert_eq!(positions(&layout), [0.0, 7.0, 17.0, 24.0]);
    fixture.text(&mut layout, &style, "X\u{A0}\u{301}X");
    assert_eq!(positions(&layout), [0.0, 7.0, 17.0, 24.0]);
    // A space starting a block that keeps its spaces takes word-spacing,
    // after its marks.
    let mut pre = style;
    pre.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    fixture.text(&mut layout, &pre, " \u{301}X");
    assert_eq!(positions(&layout), [0.0, 5.0, 10.0, 17.0]);
}

/// Spacing that makes a cluster's step negative marks the paragraph
/// `NONMONOTONE`; spacing that only narrows it doesn't.
#[test]
fn negative_spacing_that_goes_back_is_marked() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for (letter, marked) in [(-5.0, false), (-10.0, false), (-12.0, true)] {
        let mut style = sized(&AHEM_FAMILY, 10.0);
        style.text.letter_spacing = LengthPercentage::from_px(letter);
        fixture.text(&mut layout, &style, "XX XX");
        let flags = layout
            .measured()
            .text(FirstLineVariant::Standard)
            .paragraph(ParagraphId::new(0));
        assert_eq!(
            flags.contains(MeasureFlags::NONMONOTONE),
            marked,
            "{letter}"
        );
    }
}

/// Intrinsic sizes add `text-indent` to the lines it indents: the block's
/// first line, each paragraph's under `each-line`, the others under
/// `hanging`. A percentage counts as zero, as in Chrome.
#[test]
fn an_indent_widens_the_lines_it_starts() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let ahem = sized(&AHEM_FAMILY, 10.0);
    let indented = |px: f32, fraction: f32, hanging: bool, each_line: bool| ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage { px, fraction }.into(),
            hanging,
            each_line,
        },
        ..ComputedBlockStyle::new(&ahem)
    };
    let sizes = |layout: &Layout| {
        let sizes = layout.intrinsic_sizes();
        (sizes.min_content, sizes.max_content)
    };
    for (block, expected) in [
        (indented(20.0, 0.0, false, false), (40.0, 80.0)),
        (indented(20.0, 0.0, true, false), (50.0, 60.0)),
        (indented(0.0, 0.5, false, false), (30.0, 60.0)),
        (indented(20.0, 0.5, false, false), (40.0, 80.0)),
        (indented(-10.0, 0.0, false, false), (30.0, 50.0)),
    ] {
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem,
                ..block
            },
            |b| b.text(NodeKey(1), "XX XXX"),
        );
        assert_eq!(sizes(&layout), expected, "{:?}", block.text_indent);
    }
    let two = |b: &mut LayoutBuilder<'_>| {
        b.text(NodeKey(1), "XX XXX");
        b.line_break(NodeKey(2));
        b.text(NodeKey(3), "XXXX");
    };
    for (block, expected) in [
        (indented(20.0, 0.0, false, false), (40.0, 80.0)),
        (indented(20.0, 0.0, false, true), (60.0, 80.0)),
        (indented(20.0, 0.0, true, true), (50.0, 60.0)),
    ] {
        fixture.build(
            &mut layout,
            &ComputedBlockStyle {
                style: &ahem,
                ..block
            },
            two,
        );
        assert_eq!(sizes(&layout), expected, "{:?}", block.text_indent);
    }
}

/// `line-padding` is a line-edge cost at every boundary where a line may
/// start or end, from the style at that end. Intrinsic sizes pay it at both
/// ends of each line.
#[test]
fn line_padding_is_a_cost_at_each_end() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut ahem = sized(&AHEM_FAMILY, 10.0);
    ahem.line.padding = 5.0;
    fixture.text(&mut layout, &ahem, "XX XXX");
    let edges: Vec<(usize, f32, f32)> = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .edge_costs()
        .iter()
        .map(|edge| (edge.key().get(), edge.start.to_px(), edge.end.to_px()))
        .collect();
    assert_eq!(edges, [(0, 5.0, 0.0), (3, 5.0, 5.0), (6, 0.0, 5.0)]);
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (40.0, 70.0));
    // A padding that isn't finite and positive is zero.
    for bad in [f32::NAN, -5.0, f32::INFINITY] {
        ahem.line.padding = bad;
        fixture.text(&mut layout, &ahem, "XX XXX");
        assert!(
            layout
                .measured()
                .text(FirstLineVariant::Standard)
                .edge_costs()
                .is_empty(),
            "{bad}"
        );
    }
}

/// Hanging punctuation is a line-edge cost: `force-end` always and
/// `allow-end` conditionally hang a final stop or comma, `last` a closing
/// mark at the text's end, `first` an opening mark on the first line. A box
/// edge with room between the mark and the line's edge stops the hang.
#[test]
fn hanging_punctuation_is_a_cost_at_the_mark() {
    use crate::style::{HangEnd, HangingPunctuation};
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let hanging = |first: bool, last: bool, end: HangEnd| {
        let mut style = sized(&AHEM_FAMILY, 10.0);
        style.text.hanging_punctuation = HangingPunctuation { first, last, end };
        style
    };
    let costs = |layout: &Layout| -> Vec<(usize, f32, f32, LineEdgeFlags)> {
        layout
            .measured()
            .text(FirstLineVariant::Standard)
            .edge_costs()
            .iter()
            .map(|edge| {
                (
                    edge.key().get(),
                    edge.start.to_px(),
                    edge.flex.to_px(),
                    edge.flags,
                )
            })
            .collect()
    };
    let forced = LineEdgeFlags::HANG_FORCED;
    fixture.text(
        &mut layout,
        &hanging(false, false, HangEnd::Force),
        "XX, XXX.",
    );
    assert_eq!(
        costs(&layout),
        [(4, 0.0, 10.0, forced), (8, 0.0, 10.0, forced)]
    );
    fixture.text(
        &mut layout,
        &hanging(false, false, HangEnd::Allow),
        "XX, XXX.",
    );
    assert_eq!(
        costs(&layout),
        [
            (4, 0.0, 10.0, LineEdgeFlags::NONE),
            (8, 0.0, 10.0, LineEdgeFlags::NONE)
        ]
    );
    fixture.text(
        &mut layout,
        &hanging(true, true, HangEnd::None),
        "(XX) (XX)",
    );
    assert_eq!(
        costs(&layout),
        [
            (0, -10.0, 0.0, LineEdgeFlags::FIRST_LINE_ONLY),
            (9, 0.0, 10.0, forced)
        ]
    );
    // An edge with room after the comma or before the opening mark.
    let style = hanging(true, false, HangEnd::Force);
    let padded = edged(&style, [0.0, 0.0, 5.0], [0.0, 0.0, 5.0]);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        b.open_box(NodeKey(1), &padded, None);
        b.text(NodeKey(2), "(XX,");
        b.close_box();
        b.text(NodeKey(3), " XX, XX");
    });
    assert_eq!(costs(&layout), [(9, 0.0, 10.0, forced)]);
}

/// `first` hangs a U+3000 starting the block, as CSS Text 4 lists it.
#[test]
fn first_hangs_an_ideographic_space() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let mut style = sized(&AHEM_FAMILY, 10.0);
    style.text.hanging_punctuation = HangingPunctuation {
        first: true,
        last: false,
        end: HangEnd::None,
    };
    fixture.text(&mut layout, &style, "\u{3000}XX");
    let costs: Vec<(usize, f32, LineEdgeFlags)> = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .edge_costs()
        .iter()
        .map(|edge| (edge.key().get(), edge.start.to_px(), edge.flags))
        .collect();
    assert_eq!(costs, [(0, -10.0, LineEdgeFlags::FIRST_LINE_ONLY)]);
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (20.0, 20.0));
}

/// `hang::opens` and `hang::closes` match CSS Text 4's lists for every
/// character.
#[test]
fn hanging_marks_are_the_spec_categories() {
    use icu_properties::CodePointMapData;
    use icu_properties::props::GeneralCategory;
    let categories = CodePointMapData::<GeneralCategory>::new();
    for ch in (0..=0x10FFFF_u32).filter_map(char::from_u32) {
        let category = categories.get(ch);
        let quote = matches!(ch, '"' | '\'');
        let quoting = matches!(
            category,
            GeneralCategory::InitialPunctuation | GeneralCategory::FinalPunctuation
        );
        let opening =
            quote || quoting || ch == '\u{3000}' || category == GeneralCategory::OpenPunctuation;
        let closing = quote || quoting || category == GeneralCategory::ClosePunctuation;
        assert_eq!(hang::opens(ch), opening, "U+{:04X}", u32::from(ch));
        assert_eq!(hang::closes(ch), closing, "U+{:04X}", u32::from(ch));
    }
}

/// With a zero interval a tab is as wide as the letter-spacing, rounded up
/// and possibly negative, as in Chrome's `Font::TabWidth`.
#[test]
fn a_tab_with_no_stops_is_its_letter_spacing() {
    let space = px(10.0);
    for (letter, width) in [
        (3.0, 3.0),
        (0.0, 0.0),
        (-2.0, -2.0),
        (1.0 / 128.0, 1.0 / 64.0),
    ] {
        let stops = TabStops::new(
            TabSize::Px(0.0),
            space,
            TextUnit::from_px(letter),
            TextUnit::from_raw(0),
        );
        assert_eq!(
            tab_advance(LayoutUnit::ZERO, px(15.0), stops).to_px(),
            width,
            "{letter}"
        );
    }
}

/// Huge, negative and NaN spacing and indents measure without panicking.
#[test]
fn any_spacing_gives_valid_measurements() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    for bad in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        -1e30,
        1e-30,
    ] {
        let mut style = sized(&AHEM_FAMILY, 10.0);
        style.text.letter_spacing = LengthPercentage::from_px(bad);
        style.text.word_spacing = LengthPercentage {
            px: bad,
            fraction: bad,
        };
        style.line.padding = bad;
        style.text.hanging_punctuation = HangingPunctuation {
            first: true,
            last: true,
            end: HangEnd::Allow,
        };
        let block = ComputedBlockStyle {
            text_indent: TextIndent {
                amount: LengthPercentage {
                    px: bad,
                    fraction: bad,
                }
                .into(),
                hanging: false,
                each_line: true,
            },
            ..ComputedBlockStyle::new(&style)
        };
        fixture.build(&mut layout, &block, |b| {
            b.text(NodeKey(1), "(a, b\t.)");
            b.line_break(NodeKey(2));
            b.text(NodeKey(3), "c");
        });
        let _ = layout.intrinsic_sizes();
    }
}
