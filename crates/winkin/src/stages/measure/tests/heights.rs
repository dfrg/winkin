//! Height tests. They pin:
//! - item extents and the strut under each line-metrics source;
//! - `line-height: normal` taking every used font;
//! - what holds nothing of the line taking no room;
//! - the em boxes of emphasis marks and of each text item.

use super::*;
use crate::data::IdRange;
use crate::style::FirstLineVariant;

/// The strut and a text item's extent are the font's ascent and descent
/// around the line height, the leading halved and floored to a whole pixel
/// over the baseline and the rest under, from the metrics each `Config`
/// source reads. `normal` is the font's own line spacing; a number
/// multiplies the computed size; a length is itself; a line height under
/// the font's own gives negative leading.
#[test]
fn extents_and_the_strut_follow_each_metrics_source() {
    let sources = [
        (Config::chrome_windows(), (18.0, 6.0, 0.0)),
        (Config::chrome_mac(), (16.0, 4.0, 2.0)),
        (Config::chrome_linux(), (14.0, 6.0, 4.0)),
    ];
    for (config, (ascent, descent, gap)) in sources {
        let mut fixture = fixture();
        fixture.cx.set_config(config);
        let line = |height| ComputedStyle {
            line: LineGroup {
                height,
                ..LineGroup::INITIAL
            },
            ..sized(&METRICS, 20.0)
        };
        let leaded = |height: f32| {
            let leading = (height - ascent - descent) * 64.0;
            let over = ((leading as i32 / 2) >> 6) as f32;
            (ascent + over, descent + leading / 64.0 - over)
        };
        let cases = [
            (LineHeight::Normal, ascent + descent + gap),
            (LineHeight::Px(30.0), 30.0),
            (LineHeight::Factor(1.25), 25.0),
            (LineHeight::Px(10.0), 10.0),
            (LineHeight::Px(11.0), 11.0),
            (LineHeight::Px(24.5), 24.5),
        ];
        for (height, px) in cases {
            let mut layout = Layout::new();
            let style = line(height);
            fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
                b.text(NodeKey(1), "Ab");
                b.open_box(NodeKey(2), &style, None);
                b.close_box();
            });
            let (over, under) = leaded(px);
            let expected = Extent::new(lu(over), lu(under));
            let text = layout.measured().text(FirstLineVariant::Standard);
            assert_eq!(text.extents.strut, expected, "{config:?} {height:?}");
            // The text, and the box's opening edge: every box has a strut.
            for item in [0, 1] {
                assert_eq!(
                    text.extents.get(ItemId::new(item)),
                    expected,
                    "{config:?} {height:?} item {item}"
                );
            }
            assert!(
                text.extents.get(ItemId::new(2)).is_none(),
                "a close holds nothing"
            );
        }
    }
}

/// With `line-height: normal`, a text item reaches as far as every used font
/// its clusters are set in, each with its own leading, as Blink accumulates
/// used fonts; with any other line height, only its primary font counts.
/// Ahem at 20 px is 16 over and 4 under; Test Han 14 and 8 with a gap of 4,
/// so 16 and 10 once leaded.
#[test]
fn normal_line_height_unites_the_used_fonts() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let ahem = sized(&AHEM_FAMILY, 20.0);
    fixture.text(&mut layout, &ahem, "ab漢字cd");
    let text = layout.measured().text(FirstLineVariant::Standard);
    assert_eq!(text.extents.strut, Extent::new(lu(16.0), lu(4.0)));
    assert_eq!(
        text.extents.get(ItemId::new(0)),
        Extent::new(lu(16.0), lu(10.0))
    );
    let fixed = ComputedStyle {
        line: LineGroup {
            height: LineHeight::Px(20.0),
            ..LineGroup::INITIAL
        },
        ..ahem
    };
    fixture.text(&mut layout, &fixed, "ab漢字cd");
    let text = layout.measured().text(FirstLineVariant::Standard);
    assert_eq!(
        text.extents.get(ItemId::new(0)),
        Extent::new(lu(16.0), lu(4.0))
    );
    // Latin alone is its primary font.
    fixture.text(&mut layout, &ahem, "abcd");
    let text = layout.measured().text(FirstLineVariant::Standard);
    assert_eq!(text.extents.get(ItemId::new(0)), text.extents.strut);
}

/// A float's anchor, a closing edge and annotation text take no room across
/// the line, and a ruby annotation's clusters none along it: line layout sets
/// them as a line of their own. The column is as wide as its
/// annotation, the room past its base taken where it ends, less the half of
/// the annotation's font size it reaches over the `E` after it; not over
/// the `A`, a float standing between, as in Chrome.
#[test]
fn what_holds_nothing_of_the_line_takes_no_room() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "A");
        b.float(NodeKey(2), &ahem, FloatSide::Left, BoxSize::default());
        b.open_ruby(NodeKey(3), &ahem, None);
        b.text(NodeKey(4), "B");
        b.open_annotation(NodeKey(5), &ahem, None);
        b.text(NodeKey(6), "CD");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(7), "E");
    });
    assert_eq!(positions(&layout), [0.0, 16.0, 32.0, 32.0, 40.0, 56.0]);
    let text = layout.measured().text(FirstLineVariant::Standard);
    for (id, item) in layout.content().items.iter() {
        let annotation = item.flags.contains(ItemFlags::ANNOTATION);
        let none = annotation
            || matches!(
                item.kind,
                ItemKind::Float | ItemKind::RubyClose | ItemKind::Close
            );
        assert_eq!(text.extents.get(id).is_none(), none, "{item:?}");
    }
}

/// Text that sets emphasis marks has the em box they are drawn in, among its
/// text metrics: the primary font's at half the computed size, rounded to a
/// pixel, 8 tall for 16px Ahem, 6.4 of it over the baseline; text with none
/// has none.
#[test]
fn a_marked_style_has_its_marks_em_box() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let ahem = families_style(&AHEM_FAMILY);
    let mut marked = ahem;
    marked.text.emphasis.marks = true;
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "a");
        b.open_box(NodeKey(2), &marked, None);
        b.text(NodeKey(3), "b");
        b.close_box();
    });
    let mark = |node: NodeId| {
        let text = layout
            .content()
            .nodes
            .text_facts(node, FirstLineVariant::Standard);
        layout.measured().text_metrics(text).expect("a row").mark
    };
    let marked = mark(NodeId::new(2));
    assert_eq!(marked.height(), LayoutUnit::from_px(8.0));
    assert_eq!(marked.ascent(), LayoutUnit::from_px(6.4));
    assert!(mark(NodeId::BLOCK).is_none());
}

/// Where the content has ruby, each text item of the base line keeps the em
/// box whatever of it a line holds has, as `em_box` finds it for every
/// part of it: `AB`, drawn in one font, does, and so does the base; `A水B`,
/// whose `水` falls back to a font of a taller em box, keeps none, its
/// parts' boxes differing; the annotation's text and what is not text keep
/// none. Where the content has neither ruby nor marks, nothing does.
#[test]
fn a_text_item_keeps_the_em_box_all_of_its_text_has() {
    let mut punct = TestFont::cjk("Test Punct", true);
    punct.typo = (880, -120, 0);
    let mut fixture = Fixture::new(
        &[punct, TestFont::new("Test ASCII", &[(0x20, 0x7E)])],
        han_fallback("Test Punct"),
        StageCheck::Built(check),
    );
    let ascii = families_style(&ASCII_ONLY);
    let mut layout = Layout::new();
    for ruby in [true, false] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&ascii), |b| {
            b.text(NodeKey(1), "AB");
            b.text(NodeKey(2), "A\u{6C34}B");
            if ruby {
                b.open_ruby(NodeKey(3), &ascii, None);
                b.text(NodeKey(4), "CD");
                b.open_annotation(NodeKey(5), &ascii, None);
                b.text(NodeKey(6), "ef");
                b.close_annotation();
                b.close_ruby();
            }
        });
        let (content, analysis) = (layout.content(), layout.analysis());
        let em_boxes = layout
            .measured()
            .text(FirstLineVariant::Standard)
            .em_boxes();
        let mut kept = Vec::new();
        for (id, item) in content.items.iter() {
            let clusters = analysis.item_clusters.range(id);
            let Some(em) = em_boxes.get(id) else {
                continue;
            };
            assert!(item.kind == ItemKind::Text, "{item:?}");
            assert!(!item.flags.contains(ItemFlags::ANNOTATION), "{item:?}");
            for from in clusters.clone().ids() {
                for to in from.get() + 1..=clusters.end.get() {
                    let part = from..ClusterId::new(to);
                    assert_eq!(em, node_em_box(&layout, item.node, part), "{item:?}");
                }
            }
            let source = content.text(FirstLineVariant::Standard);
            let text: String = clusters
                .ids()
                .map(|at| source.slice(analysis.clusters.range(at)))
                .collect();
            kept.push(text);
        }
        let expected: &[&str] = if ruby { &["AB", "CD"] } else { &[] };
        assert_eq!(kept, expected, "ruby: {ruby}");
    }
    // The mixed item's parts' boxes do differ, so none holds for all of it.
    let mixed = ItemId::new(1);
    let clusters = layout.analysis().item_clusters.range(mixed);
    let node = layout.content().item_node(mixed);
    let em = |part: Range<ClusterId>| node_em_box(&layout, node, part);
    let letter = em(clusters.start..ClusterId::new(clusters.start.get() + 1));
    let whole = em(clusters);
    assert_ne!(letter, whole);
}

/// First-line measurement stops at a preserved separator, but a cached item
/// em box describes all of that item. Cover a partial item, a new item at
/// the cutoff, empty boxes, fallback fonts and both shaped/reused variants.
#[test]
fn first_line_em_boxes_keep_whole_items_at_the_paragraph_cutoff() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let families = [
        FontFamilyName::named("Test Metrics"),
        FontFamilyName::named("Test Han"),
    ];
    let mut base = families_style(&families);
    base.text.emphasis.marks = true;
    base.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    for reshape in [false, true] {
        let mut first = base;
        first.line.height = LineHeight::Factor(2.0);
        if reshape {
            first.font.size = 24.0;
        }
        let block = ComputedBlockStyle {
            first_line: Some(&first),
            ..ComputedBlockStyle::new(&base)
        };
        for split in [false, true] {
            fixture.build(&mut layout, &block, |b| {
                if split {
                    b.text(NodeKey(1), "AB\n");
                    b.open_box(NodeKey(2), &base, None);
                    b.close_box();
                    b.text(NodeKey(3), "A水B");
                } else {
                    b.text(NodeKey(1), "AB\nA水B");
                }
            });
            assert!(layout.measured().first_line().is_some());
        }
    }
}
