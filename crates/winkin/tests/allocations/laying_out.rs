//! Line layout and reading back:
//! - laying out again allocates nothing, at any width laid out at before;
//! - reading a layout back and selecting allocate nothing;
//! - word motion allocates inside ICU and only there;
//! - tabs, preserved white space, justification, spacing and indents
//!   allocate nothing warm.

use super::test_fonts::{self, TestFont, ahem_fallback};
use super::{arabic, count_allocations, relayout, text};
use fontwich::Collection;
use winkin::style::{
    BaseDirection, ComputedStyle, EdgesGroup, FontFamilyName, FontGroup, Sides, TextAlign,
    TextAlignLast, TextGroup, WhiteSpaceCollapse, WordBreak,
};
use winkin::{
    Area, BoxSize, BuildOptions, ComputedBlockStyle, Context, Item, Layout, NoExclusions, NodeKey,
};

/// Ahem; a Latin font with ligatures, kerning pairs and the combining
/// marks; an Arabic font with joining forms.
fn collection() -> Collection {
    let mut latin = TestFont::new("Test Latin", &[(0x20, 0x7E), (0x300, 0x36F)]);
    latin.ligatures = vec![vec!['f', 'f', 'i'], vec!['f', 'i'], vec!['f', 'l']];
    latin.kerning = vec![('A', 'V', -100), ('T', 'o', -80)];
    test_fonts::collection(&[latin, arabic()], ahem_fallback())
}

/// How a document is set: its family, its block's direction, and its
/// alignment and its last lines'.
type How = (&'static str, BaseDirection, TextAlign, TextAlignLast);

/// `text` in paragraphs, set as `how` says, inside a box with room,
/// its last grapheme divided by a bare span holding a mark, shaped with
/// the text before it: each paragraph again in a box that paints and
/// breaks between any two letters, so lines end and start inside kerned
/// pairs and ligatures and a kept box's parts open and close across
/// lines; in a box that paints nothing, which is culled; an empty box
/// that paints; an atomic inline; and preserved white space and a tab,
/// which hang. So every path of line layout is taken: pieces of text,
/// a divided grapheme's in each part's item, tabs, what hangs, atomics
/// and edges, alignment of every kind, a line read either way, boxes
/// kept, culled and empty, and the items written.
fn prose(layout: &mut Layout, cx: &mut Context, how: How, text: &str) {
    let (family, direction, align, last) = how;
    let families = [FontFamilyName::named(family)];
    let root = ComputedStyle {
        font: FontGroup {
            families: &families,
            ..FontGroup::INITIAL
        },
        ..ComputedStyle::initial()
    };
    let painted = ComputedStyle {
        paints: true,
        text: TextGroup {
            word_break: WordBreak::BreakAll,
            ..root.text
        },
        font: FontGroup {
            size: 20.0,
            ..root.font
        },
        ..root
    };
    let preserved = ComputedStyle {
        text: TextGroup {
            white_space_collapse: WhiteSpaceCollapse::Preserve,
            ..root.text
        },
        ..root
    };
    let roomy = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::from_px(1.5),
            padding: Sides::from_px(3.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let size = BoxSize {
        inline: 24.0,
        block: 18.0,
        baseline: Some(14.0),
    };
    let block = ComputedBlockStyle {
        direction,
        text_align: align,
        text_align_last: last,
        ..ComputedBlockStyle::new(&root)
    };
    let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
    for (at, paragraph) in (0..).zip(text.split('\n')) {
        let key = |n: u64| NodeKey(16 * at + n);
        b.open_box(key(0), &roomy, None);
        b.text(key(1), paragraph);
        b.open_box(key(11), &root, None);
        b.text(key(12), "\u{301}");
        b.close_box();
        b.open_box(key(2), &painted, None);
        b.text(key(3), paragraph);
        b.close_box();
        b.open_box(key(4), &root, None);
        b.text(key(5), paragraph);
        b.close_box();
        b.open_box(key(6), &painted, None);
        b.close_box();
        b.atomic(key(7), &roomy, None, size);
        b.open_box(key(8), &preserved, None);
        b.text(key(9), " \tX   ");
        b.close_box();
        b.close_box();
        b.line_break(key(10));
    }
    assert!(b.finish(cx).is_complete());
}

/// Reads `layout` back as a host does, every part of it:
/// each line's metrics and items; each text run's key, text, place,
/// font, glyphs and clusters; each atomic inline and box part; every
/// part of every box by key, culled ones found from what they hold;
/// and the block's metrics. Returns a sum of what was read, so none of
/// it is left unread.
fn read(layout: &Layout, keys: u64) -> f32 {
    let mut sum = 0.0;
    for line in layout.lines() {
        let metrics = line.metrics();
        sum += metrics.left + metrics.top + metrics.baseline + metrics.width;
        sum += line.text_range().len() as f32;
        for item in line.all_items() {
            match item {
                Item::Text(run) | Item::Generated(run) => {
                    sum += run.key().0 as f32 + run.text_range().len() as f32;
                    sum += run.inline().left + run.advance() + run.baseline() + run.block().under;
                    if let Some(font) = run.font() {
                        sum += font.size + font.data().len() as f32 + font.index as f32;
                        sum += font.coords.len() as f32 + font.skew.unwrap_or(0.0);
                        sum += f32::from(u8::from(font.embolden));
                    }
                    for glyph in run.glyphs() {
                        sum += glyph.id as f32 + glyph.x + glyph.y + glyph.advance;
                        sum += glyph.text_offset as f32;
                    }
                    for cluster in run.clusters() {
                        sum += cluster.inline().left + cluster.advance();
                        sum += cluster.text_range().len() as f32;
                    }
                }
                Item::Atomic(atomic) => {
                    sum += atomic.inline().right + atomic.block().under + atomic.baseline();
                }
                Item::Box(piece) => {
                    sum += piece.inline().right + piece.block().under + piece.edges().left;
                    sum += piece.descendants() as f32;
                }
            }
        }
    }
    for key in 0..keys {
        for piece in layout.box_fragments(NodeKey(key)) {
            sum += piece.inline().right + piece.block().over + piece.line() as f32;
        }
    }
    let block = layout.metrics();
    sum + block.block_end + block.last_baseline.unwrap_or(0.0)
}

/// Reading a layout back allocates nothing, the first time as every
/// time: every line, item, glyph and cluster is a view of the stages,
/// every walk holds a few words, and a culled box is answered by a walk
/// over the lines it reaches. So for Latin with ligatures and kerning
/// broken inside them, Arabic read right to left, kept, culled and empty
/// boxes, atomics and what hangs, at widths from one letter to one line.
#[test]
fn reading_back_allocates_nothing() {
    let arabic =
        "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{646}\u{64A} \u{628}\u{644}\u{62A}\u{645}\u{633}.";
    let latin = "To office AVAVAIL fit, flat waffle: 1,000 times ffi officially.";
    let documents: [(How, &str); 2] = [
        (
            (
                "Test Latin",
                BaseDirection::Ltr,
                TextAlign::Center,
                TextAlignLast::End,
            ),
            latin,
        ),
        (
            (
                "Test Arabic",
                BaseDirection::Rtl,
                TextAlign::Start,
                TextAlignLast::Auto,
            ),
            arabic,
        ),
    ];
    for (how, line) in documents {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        prose(&mut layout, &mut cx, how, &text(line, 6));
        for width in [3.0, 37.0, 81.25, 600.0] {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            let mut sum = 0.0;
            let read = count_allocations(|| sum = read(&layout, 16 * 6));
            assert!(sum != 0.0, "{how:?} at {width}: something was read");
            assert_eq!(read, 0, "{how:?} at {width}: reading back allocated");
            // The divided grapheme's mark is an item of its own, so its
            // span was answered.
            assert!(
                layout.box_fragments(NodeKey(11)).next().is_some(),
                "{how:?} at {width}: the mark's span"
            );
        }
    }
}

/// Selection reads the stages as the output does, and allocates nothing
/// from its first call: carets, hits, every motion, word
/// motion segmenting in place and stopping as either platform does,
/// rectangles and copy, over prose in either direction with boxes, an
/// atomic inline and hanging white space in it.
#[test]
fn selecting_allocates_nothing() {
    use winkin::selection::{
        CopyKind, Granularity, MotionDirection, Position, Selection, WordMotion,
    };
    let documents: [(How, &str); 2] = [
        (
            (
                "Test Latin",
                BaseDirection::Ltr,
                TextAlign::Start,
                TextAlignLast::Auto,
            ),
            "To office AVAVAIL fit, flat waffle: 1,000 times.",
        ),
        (
            (
                "Test Arabic",
                BaseDirection::Rtl,
                TextAlign::Start,
                TextAlignLast::Auto,
            ),
            "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{646}\u{64A} \u{628}\u{644}\u{62A}.",
        ),
    ];
    let directions = [
        MotionDirection::Forward,
        MotionDirection::Backward,
        MotionDirection::Left,
        MotionDirection::Right,
    ];
    let granularities = [
        Granularity::Character,
        Granularity::Word,
        Granularity::Line,
        Granularity::LineBoundary,
        Granularity::ParagraphBoundary,
        Granularity::DocumentBoundary,
    ];
    for (how, line) in documents {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        prose(&mut layout, &mut cx, how, &text(line, 3));
        layout.break_lines(&mut cx, Area::new(81.25), &mut NoExclusions);
        let len = layout.text().len();
        let select = || {
            let mut sum = 0.0;
            for at in (0..=len).filter(|&at| layout.text().is_char_boundary(at)) {
                let position = Position::from(at);
                if let Some(caret) = layout.caret(position) {
                    let metrics = layout.line(caret.line).map(|line| line.metrics());
                    let (left, top) = metrics.map_or((0.0, 0.0), |m| (m.left, m.top));
                    let hit = layout.hit_test(
                        left + caret.inline.left,
                        top + 1.0,
                        winkin::config::PastLines::platform(),
                    );
                    sum += caret.inline.left + hit.map_or(0.0, |hit| hit.offset as f32);
                }
                for direction in directions {
                    for granularity in granularities {
                        for stops in [WordMotion::SkipSpaces, WordMotion::StopAtWordEnd] {
                            let motion = direction.extending(granularity).with_word_motion(stops);
                            let mut selection = Selection::from(position);
                            selection.modify(&layout, motion);
                            sum += selection.focus().offset as f32;
                        }
                    }
                }
                sum += layout.selection_rects(0..at).count() as f32;
                sum += layout.selected_text(0..at, CopyKind::Clipboard).count() as f32;
            }
            sum
        };
        let mut sum = 0.0;
        let cold = count_allocations(|| sum = select());
        assert!(sum != 0.0, "{how:?}: something was read");
        assert_eq!(cold, 0, "{how:?}: selecting allocated");
    }
}

/// The one exception: a word motion through Thai, Lao, Khmer or
/// Myanmar, and with the `dictionaries` feature through ideographs and
/// hiragana, meets runs ICU's word segmenter hands its dictionaries or
/// its LSTM, and its iterator copies each such run into a string of its
/// own to do so (icu_segmenter 2.3, `handle_complex_utf8`), whether the
/// run is segmented whole or its edge among the few clusters around a
/// stop. Without dictionaries an ideograph or a hiragana is stood in for
/// and segmented by UAX #29's rules, and a word motion through them
/// allocates nothing. Every other query over the same text allocates
/// nothing. Should ICU stop allocating, this fails.
#[test]
fn word_motion_allocates_inside_icu_and_only_there() {
    use winkin::selection::{Granularity, MotionDirection, Position, Selection};
    // Each text, and whether ICU's dictionaries or its LSTM segment it.
    let documents = [
        (
            "\u{E20}\u{E32}\u{E29}\u{E32}\u{E44}\u{E17}\u{E22} \u{E40}\u{E1B}\u{E47}\u{E19}",
            true,
        ),
        (
            "\u{6F22}\u{5B57}\u{3068}\u{304B}\u{306A} \u{6587}\u{5B57}",
            cfg!(feature = "dictionaries"),
        ),
    ];
    let how = (
        "Test Latin",
        BaseDirection::Ltr,
        TextAlign::Start,
        TextAlignLast::Auto,
    );
    let directions = [
        MotionDirection::Forward,
        MotionDirection::Backward,
        MotionDirection::Left,
        MotionDirection::Right,
    ];
    for (line, icus) in documents {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        prose(&mut layout, &mut cx, how, &text(line, 2));
        layout.break_lines(&mut cx, Area::new(81.25), &mut NoExclusions);
        let len = layout.text().len();
        let moving = |granularities: &[Granularity]| {
            let mut sum = 0;
            for at in (0..=len).filter(|&at| layout.text().is_char_boundary(at)) {
                for direction in directions {
                    for &granularity in granularities {
                        let motion = direction.moving(granularity);
                        let mut selection = Selection::from(Position::from(at));
                        selection.modify(&layout, motion);
                        sum += selection.focus().offset;
                    }
                }
            }
            sum
        };
        let mut sum = 0;
        let words = count_allocations(|| sum = moving(&[Granularity::Word]));
        assert!(sum > 0, "{line}: something was read");
        if icus {
            assert!(
                words > 0,
                "{line}: ICU's word iterator no longer allocates: update this test"
            );
        } else {
            assert_eq!(words, 0, "{line}: words by UAX #29's rules allocated");
        }
        let rest = count_allocations(|| {
            moving(&[
                Granularity::Character,
                Granularity::Line,
                Granularity::LineBoundary,
                Granularity::ParagraphBoundary,
            ]);
        });
        assert_eq!(rest, 0, "{line}: selecting but by words allocated");
    }
}

/// Reading every run's font allocates nothing, and neither does keeping
/// its bytes: a used font holds a count on its instance's bytes and its
/// coordinates in the layout's own arena, so a run's font
/// borrows the layout, and cloning the bytes counts a reference on their
/// `Arc` and copies nothing. So for Latin and Arabic at two sizes, with
/// the context they were built in gone.

#[test]
fn reading_every_runs_font_allocates_nothing() {
    let documents: [(How, &str); 2] = [
        (
            (
                "Test Latin",
                BaseDirection::Ltr,
                TextAlign::Start,
                TextAlignLast::Auto,
            ),
            "To office AVAVAIL fit, flat waffle.",
        ),
        (
            (
                "Test Arabic",
                BaseDirection::Rtl,
                TextAlign::Start,
                TextAlignLast::Auto,
            ),
            "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{646}\u{64A}.",
        ),
    ];
    for (how, line) in documents {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        prose(&mut layout, &mut cx, how, &text(line, 4));
        layout.break_lines(&mut cx, Area::new(81.25), &mut NoExclusions);
        drop(cx);
        let runs = layout
            .lines()
            .flat_map(|line| line.all_items())
            .filter(|item| matches!(item, Item::Text(_) | Item::Generated(_)))
            .count();
        let mut kept = Vec::with_capacity(runs);
        let (mut sum, mut sizes) = (0.0, [false; 2]);
        let read = count_allocations(|| {
            for line in layout.lines() {
                for item in line.all_items() {
                    let (Item::Text(run) | Item::Generated(run)) = item else {
                        continue;
                    };
                    let font = run.font().expect("every run is drawn");
                    sum += font.data().len() as f32 + font.key().index as f32;
                    sum += font.coords.len() as f32 + font.skew.unwrap_or(0.0);
                    sum += f32::from(u8::from(font.embolden));
                    sizes[0] |= font.size == 16.0;
                    sizes[1] |= font.size == 20.0;
                    kept.push(font.bytes.clone());
                }
            }
        });
        assert_eq!(read, 0, "{how:?}: reading the fonts allocated");
        assert!(sum > 0.0 && sizes == [true; 2], "{how:?}: {sizes:?}");
        assert_eq!(kept.len(), runs);
    }
}

/// Tabbed and preserved text, rebuilt, relaid and read back, allocates
/// nothing once warm: tabs sized where they land in the intrinsic sizes,
/// the breaker's walk and line layout, each from its style's space read
/// off the font, under `pre-wrap`, `break-spaces` and `pre`, set at its
/// start and at its end, in a band away from zero.
#[test]
fn tabs_and_preserved_white_space_allocate_nothing_warm() {
    let text = "name\tvalue\tnote i i\t \n\ta much longer name\tv\t\ti\t  \nlast\t";
    let families = [FontFamilyName::named("Test Latin")];
    let build = |layout: &mut Layout, cx: &mut Context, align: TextAlign, text: &str| {
        let root = ComputedStyle {
            text: TextGroup {
                white_space_collapse: WhiteSpaceCollapse::Preserve,
                tab_size: winkin::style::TabSize::Spaces(4.0),
                ..ComputedStyle::initial().text
            },
            font: FontGroup {
                families: &families,
                ..FontGroup::INITIAL
            },
            ..ComputedStyle::initial()
        };
        let spaces = ComputedStyle {
            text: TextGroup {
                white_space_collapse: WhiteSpaceCollapse::BreakSpaces,
                tab_size: winkin::style::TabSize::Px(33.0),
                ..root.text
            },
            ..root
        };
        let pre = ComputedStyle {
            text: TextGroup {
                wrap_mode: winkin::style::TextWrapMode::NoWrap,
                ..root.text
            },
            ..root
        };
        let block = ComputedBlockStyle {
            text_align: align,
            ..ComputedBlockStyle::new(&root)
        };
        let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
        b.text(NodeKey(1), text);
        b.open_box(NodeKey(2), &spaces, None);
        b.text(NodeKey(3), text);
        b.close_box();
        b.open_box(NodeKey(4), &pre, None);
        b.text(NodeKey(5), text);
        b.close_box();
        assert!(b.finish(cx).is_complete());
    };
    let widths = [37.0, 81.25, 12.0, 250.0, 600.0];
    let area = |width: f32| Area {
        inline: winkin::InlineExtents {
            left: 11.5,
            right: 11.5 + width,
        },
        block_start: 0.0,
        block_end: None,
        room_above: 0.0,
    };
    let relayout = |layout: &mut Layout, cx: &mut Context| {
        let mut sum = 0.0;
        for width in widths {
            layout.break_lines(cx, area(width), &mut NoExclusions);
            sum += read(layout, 6);
        }
        sum + layout.intrinsic_sizes().max_content
    };
    for align in [TextAlign::Start, TextAlign::End] {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        build(&mut layout, &mut cx, align, text);
        let cold = count_allocations(|| {
            relayout(&mut layout, &mut cx);
        });
        assert!(cold > 0, "a first layout grows the stages");
        let warm = count_allocations(|| {
            build(&mut layout, &mut cx, align, text);
        });
        assert_eq!(warm, 0, "{align:?}: rebuilding allocated");
        let warm = count_allocations(|| {
            assert!(relayout(&mut layout, &mut cx) > 0.0);
        });
        assert_eq!(warm, 0, "{align:?}: laying out and reading allocated");
    }
}

/// Justified, spaced and indented text, rebuilt, relaid and read back,
/// allocates nothing once warm: letter-spacing, which
/// turns the common ligatures off in an instance of its own, and
/// word-spacing of a length and a percentage of the space, in the
/// prefix and on the edges a break inside a kerned pair reshapes, by
/// Chrome's rule and by CSS's;
/// `text-indent` of a length and a percentage, `hanging` and
/// `each-line`; justification under each `text-justify`, a line's
/// opportunities counted and read back per cluster, with tabs stretched
/// and kept at their stops; `line-padding`; hanging punctuation at both
/// ends; and `text-group-align`, in a block reading either way.
#[test]
fn justified_spaced_and_indented_text_allocates_nothing_warm() {
    use winkin::style::{
        HangEnd, HangingPunctuation, LengthPercentage, TextGroupAlign, TextIndent, TextJustify,
    };
    let text = "(To office AVAVAIL fit, flat\twaffle: 1,000 times\u{A0}ffi officially.)\n\
                \u{65E5}\u{672C}\u{8A9E}\u{306E}\u{6587}\u{7AE0} and more, then some.\n\
                last, short.";
    let families = [FontFamilyName::named("Test Latin")];
    let build =
        |layout: &mut Layout, cx: &mut Context, direction: BaseDirection, justify: TextJustify| {
            let root = ComputedStyle {
                text: TextGroup {
                    letter_spacing: 0.75,
                    word_spacing: LengthPercentage {
                        px: 1.5,
                        fraction: 0.25,
                    },
                    justify,
                    hanging_punctuation: HangingPunctuation {
                        first: true,
                        last: true,
                        end: HangEnd::Allow,
                    },
                    white_space_collapse: WhiteSpaceCollapse::PreserveSpaces,
                    ..ComputedStyle::initial().text
                },
                line: winkin::style::LineGroup {
                    padding: 2.5,
                    ..winkin::style::LineGroup::INITIAL
                },
                font: FontGroup {
                    families: &families,
                    ..FontGroup::INITIAL
                },
                ..ComputedStyle::initial()
            };
            let broken = ComputedStyle {
                text: TextGroup {
                    word_break: WordBreak::BreakAll,
                    letter_spacing: -0.5,
                    ..root.text
                },
                paints: true,
                ..root
            };
            let block = ComputedBlockStyle {
                direction,
                text_align: TextAlign::Justify,
                text_align_last: TextAlignLast::Center,
                text_group_align: TextGroupAlign::Center,
                text_indent: TextIndent {
                    amount: LengthPercentage {
                        px: 12.0,
                        fraction: 0.05,
                    },
                    hanging: justify == TextJustify::InterWord,
                    each_line: true,
                },
                ..ComputedBlockStyle::new(&root)
            };
            let mut b = layout.builder(NodeKey(0), &block, BuildOptions::default());
            b.text(NodeKey(1), text);
            b.open_box(NodeKey(2), &broken, None);
            b.text(NodeKey(3), text);
            b.close_box();
            assert!(b.finish(cx).is_complete());
        };
    let widths = [37.0, 81.25, 12.0, 250.0, 600.0];
    let relayout = |layout: &mut Layout, cx: &mut Context| {
        let mut sum = 0.0;
        for width in widths {
            layout.break_lines(cx, Area::new(width), &mut NoExclusions);
            sum += read(layout, 4);
        }
        sum + layout.intrinsic_sizes().max_content
    };
    for direction in [BaseDirection::Ltr, BaseDirection::Rtl] {
        for justify in [
            TextJustify::Auto,
            TextJustify::InterWord,
            TextJustify::InterCharacter,
        ] {
            let mut cx = Context::new(collection());
            let mut layout = Layout::new();
            build(&mut layout, &mut cx, direction, justify);
            let cold = count_allocations(|| {
                relayout(&mut layout, &mut cx);
            });
            assert!(cold > 0, "a first layout grows the stages");
            assert!(layout.lines().next().is_some(), "lines");
            let warm = count_allocations(|| {
                build(&mut layout, &mut cx, direction, justify);
            });
            assert_eq!(warm, 0, "{direction:?} {justify:?}: rebuilding allocated");
            let warm = count_allocations(|| {
                assert!(relayout(&mut layout, &mut cx) != 0.0);
            });
            assert_eq!(warm, 0, "{direction:?} {justify:?}: relaying out allocated");
            // Tabs kept at their stops, which the next break reads.
            let mut config = *cx.config();
            config.tab_justification = winkin::config::TabJustification::KeepStops;
            cx.set_config(config);
            let warm = count_allocations(|| {
                relayout(&mut layout, &mut cx);
            });
            assert_eq!(
                warm, 0,
                "{direction:?} {justify:?}: keeping stops allocated"
            );
            // CSS's word separators, which the next build reads.
            config.word_spacing = winkin::config::WordSpacing::WordSeparators;
            cx.set_config(config);
            let warm = count_allocations(|| {
                build(&mut layout, &mut cx, direction, justify);
                relayout(&mut layout, &mut cx);
            });
            assert_eq!(
                warm, 0,
                "{direction:?} {justify:?}: CSS's word-spacing allocated"
            );
        }
    }
}

/// Laying out again allocates nothing, at any width laid out at before:
/// the fragment items, each line's head and the justification table
/// keep their capacity, and so does line layout's scratch in the
/// context, its pieces, its order, its boxes' parts and the boxes
/// open from line to line. So for Latin with kerning and ligatures set
/// at its start, centered and at its right, and Arabic read right to
/// left, with reshaped edges, kept, culled and empty boxes, atomics and
/// hanging white space; and for a second layout in the same context.
#[test]
fn laying_out_allocates_nothing_warm() {
    let arabic =
        "\u{628}\u{62A}\u{633}\u{645} \u{644}\u{646}\u{64A} \u{628}\u{644}\u{62A}\u{645}\u{633}.";
    let latin = "To office AVAVAIL fit, flat waffle: 1,000 times ffi officially.";
    let documents: [(How, &str); 4] = [
        (
            (
                "Test Latin",
                BaseDirection::Ltr,
                TextAlign::Start,
                TextAlignLast::Auto,
            ),
            latin,
        ),
        (
            (
                "Test Latin",
                BaseDirection::Ltr,
                TextAlign::Center,
                TextAlignLast::End,
            ),
            latin,
        ),
        (
            (
                "Test Latin",
                BaseDirection::Ltr,
                TextAlign::Right,
                TextAlignLast::Left,
            ),
            latin,
        ),
        (
            (
                "Test Arabic",
                BaseDirection::Rtl,
                TextAlign::Start,
                TextAlignLast::Center,
            ),
            arabic,
        ),
    ];
    let widths = [37.0, 55.5, 81.25, 12.0, 120.0, 250.0, 600.0, 3.0];
    for (how, line) in documents {
        let mut cx = Context::new(collection());
        let mut layout = Layout::new();
        prose(&mut layout, &mut cx, how, &text(line, 12));
        let cold = count_allocations(|| {
            relayout(&mut layout, &mut cx, &widths);
        });
        assert!(cold > 0, "a first layout grows the stages");
        let warm = count_allocations(|| {
            assert!(relayout(&mut layout, &mut cx, &widths) > 0);
        });
        assert_eq!(warm, 0, "{how:?}: laying out again allocated");
        let warm = count_allocations(|| {
            relayout(&mut layout, &mut cx, &[81.25, 3.0, 600.0, 37.0]);
        });
        assert_eq!(warm, 0, "{how:?}: laying out in another order allocated");
        let mut other = Layout::new();
        prose(&mut other, &mut cx, how, &text(line, 12));
        relayout(&mut other, &mut cx, &widths);
        let warm = count_allocations(|| {
            relayout(&mut other, &mut cx, &widths);
        });
        assert_eq!(warm, 0, "{how:?}: a second layout allocated");
    }
}
