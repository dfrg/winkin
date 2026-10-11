//! Bidi level tests. They pin:
//! - controls in the text and those a style opens, and where an information
//!   separator ends them;
//! - each cluster's level and the runs' levels, as the resolver gives them;
//! - the fast path that keeps no levels;
//! - `unicode-bidi` on boxes, and annotations isolated;
//! - deep nesting and unbalanced controls.

use super::*;
use crate::stages::analysis::bidi::BidiInput;

/// Whether some paragraph has clusters at other levels than its own.
fn mixed(layout: &Layout) -> bool {
    analysis(layout)
        .flags
        .contains(ParagraphFlags::MIXED_LEVELS)
}

/// A style reading in `direction` with `unicode-bidi`.
fn bidi(direction: Direction, unicode_bidi: UnicodeBidi) -> ComputedStyle<'static> {
    styled(|style| {
        style.bidi.direction = direction;
        style.bidi.unicode_bidi = unicode_bidi;
    })
}

/// Bidi controls written in the text are clusters and keep the levels the
/// resolver gives them. Those a style synthesizes are units with no cluster.
///
/// The text is an embedding written around Hebrew, then a box isolated right
/// to left around a Latin letter. The written RLE and PDF take the level
/// before them, as the resolver fills in what X9 removes. The Hebrew is at 1
/// and the Latin inside the isolate at 2. The synthesized RLI and PDI leave
/// no level behind, so there is still one level per cluster.
#[test]
fn controls_in_the_text_keep_their_levels_and_a_styles_leave_none() {
    let layout = build(|b| {
        b.text(key(1), "a\u{202B}\u{5D0}\u{202C}");
        b.open_box(key(2), &bidi(Direction::Rtl, UnicodeBidi::Isolate), None);
        b.text(key(3), "b");
        b.close_box();
        b.text(key(4), "c");
    });
    assert_eq!(
        clusters(&layout),
        ["a", "\u{202B}", "\u{5D0}", "\u{202C}", "b", "c"]
    );
    assert_eq!(cluster_levels(&layout), [0, 0, 1, 1, 2, 0]);
    assert!(mixed(&layout));
}

/// U+001C to U+001E end a bidi paragraph without ending a line. They are bidi
/// class B, a boundary for the resolver, and never a line break.
///
/// Under `plaintext` what follows takes its own direction, as Chrome reads
/// `אב␜XY` as `␜בא XY`. That is one paragraph of clusters and two bidi
/// paragraphs: the first right to left with the separator at its level, the
/// second left to right. Weak types resolve within their own bidi paragraph.
/// A digit after Arabic and U+001C is a European number again, at 0, where it
/// would be an Arabic one at 2.
#[test]
fn an_information_separator_ends_a_bidi_paragraph_and_no_line() {
    let plaintext = bidi(Direction::Ltr, UnicodeBidi::Plaintext);
    for separator in ['\u{1C}', '\u{1D}', '\u{1E}'] {
        let layout = root_text(&plaintext, &format!("\u{5D0}\u{5D1}{separator}XY"));
        assert_eq!(
            paragraphs(&layout).len(),
            1,
            "U+{:04X}",
            u32::from(separator)
        );
        assert_eq!(levels(&layout), [1], "the first bidi paragraph's");
        assert_eq!(cluster_levels(&layout), [1, 1, 1, 0, 0]);
        assert!(breaks(&layout).is_empty(), "no line break");
        let layout = plain(&format!("\u{627}{separator}1"));
        assert_eq!(cluster_levels(&layout), [1, 0, 0]);
    }
    // U+001F, class S, ends no paragraph: the digit after it is still an
    // Arabic number.
    assert_eq!(cluster_levels(&plain("\u{627}\u{1F}1")), [1, 0, 2]);
}

/// A bidi paragraph ending inside a box closes the controls the box's
/// `unicode-bidi` makes and re-opens them after it. CSS Writing Modes 4,
/// section 2.4.4, says so for any bidi paragraph boundary.
///
/// So a right-to-left embedding spanning U+001C embeds the text on both
/// sides, and the block's own override starts the next bidi paragraph too.
/// An embedding written in the text ends with its bidi paragraph (X8), which
/// CSS leaves as Unicode has it.
///
/// Chrome re-opens its controls only at `\n`, so there a box's embedding ends
/// at U+001C. The analysis does not copy this.
#[test]
fn a_styles_controls_reopen_after_an_information_separator() {
    let layout = build(|b| {
        b.open_box(key(1), &bidi(Direction::Rtl, UnicodeBidi::Embed), None);
        b.text(key(2), "ab\u{1C}cd");
        b.close_box();
    });
    assert_eq!(cluster_levels(&layout), [2, 2, 0, 2, 2]);
    let root = bidi(Direction::Rtl, UnicodeBidi::BidiOverride);
    let overridden = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&root)
    };
    let layout = build_with(&overridden, |b| b.text(key(1), "ab\u{1C}cd"));
    assert_eq!(cluster_levels(&layout), [3, 3, 1, 3, 3]);
    let layout = plain("\u{202B}ab\u{1C}cd");
    let levels = cluster_levels(&layout);
    assert_eq!(levels.get(1..), Some(&[2, 2, 0, 0, 0][..]), "{levels:?}");
}

/// Each cluster takes the level the resolver gives its first character. The
/// runs split where the level changes, so shaping sets each in one direction.
///
/// In a left-to-right paragraph, Latin, a space, Hebrew, a space and digits
/// are at 0, 0, 1, 1 and 2. The space between the Hebrew and the digits takes
/// the right-to-left side, as UAX #9 N1 has it.
#[test]
fn each_cluster_is_at_its_resolved_level() {
    let layout = plain("abc \u{5D0}\u{5D1}\u{5D2} 123");
    assert_eq!(cluster_levels(&layout), [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2]);
    assert!(mixed(&layout));
    let runs: Vec<u8> = analysis(&layout)
        .runs
        .iter()
        .map(|(_, run)| run.level.get())
        .collect();
    assert_eq!(runs, [0, 1, 2]);
    // Two scripts at one level, then a change inside the second: Hebrew
    // after Arabic and a space is still at 1 where its run starts, and the
    // space before the Latin at 0.
    let layout = build(|b| {
        b.text(key(1), "\u{627}\u{628} ");
        b.text(key(2), "\u{5D0}\u{5D1} x");
    });
    assert_eq!(cluster_levels(&layout), [1, 1, 1, 1, 1, 0, 0]);
    let runs: Vec<(String, u8)> = analysis(&layout)
        .runs
        .iter()
        .map(|(_, run)| (String::from(run.script.as_str()), run.level.get()))
        .collect();
    assert_eq!(
        runs,
        [
            ("Arab".into(), 1),
            ("Hebr".into(), 1),
            ("Hebr".into(), 0),
            ("Latn".into(), 0)
        ]
    );
    // One script, three levels: an embedding splits a Latin run in three.
    let embedded = bidi(Direction::Rtl, UnicodeBidi::Embed);
    let layout = build(|b| {
        b.text(key(1), "ab");
        b.open_box(key(2), &embedded, None);
        b.text(key(3), "cd");
        b.close_box();
        b.text(key(4), "ef");
    });
    assert_eq!(cluster_levels(&layout), [0, 0, 2, 2, 0, 0]);
    assert_eq!(
        scripts(&layout),
        [
            ("ab", "Latn".into()),
            ("cd", "Latn".into()),
            ("ef", "Latn".into())
        ]
    );
}

/// The runs carry the levels the resolver gives each cluster, wherever the
/// levels change relative to the scripts.
///
/// The text strings together words of Latin, Hebrew, Arabic, European and
/// Arabic digits, separators and written embeddings, a thousand ways. Each
/// cluster's run is at the level the resolver gives its class in a
/// left-to-right paragraph, or at 0 where nothing in it is right to left,
/// as Blink sets a unidirectional paragraph.
#[test]
fn the_runs_are_at_the_resolvers_levels() {
    let words = [
        "ab",
        " ",
        "\u{5D0}\u{5D1}",
        "\u{627}\u{628}",
        "12",
        "\u{661}\u{662}",
        ",",
        "-",
        "\u{202B}",
        "\u{202A}",
        "\u{202C}",
        "\u{65E5}",
    ];
    let mut input = BidiInput::new();
    let mut state = 0x2545_F491_u32;
    let mut text = String::new();
    for _ in 0..1000 {
        text.clear();
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        for _ in 0..1 + (state >> 16) % 12 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            text.push_str(words[(state >> 16) as usize % words.len()]);
        }
        let layout = plain(&text);
        let classes: Vec<_> = clusters(&layout)
            .iter()
            .map(|cluster| {
                let first = cluster.chars().next().unwrap_or(' ');
                unicode::core_props(first).bidi_class()
            })
            .collect();
        input.set_classes(&classes);
        let (_, mut resolved) = input.levels_resolved(Some(0));
        if !text
            .chars()
            .any(|ch| "\u{5D0}\u{627}\u{661}\u{202B}".contains(ch))
        {
            resolved.fill(0);
        }
        assert_eq!(cluster_levels(&layout), resolved, "{text:?}");
    }
}

/// Text with nothing left to right, no number and no control is at level 1
/// throughout in a right-to-left block, and under `plaintext`, as the
/// resolver gives it. Under `plaintext`, an information separator inside
/// starts a bidi paragraph that finds its own direction.
#[test]
fn pure_right_to_left_text_is_at_the_resolvers_levels() {
    let words = [
        "\u{5D0}\u{5D1}",
        "\u{627}\u{628}",
        "\u{5D0}\u{5B4}",
        " ",
        ",",
        "!",
        "-",
        "\u{1C}",
    ];
    let rtl = ComputedStyle::initial();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&rtl)
    };
    let plaintext = bidi(Direction::Ltr, UnicodeBidi::Plaintext);
    let mut input = BidiInput::new();
    let mut state = 0x1B87_3593_u32;
    let mut text = String::new();
    for _ in 0..1000 {
        text.clear();
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        for _ in 0..1 + (state >> 16) % 8 {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            text.push_str(words[(state >> 16) as usize % words.len()]);
        }
        let ruled = [
            (build_with(&rtl, |b| b.text(key(1), &text)), Some(1)),
            (root_text(&plaintext, &text), None),
        ];
        for (layout, requested) in ruled {
            let classes: Vec<_> = clusters(&layout)
                .iter()
                .map(|cluster| {
                    let first = cluster.chars().next().unwrap_or(' ');
                    unicode::core_props(first).bidi_class()
                })
                .collect();
            input.set_classes(&classes);
            let (_, resolved) = input.levels_resolved(requested);
            assert_eq!(cluster_levels(&layout), resolved, "{text:?}");
        }
    }
}

/// The fast path skips the resolver for a paragraph with nothing right to left
/// and no control, in a block that is not right to left. Its runs are at its
/// level.
///
/// A resolved paragraph whose clusters all come out at its own level, such as
/// pure Hebrew in a right-to-left block, has its runs at its level too. So
/// does a paragraph after one whose levels differ.
#[test]
fn text_at_its_paragraphs_level_keeps_no_levels() {
    let painted = styled(|style| style.paints = true);
    let layout = build(|b| {
        b.text(key(1), "plain ");
        b.open_box(key(2), &painted, None);
        b.text(key(3), "Latin (with brackets) 123");
        b.close_box();
    });
    assert!(!mixed(&layout));
    let initial = ComputedStyle::initial();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&initial)
    };
    let hebrew = build_with(&rtl, |b| {
        b.text(
            key(1),
            "\u{5E9}\u{5DC}\u{5D5}\u{5DD} \u{5E2}\u{5D5}\u{5DC}\u{5DD}",
        )
    });
    assert!(!mixed(&hebrew));
    assert_eq!(cluster_levels(&hebrew), [1; 9]);
    // A paragraph of Latin, then one whose levels differ, then Latin: only
    // the second's runs are split.
    let layout = build(|b| {
        b.text(key(1), "ab");
        b.line_break(key(2));
        b.text(key(3), "c\u{5D0}");
        b.line_break(key(4));
        b.text(key(5), "de");
    });
    let flags: Vec<bool> = analysis(&layout)
        .paragraphs
        .iter()
        .map(|(_, paragraph)| paragraph.flags.contains(ParagraphFlags::MIXED_LEVELS))
        .collect();
    assert_eq!(flags, [false, true, false]);
    assert_eq!(cluster_levels(&layout), [0, 0, 0, 0, 1, 0, 0, 0]);
}

/// Controls come from styles, as Blink injects them, and the text stays as it
/// was written:
/// - an isolate is one level up in the box's direction;
/// - an embedding is the same but not isolated;
/// - an override forces the box's direction on what it holds;
/// - `isolate-override` is a first-strong isolate with an override inside it;
/// - `plaintext` is an isolate of its text's own direction.
#[test]
fn a_boxs_unicode_bidi_is_the_controls_it_opens_and_closes() {
    let levels_in = |style: &ComputedStyle<'_>, inner: &str| {
        let layout = build(|b| {
            b.text(key(1), "x ");
            b.open_box(key(2), style, None);
            b.text(key(3), inner);
            b.close_box();
            b.text(key(4), " y");
        });
        assert_eq!(layout.content().text, format!("x {inner} y"));
        cluster_levels(&layout)
    };
    use Direction::{Ltr, Rtl};
    let isolate = bidi(Rtl, UnicodeBidi::Isolate);
    assert_eq!(levels_in(&isolate, "ab"), [0, 0, 2, 2, 0, 0]);
    // An isolate keeps what it holds from what is around it: the digits
    // after a Hebrew isolate follow the Latin, where after an embedding
    // they follow the Hebrew.
    let layout = |style: &ComputedStyle<'_>| {
        cluster_levels(&build(|b| {
            b.text(key(1), "a ");
            b.open_box(key(2), style, None);
            b.text(key(3), "\u{5D0}");
            b.close_box();
            b.text(key(4), " 1");
        }))
    };
    assert_eq!(layout(&isolate), [0, 0, 1, 0, 0]);
    let embedding = bidi(Rtl, UnicodeBidi::Embed);
    assert_eq!(layout(&embedding), [0, 0, 1, 1, 2]);
    let overriding = bidi(Rtl, UnicodeBidi::BidiOverride);
    assert_eq!(levels_in(&overriding, "ab"), [0, 0, 1, 1, 0, 0]);
    let left = bidi(Ltr, UnicodeBidi::BidiOverride);
    assert_eq!(levels_in(&left, "\u{5D0}\u{5D1}"), [0, 0, 2, 2, 0, 0]);
    // FSI, then RLO: an isolate the direction of 'a', then forced right to
    // left inside it.
    let both = bidi(Rtl, UnicodeBidi::IsolateOverride);
    assert_eq!(levels_in(&both, "ab"), [0, 0, 3, 3, 0, 0]);
    // An isolate right to left, as its first strong character is, whatever
    // the box's `direction`; the Latin after it inside is one level up.
    let plaintext = bidi(Ltr, UnicodeBidi::Plaintext);
    assert_eq!(levels_in(&plaintext, "\u{5D0} b"), [0, 0, 1, 1, 2, 0, 0]);
}

/// A box's controls that sit below the clusters on both sides keep their
/// level for reordering; others keep none.
///
/// In `לום<span isolate>LI</span>` the Hebrew is at 1 and the
/// Latin at 2. The LRI between them is at the paragraph's level, 0, below
/// both. The PDI ends the paragraph, with no cluster after it, so it splits
/// nothing. Blink reorders with every control's level
/// (`LogicalLineBuilder::BidiReorder`).
#[test]
fn a_controls_level_below_both_neighbours_is_kept() {
    let isolate = bidi(Direction::Ltr, UnicodeBidi::Isolate);
    let layout = build(|b| {
        b.text(key(1), "\u{5DC}\u{5D5}\u{5DD}");
        b.open_box(key(2), &isolate, None);
        b.text(key(3), "LI");
        b.close_box();
    });
    assert_eq!(cluster_levels(&layout), [1, 1, 1, 2, 2]);
    let kept: Vec<(usize, u8)> = (analysis(&layout).control_levels().iter())
        .map(|control| (control.item.get(), control.level.get()))
        .collect();
    assert_eq!(kept, [(1, 0)], "the open item's LRI");
    // Between two Hebrew letters, the isolate's controls split nothing.
    let layout = build(|b| {
        b.text(key(1), "a \u{5D0}");
        b.open_box(key(2), &isolate, None);
        b.text(key(3), "b");
        b.close_box();
        b.text(key(4), "\u{5D1}");
    });
    assert_eq!(cluster_levels(&layout), [0, 0, 1, 2, 1]);
    assert!(analysis(&layout).control_levels().is_empty());
}

/// The block's own override is pushed at the start of every paragraph.
///
/// A box open across a forced break has its controls re-opened in the
/// paragraph after it, as Blink closes and re-opens them around a forced
/// break. `plaintext` on the block takes each paragraph's direction from its
/// first strong character.
#[test]
fn a_paragraph_starts_inside_the_controls_open_across_it() {
    let overriding = bidi(Direction::Rtl, UnicodeBidi::BidiOverride);
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&overriding)
    };
    let layout = build_with(&rtl, |b| {
        b.text(key(1), "ab");
        b.line_break(key(2));
        b.text(key(3), "cd");
    });
    // Every letter forced right to left, an override being the next odd
    // level up from the paragraph's, as Blink's `EnterBlock` pushes it; the
    // separator at the paragraph's own.
    assert_eq!(cluster_levels(&layout), [3, 3, 1, 3, 3]);
    let plain = build_with(
        &ComputedBlockStyle {
            style: &ComputedStyle::initial(),
            ..rtl
        },
        |b| b.text(key(1), "ab"),
    );
    assert_eq!(cluster_levels(&plain), [2, 2]);

    let isolate = bidi(Direction::Rtl, UnicodeBidi::Isolate);
    let layout = build(|b| {
        b.open_box(key(1), &isolate, None);
        b.text(key(2), "ab");
        b.line_break(key(3));
        b.text(key(4), "cd");
        b.close_box();
        b.line_break(key(5));
        b.text(key(6), "ef");
    });
    assert_eq!(cluster_levels(&layout), [2, 2, 0, 2, 2, 0, 0, 0]);

    let plaintext = styled(|style| style.bidi.unicode_bidi = UnicodeBidi::Plaintext);
    let layout = build_with(&ComputedBlockStyle::new(&plaintext), |b| {
        b.text(key(1), "ab");
        b.line_break(key(2));
        b.text(key(3), "\u{5D0}b");
    });
    assert_eq!(levels(&layout), [0, 1]);
}

/// A ruby annotation's content is an isolate of its own direction, so ruby
/// text neither moves nor is moved by its base line. Upright text in a
/// vertical line reads left to right.
#[test]
fn annotations_are_isolated_and_upright_text_reads_left_to_right() {
    let layout = build(|b| {
        b.text(key(1), "a");
        b.open_ruby(key(2), &ComputedStyle::initial(), None);
        b.text(key(3), "b");
        b.open_annotation(key(4), &ComputedStyle::initial(), None);
        b.text(key(5), "\u{5D0}\u{5D1} 1");
        b.close_annotation();
        b.close_ruby();
        b.text(key(6), " 2");
    });
    // The annotation's digit follows its Hebrew; the base's digit, after
    // it, follows the Latin, as though the annotation were not there.
    assert_eq!(cluster_levels(&layout), [0, 0, 1, 1, 1, 2, 0, 0]);

    let upright = styled(|style| {
        style.orientation.text_orientation = TextOrientation::Upright;
    });
    let vertical = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&upright)
    };
    let hebrew = "\u{5D0}\u{5D1}";
    let layout = build_with(&vertical, |b| b.text(key(1), hebrew));
    assert_eq!(cluster_levels(&layout), [0, 0]);
    let layout = build_with(
        &ComputedBlockStyle {
            style: &ComputedStyle::initial(),
            ..vertical
        },
        |b| b.text(key(1), hebrew),
    );
    assert_eq!(cluster_levels(&layout), [1, 1]);
}

/// Whitespace or a separator carrying a mark resolves as the neutral that
/// `unicode::cluster_bidi_class` gives the pair per character. `tests/bidi_conformance.rs`
/// holds that function to both conformance files.
#[test]
fn a_space_or_a_separator_carrying_a_mark_is_a_neutral() {
    use crate::unicode::{BidiClass as C, cluster_bidi_class};
    let class = |text: &str| {
        let mut classes = text.chars().map(|ch| unicode::core_props(ch).bidi_class());
        let first = classes.next().unwrap_or(C::OTHER_NEUTRAL);
        cluster_bidi_class(first, classes)
    };
    assert_eq!(class(" \u{301}"), C::OTHER_NEUTRAL);
    assert_eq!(class("+\u{301}"), C::OTHER_NEUTRAL);
    assert_eq!(class(",\u{301}"), C::OTHER_NEUTRAL);
    // What X9 removes is carried by nothing, and anything else keeps its
    // first character's class.
    assert_eq!(class(" \u{200D}"), C::WHITE_SPACE);
    assert_eq!(class(" "), C::WHITE_SPACE);
    assert_eq!(class("a\u{301}"), C::LEFT_TO_RIGHT);
    assert_eq!(class("$\u{301}"), C::EUROPEAN_TERMINATOR);
    assert_eq!(class("\u{5D0}\u{5B8}"), C::RIGHT_TO_LEFT);
}

/// Nothing a caller nests, and nothing the text holds, panics the resolver:
/// isolates and overrides three hundred deep, past UAX #9's 125 levels, and
/// controls written into the text that close more than they open, or open
/// and never close. Every level stays within the algorithm's.
#[test]
fn deep_nesting_and_unbalanced_controls_are_resolved() {
    for unicode_bidi in [
        UnicodeBidi::Isolate,
        UnicodeBidi::Embed,
        UnicodeBidi::BidiOverride,
        UnicodeBidi::IsolateOverride,
        UnicodeBidi::Plaintext,
    ] {
        for direction in [Direction::Ltr, Direction::Rtl] {
            let style = bidi(direction, unicode_bidi);
            let layout = build(|b| {
                for depth in 0..300 {
                    b.open_box(key(2 * depth), &style, None);
                    b.text(key(2 * depth + 1), "a\u{5D0}1");
                }
                b.line_break(key(1000));
                b.text(key(1001), "\u{202C}\u{2069}\u{202B}x");
                for _ in 0..300 {
                    b.close_box();
                }
            });
            let levels = cluster_levels(&layout);
            assert!(levels.iter().all(|&level| level <= 126), "{unicode_bidi:?}");
        }
    }
    let written =
        pre("\u{2069}\u{2069}\u{202C}a\u{202B}\u{202E}\u{2067}\u{5D0}\n\u{2066}\u{2066}b");
    assert!(mixed(&written));
}
