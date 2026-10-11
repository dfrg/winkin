//! Spacing tests. They pin:
//! - letter and word spacing in the prefix, as Chrome spaces each cluster;
//! - negative spacing that goes back marked;
//! - `text-indent`, line padding and hanging punctuation as costs;
//! - spacing of any size measured into valid data.

use super::*;
use crate::stages::content::FixedSpacing;
use crate::style::FirstLineVariant;

/// A style of `layout`'s, the root's, as the stages read it.
/// The spacing of the block's own text, its word-spacing going where
/// `words` says.
fn root_spacing(layout: &Layout, words: super::WordSpacingRule) -> LetterWordSpacing {
    let content = layout.content();
    let text = content
        .nodes
        .text_facts(NodeId::BLOCK, FirstLineVariant::Standard);
    let facts = &content.facts;
    LetterWordSpacing::from_shaping(facts, facts.text(text).shaping, words)
}

/// `letter-spacing` goes after every cluster that draws, the last included,
/// and `word-spacing` after each word separator, both into the prefix where
/// the cluster's advance goes; a percentage of `word-spacing`
/// is of the font size. Text with none is untouched.
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
    // A forced break, a tab and an atomic inline take none: they draw no
    // glyph of the text's.
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

/// A percentage of `word-spacing` is of the style's own computed font size
/// (CSS Text 4, "word-spacing", and Chrome 153's
/// `FontDescription::WordSpacing`), not of the space it widens:
/// `word-spacing: 50%` is as wide as `0.5em` in Chrome 153. Ahem's space is
/// an em, so
/// its pages cannot tell the two apart (WPT `word-spacing-001` and `-002`);
/// Test Latin's is half an em. At 20 px its space is 10 px wide, and
/// `word-spacing: 50%` adds 10 px after it, not 5, beside 2 px of
/// letter-spacing after every cluster. A span at 40 px holding the same
/// percentage, as a percentage inherits, takes it of its own size: 20 px.
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
    // Each letter 10 px and 2 px of letter-spacing; the space 10 px, 2 px
    // and 10 px of word-spacing.
    assert_eq!(positions(&layout), [0.0, 12.0, 24.0, 46.0, 58.0, 70.0]);
    // A length and a percentage together, `calc(3px + 50%)`, and a
    // percentage in `em`, which it is: the same 10 px either way.
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
    // A span twice the size, in the same percentages.
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

/// `letter-spacing: 50%` adds half the font size after each cluster: 10 px
/// at 20 px, and 20 px in a span at 40 px holding the same percentage, as a
/// percentage inherits. A length with no percentage is fixed, which
/// `text-fit` keeps; a percentage, or a length with one, is not.
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

/// The one rule a cluster is spaced by, which the prefix and a reshaped
/// line edge share, read off each kind of cluster.
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
    // Chrome reads U+0020 and U+00A0 alone, and not a U+0020 that starts
    // the text of a block that collapses its spaces.
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
    // Cursive: an Arabic letter, a digit of the script, a mark on one, the
    // tatweel, a joiner, take none; a Latin letter with a mark does.
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
    // Under `WordSpacing::WordSeparators`, CSS Text's list, the text's start included.
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

/// Where a U+0020 starts a block's text, which only an inline element that
/// keeps its spaces in a block that collapses its own can put there, Chrome
/// gives it no word-spacing; it does where the block keeps its spaces, as
/// WPT `word-spacing-003` has it, and a U+00A0 there takes it either way.
/// `WordSpacing::WordSeparators` spaces it wherever it is, and CSS Text's separators
/// of other scripts, which Chrome does not. The breaker's
/// reshaped edges read the rule the prefix was built with, whatever the
/// context's config has become since. Ahem at 10 px, 5 px word-spacing.
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
        // A block that keeps its spaces spaces its first.
        fixture.text(&mut layout, &pre, " a");
        assert_eq!(positions(&layout), [0.0, 15.0, 25.0], "{word_spacing:?}");
        // U+00A0 is spaced wherever it is.
        spans(&mut fixture, &mut layout, "\u{A0}a");
        assert_eq!(positions(&layout), [0.0, 15.0, 25.0], "{word_spacing:?}");
        // The Ethiopic word space is a separator to CSS alone.
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
    // Built under CSS's rule and broken under Chrome's, the layout keeps
    // the rule it was built with.
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

/// Spacing that takes a cluster's step below nothing makes the prefix go
/// back, which marks the paragraph; spacing that only narrows it does not.
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

/// `text-indent` in the intrinsic sizes: its length, where a line of
/// max-content (a paragraph) or a min-content line would
/// take it, the block's first under the initial value, each paragraph's
/// first under `each-line`, every other one under `hanging`; a percentage
/// counts for nothing, as Chrome resolves it in those modes.
#[test]
fn an_indent_widens_the_lines_it_starts() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let ahem = sized(&AHEM_FAMILY, 10.0);
    let indented = |px: f32, fraction: f32, hanging: bool, each_line: bool| ComputedBlockStyle {
        text_indent: TextIndent {
            amount: LengthPercentage { px, fraction },
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

/// `line-padding` is a line-edge cost at every boundary a line may start or
/// end at, the padding of the style at that end, and the
/// intrinsic sizes pay it at both ends of every line they stand for.
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
    // A padding that is not a finite positive length is none.
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

/// Hanging punctuation is a line-edge cost: under
/// `force-end` a stop or comma ending a line gives its advance back always,
/// under `allow-end` where the line needs it; `last` a closing mark at the
/// text's end; `first` an opening mark at the block's start, for the first
/// line only. A box edge that takes room between the mark and the line's
/// edge keeps it from hanging.
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
    // An edge with room after the comma, or before the opening mark, and
    // it is no longer at the line's edge.
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

/// `first` hangs an ideographic space starting the block, as it hangs an
/// opening bracket: CSS Text 4 lists U+3000 among what `first` hangs.
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

/// What `first` and `last` hang is CSS Text 4's list: Unicode's Ps, Pi and
/// Pf for `first`, with U+3000, and Pe, Pi and Pf for `last`, each with the
/// two straight quotes, for every character.
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

/// Where a `tab-size` puts its stops nothing apart, a tab is as wide as its
/// style's letter-spacing, rounded up onto the grid, as Chrome's
/// `Font::TabWidth` has it: negative too.
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

/// Spacing and indents of every size a caller can give -- huge, negative,
/// not a number -- measure into a valid stage.
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
                },
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
