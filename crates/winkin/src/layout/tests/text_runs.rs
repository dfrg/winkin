//! Text run tests. They pin:
//! - a line read where line layout placed it;
//! - a run's font, held by the layout and read on another thread;
//! - clusters and glyphs adding up to their run, read either way;
//! - walks that clone where they stand.

use super::*;

/// A line is placed where its `Line` item puts it: centred, from the area's
/// line-left, its top and baseline from the area's block start; its run
/// where its glyphs are, at the ems Ahem draws.
#[test]
fn a_line_is_read_where_line_layout_placed_it() {
    let mut cx = context();
    let mut layout = Layout::new();
    let style = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle {
        text_align: TextAlign::Center,
        ..ComputedBlockStyle::new(&style)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(7), "XXX XXX")
    });
    let area = Area {
        inline: InlineExtents {
            left: 20.0,
            right: 120.0,
        },
        block_start: 5.0,
        block_end: None,
        room_above: 0.0,
    };
    layout.break_lines(&mut cx, area, &mut NoExclusions);
    let line = layout.line(0).expect("a line");
    let metrics = line.metrics();
    assert_eq!(
        (metrics.left, metrics.top, metrics.baseline),
        (35.0, 5.0, 13.0)
    );
    assert_eq!(
        (metrics.ascent, metrics.descent, metrics.width),
        (8.0, 2.0, 70.0)
    );
    assert_eq!(layout.text(), "XXX XXX");
    let runs = runs(&layout, 0);
    assert_eq!(runs.len(), 1);
    let run = runs[0];
    assert_eq!(run.key(), NodeKey(7));
    assert_eq!(run.text_range(), 0..7);
    assert_eq!(
        (run.inline(), run.advance(), run.baseline()),
        (along(0.0, 70.0), 70.0, 8.0)
    );
    assert_eq!(run.block(), across(0.0, 10.0), "Ahem's ascent and descent");
    assert!(!run.is_rtl() && !run.is_tab() && !run.is_hanging() && !run.is_hidden());
    assert_eq!(run.generated(), None);
    let glyphs: Vec<(f32, f32, f32, usize)> = run
        .glyphs()
        .map(|glyph| (glyph.x, glyph.y, glyph.advance, glyph.text_offset))
        .collect();
    let expected: Vec<(f32, f32, f32, usize)> =
        (0..7).map(|i| (10.0 * i as f32, 8.0, 10.0, i)).collect();
    assert_eq!(glyphs, expected);
    let block = layout.metrics();
    assert_eq!(block.block_end, 15.0);
    assert_eq!(
        (block.first_baseline, block.last_baseline),
        (Some(13.0), Some(13.0))
    );
    assert_eq!((block.trim_start, block.trim_end), (0.0, 0.0));
}

/// A run's font is the layout's own: the font's
/// bytes, shared with the context's instance and never copied, its index,
/// the size its glyphs are drawn at, and nothing varied or faked. It is
/// read with the context gone, and an empty collection draws with none.
#[test]
fn a_run_is_drawn_in_a_font_the_layout_holds() {
    let mut cx = context();
    let mut layout = Layout::new();
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&sized(&LATIN, 13.5)),
        |b| b.text(NodeKey(1), "office"),
    );
    layout.break_lines(&mut cx, Area::new(500.0), &mut NoExclusions);
    // A second layout in the context, set in the same instance at another
    // size: the same font, a used font of its own.
    let mut other = Layout::new();
    build(
        &mut cx,
        &mut other,
        &ComputedBlockStyle::new(&sized(&LATIN, 20.0)),
        |b| b.text(NodeKey(2), "flat"),
    );
    other.break_lines(&mut cx, Area::new(500.0), &mut NoExclusions);
    let used = &layout.fonts().used;
    let key = used
        .get(UsedFontId::new(0))
        .and_then(|font| font.instance.as_ref())
        .map(|drawn| drawn.key());
    let instance = cx
        .font_context()
        .instances()
        .first_instance(key.expect("set in a font"))
        .expect("the context's")
        .used()
        .bytes
        .clone();
    drop(cx);

    let font = runs(&layout, 0)[0].font().expect("a font");
    assert_eq!(font.size, 13.5);
    assert!(!font.data().is_empty());
    assert_eq!(font.index, 0);
    assert_eq!(font.key().index, 0);
    assert_eq!(font.key().source, instance.id());
    assert!(font.coords.is_empty());
    assert!(!font.embolden);
    assert_eq!(font.skew, None);
    // The bytes are the instance's allocation, counted and not copied.
    assert!(Arc::ptr_eq(font.bytes.arc(), instance.arc()));
    let second = runs(&other, 0)[0].font().expect("a font");
    assert_eq!((second.key(), second.size), (font.key(), 20.0));
    assert!(Arc::ptr_eq(second.bytes.arc(), instance.arc()));

    // An empty collection draws with none.
    let mut bare = Context::new(Collection::new());
    let mut layout = Layout::new();
    build(
        &mut bare,
        &mut layout,
        &ComputedBlockStyle::new(&sized(&LATIN, 13.5)),
        |b| b.text(NodeKey(1), "office"),
    );
    layout.break_lines(&mut bare, Area::new(500.0), &mut NoExclusions);
    for run in runs(&layout, 0) {
        assert!(run.font().is_none());
        assert_eq!(run.glyphs().count(), 0);
    }
}

/// A layout is read without its context: one thread paints it, fonts and
/// glyphs, while the context builds and breaks the next layout on another.
#[test]
fn a_layout_is_read_on_one_thread_while_the_context_builds_on_another() {
    fn send_and_share<T: Send + Sync>() {}
    send_and_share::<Layout>();
    send_and_share::<crate::FontInstance<'static>>();

    let mut cx = context();
    let mut layout = Layout::new();
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&sized(&LATIN, 13.5)),
        |b| b.text(NodeKey(1), "To office AVAVAIL fit, flat waffle."),
    );
    layout.break_lines(&mut cx, Area::new(120.0), &mut NoExclusions);
    let paint = |layout: &Layout| {
        let mut drawn = 0;
        for line in layout.lines() {
            for item in line.items() {
                if let Item::Text(run) = item {
                    let font = run.font().expect("a font");
                    assert!(!font.data().is_empty() && font.size == 13.5);
                    drawn += run.glyphs().count();
                }
            }
        }
        drawn
    };
    let alone = paint(&layout);
    assert!(alone > 0);
    let mut next = Layout::new();
    thread::scope(|scope| {
        let painter = scope.spawn(|| paint(&layout));
        build(
            &mut cx,
            &mut next,
            &ComputedBlockStyle::new(&sized(&ARABIC, 20.0)),
            |b| b.text(NodeKey(2), "\u{628}\u{62A}\u{633}\u{645}"),
        );
        next.break_lines(&mut cx, Area::new(120.0), &mut NoExclusions);
        assert_eq!(painter.join().ok(), Some(alone));
    });
    assert!(runs(&next, 0)[0].font().is_some());
}

/// A run's clusters and its glyphs add up to its advance, and stand where
/// line layout's walk puts them: across ligatures and a kerned pair, in a font
/// whose advances fall between layout's grid, with a padded box among the
/// runs, and on lines broken inside a ligature and a kerned pair, whose
/// edges are the lines' own.
#[test]
fn clusters_and_glyphs_add_up_to_their_run() {
    let mut cx = context();
    let mut layout = Layout::new();
    let mut latin = sized(&LATIN, 20.0);
    latin.text.word_break = WordBreak::BreakAll;
    let narrow = sized(&NARROW, 16.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(3.3),
            margin: Sides::from_px(1.7),
            ..EdgesGroup::INITIAL
        },
        ..narrow
    };
    build(
        &mut cx,
        &mut layout,
        &ComputedBlockStyle::new(&latin),
        |b| {
            b.text(NodeKey(1), "office AVAVAV ");
            b.open_box(NodeKey(2), &padded, None);
            b.text(NodeKey(3), "iwi wiw");
            b.close_box();
            b.text(NodeKey(4), " waffle");
        },
    );
    for width in [1000.0, 57.3, 28.0, 12.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        walk(&layout);
        let input = layout.read_input();
        for (id, record) in layout.line_records().lines.iter() {
            let text_items: Vec<_> = layout
                .fragments()
                .line_items(id)
                .iter()
                .filter(|item| item.kind() == FragmentItemKind::Text)
                .collect();
            let runs = runs(&layout, id.get());
            assert_eq!(runs.len(), text_items.len());
            for (run, item) in runs.iter().zip(text_items) {
                let advance = run.advance();
                // Its advance is its item's, and its size that rounded up
                // onto the grid.
                assert_eq!(advance, item.advance().to_px());
                assert_eq!(item.size, item.advance().to_layout());
                let clusters: Vec<_> = run.clusters().collect();
                let sum: f32 = clusters.iter().map(|cluster| cluster.advance()).sum();
                assert!(
                    (sum - advance).abs() < 1e-3,
                    "{width}: {sum} against {advance}"
                );
                // Each cluster starts where the one before it ends.
                for pair in clusters.windows(2) {
                    assert!((pair[0].inline().right - pair[1].inline().left).abs() < 1e-3);
                }
                if let (Some(first), Some(last)) = (clusters.first(), clusters.last()) {
                    assert!((first.inline().left - run.inline().left).abs() < 1e-3);
                    assert!((last.inline().right - run.inline().right).abs() < 1e-3);
                    assert_eq!(first.text_range().start, run.text_range().start);
                    assert_eq!(last.text_range().end, run.text_range().end);
                }
                let glyphs: Vec<_> = run.glyphs().collect();
                let sum: f32 = glyphs.iter().map(|glyph| glyph.advance).sum();
                assert!(
                    (sum - advance).abs() < 1e-3,
                    "{width}: {sum} against {advance}"
                );
                let placed: Vec<_> = GlyphWalk::new(&input, id, record, item).collect();
                assert_eq!(glyphs.len(), placed.len());
                for (glyph, walked) in glyphs.iter().zip(&placed) {
                    assert_eq!(glyph.id, walked.id);
                    assert_eq!(glyph.x, walked.x.to_px());
                    let offset = layout.analysis().clusters.start(walked.cluster).get();
                    assert_eq!(glyph.text_offset, offset);
                    assert!(run.text_range().contains(&offset));
                }
            }
        }
    }
    // Broken inside `ffi`, each line draws its own `f` a whole glyph wide.
    layout.break_lines(&mut cx, Area::new(12.0), &mut NoExclusions);
    let second = runs(&layout, 1);
    assert_eq!(layout.text().get(second[0].text_range()), Some("f"));
    assert_eq!(second[0].glyphs().count(), 1);
}

/// A run's glyphs and clusters are views over the layout, so a clone walks
/// on from where the walk it was made of stood, and the two give the same:
/// over a justified paragraph with ligatures, a kerned
/// pair and a reshaped edge, reading both ways, and a hyphen and an
/// ellipsis, each walk cloned at every place along it.
#[test]
fn a_cloned_walk_goes_on_where_its_original_stood() {
    use crate::style::{LineClamp, TextAlign};
    let mut cx = context();
    let mut layout = Layout::new();
    for (direction, text) in [
        (
            Direction::Ltr,
            "office AVAVA fit\u{AD}ted, flat waffles in the ffi hall",
        ),
        (
            Direction::Rtl,
            "\u{628}\u{62A}\u{633} AVAVA \u{645}\u{628} office",
        ),
    ] {
        let root = ComputedStyle {
            bidi: BidiGroup {
                direction,
                ..BidiGroup::INITIAL
            },
            ..sized(&LATIN, 20.0)
        };
        let block = ComputedBlockStyle {
            text_align: TextAlign::Justify,
            line_clamp: LineClamp::Lines(2),
            ..ComputedBlockStyle::new(&root)
        };
        build(&mut cx, &mut layout, &block, |b| b.text(NodeKey(1), text));
        layout.break_lines(&mut cx, Area::new(130.0), &mut NoExclusions);
        let mut walks = 0;
        for line in layout.lines() {
            for item in line.all_items() {
                let (Item::Text(run) | Item::Generated(run)) = item else {
                    continue;
                };
                let glyphs: Vec<_> = run.glyphs().collect();
                let clusters: Vec<_> = run.clusters().collect();
                for at in 0..=glyphs.len() {
                    let mut walk = run.glyphs();
                    for _ in 0..at {
                        walk.next();
                    }
                    let rest: Vec<_> = walk.clone().collect();
                    assert_eq!(rest, glyphs[at..], "{direction:?} at {at}");
                    assert_eq!(walk.collect::<Vec<_>>(), glyphs[at..]);
                }
                for at in 0..=clusters.len() {
                    let mut walk = run.clusters();
                    for _ in 0..at {
                        walk.next();
                    }
                    assert_eq!(walk.clone().len(), clusters.len() - at);
                    assert_eq!(walk.clone().collect::<Vec<_>>(), clusters[at..]);
                }
                walks += 1;
            }
        }
        assert!(walks >= 3, "{direction:?}: {walks}");
    }
}

/// A run read right to left hands its clusters out in logical order, the
/// first at the run's right, and its glyphs in drawing order, left to
/// right.
#[test]
fn a_right_to_left_run_reads_its_clusters_from_the_right() {
    let mut cx = context();
    let mut layout = Layout::new();
    let mut style = sized(&ARABIC, 20.0);
    style.text.white_space_collapse = WhiteSpaceCollapse::Preserve;
    let block = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.text(
            NodeKey(1),
            "\u{628}\u{62A}\u{633}\u{645} \u{628}\u{62A}\u{633}\u{645}",
        )
    });
    layout.break_lines(&mut cx, Area::new(60.0), &mut NoExclusions);
    // The preserved space hangs past the line box's left, and comes first.
    let runs = runs(&layout, 0);
    assert!(runs[0].is_hanging() && runs[0].inline().left < 0.0);
    let run = runs[1];
    assert!(run.is_rtl());
    let lefts: Vec<f32> = run
        .clusters()
        .map(|cluster| cluster.inline().left)
        .collect();
    assert_eq!(lefts, [30.0, 20.0, 10.0, 0.0]);
    let offsets: Vec<usize> = run
        .clusters()
        .map(|cluster| cluster.text_range().start)
        .collect();
    assert_eq!(offsets, [0, 2, 4, 6]);
    let xs: Vec<f32> = run.glyphs().map(|glyph| glyph.x).collect();
    assert_eq!(xs, [0.0, 10.0, 20.0, 30.0]);
    let drawn: Vec<usize> = run.glyphs().map(|glyph| glyph.text_offset).collect();
    assert_eq!(drawn, [6, 4, 2, 0]);
    walk(&layout);
}
