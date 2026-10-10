//! Ruby tests. They pin:
//! - annotations read apart from a line's items, each on its own side;
//! - a column starting a line keeping its annotation;
//! - justified bases, and columns flush with a justified line's edge;
//! - boxes inside annotations, and annotations over shifted bases;
//! - nested columns, aligned and kept whole;
//! - rubies split across lines;
//! - base text with no annotation after it laid out as the line's text;
//! - valid reads under any area.

use super::*;

/// A ruby base under a wider annotation: `A`, `X` under `XXXX` at half its
/// size, `B`, in 20px Ahem. The column is the annotation's 40 wide and
/// reaches 5 (half the annotation's size) over each neighbour, so its box
/// runs from 15 to 55, the base centred in it at 25 and `B` at 50. The
/// annotation's em box stands on the base's, 18 over the baseline, and
/// reaches 10 past the line box, which grows by that, as Chrome's does:
/// the baseline goes to 26 and the annotation's to 8. The line's items are
/// its bases' and its own text; the annotation is read apart, and painted
/// after the ruby container's box and before the line's text.
#[test]
fn a_line_reads_its_ruby_annotations_apart_from_its_items() {
    let mut cx = context();
    let mut layout = Layout::new();
    let base = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.text(NodeKey(1), "A");
        b.open_ruby(NodeKey(2), &base, None);
        b.text(NodeKey(3), "X");
        b.open_annotation(NodeKey(4), &small, None);
        b.text(NodeKey(5), "XXXX");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), "B");
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(layout.lines().len(), 1);
    let line = layout.line(0).expect("a line");
    let metrics = line.metrics();
    assert_eq!((metrics.ascent, metrics.height()), (26.0, 30.0));
    let lefts: Vec<(NodeKey, f32)> = runs(&layout, 0)
        .iter()
        .map(|run| (run.key(), run.glyphs().next().map_or(0.0, |glyph| glyph.x)))
        .collect();
    assert_eq!(
        lefts,
        [(NodeKey(1), 0.0), (NodeKey(3), 25.0), (NodeKey(6), 50.0)]
    );
    // The base's run takes in the room `ruby-align` leaves beside it, as
    // Chrome's text fragment does: the column's box.
    assert_eq!(runs(&layout, 0)[1].inline(), along(15.0, 55.0));
    let annotations: Vec<_> = line.annotations().collect();
    assert_eq!(annotations.len(), 1);
    let annotation = annotations[0];
    assert_eq!(annotation.key(), NodeKey(4));
    assert_eq!(annotation.inline(), along(15.0, 55.0));
    assert_eq!(annotation.baseline(), 8.0);
    assert!(!annotation.is_under());
    let text: Vec<TextRun<'_>> = annotation.runs().collect();
    assert_eq!(text.len(), 1);
    assert_eq!(text[0].key(), NodeKey(5));
    assert_eq!(text[0].inline(), along(15.0, 55.0));
    assert_eq!(text[0].baseline(), 8.0);
    let xs: Vec<f32> = text[0].glyphs().map(|glyph| glyph.x).collect();
    assert_eq!(xs, [15.0, 25.0, 35.0, 45.0]);
    let clusters: Vec<InlineExtents> = text[0].clusters().map(|c| c.inline()).collect();
    assert_eq!(clusters.get(3).copied(), Some(along(45.0, 55.0)));
    // Painted after the background and the container's box, before the
    // line's own text.
    let kinds: Vec<&str> = line
        .paints(|_| Decorates::None)
        .map(|item| match item {
            Paint::Background(_) => "background",
            Paint::Box(_) => "box",
            Paint::Annotation(_) => "annotation",
            Paint::Text(_) => "text",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        ["background", "box", "annotation", "text", "text", "text"]
    );
}

/// A ruby container broken between its columns sets each column's
/// annotation over its base on the base's line: the column starting the
/// next line keeps its annotation, which the line before once took and
/// never set. Chrome 154 (153 agrees), 20px Ahem on lines of 20 with 10px
/// annotations, 200 wide, `一二三四五六七<ruby>八<rt>xx</rt>九<rt>xxx</rt>
/// 十<rt>xx</rt>土<rt>xx</rt></ruby>木`: the first line's annotations from
/// 140 to 160 and 160 to 190, the second's from 0 to 20 and 20 to 40; with
/// `十`'s annotation a `<b>` padded 5px either side along the line around
/// one `x`, the box from 0 to 20 and its `x` from 5 to 15; and in
/// `vertical-rl` the same along the lines.
#[test]
fn a_ruby_column_starting_a_line_keeps_its_annotation() {
    use crate::style::WritingMode;
    let mut cx = context();
    let mut layout = Layout::new();
    let mut root = sized(&AHEM_FAMILY, 20.0);
    root.line.height = LineHeight::Px(20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    // Padded 5px either side along the line: left and right across a
    // horizontal one, top and bottom down a vertical one.
    let padded = |vertical: bool| ComputedStyle {
        edges: EdgesGroup {
            padding: if vertical {
                Sides::<f32> {
                    top: 5.0,
                    bottom: 5.0,
                    ..Sides::ZERO
                }
            } else {
                Sides::<f32> {
                    left: 5.0,
                    right: 5.0,
                    ..Sides::ZERO
                }
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..small
    };
    // Each line's annotations: their keys and where their boxes are.
    let lines = |layout: &Layout| -> Vec<Vec<(NodeKey, (f32, f32))>> {
        layout
            .lines()
            .map(|line| {
                line.annotations()
                    .map(|annotation| (annotation.key(), ends(annotation.inline())))
                    .collect()
            })
            .collect()
    };
    for mode in [WritingMode::HorizontalTb, WritingMode::VerticalRl] {
        for boxed in [false, true] {
            let block = ComputedBlockStyle {
                writing_mode: mode,
                ..ComputedBlockStyle::new(&root)
            };
            let padding = padded(mode == WritingMode::VerticalRl);
            build(&mut cx, &mut layout, &block, |b| {
                b.text(NodeKey(1), "一二三四五六七");
                b.open_ruby(NodeKey(2), &root, None);
                for (n, (base, reading)) in
                    [("八", "xx"), ("九", "xxx"), ("十", "xx"), ("土", "xx")]
                        .into_iter()
                        .enumerate()
                {
                    let key = 10 * (n as u64 + 1);
                    b.text(NodeKey(key), base);
                    b.open_annotation(NodeKey(key + 1), &small, None);
                    if boxed && base == "十" {
                        b.open_box(NodeKey(key + 2), &padding, None);
                        b.text(NodeKey(key + 3), "x");
                        b.close_box();
                    } else {
                        b.text(NodeKey(key + 3), reading);
                    }
                    b.close_annotation();
                }
                b.close_ruby();
                b.text(NodeKey(3), "木");
            });
            layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
            let case = (mode, boxed);
            assert_eq!(
                lines(&layout),
                [
                    vec![(NodeKey(11), (140.0, 160.0)), (NodeKey(21), (160.0, 190.0))],
                    vec![(NodeKey(31), (0.0, 20.0)), (NodeKey(41), (20.0, 40.0))],
                ],
                "{case:?}"
            );
            let line = layout.line(1).expect("a second line");
            let first = line.annotations().next().expect("an annotation");
            let text: Vec<(f32, f32)> = first.runs().map(|run| ends(run.inline())).collect();
            let spans: Vec<(NodeKey, (f32, f32))> = first
                .boxes()
                .map(|span| (span.key(), ends(span.inline())))
                .collect();
            if boxed {
                assert_eq!(text, [(5.0, 15.0)], "{case:?}");
                assert_eq!(spans, [(NodeKey(32), (0.0, 20.0))], "{case:?}");
            } else {
                assert_eq!(text, [(0.0, 20.0)], "{case:?}");
                assert!(spans.is_empty(), "{case:?}");
            }
        }
    }
}

/// An annotation is set on the side its own ruby container's
/// `ruby-position` gives, as Chrome reads it off the annotation's parent
/// ruby (`LineBreaker::HandleRuby`), whatever the annotation's own style
/// says: so a ruby inside another's base, whose annotations are the outer
/// column's levels here, sets its own over the base where
/// it says `over`, the outer one's under where it says `under`. Chrome 153,
/// Ahem at 20 px on lines of 20 with 10 px annotations:
/// `<ruby style="ruby-position: under"><ruby
/// style="ruby-position: over">XX<rt>yy</rt></ruby><rt>zz</rt></ruby>` sets
/// `yy` over the base, 10 px above its top, and `zz` under it, 10 px below
/// its bottom; and an `rt` saying `ruby-position: under` itself in a ruby
/// saying `over` is set over.
#[test]
fn a_nested_rubys_annotation_takes_its_own_containers_side() {
    use crate::style::RubyPosition;
    let mut cx = context();
    let mut layout = Layout::new();
    let mut base = sized(&AHEM_FAMILY, 20.0);
    base.line.height = LineHeight::Px(20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let mut over = base;
    over.ruby.position = RubyPosition::Over;
    let mut under = base;
    under.ruby.position = RubyPosition::Under;
    // The annotations' own styles inherit the side of the ruby they are in,
    // as a cascade gives them.
    let mut small_over = small;
    small_over.ruby.position = RubyPosition::Over;
    let mut small_under = small;
    small_under.ruby.position = RubyPosition::Under;
    let block = ComputedBlockStyle::new(&base);
    let blocks_of = |layout: &Layout| -> Vec<(NodeKey, CrossExtents)> {
        let line = layout.line(0).expect("a line");
        line.annotations()
            .flat_map(|annotation| annotation.runs().collect::<Vec<_>>())
            .map(|run| (run.key(), run.block()))
            .collect()
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.open_ruby(NodeKey(2), &under, None);
        b.open_ruby(NodeKey(3), &over, None);
        b.text(NodeKey(4), "XX");
        b.open_annotation(NodeKey(5), &small_over, None);
        b.text(NodeKey(6), "yy");
        b.close_annotation();
        b.close_ruby();
        b.open_annotation(NodeKey(7), &small_under, None);
        b.text(NodeKey(8), "zz");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let line = layout.line(0).expect("a line");
    assert_eq!(line.metrics().height(), 40.0);
    assert_eq!(runs(&layout, 0)[0].block(), across(10.0, 30.0));
    let mut found = blocks_of(&layout);
    found.sort_by_key(|(key, _)| key.0);
    assert_eq!(
        found,
        [
            (NodeKey(6), across(0.0, 10.0)),
            (NodeKey(8), across(30.0, 40.0))
        ]
    );
    // An annotation's own `ruby-position` is not its container's.
    build(&mut cx, &mut layout, &block, |b| {
        b.open_ruby(NodeKey(2), &over, None);
        b.text(NodeKey(3), "XX");
        b.open_annotation(NodeKey(4), &small_under, None);
        b.text(NodeKey(5), "yy");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(runs(&layout, 0)[0].block(), across(10.0, 30.0));
    assert_eq!(blocks_of(&layout), [(NodeKey(5), across(0.0, 10.0))]);
}

/// A ruby container's box is its base's, and an annotation answers its own
/// box, as wide as its column and standing on its side. Chrome 153, 20px
/// Ahem with 10px annotations, client rects from the line's top:
/// - `<ruby>base<rt>annotation</rt></ruby>`: the ruby from 10 to 30, the
///   annotation from 0 to 10, both from 0 to 100;
/// - the same under: the ruby from 0 to 20, the annotation from 20 to 30,
///   and so where the annotation's own style says over;
/// - `X<ruby>basebase<rt>ab</rt></ruby>X` under: both from 20 to 180, the
///   ruby from 0 to 20 and the annotation from 20 to 30.
///
/// An annotation in an annotation container takes the container's side:
/// one under in a ruby over is set as one under a ruby under.
#[test]
fn a_ruby_box_is_its_bases_and_an_annotation_answers_its_own() {
    use crate::style::RubyPosition;
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let mut under = root;
    under.ruby.position = RubyPosition::Under;
    // Each box's parts by its key, along the line and across it.
    let parts = |layout: &Layout, key: u64| -> Vec<((f32, f32), CrossExtents)> {
        layout
            .box_fragments(NodeKey(key))
            .map(|part| (ends(part.inline()), part.block()))
            .collect()
    };
    let mut small_over = small;
    small_over.ruby.position = RubyPosition::Over;
    // The ruby's parts and the annotation's, for a ruby in `ruby` around
    // `base`, and `reading` in `annotation`, in an annotation container of
    // side `container` where there is one, `X` either side where `beside`.
    let mut place = |ruby: &ComputedStyle<'_>,
                     annotation: &ComputedStyle<'_>,
                     container: Option<RubyPosition>,
                     base: &str,
                     reading: &str,
                     beside: bool| {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            if beside {
                b.text(NodeKey(1), "X");
            }
            b.open_ruby(NodeKey(2), ruby, None);
            b.text(NodeKey(3), base);
            match container {
                Some(position) => {
                    b.open_annotation_with_position(NodeKey(4), annotation, None, position)
                }
                None => b.open_annotation(NodeKey(4), annotation, None),
            }
            b.text(NodeKey(5), reading);
            b.close_annotation();
            b.close_ruby();
            if beside {
                b.text(NodeKey(6), "X");
            }
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        (parts(&layout, 2), parts(&layout, 4))
    };
    assert_eq!(
        place(&root, &small, None, "base", "annotation", false),
        (
            vec![((0.0, 100.0), across(10.0, 30.0))],
            vec![((0.0, 100.0), across(0.0, 10.0))]
        )
    );
    let set_under = (
        vec![((0.0, 100.0), across(0.0, 20.0))],
        vec![((0.0, 100.0), across(20.0, 30.0))],
    );
    assert_eq!(
        place(&under, &small, None, "base", "annotation", false),
        set_under
    );
    assert_eq!(
        place(&under, &small_over, None, "base", "annotation", false),
        set_under
    );
    assert_eq!(
        place(&under, &small, None, "basebase", "ab", true),
        (
            vec![((20.0, 180.0), across(0.0, 20.0))],
            vec![((20.0, 180.0), across(20.0, 30.0))]
        )
    );
    assert_eq!(
        place(
            &root,
            &small_over,
            Some(RubyPosition::Under),
            "base",
            "annotation",
            false
        ),
        set_under
    );
}

/// Every degenerate area reads a layout with ruby and emphasis marks --
/// annotations and their runs, marks, and the paint -- without panicking,
/// every figure finite, and every annotation's runs inside its box.
#[test]
fn any_area_gives_valid_ruby_reads() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = marked(20.0, EmphasisSkip::INITIAL);
    let mut under = root;
    under.ruby.position = RubyPosition::Under;
    let small = sized(&LATIN, 10.0);
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        for n in 0..4u64 {
            let ruby = if n % 2 == 0 { &root } else { &under };
            b.text(NodeKey(10 * n + 1), "ab ");
            b.open_ruby(NodeKey(10 * n + 2), ruby, None);
            b.text(NodeKey(10 * n + 3), "X");
            b.open_annotation(NodeKey(10 * n + 4), &small, None);
            b.text(NodeKey(10 * n + 5), "fiffle");
            b.close_annotation();
            b.open_annotation(NodeKey(10 * n + 6), &root, None);
            b.text(NodeKey(10 * n + 7), "yy");
            b.close_annotation();
            b.close_ruby();
        }
    });
    let finite = |x: f32| x.is_finite();
    for width in [0.0, -5.0, f32::NAN, f32::INFINITY, 1.0, 33.3, 1000.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        for line in layout.lines() {
            for annotation in line.annotations() {
                let InlineExtents { left, right } = annotation.inline();
                assert!(finite(left) && finite(right) && finite(annotation.baseline()));
                let _ = (
                    annotation.key(),
                    annotation.is_under(),
                    alloc::format!("{annotation:?}"),
                );
                for run in annotation.runs() {
                    let InlineExtents {
                        left: from,
                        right: to,
                    } = run.inline();
                    assert!(
                        from >= left - 0.02 && to <= right + 0.02,
                        "{run:?} in {annotation:?}"
                    );
                    assert!(run.glyphs().all(|glyph| finite(glyph.x) && finite(glyph.y)));
                    assert!(run.clusters().all(|cluster| finite(cluster.advance())));
                    assert!(
                        run.emphasis_marks()
                            .all(|mark| finite(mark.x) && finite(mark.baseline))
                    );
                }
            }
            for item in line.items() {
                if let Item::Text(run) = item {
                    assert!(
                        run.emphasis_marks()
                            .all(|mark| finite(mark.x) && finite(mark.baseline))
                    );
                }
            }
            assert!(line.paints(|_| Decorates::Both).count() > 0);
        }
    }
}

/// A justified line justifies a ruby base as its own text where the base is
/// at least as wide as its annotations, the column and its annotation's box
/// growing with it; and where an annotation is wider it takes the column as
/// one object, spreading nothing inside it, and never an annotation's text,
/// which is on a line of its own (Chrome's `JustifyResults`). In 20px Ahem
/// 200 wide, `A C C D` under a 10px `xx` over `C C` has three spaces for its
/// 60 of room, 20 each, one inside the base, whose box goes from 60 to 140
/// with the annotation centred in it; `A`, `C` under a 10px `x x x x`, and
/// ` D E` has two, the spaces after the column, 20 each, and none in the
/// column or its annotation.
#[test]
fn a_justified_line_justifies_a_ruby_base_as_chrome_does() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let block = ComputedBlockStyle {
        text_align: TextAlign::Justify,
        text_align_last: TextAlignLast::Justify,
        ..ComputedBlockStyle::new(&root)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "A ");
        b.open_ruby(NodeKey(2), &root, None);
        b.text(NodeKey(3), "C C");
        b.open_annotation(NodeKey(4), &small, None);
        b.text(NodeKey(5), "xx");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), " D");
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    let starts: Vec<f32> = runs(&layout, 0)
        .iter()
        .flat_map(|run| {
            run.clusters()
                .map(|cluster| cluster.inline().left)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(starts, [0.0, 20.0, 60.0, 80.0, 120.0, 140.0, 180.0]);
    let line = layout.line(0).expect("a line");
    let annotation = line.annotations().next().expect("an annotation");
    assert_eq!(annotation.inline(), along(60.0, 140.0));
    // Chrome's text fragment takes in the room centring it, from 60 to
    // 140, its glyphs from 90.
    let text = annotation.runs().next().expect("its text");
    assert_eq!(text.inline(), along(60.0, 140.0));
    assert_eq!(text.glyphs().next().map(|glyph| glyph.x), Some(90.0));

    build(&mut cx, &mut layout, &block, |b| {
        b.text(NodeKey(1), "A");
        b.open_ruby(NodeKey(2), &root, None);
        b.text(NodeKey(3), "C");
        b.open_annotation(NodeKey(4), &small, None);
        b.text(NodeKey(5), "x x x x");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), " D E");
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    let starts: Vec<(NodeKey, f32)> = runs(&layout, 0)
        .iter()
        .flat_map(|run| {
            let key = run.key();
            run.glyphs()
                .map(move |glyph| (key, glyph.x))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        starts,
        [
            (NodeKey(1), 0.0),
            (NodeKey(3), 40.0),
            (NodeKey(6), 80.0),
            (NodeKey(6), 120.0),
            (NodeKey(6), 140.0),
            (NodeKey(6), 180.0)
        ]
    );
    let line = layout.line(0).expect("a line");
    let annotation = line.annotations().next().expect("an annotation");
    assert_eq!(annotation.inline(), along(15.0, 85.0));
}

/// On a justified line a ruby column at the line's edge sits with its base
/// flush to it, all its room on the other side, as Chrome's `ApplyRubyAlign`
/// places it with `on_start_edge` and `on_end_edge`. Measured with Chrome 153, 20px Ahem under 10px
/// annotations 200 wide, where each base's ink is: `X` under `AAAAAA`
/// starting a justified line at 0, and at 20 under `text-align: left` or
/// `ruby-align: center`; starting the last line, `text-align-last` being
/// start, at 0 too; `XX` under eight `A`s at 0; `X X` under ten at 0 and
/// 60, the room inside it as ever, the rest after it; ending a line that
/// `text-align-last: justify` justifies, `X X` at 120 and 180; ending the
/// first line with a space after it, not flush, at 165; in a paragraph
/// that reads right to left, never flush. A column that is all a
/// justified line holds spreads its base over the whole line: `X` at 90,
/// `XX` at 80, `X X` at 0 and 180.
#[test]
fn a_ruby_column_at_a_justified_line_edge_sits_flush_with_it() {
    use crate::style::RubyAlign;
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let justify = ComputedBlockStyle {
        text_align: TextAlign::Justify,
        ..ComputedBlockStyle::new(&root)
    };
    let both = ComputedBlockStyle {
        text_align_last: TextAlignLast::Justify,
        ..justify
    };
    // Where the base's glyphs are on the first line.
    let base = |cx: &mut Context,
                layout: &mut Layout,
                block: &ComputedBlockStyle,
                ruby: &ComputedStyle<'_>,
                (before, x, reading, after): (&str, &str, &str, &str)| {
        build(
            cx,
            layout,
            &ComputedBlockStyle {
                style: &root,
                ..*block
            },
            |b| {
                b.text(NodeKey(1), before);
                b.open_ruby(NodeKey(2), ruby, None);
                b.text(NodeKey(3), x);
                b.open_annotation(NodeKey(4), &small, None);
                b.text(NodeKey(5), reading);
                b.close_annotation();
                b.close_ruby();
                b.text(NodeKey(6), after);
            },
        );
        layout.break_lines(cx, Area::new(200.0), &mut NoExclusions);
        let text = layout.text();
        runs(layout, 0)
            .iter()
            .filter(|run| run.key() == NodeKey(3))
            .flat_map(|run| run.glyphs().collect::<Vec<_>>())
            .filter(|glyph| {
                text.get(glyph.text_offset..)
                    .is_some_and(|t| t.starts_with('X'))
            })
            .map(|glyph| glyph.x)
            .collect::<Vec<f32>>()
    };
    let words = " XX XX XX XX XX XX XX";
    let left = ComputedBlockStyle {
        text_align: TextAlign::Left,
        ..justify
    };
    let mut centred = root;
    centred.ruby.align = RubyAlign::Center;
    for (block, ruby, parts, xs) in [
        (&justify, &root, ("", "X", "AAAAAA", words), &[0.0][..]),
        (&left, &root, ("", "X", "AAAAAA", words), &[20.0]),
        (&justify, &centred, ("", "X", "AAAAAA", words), &[20.0]),
        (&justify, &root, ("", "X", "AAAAAA", " XX"), &[0.0]),
        (&justify, &root, ("", "XX", "AAAAAAAA", words), &[0.0, 20.0]),
        (
            &justify,
            &root,
            ("", "X X", "AAAAAAAAAA", words),
            &[0.0, 60.0],
        ),
        (
            &both,
            &root,
            ("XX ", "X X", "AAAAAAAAAA", ""),
            &[120.0, 180.0],
        ),
        (
            &justify,
            &root,
            ("XX XX ", "X", "AAAAAA", " XX XX XX XX"),
            &[165.0],
        ),
        (&both, &root, ("", "X", "AAAAAA", ""), &[90.0]),
        (&both, &root, ("", "XX", "AAAAAAAA", ""), &[80.0, 100.0]),
        (&both, &root, ("", "X X", "AAAAAAAAAA", ""), &[0.0, 180.0]),
    ] {
        let found = base(&mut cx, &mut layout, block, ruby, parts);
        assert_eq!(found, xs, "{parts:?}");
    }
    // The column all a line holds is as wide as the line, its annotation
    // centred in it (Chrome: the ruby's and the annotation's boxes 0 to
    // 200, the `A`s from 70).
    base(&mut cx, &mut layout, &both, &root, ("", "X", "AAAAAA", ""));
    let line = layout.line(0).expect("a line");
    let annotation = line.annotations().next().expect("an annotation");
    assert_eq!(annotation.inline(), along(0.0, 200.0));
    let text = annotation.runs().next().expect("its text");
    assert_eq!(text.inline(), along(0.0, 200.0));
    assert_eq!(text.glyphs().next().map(|glyph| glyph.x), Some(70.0));
    // Right to left, bidi is at work, and nothing is flush: the base, whose
    // `X` reads left to right with the words after it, stands centred in
    // its column at the line's left, not at 0. (Chrome sets a column as an
    // object replacement character, neutral, which an RTL paragraph puts
    // at its right edge; its base's ink is at 165 there, not flush either.)
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..justify
    };
    let found = base(
        &mut cx,
        &mut layout,
        &rtl,
        &root,
        ("", "X", "AAAAAA", words),
    );
    assert_eq!(found, [20.0]);
}

/// A box inside a ruby annotation has a fragment of its own on the
/// annotation line, and its edges take room there. Measured with Chrome 153, 20px Ahem bases and
/// 10px annotations: `A<span>B</span>C` over `XXXX`, the span with a 4px
/// left margin, a 3px left border and 2px of left padding, sets `A` at
/// 20.5, `B` at 39.5 and `C` at 49.5, and the span's border box from 34.5
/// to 49.5; with a 2px border all round, 3px of padding and 1px of margin
/// either side and a background, `A` at 19, `B` at 35, `C` at 51, and the
/// span from 30 to 50 and 2px past the annotation's em box over and under;
/// nested, a painted span around `B` and a padded one around `C`, `A` at
/// 15, `B` at 25, `C` at 40, `D` at 55, the spans from 25 and from 35 to
/// 55. Each is painted before the annotation's text, and answers
/// `box_fragments` by its key.
#[test]
fn a_box_inside_an_annotation_has_its_own_fragment() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let lefts = |layout: &Layout| -> Vec<(f32, f32)> {
        let line = layout.line(0).expect("a line");
        let annotation = line.annotations().next().expect("an annotation");
        annotation
            .runs()
            .flat_map(|run| run.glyphs().collect::<Vec<_>>())
            .map(|glyph| (glyph.x, glyph.advance))
            .collect()
    };
    // Each box's key, and its border box along the line and across it.
    type Spans = Vec<(NodeKey, InlineExtents, CrossExtents)>;
    let spans = |layout: &Layout| -> Spans {
        let line = layout.line(0).expect("a line");
        let annotation = line.annotations().next().expect("an annotation");
        annotation
            .boxes()
            .map(|span| (span.key(), span.inline(), span.block()))
            .collect()
    };
    // One side's edges.
    let left = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: 4.0,
                ..Sides::ZERO
            }
            .into(),
            border: Sides {
                left: 3.0,
                ..Sides::ZERO
            },
            padding: Sides::<f32> {
                left: 2.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..small
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &root, None);
        b.text(NodeKey(2), "XXXX");
        b.open_annotation(NodeKey(3), &small, None);
        b.text(NodeKey(4), "A");
        b.open_box(NodeKey(5), &left, None);
        b.text(NodeKey(6), "B");
        b.close_box();
        b.text(NodeKey(7), "C");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    let xs: Vec<f32> = lefts(&layout).iter().map(|&(x, _)| x).collect();
    assert_eq!(xs, [20.5, 39.5, 49.5]);
    assert_eq!(
        spans(&layout),
        [(NodeKey(5), along(34.5, 49.5), across(0.0, 10.0))]
    );
    // Edges all round, painted.
    let round = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: 1.0,
                right: 1.0,
                ..Sides::ZERO
            }
            .into(),
            border: Sides::all(2.0),
            padding: Sides::<f32> {
                left: 3.0,
                right: 3.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..small
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &root, None);
        b.text(NodeKey(2), "XXXX");
        b.open_annotation(NodeKey(3), &small, None);
        b.text(NodeKey(4), "A");
        b.open_box(NodeKey(5), &round, None);
        b.text(NodeKey(6), "B");
        b.close_box();
        b.text(NodeKey(7), "C");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    let xs: Vec<f32> = lefts(&layout).iter().map(|&(x, _)| x).collect();
    assert_eq!(xs, [19.0, 35.0, 51.0]);
    assert_eq!(
        spans(&layout),
        [(NodeKey(5), along(30.0, 50.0), across(-2.0, 12.0))]
    );
    let found: Vec<InlineExtents> = layout
        .box_fragments(NodeKey(5))
        .map(|span| span.inline())
        .collect();
    assert_eq!(found, [along(30.0, 50.0)]);
    let line = layout.line(0).expect("a line");
    let order: Vec<&str> = line
        .paints(|_| Decorates::None)
        .map(|item| match item {
            Paint::Box(_) => "box",
            Paint::Annotation(_) => "annotation",
            Paint::Text(_) => "text",
            _ => "other",
        })
        .filter(|kind| *kind != "other")
        .collect();
    // The ruby container's box, then the span's.
    assert_eq!(
        order,
        [
            "box",
            "box",
            "annotation",
            "annotation",
            "annotation",
            "text"
        ]
    );
    // Nested: a painted box with no edges around `B` and a padded one
    // around `C`.
    let painted = ComputedStyle {
        paints: true,
        ..small
    };
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 5.0,
                right: 5.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..small
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &root, None);
        b.text(NodeKey(2), "XXXX");
        b.open_annotation(NodeKey(3), &small, None);
        b.text(NodeKey(4), "A");
        b.open_box(NodeKey(5), &painted, None);
        b.text(NodeKey(6), "B");
        b.open_box(NodeKey(7), &padded, None);
        b.text(NodeKey(8), "C");
        b.close_box();
        b.close_box();
        b.text(NodeKey(9), "D");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    let xs: Vec<f32> = lefts(&layout).iter().map(|&(x, _)| x).collect();
    assert_eq!(xs, [15.0, 25.0, 40.0, 55.0]);
    let found: Vec<(NodeKey, InlineExtents)> = spans(&layout)
        .iter()
        .map(|&(key, inline, _)| (key, inline))
        .collect();
    assert_eq!(
        found,
        [
            (NodeKey(5), along(25.0, 55.0)),
            (NodeKey(7), along(35.0, 55.0))
        ]
    );
}

/// An annotation over a base whose text is shifted stands on the shifted
/// text's em boxes, not on the line's baseline, as Chrome's
/// `ComputeEmHeight` takes each item where it is placed. Measured with Chrome 153, 20px Ahem under 10px
/// annotations: `XX` raised 10 in a span, with 30 of room over the line,
/// makes a line 30 tall, the annotation's em box from -10 to 0 over the
/// raised text's top, its baseline at -2; raising only the second `X` puts
/// it over the higher; lowered 10, with no room over it, the annotation
/// stands on the lowered text at 0 to 10 in a line 30 tall, and under it,
/// from 30 to 40 in a line 40 tall; `vertical-align: super` raises it with
/// its text, 0 to 10 in a line 37.65625 tall.
#[test]
fn an_annotation_stands_on_its_shifted_base() {
    use crate::style::{RubyPosition, VerticalAlign};
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let shifted = |align: VerticalAlign| {
        let mut style = root;
        style.line.vertical_align = align;
        style
    };
    let raised = shifted(VerticalAlign::Px(10.0));
    let lowered = shifted(VerticalAlign::Px(-10.0));
    let sup = shifted(VerticalAlign::Super);
    let mut under = root;
    under.ruby.position = RubyPosition::Under;
    // The line's height and its annotation's baseline, from its top.
    let mut place = |ruby: &ComputedStyle<'_>,
                     before: Option<&ComputedStyle<'_>>,
                     span: &ComputedStyle<'_>,
                     after: &str,
                     reading: &str,
                     room: f32| {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.open_ruby(NodeKey(1), ruby, None);
            if let Some(before) = before {
                b.open_box(NodeKey(2), before, None);
                b.text(NodeKey(3), "X");
                b.close_box();
            }
            b.open_box(NodeKey(4), span, None);
            b.text(NodeKey(5), after);
            b.close_box();
            b.open_annotation(NodeKey(6), &small, None);
            b.text(NodeKey(7), reading);
            b.close_annotation();
            b.close_ruby();
        });
        let area = Area {
            room_above: room,
            ..Area::new(200.0)
        };
        layout.break_lines(&mut cx, area, &mut NoExclusions);
        let line = layout.line(0).expect("a line");
        let annotation = line.annotations().next().expect("an annotation");
        (line.metrics().height(), annotation.baseline())
    };
    assert_eq!(place(&root, None, &raised, "XX", "AA", 30.0), (30.0, -2.0));
    assert_eq!(
        place(&root, Some(&root), &raised, "X", "AAAA", 30.0),
        (30.0, -2.0)
    );
    assert_eq!(place(&root, None, &root, "XX", "AA", 30.0), (20.0, -2.0));
    assert_eq!(place(&root, None, &lowered, "XX", "AA", 0.0), (30.0, 8.0));
    assert_eq!(place(&under, None, &lowered, "XX", "AA", 0.0), (40.0, 38.0));
    assert_eq!(place(&root, None, &sup, "XX", "AA", 0.0), (37.65625, 8.0));
}

// Nesting ------------------------------------------------------------------

/// An outer column keeps multiple inner columns together; each annotation
/// belongs to its own column, with the outer annotation spanning the base.
#[test]
fn nested_ruby_columns_place_separate_annotations_and_stay_whole() {
    use crate::style::WritingMode;
    let mut cx = context();
    let mut base = sized(&AHEM_FAMILY, 16.0);
    base.text.word_break = WordBreak::BreakAll;
    let small = sized(&AHEM_FAMILY, 8.0);
    let mut layout = Layout::new();
    for mode in [
        WritingMode::HorizontalTb,
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
    ] {
        build(
            &mut cx,
            &mut layout,
            &ComputedBlockStyle {
                writing_mode: mode,
                ..ComputedBlockStyle::new(&base)
            },
            |b| {
                b.open_ruby(NodeKey(1), &base, None);
                b.open_ruby(NodeKey(2), &base, None);
                b.text(NodeKey(3), "A");
                b.open_annotation(NodeKey(4), &small, None);
                b.text(NodeKey(5), "abcd");
                b.close_annotation();
                b.text(NodeKey(6), "B");
                b.open_annotation(NodeKey(7), &small, None);
                b.text(NodeKey(8), "xyz");
                b.close_annotation();
                b.close_ruby();
                b.text(NodeKey(9), "C");
                b.open_annotation(NodeKey(10), &small, None);
                b.text(NodeKey(11), "reading");
                b.close_annotation();
                b.close_ruby();
            },
        );
        for width in [400.0, 40.0, 400.0] {
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            assert_eq!(layout.lines().count(), 1);
            let line = layout.line(0).unwrap();
            let mut annotations = line
                .annotations()
                .map(|annotation| (annotation.key(), annotation.inline()))
                .collect::<Vec<_>>();
            annotations.sort_by_key(|(key, _)| key.0);
            assert_eq!(
                annotations,
                [
                    (NodeKey(4), along(0.0, 32.0)),
                    (NodeKey(7), along(32.0, 56.0)),
                    (NodeKey(10), along(0.0, 72.0)),
                ]
            );
            // Each base's glyphs; its run takes in the room its own column
            // spreads beside it.
            let base_runs = runs(&layout, 0);
            assert_eq!(
                base_runs
                    .iter()
                    .map(|run| (run.key(), run.glyphs().next().map_or(0.0, |glyph| glyph.x)))
                    .collect::<Vec<_>>(),
                [(NodeKey(3), 8.0), (NodeKey(6), 36.0), (NodeKey(9), 56.0)]
            );
        }
    }
}

#[test]
fn outer_ruby_alignment_spreads_inner_columns_as_units() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 16.0);
    let small = sized(&AHEM_FAMILY, 8.0);
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.open_ruby(NodeKey(2), &base, None);
        b.text(NodeKey(3), "A");
        b.open_annotation(NodeKey(4), &small, None);
        b.text(NodeKey(5), "a");
        b.close_annotation();
        b.text(NodeKey(6), "B");
        b.open_annotation(NodeKey(7), &small, None);
        b.text(NodeKey(8), "b");
        b.close_annotation();
        b.close_ruby();
        b.open_annotation(NodeKey(9), &small, None);
        b.text(NodeKey(10), "abcdefghijklmnopqrst");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        runs(&layout, 0)
            .iter()
            .map(|run| run.glyphs().next().map_or(0.0, |glyph| glyph.x))
            .collect::<Vec<_>>(),
        [32.0, 112.0]
    );
    let annotations = layout
        .line(0)
        .unwrap()
        .annotations()
        .map(|annotation| (annotation.key(), annotation.inline()))
        .collect::<Vec<_>>();
    assert_eq!(
        annotations,
        [
            (NodeKey(4), along(32.0, 48.0)),
            (NodeKey(7), along(112.0, 128.0)),
            (NodeKey(9), along(0.0, 160.0)),
        ]
    );
}

#[test]
fn three_nested_rubies_stack_and_center_each_base_independently() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 16.0);
    let small = sized(&AHEM_FAMILY, 8.0);
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.open_ruby(NodeKey(2), &base, None);
        b.open_ruby(NodeKey(3), &base, None);
        b.text(NodeKey(4), "A");
        b.open_annotation(NodeKey(5), &small, None);
        b.text(NodeKey(6), "abcd");
        b.close_annotation();
        b.close_ruby();
        b.open_annotation(NodeKey(7), &small, None);
        b.text(NodeKey(8), "middlereading");
        b.close_annotation();
        b.close_ruby();
        b.open_annotation(NodeKey(9), &small, None);
        b.text(NodeKey(10), "outer");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        runs(&layout, 0)[0].glyphs().next().map(|glyph| glyph.x),
        Some(44.0)
    );
    let annotations = layout.line(0).unwrap().annotations().collect::<Vec<_>>();
    assert_eq!(
        annotations
            .iter()
            .map(|annotation| (annotation.key(), annotation.inline()))
            .collect::<Vec<_>>(),
        [
            (NodeKey(5), along(36.0, 68.0)),
            (NodeKey(7), along(0.0, 104.0)),
            (NodeKey(9), along(0.0, 104.0)),
        ]
    );
    assert!(annotations[0].baseline() > annotations[1].baseline());
    assert!(annotations[1].baseline() > annotations[2].baseline());
}

#[test]
fn deep_ruby_nesting_is_bounded_and_keeps_balanced_annotations() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 16.0);
    let small = sized(&AHEM_FAMILY, 8.0);
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        for level in 0..40 {
            b.open_ruby(NodeKey(level + 1), &base, None);
        }
        b.text(NodeKey(100), "A");
        for level in 0..40 {
            b.open_annotation(NodeKey(level + 200), &small, None);
            b.text(NodeKey(level + 300), "x");
            b.close_annotation();
            b.close_ruby();
        }
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(layout.lines().count(), 1);
    assert_eq!(layout.line(0).unwrap().annotations().count(), 40);
    assert_eq!(runs(&layout, 0)[0].key(), NodeKey(100));
}

#[test]
fn empty_nested_annotation_edges_do_not_enlarge_neighboring_glyphs() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 16.0);
    let mut padded = base;
    padded.edges.padding = Sides::<f32> {
        left: 3.0,
        right: 5.0,
        ..Sides::default()
    }
    .into();
    let mut layout = Layout::new();
    for before in [false, true] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
            b.open_ruby(NodeKey(1), &base, None);
            if before {
                b.text(NodeKey(2), "A");
            }
            b.open_ruby(NodeKey(3), &base, None);
            b.open_annotation(NodeKey(4), &padded, None);
            b.close_annotation();
            b.close_ruby();
            if !before {
                b.text(NodeKey(2), "A");
            }
            b.open_annotation(NodeKey(5), &base, None);
            b.close_annotation();
            b.close_ruby();
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        assert_eq!(
            runs(&layout, 0)[0].inline(),
            if before {
                along(0.0, 16.0)
            } else {
                along(8.0, 24.0)
            }
        );
    }
}

// Splitting ----------------------------------------------------------------

#[test]
fn a_long_ruby_resumes_its_base_and_annotation_independently() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.text(NodeKey(1), "XX ");
        b.open_ruby(NodeKey(2), &base, None);
        b.text(NodeKey(3), "AAAA BBBB CCCC DDDD");
        b.open_annotation(NodeKey(4), &small, None);
        b.text(NodeKey(5), "aaaa bbbb cccc dddd");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), " ZZ");
    });
    for width in [200.0, 400.0, 200.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        let rows: Vec<_> = layout
            .lines()
            .map(|line| {
                let base = line
                    .items()
                    .filter_map(|item| match item {
                        Item::Text(run) => Some(run),
                        _ => None,
                    })
                    .map(|run| &layout.text()[run.text_range()])
                    .collect::<String>();
                let annotation = line
                    .annotations()
                    .flat_map(|annotation| annotation.runs())
                    .map(|run| &layout.text()[run.text_range()])
                    .collect::<String>();
                (base, annotation)
            })
            .collect();
        if width == 200.0 {
            assert_eq!(
                rows,
                [
                    ("XX AAAA".into(), "aaaa".into()),
                    ("BBBB CCCC".into(), "bbbb cccc".into()),
                    ("DDDD ZZ".into(), "dddd".into())
                ]
            );
        }
    }
}

fn split_ruby_rows(layout: &Layout) -> Vec<(String, String)> {
    layout
        .lines()
        .map(|line| {
            (
                line.items()
                    .filter_map(|item| match item {
                        Item::Text(run) => Some(run),
                        _ => None,
                    })
                    .map(|run| &layout.text()[run.text_range()])
                    .collect(),
                line.annotations()
                    .flat_map(|annotation| annotation.runs())
                    .map(|run| &layout.text()[run.text_range()])
                    .collect(),
            )
        })
        .collect()
}

#[test]
fn a_ruby_splits_across_lines_in_every_writing_mode() {
    use crate::style::{TextWrapMode, TextWrapStyle, WritingMode};
    let mut cx = context();
    let mut base = sized(&AHEM_FAMILY, 20.0);
    let mut small = sized(&AHEM_FAMILY, 10.0);
    for mode in [
        WritingMode::HorizontalTb,
        WritingMode::VerticalRl,
        WritingMode::VerticalLr,
    ] {
        for (before, text, annotation, after, width, all, expected) in [
            (
                "",
                "ABCD",
                "abcdefgh",
                "",
                40.0,
                true,
                vec![("ABCD", "abcdefgh")],
            ),
            (
                "",
                "ABCDE",
                "abcdefgh",
                "",
                40.0,
                true,
                vec![("AB", "abc"), ("CDE", "defgh")],
            ),
            (
                "XX ",
                "AAAA BBBB CCCC DDDD",
                "abcdefghijklmnopqrs",
                " ZZ",
                200.0,
                false,
                vec![
                    ("XX", ""),
                    ("AAAA BBBB", "abcdefghijklmnopqrs"),
                    ("CCCC DDDD", ""),
                    ("ZZ", ""),
                ],
            ),
            (
                "XX ",
                "AAA BBB CC",
                "aaa bbb cc",
                "YYYY",
                300.0,
                false,
                vec![("XX AAA BBB", "aaa bbb"), ("CCYYYY", "cc")],
            ),
        ] {
            base.text.word_break = if all {
                WordBreak::BreakAll
            } else {
                WordBreak::Normal
            };
            small.text.word_break = base.text.word_break;
            let block = ComputedBlockStyle {
                writing_mode: mode,
                ..ComputedBlockStyle::new(&base)
            };
            let mut layout = Layout::new();
            build(&mut cx, &mut layout, &block, |b| {
                b.text(NodeKey(1), before);
                b.open_ruby(NodeKey(2), &base, None);
                b.text(NodeKey(3), text);
                b.open_annotation(NodeKey(4), &small, None);
                b.text(NodeKey(5), annotation);
                b.close_annotation();
                b.close_ruby();
                b.text(NodeKey(6), after);
            });
            layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
            assert_eq!(
                split_ruby_rows(&layout),
                expected
                    .iter()
                    .map(|(base, annotation)| ((*base).into(), (*annotation).into()))
                    .collect::<Vec<_>>(),
                "{mode:?} {text:?}"
            );
        }
    }
    for mode in [TextWrapStyle::Pretty, TextWrapStyle::Balance] {
        let block = ComputedBlockStyle {
            text_wrap_style: mode,
            ..ComputedBlockStyle::new(&base)
        };
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &block, |b| {
            b.open_ruby(NodeKey(1), &base, None);
            b.text(NodeKey(2), "AAAA BBBB CCCC");
            b.open_annotation(NodeKey(3), &small, None);
            b.text(NodeKey(4), "aaaa bbbb cccc");
            b.close_ruby();
        });
        layout.break_lines(&mut cx, Area::new(40.0), &mut NoExclusions);
        assert_eq!(layout.lines().count(), 1);
    }
    base.text.wrap_mode = TextWrapMode::NoWrap;
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "AAAA BBBB CCCC");
        b.open_annotation(NodeKey(3), &small, None);
        b.text(NodeKey(4), "aaaa bbbb cccc");
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(40.0), &mut NoExclusions);
    assert_eq!(layout.lines().count(), 1);
}

/// Sliced annotation boxes rebuild their open-box context on continuation
/// lines, charging their opening and closing padding only at the real edges.
/// Under `ruby-align: start` each part reaches the column's end, over the
/// room after its text, as Chrome's client rects do.
#[test]
fn split_ruby_slices_annotation_box_edges() {
    let mut cx = context();
    let mut layout = Layout::new();
    let mut base = sized(&AHEM_FAMILY, 20.0);
    base.ruby.align = RubyAlign::Start;
    let small = sized(&AHEM_FAMILY, 10.0);
    let mut boxed = small;
    boxed.edges.padding = Sides::<f32> {
        left: 5.0,
        right: 7.0,
        top: 0.0,
        bottom: 0.0,
    }
    .into();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "AAAA BBBB CCCC DDDD");
        b.open_annotation(NodeKey(3), &small, None);
        b.open_box(NodeKey(4), &boxed, None);
        b.text(NodeKey(5), "aaaa bbbb cccc dddd");
        b.close_box();
        b.close_annotation();
        b.close_ruby();
    });
    for width in [200.0, 120.0, 400.0, 200.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        let annotations: Vec<_> = layout.lines().flat_map(|line| line.annotations()).collect();
        assert!(!annotations.is_empty());
        for (index, annotation) in annotations.iter().enumerate() {
            let span = annotation
                .boxes()
                .find(|span| span.key() == NodeKey(4))
                .unwrap();
            let runs: Vec<_> = annotation.runs().collect();
            let left = runs
                .iter()
                .map(|run| run.inline().left)
                .fold(f32::INFINITY, f32::min);
            let right = runs
                .iter()
                .map(|run| run.inline().right)
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(
                (left - span.inline().left - if index == 0 { 5.0 } else { 0.0 }).abs() < 0.02,
                "{width}, {index}: leading edge"
            );
            // The room `start` leaves after the text is inside the box, so
            // the box reaches the column's end, as Chrome's does.
            assert!(
                (span.inline().right - annotation.inline().right).abs() < 0.02
                    && span.inline().right >= right,
                "{width}, {index}: trailing edge"
            );
        }
    }
}

/// Ruby's annotation height can narrow a host's band after a tentative
/// fit. Refitting and an automatic height clamp discard tentative cursors.
#[test]
fn split_ruby_refits_and_rewinds_with_exclusions() {
    use core::cell::Cell;
    #[derive(Default)]
    struct Bands {
        wide: Cell<usize>,
        narrow: Cell<usize>,
        rewinds: usize,
    }
    impl crate::Exclusions for Bands {
        fn band(&self, _line: usize, block: crate::BlockExtents) -> InlineExtents {
            let width = if block.end - block.start > 20.0 {
                self.narrow.set(self.narrow.get() + 1);
                200.0
            } else {
                self.wide.set(self.wide.get() + 1);
                400.0
            };
            InlineExtents {
                left: 0.0,
                right: width,
            }
        }
        fn below(&self, _top: f32) -> Option<f32> {
            None
        }
        fn place(&mut self, _float: crate::FloatRequest) -> crate::PlacedFloat {
            crate::PlacedFloat::default()
        }
        fn checkpoint(&self) -> crate::ExclusionsCheckpoint {
            crate::ExclusionsCheckpoint(0)
        }
        fn rewind(&mut self, _to: crate::ExclusionsCheckpoint) {
            self.rewinds += 1;
        }
    }
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    for clamp in [LineClamp::None, LineClamp::Auto] {
        let mut layout = Layout::new();
        let block = ComputedBlockStyle {
            line_clamp: clamp,
            ..ComputedBlockStyle::new(&base)
        };
        build(&mut cx, &mut layout, &block, |b| {
            b.open_ruby(NodeKey(1), &base, None);
            b.text(NodeKey(2), "AAAA BBBB CCCC DDDD EEEE FFFF");
            b.open_annotation(NodeKey(3), &small, None);
            b.text(NodeKey(4), "aaaa bbbb cccc dddd eeee ffff");
            b.close_ruby();
        });
        let area = Area {
            block_end: Some(40.0),
            ..Area::new(400.0)
        };
        let mut host = Bands::default();
        layout.break_lines(&mut cx, area, &mut host);
        assert!(host.wide.get() > 0 && host.narrow.get() > 0);
        if clamp == LineClamp::Auto {
            assert!(host.rewinds > 0);
        }
        let actual = split_ruby_rows(&layout);
        let count = layout.lines().count();
        if let Some(ruby) = &layout.line_records().ruby {
            assert!(
                ruby.pieces
                    .iter()
                    .all(|(_, piece)| piece.line.get() < count)
            );
        }
        layout.break_lines(
            &mut cx,
            Area {
                block_end: Some(40.0),
                ..Area::new(200.0)
            },
            &mut NoExclusions,
        );
        assert_eq!(split_ruby_rows(&layout), actual, "{clamp:?}");
    }
}

/// The stricter annotation-opportunity requirement affects intrinsic
/// sizing as well as fitting; independently unbreakable annotations keep
/// the whole base in Spec mode.
#[test]
fn ruby_break_within_configuration_agrees_with_intrinsic_sizing() {
    use crate::config::RubyBreakWithin;
    for mode in [
        RubyBreakWithin::BaseOpportunities,
        RubyBreakWithin::AllLevels,
    ] {
        let mut cx = context();
        let mut config = Config::chrome_windows();
        config.ruby_break_within = mode;
        cx.set_config(config);
        let base = sized(&AHEM_FAMILY, 20.0);
        let small = sized(&AHEM_FAMILY, 10.0);
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
            b.open_ruby(NodeKey(1), &base, None);
            b.text(NodeKey(2), "AAAA BBBB CCCC DDDD");
            b.open_annotation(NodeKey(3), &small, None);
            b.text(NodeKey(4), "abcdefghijklmnopqrs");
            b.close_ruby();
        });
        layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
        if mode == RubyBreakWithin::AllLevels {
            assert_eq!(layout.lines().count(), 1);
            assert_eq!(layout.intrinsic_sizes().min_content, 380.0);
        } else {
            assert!(layout.lines().count() > 1);
            assert!(layout.intrinsic_sizes().min_content < 380.0);
        }
        assert_eq!(layout.intrinsic_sizes().max_content, 380.0);
    }
}

#[test]
fn adjacent_short_ruby_keeps_its_last_base_on_the_line() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.text(NodeKey(1), "nihongo | ");
        for (index, (text, annotation)) in [("n ", "n"), ("hon", "hon"), ("go ", "go")]
            .into_iter()
            .enumerate()
        {
            let key = (index * 4 + 2) as u64;
            b.open_ruby(NodeKey(key), &base, None);
            b.text(NodeKey(key + 1), text);
            b.open_annotation(NodeKey(key + 2), &small, None);
            b.text(NodeKey(key + 3), annotation);
            b.close_ruby();
        }
        b.text(NodeKey(20), "|");
    });
    layout.break_lines(&mut cx, Area::new(240.0), &mut NoExclusions);
    let rows = split_ruby_rows(&layout);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0.trim_end(), "nihongo | n");
    assert_eq!(rows[0].1, "n");
    assert_eq!(rows[1], (String::from("hongo |"), String::from("hongo")));
}

/// A base with no text keeps its annotation, set over the ruby
/// container's em box, and nothing overhangs beside it. Chrome 153, 20px
/// Ahem with 10px annotations, `A<ruby><rt>XXXX</rt></ruby>B`: the
/// annotation from 20 to 60 with its top at the line's, `B` at 60, the line
/// 30 tall with its baseline at 26. A base holding only a float or an
/// out-of-flow box is laid out the same.
#[test]
fn an_empty_ruby_base_keeps_its_annotation() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    for float in [false, true] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "A");
            b.open_ruby(NodeKey(2), &root, None);
            if float {
                let size = BoxSize {
                    inline: 5.0,
                    block: 5.0,
                    baseline: None,
                };
                b.float(NodeKey(3), &root, crate::FloatSide::Left, size);
            }
            b.open_annotation(NodeKey(4), &small, None);
            b.text(NodeKey(5), "XXXX");
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(6), "B");
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        let line = layout.line(0).expect("a line");
        let metrics = line.metrics();
        assert_eq!((metrics.ascent, metrics.height()), (26.0, 30.0), "{float}");
        let lefts: Vec<(NodeKey, f32)> = runs(&layout, 0)
            .iter()
            .map(|run| (run.key(), run.inline().left))
            .collect();
        assert_eq!(lefts, [(NodeKey(1), 0.0), (NodeKey(6), 60.0)], "{float}");
        let annotations: Vec<_> = line
            .annotations()
            .map(|annotation| (annotation.key(), annotation.inline(), annotation.baseline()))
            .collect();
        assert_eq!(
            annotations,
            [(NodeKey(4), along(20.0, 60.0), 8.0)],
            "{float}"
        );
    }
}

/// A ruby container keeps a box fragment like any inline box, its edges
/// taking room at both ends, with or without annotations. Chrome 153, 20px
/// Ahem with 10px annotations, the ruby with margins 20 and 10, borders 5
/// and 10, and padding 10 and 20, left and right:
/// - `A<ruby>XX<rt>XXXX</rt></ruby>B`: the box from 40 to 125, `XX` at 55,
///   `B` at 135;
/// - with a second column `YY` over `yy`: the box from 40 to 165, `B` at
///   175;
/// - `XYZ<ruby><rb>ABC</rb><rb>DEF</rb></ruby>XYZ`, no annotation: the box
///   from 80 to 245, the last `XYZ` at 255, 315 wide at min-content and at
///   max-content.
#[test]
fn a_ruby_container_keeps_a_box_whose_edges_take_room() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let edged = ComputedStyle {
        edges: EdgesGroup {
            margin: Sides::<f32> {
                left: 20.0,
                right: 10.0,
                ..Sides::ZERO
            }
            .into(),
            border: Sides {
                left: 5.0,
                right: 10.0,
                ..Sides::ZERO
            },
            padding: Sides::<f32> {
                left: 10.0,
                right: 20.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        paints: true,
        ..root
    };
    let lefts = |layout: &Layout| -> Vec<(NodeKey, f32)> {
        runs(layout, 0)
            .iter()
            .map(|run| (run.key(), run.inline().left))
            .collect()
    };
    let ruby = |layout: &Layout| -> Vec<(f32, f32)> {
        layout
            .box_fragments(NodeKey(2))
            .map(|found| ends(found.inline()))
            .collect()
    };
    for columns in [1, 2] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "A");
            b.open_ruby(NodeKey(2), &edged, None);
            b.text(NodeKey(3), "XX");
            b.open_annotation(NodeKey(4), &small, None);
            b.text(NodeKey(5), "XXXX");
            b.close_annotation();
            if columns == 2 {
                b.text(NodeKey(7), "YY");
                b.open_annotation(NodeKey(8), &small, None);
                b.text(NodeKey(9), "yy");
                b.close_annotation();
            }
            b.close_ruby();
            b.text(NodeKey(6), "B");
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        if columns == 1 {
            assert_eq!(ruby(&layout), [(40.0, 125.0)]);
            assert_eq!(
                lefts(&layout),
                [(NodeKey(1), 0.0), (NodeKey(3), 55.0), (NodeKey(6), 135.0)]
            );
        } else {
            assert_eq!(ruby(&layout), [(40.0, 165.0)]);
            assert_eq!(lefts(&layout).last(), Some(&(NodeKey(6), 175.0)));
        }
    }
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "XYZ");
        b.open_ruby(NodeKey(2), &edged, None);
        b.open_box(NodeKey(3), &root, None);
        b.text(NodeKey(4), "ABC");
        b.close_box();
        b.open_box(NodeKey(5), &root, None);
        b.text(NodeKey(6), "DEF");
        b.close_box();
        b.close_ruby();
        b.text(NodeKey(7), "XYZ");
    });
    let sizes = layout.intrinsic_sizes();
    assert_eq!((sizes.min_content, sizes.max_content), (315.0, 315.0));
    for width in [315.0, 1000.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        assert_eq!(ruby(&layout), [(80.0, 245.0)], "{width}");
        assert_eq!(
            lefts(&layout),
            [
                (NodeKey(1), 0.0),
                (NodeKey(4), 95.0),
                (NodeKey(6), 155.0),
                (NodeKey(7), 255.0)
            ],
            "{width}"
        );
    }
}

/// An annotation outside every ruby container keeps its own style, set over
/// an empty base in an anonymous container, as Chrome wraps a `ruby-text`
/// box whose parent is no ruby. Chrome 153, 20px Ahem with 10px
/// annotations, each a `<span style="display: ruby-text">`:
/// - `A<rt>XXXX</rt>B`: the annotation from 20 to 60 with its top at the
///   line's, `B` at 60, the line 30 tall;
/// - two in a row, `A<rt>XX</rt><rt>yy</rt>C`: from 20 to 40 and from 40
///   to 60, `C` at 60;
/// - one with a 3px left border and 5px of padding either side: its box
///   from 20 to 53, its text at 28, `C` at 53.
///
/// A query by the annotation's key answers its own box, from 20 to 60 and
/// from 0 to 10 in the first, as Chrome's client rect is; the anonymous
/// container answers none.
#[test]
fn an_annotation_outside_ruby_keeps_its_own_style() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    type Found = Vec<(NodeKey, (f32, f32), f32)>;
    let found = |layout: &Layout| -> Found {
        let line = layout.line(0).expect("a line");
        line.annotations()
            .map(|annotation| {
                (
                    annotation.key(),
                    ends(annotation.inline()),
                    annotation.baseline(),
                )
            })
            .collect()
    };
    let lefts = |layout: &Layout| -> Vec<(NodeKey, f32)> {
        runs(layout, 0)
            .iter()
            .map(|run| (run.key(), run.inline().left))
            .collect()
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_annotation(NodeKey(2), &small, None);
        b.text(NodeKey(3), "XXXX");
        b.close_annotation();
        b.text(NodeKey(4), "B");
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let metrics = layout.line(0).expect("a line").metrics();
    assert_eq!((metrics.ascent, metrics.height()), (26.0, 30.0));
    assert_eq!(found(&layout), [(NodeKey(2), (20.0, 60.0), 8.0)]);
    assert_eq!(lefts(&layout), [(NodeKey(1), 0.0), (NodeKey(4), 60.0)]);
    let own: Vec<_> = layout
        .box_fragments(NodeKey(2))
        .map(|part| (ends(part.inline()), part.block()))
        .collect();
    assert_eq!(own, [((20.0, 60.0), across(0.0, 10.0))]);
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_annotation(NodeKey(2), &small, None);
        b.text(NodeKey(3), "XX");
        b.close_annotation();
        b.open_annotation(NodeKey(5), &small, None);
        b.text(NodeKey(6), "yy");
        b.close_annotation();
        b.text(NodeKey(4), "C");
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        found(&layout),
        [
            (NodeKey(2), (20.0, 40.0), 8.0),
            (NodeKey(5), (40.0, 60.0), 8.0)
        ]
    );
    assert_eq!(lefts(&layout), [(NodeKey(1), 0.0), (NodeKey(4), 60.0)]);
    let edged = ComputedStyle {
        edges: EdgesGroup {
            border: Sides {
                left: 3.0,
                ..Sides::ZERO
            },
            padding: Sides::<f32> {
                left: 5.0,
                right: 5.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..small
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "A");
        b.open_annotation(NodeKey(2), &edged, None);
        b.text(NodeKey(3), "XX");
        b.close_annotation();
        b.text(NodeKey(4), "C");
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(found(&layout), [(NodeKey(2), (20.0, 53.0), 8.0)]);
    let line = layout.line(0).expect("a line");
    let text: Vec<f32> = line
        .annotations()
        .flat_map(|annotation| annotation.runs().collect::<Vec<_>>())
        .map(|run| run.inline().left)
        .collect();
    assert_eq!(text, [28.0]);
    assert_eq!(lefts(&layout), [(NodeKey(1), 0.0), (NodeKey(4), 53.0)]);
}

/// A forced break beside a ruby that clears floats moves the next line,
/// or the block's end, below them; inside a ruby it is a space and clears
/// nothing. Chrome 153, a 100px square float on the left from a block
/// before, lines of 20, `A<ruby>X<rt>x</rt></ruby><br clear=all>B`: the
/// first line beside the float, the next at 0 and 100 down; with nothing
/// after the break, the block ends at 100; `<br clear=right>` clears
/// nothing; with the break inside the ruby, one line. The first line is 30
/// tall here, holding its annotation, with no room lent above.
#[test]
fn a_clearing_break_beside_ruby_moves_below_the_floats() {
    /// A float 100 wide and 100 tall on the left, from a block before.
    struct Floated;
    impl crate::Exclusions for Floated {
        fn band(&self, _line: usize, block: crate::BlockExtents) -> InlineExtents {
            let left = if block.start < 100.0 { 100.0 } else { 0.0 };
            InlineExtents { left, right: 400.0 }
        }
        fn below(&self, top: f32) -> Option<f32> {
            (top < 100.0).then_some(100.0)
        }
        fn place(&mut self, _float: crate::FloatRequest) -> crate::PlacedFloat {
            crate::PlacedFloat::default()
        }
        fn checkpoint(&self) -> crate::ExclusionsCheckpoint {
            crate::ExclusionsCheckpoint(0)
        }
        fn rewind(&mut self, _to: crate::ExclusionsCheckpoint) {}
    }
    let mut cx = context();
    let mut layout = Layout::new();
    let mut root = sized(&AHEM_FAMILY, 20.0);
    root.line.height = LineHeight::Px(20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let lines = |layout: &Layout| -> Vec<(f32, f32)> {
        layout
            .lines()
            .map(|line| (line.metrics().top, line.metrics().left))
            .collect()
    };
    for (clear, after, inside) in [
        (crate::Clear::Both, true, false),
        (crate::Clear::Both, false, false),
        (crate::Clear::Right, true, false),
        (crate::Clear::Both, true, true),
    ] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "A");
            b.open_ruby(NodeKey(2), &root, None);
            b.text(NodeKey(3), "X");
            if inside {
                b.line_break_clearing(NodeKey(6), clear);
                b.text(NodeKey(7), "Y");
            }
            b.open_annotation(NodeKey(4), &small, None);
            b.text(NodeKey(5), "x");
            b.close_annotation();
            b.close_ruby();
            if !inside {
                b.line_break_clearing(NodeKey(6), clear);
            }
            if after {
                b.text(NodeKey(8), "B");
            }
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut Floated);
        let case = (clear, after, inside);
        match case {
            (crate::Clear::Both, true, false) => {
                assert_eq!(lines(&layout), [(0.0, 100.0), (100.0, 0.0)]);
            }
            (crate::Clear::Both, false, false) => {
                assert_eq!(lines(&layout), [(0.0, 100.0)]);
                assert_eq!(layout.metrics().block_end, 100.0);
            }
            (crate::Clear::Right, ..) => {
                assert_eq!(lines(&layout), [(0.0, 100.0), (30.0, 100.0)]);
            }
            _ => {
                assert_eq!(lines(&layout).len(), 1, "{case:?}");
                assert_eq!(layout.metrics().block_end, 30.0);
            }
        }
    }
}

/// An atomic inline inside an annotation is placed on the annotation's
/// line, read with it and painted with its text. Chrome 153, 20px Ahem with
/// 10px annotations, `<ruby>XXXX<rt><span></span></rt></ruby>`, the span an
/// inline-block 10px square: it sits from 35 to 45, its margin box's bottom
/// on the annotation's baseline and on the base's top.
#[test]
fn an_atomic_inline_inside_an_annotation_is_placed_on_it() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &root, None);
        b.text(NodeKey(2), "XXXX");
        b.open_annotation(NodeKey(3), &small, None);
        let size = BoxSize {
            inline: 10.0,
            block: 10.0,
            baseline: None,
        };
        b.atomic(NodeKey(4), &small, None, size);
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let line = layout.line(0).expect("a line");
    let annotation = line.annotations().next().expect("an annotation");
    let atomics: Vec<(NodeKey, InlineExtents, CrossExtents, f32)> = annotation
        .atomics()
        .map(|atomic| {
            (
                atomic.key(),
                atomic.inline(),
                atomic.block(),
                atomic.baseline(),
            )
        })
        .collect();
    let base_top = runs(&layout, 0)[0].block().over;
    assert_eq!(
        atomics,
        [(
            NodeKey(4),
            along(35.0, 45.0),
            across(base_top - 10.0, base_top),
            annotation.baseline()
        )]
    );
    assert!(
        line.paints(|_| Decorates::None)
            .any(|paint| matches!(paint, Paint::Atomic(atomic) if atomic.key() == NodeKey(4)))
    );
    assert!(line.items().all(|item| !matches!(item, Item::Atomic(_))));
}

/// The text of line 0's base clusters and of its annotations' clusters,
/// each left to right.
fn visual_text(layout: &Layout) -> (String, String) {
    let mut base: Vec<(f32, &str)> = runs(layout, 0)
        .iter()
        .flat_map(|run| run.clusters().collect::<Vec<_>>())
        .map(|cluster| (cluster.inline().left, &layout.text()[cluster.text_range()]))
        .collect();
    let line = layout.line(0).expect("a line");
    let mut annotation: Vec<(f32, &str)> = line
        .annotations()
        .flat_map(|annotation| annotation.runs().collect::<Vec<_>>())
        .flat_map(|run| run.clusters().collect::<Vec<_>>())
        .map(|cluster| (cluster.inline().left, &layout.text()[cluster.text_range()]))
        .collect();
    base.sort_by(|a, b| a.0.total_cmp(&b.0));
    annotation.sort_by(|a, b| a.0.total_cmp(&b.0));
    (
        base.into_iter().map(|(_, text)| text).collect(),
        annotation.into_iter().map(|(_, text)| text).collect(),
    )
}

/// A ruby container's own `direction` and `unicode-bidi` set its base's
/// levels, as a box's do, and its column's box brackets the base however
/// it reorders. Chrome 153, 20px Ahem with 10px annotations,
/// `Q<ruby dir=rtl>1 2 3<rt>4 5 6</rt></ruby>Z`, the ruby isolated: the
/// base reads `3 2 1` from 20 to 120, and the annotation's box and the
/// ruby's run from 20 to 120.
#[test]
fn a_ruby_containers_own_bidi_orders_its_base() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let rtl = ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::Isolate,
        },
        ..root
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.text(NodeKey(1), "Q");
        b.open_ruby(NodeKey(2), &rtl, None);
        b.text(NodeKey(3), "1 2 3");
        b.open_annotation(NodeKey(4), &small, None);
        b.text(NodeKey(5), "4 5 6");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), "Z");
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let (base, _) = visual_text(&layout);
    assert_eq!(base, "Q3 2 1Z");
    let line = layout.line(0).expect("a line");
    let columns: Vec<InlineExtents> = line.annotations().map(|found| found.inline()).collect();
    assert_eq!(columns, [along(20.0, 120.0)]);
    let ruby: Vec<InlineExtents> = layout
        .box_fragments(NodeKey(2))
        .map(|found| found.inline())
        .collect();
    assert_eq!(ruby, [along(20.0, 120.0)]);
}

/// An annotation's own `direction` and `unicode-bidi` set its text's
/// levels, inside the isolate it is. Chrome 153, 20px Ahem with 10px
/// annotations, `Q<ruby>1 2 3<rt dir=rtl>4 5 6</rt></ruby>Z`, the `rt`
/// isolated: the annotation reads `6 5 4`, its glyphs at 28.33, 65 and
/// 101.67, over a base that reads `1 2 3`; with the base in an isolated
/// `<span dir=rtl>`, the base reads `3 2 1` and the annotation the same.
/// The digits' client rects take in the room `ruby-align` leaves at the
/// annotation's ends: `6` runs from 20, `5` from 65 and `4` from 101.67 to
/// 120.
#[test]
fn an_annotations_own_bidi_orders_its_text() {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let rtl = |style: &ComputedStyle<'static>| ComputedStyle {
        bidi: BidiGroup {
            direction: Direction::Rtl,
            unicode_bidi: UnicodeBidi::Isolate,
        },
        ..*style
    };
    for span in [false, true] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.text(NodeKey(1), "Q");
            b.open_ruby(NodeKey(2), &root, None);
            if span {
                b.open_box(NodeKey(7), &rtl(&root), None);
            }
            b.text(NodeKey(3), "1 2 3");
            if span {
                b.close_box();
            }
            b.open_annotation(NodeKey(4), &rtl(&small), None);
            b.text(NodeKey(5), "4 5 6");
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(6), "Z");
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        let (base, annotation) = visual_text(&layout);
        let expected = if span { "Q3 2 1Z" } else { "Q1 2 3Z" };
        assert_eq!((base.as_str(), annotation.as_str()), (expected, "6 5 4"));
        let line = layout.line(0).expect("a line");
        let found = line.annotations().next().expect("an annotation");
        assert_eq!(found.inline(), along(20.0, 120.0), "{span}");
        let mut digits: Vec<(f32, &str)> = found
            .runs()
            .flat_map(|run| run.clusters().collect::<Vec<_>>())
            .map(|cluster| (cluster.inline().left, &layout.text()[cluster.text_range()]))
            .filter(|(_, text)| *text != " ")
            .collect();
        digits.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(
            digits,
            [(20.0, "6"), (65.0, "5"), (101.671875, "4")],
            "{span}"
        );
        let mut glyphs: Vec<f32> = found
            .runs()
            .flat_map(|run| run.glyphs().map(|glyph| glyph.x).collect::<Vec<_>>())
            .collect();
        glyphs.sort_by(f32::total_cmp);
        glyphs.dedup();
        assert!(
            [28.328125, 65.0, 101.671875]
                .iter()
                .all(|x| glyphs.contains(x)),
            "{span}: {glyphs:?}"
        );
    }
}

/// An outer column may split between children; each child's annotation
/// stays with its entire base on the same physical line.
#[test]
fn split_outer_ruby_keeps_inner_columns_whole() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 20.0);
    let small = sized(&AHEM_FAMILY, 10.0);
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        for (index, (text, annotation)) in [("AB", "ab"), ("CD", "cd"), ("EF", "ef"), ("GH", "gh")]
            .into_iter()
            .enumerate()
        {
            if index > 0 {
                b.text(NodeKey(30 + index as u64), " ");
            }
            let key = 2 + index as u64 * 4;
            b.open_ruby(NodeKey(key), &base, None);
            b.text(NodeKey(key + 1), text);
            b.open_annotation(NodeKey(key + 2), &small, None);
            b.text(NodeKey(key + 3), annotation);
            b.close_ruby();
        }
        b.open_annotation(NodeKey(40), &small, None);
        b.text(NodeKey(41), "aa bb cc dd");
        b.close_ruby();
    });
    for width in [100.0, 300.0, 100.0] {
        layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
        let rows = split_ruby_rows(&layout);
        if width == 100.0 {
            assert_eq!(
                rows.iter().map(|row| row.0.trim_end()).collect::<Vec<_>>(),
                ["AB CD", "EF GH"]
            );
        } else {
            assert_eq!(rows[0].0, "AB CD EF GH");
        }
        for line in layout.lines() {
            let base: String = line
                .items()
                .filter_map(|item| match item {
                    Item::Text(run) => Some(&layout.text()[run.text_range()]),
                    _ => None,
                })
                .collect();
            for annotation in line
                .annotations()
                .filter(|annotation| annotation.key() != NodeKey(40))
            {
                let text: String = annotation
                    .runs()
                    .map(|run| &layout.text()[run.text_range()])
                    .collect();
                assert!(base.contains(&text.to_uppercase()), "{base:?}, {text:?}");
                assert_eq!(text.len(), 2);
            }
        }
    }
}

/// A clearing break ends the block below the floats, and an annotation
/// under the line reaches past that end only by what the clearance leaves.
/// Chrome adds the greater of the annotation's overflow and the clearance
/// after the line, and lends back only the overflow past the clearance
/// (`BlockLayoutAlgorithm`, `AddAnyClearanceAfterLine`). The web platform
/// tests `br-clear-all-000` to `002`: a 100px float on the left from a block
/// before, `line-height: 20px` in 16px Ahem, a ruby whose base is a 20px
/// square and whose annotation under it holds a box 50, 100 or 150 tall,
/// then `<br clear=all>`. Chrome 153 puts the annotation 20 down, so it
/// reaches 45, 95 or 145 past the 25px line box, and the clearance is 75.
/// The block ends at 100, 120 and 170; the annotation reaches 0, 20 and 70
/// past it.
#[test]
fn a_clearing_break_ends_the_block_below_the_floats_and_an_annotation_under() {
    /// A float 100 wide and 100 tall on the left, from a block before.
    struct Floated;
    impl crate::Exclusions for Floated {
        fn band(&self, _line: usize, block: crate::BlockExtents) -> InlineExtents {
            let left = if block.start < 100.0 { 100.0 } else { 0.0 };
            InlineExtents { left, right: 400.0 }
        }
        fn below(&self, top: f32) -> Option<f32> {
            (top < 100.0).then_some(100.0)
        }
        fn place(&mut self, _float: crate::FloatRequest) -> crate::PlacedFloat {
            crate::PlacedFloat::default()
        }
        fn checkpoint(&self) -> crate::ExclusionsCheckpoint {
            crate::ExclusionsCheckpoint(0)
        }
        fn rewind(&mut self, _to: crate::ExclusionsCheckpoint) {}
    }
    let mut cx = context();
    let mut layout = Layout::new();
    let mut root = sized(&AHEM_FAMILY, 16.0);
    root.line.height = LineHeight::Px(20.0);
    let mut ruby = root;
    ruby.ruby.position = RubyPosition::Under;
    for (tall, end, below) in [
        (50.0, 100.0, 0.0),
        (100.0, 120.0, -20.0),
        (150.0, 170.0, -70.0),
    ] {
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
            b.open_ruby(NodeKey(1), &ruby, None);
            let square = BoxSize {
                inline: 20.0,
                block: 20.0,
                baseline: None,
            };
            b.atomic(NodeKey(2), &root, None, square);
            b.open_annotation(NodeKey(3), &ruby, None);
            let block = BoxSize {
                inline: 50.0,
                block: tall,
                baseline: None,
            };
            b.atomic(NodeKey(4), &ruby, None, block);
            b.close_annotation();
            b.close_ruby();
            b.line_break_clearing(NodeKey(5), crate::Clear::Both);
        });
        layout.break_lines(&mut cx, Area::new(400.0), &mut Floated);
        assert_eq!(layout.metrics().block_end, end, "{tall}");
        assert_eq!(layout.room_below(), below, "{tall}");
    }
}

/// A ruby container with no annotation is no column: its text breaks as
/// the line's does, as Chrome lays it out (`LineBreaker::HandleRuby`, "No
/// ruby-text"). Chrome 153, 20px Ahem, min-content: `<ruby>A B</ruby>` is
/// 20, two lines; `X <ruby>A B</ruby> X` is 20, four lines; and
/// `<ruby>ABCD</ruby>` under `word-break: break-all` is 20, four lines.
#[test]
fn a_ruby_with_no_annotation_breaks_as_the_lines_text() {
    use crate::style::WordBreak;
    let cases: [(&str, &str, &str, bool, &[&str]); 3] = [
        ("", "A B", "", false, &["A", "B"]),
        ("X ", "A B", " X", false, &["X", "A", "B", "X"]),
        ("", "ABCD", "", true, &["A", "B", "C", "D"]),
    ];
    for (before, inner, after, all, lines) in cases {
        let mut cx = context();
        let mut base = sized(&AHEM_FAMILY, 20.0);
        if all {
            base.text.word_break = WordBreak::BreakAll;
        }
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
            b.text(NodeKey(1), before);
            b.open_ruby(NodeKey(2), &base, None);
            b.text(NodeKey(3), inner);
            b.close_ruby();
            b.text(NodeKey(4), after);
        });
        assert_eq!(layout.intrinsic_sizes().min_content, 20.0, "{inner}");
        layout.break_lines(&mut cx, Area::new(20.0), &mut NoExclusions);
        let rows: Vec<_> = split_ruby_rows(&layout)
            .into_iter()
            .map(|(base, _)| String::from(base.trim()))
            .collect();
        assert_eq!(rows, lines, "{inner}");
    }
}

/// Base text after a container's last annotation is the line's text, not
/// a column. Chrome 153, 16px Ahem with 3px and 5px of annotation padding:
/// `<ruby>A<rt>a</rt>BC DE</ruby>` is 52 at min-content, the column's 24
/// less its 4 of overhang, then `BC`, with `DE` on a line of its own.
#[test]
fn base_text_after_the_last_annotation_breaks_as_the_lines_text() {
    let mut cx = context();
    let base = sized(&AHEM_FAMILY, 16.0);
    let mut padded = base;
    padded.edges.padding = Sides::<f32> {
        left: 3.0,
        right: 5.0,
        ..Sides::default()
    }
    .into();
    let mut layout = Layout::new();
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "A");
        b.open_annotation(NodeKey(3), &padded, None);
        b.text(NodeKey(4), "a");
        b.close_annotation();
        b.text(NodeKey(5), "BC DE");
        b.close_ruby();
    });
    assert_eq!(layout.intrinsic_sizes().min_content, 52.0);
    layout.break_lines(&mut cx, Area::new(52.0), &mut NoExclusions);
    let rows: Vec<_> = split_ruby_rows(&layout)
        .into_iter()
        .map(|(base, annotation)| (String::from(base.trim()), annotation))
        .collect();
    assert_eq!(
        rows,
        [
            (String::from("ABC"), String::from("a")),
            (String::from("DE"), String::new())
        ]
    );
}

/// A column with no text in its base or its annotation still takes its
/// annotation's edges. Chrome 153, 16px Ahem with 3px and 5px of annotation
/// padding: `<ruby><rt></rt></ruby>A` and `A<ruby><rt></rt></ruby>` are 24
/// at min-content and at max-content, and `A` keeps its 16 of advance.
#[test]
fn an_empty_column_takes_its_annotations_edges() {
    let base = sized(&AHEM_FAMILY, 16.0);
    let mut padded = base;
    padded.edges.padding = Sides::<f32> {
        left: 3.0,
        right: 5.0,
        ..Sides::default()
    }
    .into();
    for before in [false, true] {
        let mut cx = context();
        let mut layout = Layout::new();
        build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
            if before {
                b.text(NodeKey(1), "A");
            }
            b.open_ruby(NodeKey(2), &base, None);
            b.open_annotation(NodeKey(3), &padded, None);
            b.close_annotation();
            b.close_ruby();
            if !before {
                b.text(NodeKey(1), "A");
            }
        });
        let sizes = layout.intrinsic_sizes();
        assert_eq!(
            (sizes.min_content, sizes.max_content),
            (24.0, 24.0),
            "{before}"
        );
        layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
        let advance = runs(&layout, 0)[0].inline();
        assert_eq!(advance.right - advance.left, 16.0, "{before}");
    }
}

/// Returns `size` px Ahem at `line-height: 1`.
fn tight_ahem(size: f32) -> ComputedStyle<'static> {
    let mut style = sized(&AHEM_FAMILY, size);
    style.line.height = LineHeight::Factor(1.0);
    style
}

/// A line's top, height and baseline, and its annotations' keys and
/// baselines, in pixels.
type LineBands = (f32, f32, f32, Vec<(NodeKey, f32)>);

/// Each line's bands (see [`LineBands`]).
fn line_bands(layout: &Layout) -> Vec<LineBands> {
    layout
        .lines()
        .map(|line| {
            let metrics = line.metrics();
            let notes = line
                .annotations()
                .map(|note| (note.key(), note.baseline()))
                .collect();
            (metrics.top, metrics.height(), metrics.baseline, notes)
        })
        .collect()
}

/// A column split across lines stacks on each line only the annotations
/// that line holds.
///
/// `AA <ruby>BB<rt>cc</rt></ruby> DDDD EEEE FFFF GGGG` under a 20px
/// annotation, the nested one 40px, in 40px Ahem at `line-height: 1`, 400
/// wide. Chrome sets the nested annotation over the first line, from 20 to
/// 60, under the long one from 0 to 20, and the base from 60 to 100. The
/// later lines hold none of the nested annotation: Chrome sets the long one
/// straight on their bases, its parts from 100 to 120 and from 160 to 180,
/// over bases from 120 to 160 and from 180 to 220.
#[test]
fn a_continued_line_stacks_only_the_annotations_it_holds() {
    let mut cx = context();
    let mut layout = Layout::new();
    let (base, small, big) = (tight_ahem(40.0), tight_ahem(20.0), tight_ahem(40.0));
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "AA ");
        b.open_ruby(NodeKey(3), &base, None);
        b.text(NodeKey(4), "BB");
        b.open_annotation(NodeKey(5), &big, None);
        b.text(NodeKey(6), "cc");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(7), " DDDD EEEE FFFF GGGG");
        b.open_annotation(NodeKey(8), &small, None);
        b.text(NodeKey(9), "ee ff gg hh ii jj kk ll mm nn oo pp qq rr");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        line_bands(&layout),
        vec![
            (
                0.0,
                100.0,
                92.0,
                vec![(NodeKey(5), 52.0), (NodeKey(8), 16.0)]
            ),
            (100.0, 60.0, 152.0, vec![(NodeKey(8), 16.0)]),
            (160.0, 60.0, 212.0, vec![(NodeKey(8), 16.0)]),
        ]
    );
}

/// A box that closes inside an annotation's part on one line has no
/// fragment on the line that continues the annotation.
///
/// `AAAA BBBB CCCC DDDD` under `<span>aa</span> bb cc … kk` at 20px, in
/// 40px Ahem at `line-height: 1`, 400 wide. Chrome splits the column over
/// two lines and gives the span one client rect, on the first annotation
/// line, from 0 to 20. The second line's annotation runs from 60 to 80 and
/// its base from 80 to 120.
#[test]
fn a_box_closed_before_a_continued_annotation_stays_on_its_line() {
    let mut cx = context();
    let mut layout = Layout::new();
    let (base, small) = (tight_ahem(40.0), tight_ahem(20.0));
    let paints = ComputedStyle {
        paints: true,
        ..small
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "AAAA BBBB CCCC DDDD");
        b.open_annotation(NodeKey(3), &small, None);
        b.open_box(NodeKey(4), &paints, None);
        b.text(NodeKey(5), "aa");
        b.close_box();
        b.text(NodeKey(6), " bb cc dd ee ff gg hh ii jj kk");
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    assert_eq!(
        line_bands(&layout),
        vec![
            (0.0, 60.0, 52.0, vec![(NodeKey(3), 16.0)]),
            (60.0, 60.0, 112.0, vec![(NodeKey(3), 16.0)]),
        ]
    );
    let boxes: Vec<Vec<_>> = layout
        .lines()
        .map(|line| {
            line.annotations()
                .flat_map(|note| note.boxes().map(|found| (found.key(), found.block())))
                .collect()
        })
        .collect();
    assert_eq!(boxes, vec![vec![(NodeKey(4), across(0.0, 20.0))], vec![]]);
}

/// How a box in an annotation is styled: painted, padded 4 each side, or
/// bordered 2 all round.
#[derive(Copy, Clone)]
enum AnnotationBox {
    Painted,
    Padded,
    Bordered,
}

/// An annotation's texts, each in a box where it names one. Text `n` has
/// key `10 + 2n`, and where boxed, its box that key and its text the next.
type AnnotationParts<'a> = &'a [(&'a str, Option<AnnotationBox>)];

/// Returns the kept boxes of the annotation `parts` make over `base`, in
/// 40px Ahem with a 20px annotation, as `(key, left, right)`.
fn annotation_boxes(
    base: &str,
    parts: AnnotationParts<'_>,
    align: RubyAlign,
) -> Vec<(NodeKey, f32, f32)> {
    let mut cx = context();
    let mut layout = Layout::new();
    let (root, small) = (tight_ahem(40.0), tight_ahem(20.0));
    let mut ruby = root;
    ruby.ruby.align = align;
    let styled = |kind| {
        let edges = match kind {
            AnnotationBox::Painted => EdgesGroup::INITIAL,
            AnnotationBox::Padded => EdgesGroup {
                padding: Sides::<f32> {
                    left: 4.0,
                    right: 4.0,
                    ..Sides::ZERO
                }
                .into(),
                ..EdgesGroup::INITIAL
            },
            AnnotationBox::Bordered => EdgesGroup {
                border: Sides::all(2.0),
                ..EdgesGroup::INITIAL
            },
        };
        ComputedStyle {
            paints: true,
            edges,
            ..small
        }
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &ruby, None);
        b.text(NodeKey(2), base);
        b.open_annotation(NodeKey(3), &small, None);
        for (key, (text, boxed)) in (10..).step_by(2).zip(parts) {
            match boxed {
                Some(kind) => {
                    b.open_box(NodeKey(key), &styled(*kind), None);
                    b.text(NodeKey(key + 1), text);
                    b.close_box();
                }
                None => b.text(NodeKey(key), text),
            }
        }
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(1000.0), &mut NoExclusions);
    let line = layout.line(0).expect("a line");
    line.annotations()
        .flat_map(|note| {
            note.boxes()
                .map(|found| (found.key(), found.inline().left, found.inline().right))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A box in an annotation that `ruby-align` spreads takes in the room
/// before the annotation's first text and after its last, inside its
/// edges.
///
/// `AAAAAAAA` in 40px Ahem under `aa bb cc` at 20px leaves 160 of room.
/// Chrome spreads it as expansion of the glyphs under `space-around`: 20
/// before the text, 60 at each space and 20 after it. So a span around `aa`
/// reaches from 0 to 60, one around `cc` from 260 to 320, and one around all
/// of it from 0 to 320, while one around `bb` keeps to its glyphs, 140 to
/// 180. The room sits inside the box's edges: padded 4 each side, `aa`'s
/// box runs from 0 to 68 and `cc`'s from 252 to 320; bordered 2, `aa`'s
/// runs from 0 to 64. Under `center`, `aa`'s takes in the 80 before it, from
/// 0 to 120. Under `start` and `space-between`, and over a base narrower
/// than the annotation, no room comes before it, and it runs from 0 to 40.
#[test]
fn a_box_in_a_spread_annotation_takes_in_the_room_beside_its_text() {
    use AnnotationBox::{Bordered, Padded, Painted};
    let wide = "AAAAAAAA";
    let first = |kind| [("aa", Some(kind)), (" bb cc", None)];
    let (painted, padded, bordered) = (first(Painted), first(Padded), first(Bordered));
    let middle = [("aa ", None), ("bb", Some(Painted)), (" cc", None)];
    let last = [("aa bb ", None), ("cc", Some(Painted))];
    let last_padded = [("aa bb ", None), ("cc", Some(Padded))];
    let all = [("aa bb cc", Some(Painted))];
    let cases: [(&str, AnnotationParts<'_>, RubyAlign, (f32, f32)); 11] = [
        (wide, &painted, RubyAlign::SpaceAround, (0.0, 60.0)),
        (wide, &middle, RubyAlign::SpaceAround, (140.0, 180.0)),
        (wide, &last, RubyAlign::SpaceAround, (260.0, 320.0)),
        (wide, &all, RubyAlign::SpaceAround, (0.0, 320.0)),
        (wide, &padded, RubyAlign::SpaceAround, (0.0, 68.0)),
        (wide, &last_padded, RubyAlign::SpaceAround, (252.0, 320.0)),
        (wide, &bordered, RubyAlign::SpaceAround, (0.0, 64.0)),
        (wide, &painted, RubyAlign::Center, (0.0, 120.0)),
        (wide, &painted, RubyAlign::Start, (0.0, 40.0)),
        (wide, &painted, RubyAlign::SpaceBetween, (0.0, 40.0)),
        ("AA", &painted, RubyAlign::SpaceAround, (0.0, 40.0)),
    ];
    for (at, (base, parts, align, (left, right))) in cases.into_iter().enumerate() {
        let key = (10..)
            .step_by(2)
            .zip(parts)
            .find(|(_, (_, boxed))| boxed.is_some())
            .map_or(NodeKey(0), |(key, _)| NodeKey(key));
        assert_eq!(
            annotation_boxes(base, parts, align),
            vec![(key, left, right)],
            "case {at}"
        );
    }
}

/// Returns the fragments `Layout::box_fragments` gives `key`, in a column
/// `AAAA BBBB CCCC DDDD` in 40px Ahem, 400 wide, under `parts` at 20px, as
/// `(line, left, right)` to a hundredth. Keys are as [`AnnotationParts`]
/// gives them, each box painted.
fn split_annotation_fragments(parts: &[(&str, bool)], key: NodeKey) -> Vec<(usize, f32, f32)> {
    let mut cx = context();
    let mut layout = Layout::new();
    let (root, small) = (tight_ahem(40.0), tight_ahem(20.0));
    let painted = ComputedStyle {
        paints: true,
        ..small
    };
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &root, None);
        b.text(NodeKey(2), "AAAA BBBB CCCC DDDD");
        b.open_annotation(NodeKey(3), &small, None);
        for (key, (text, boxed)) in (10..).step_by(2).zip(parts) {
            if *boxed {
                b.open_box(NodeKey(key), &painted, None);
                b.text(NodeKey(key + 1), text);
                b.close_box();
            } else {
                b.text(NodeKey(key), text);
            }
        }
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(400.0), &mut NoExclusions);
    let round = |x: f32| (x * 100.0).round() / 100.0;
    layout
        .box_fragments(key)
        .map(|found| {
            let inline = found.inline();
            (found.line(), round(inline.left), round(inline.right))
        })
        .collect()
}

/// The box lookup finds a box inside an annotation split across lines on
/// each line that sets a part of it, and on no other.
///
/// Chrome gives these client rects to a span in the 20px annotation of a
/// column split over two lines: around its first word, one, from 0 to 48;
/// around all of it, one a line, each from 0 to 360; around `ee ff gg hh
/// ii`, from 312 to 360 on the first line and from 0 to 231.67 on the
/// second. The annotation itself has a fragment from 0 to 360 on each.
#[test]
fn the_box_lookup_finds_a_box_on_every_annotation_line_it_is_on() {
    let rest = " bb cc dd ee ff gg hh ii jj kk";
    assert_eq!(
        split_annotation_fragments(&[("aa", true), (rest, false)], NodeKey(10)),
        vec![(0, 0.0, 48.0)]
    );
    let all = [("aa bb cc dd ee ff gg hh ii jj kk", true)];
    assert_eq!(
        split_annotation_fragments(&all, NodeKey(10)),
        vec![(0, 0.0, 360.0), (1, 0.0, 360.0)]
    );
    let middle = [
        ("aa bb cc dd ", false),
        ("ee ff gg hh ii", true),
        (" jj kk", false),
    ];
    assert_eq!(
        split_annotation_fragments(&middle, NodeKey(12)),
        vec![(0, 312.0, 360.0), (1, 0.0, 231.67)]
    );
    assert_eq!(
        split_annotation_fragments(&all, NodeKey(3)),
        vec![(0, 0.0, 360.0), (1, 0.0, 360.0)]
    );
}

/// Lays out `base` in 40px Ahem under `parts` at 20px, `width` wide, under
/// `align`. Keys are as [`AnnotationParts`] gives them, and a boxed part's
/// box is culled.
fn spread_annotation(
    base: &str,
    parts: &[(&str, bool)],
    align: RubyAlign,
    width: f32,
) -> (Context, Layout) {
    let mut cx = context();
    let mut layout = Layout::new();
    let (root, small) = (tight_ahem(40.0), tight_ahem(20.0));
    let mut ruby = root;
    ruby.ruby.align = align;
    build(&mut cx, &mut layout, &ComputedBlockStyle::new(&root), |b| {
        b.open_ruby(NodeKey(1), &ruby, None);
        b.text(NodeKey(2), base);
        b.open_annotation(NodeKey(3), &small, None);
        for (key, (text, boxed)) in (10..).step_by(2).zip(parts) {
            if *boxed {
                b.open_box(NodeKey(key), &small, None);
                b.text(NodeKey(key + 1), text);
                b.close_box();
            } else {
                b.text(NodeKey(key), text);
            }
        }
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
    (cx, layout)
}

/// Returns `key`'s fragments as `(line, left, right)`, to a hundredth.
fn fragments_of(layout: &Layout, key: NodeKey) -> Vec<(usize, f32, f32)> {
    let round = |x: f32| (x * 100.0).round() / 100.0;
    layout
        .box_fragments(key)
        .map(|found| {
            let inline = found.inline();
            (found.line(), round(inline.left), round(inline.right))
        })
        .collect()
}

/// A culled box in an annotation that `ruby-align` spreads takes in the
/// room beside its text, as Chrome's client rects of an unstyled span do.
///
/// `AAAAAAAA` in 40px Ahem under `aa bb cc` at 20px, spread `space-around`:
/// 20 before the text, 60 at each space and 20 after it. Chrome's text
/// fragments take the room in, so a span around `aa` reaches from 0 to 60,
/// one around `bb cc` from 140 to 320, one around `aa bb` from 0 to 180,
/// and one around all of it from 0 to 320. Under `center`, a span around
/// all of it reaches from 0 to 320 too. Split over two lines, 400 wide
/// under `AAAA BBBB CCCC DDDD`, a span around all of an annotation reaches
/// from 0 to 360 on each.
#[test]
fn a_culled_box_in_a_spread_annotation_takes_in_the_room_beside_its_text() {
    let wide = "AAAAAAAA";
    let around = RubyAlign::SpaceAround;
    type Case<'a> = (&'a [(&'a str, bool)], RubyAlign, (f32, f32));
    let cases: [Case<'_>; 5] = [
        (&[("aa", true), (" bb cc", false)], around, (0.0, 60.0)),
        (&[("aa ", false), ("bb cc", true)], around, (140.0, 320.0)),
        (&[("aa bb", true), (" cc", false)], around, (0.0, 180.0)),
        (&[("aa bb cc", true)], around, (0.0, 320.0)),
        (&[("aa bb cc", true)], RubyAlign::Center, (0.0, 320.0)),
    ];
    for (at, (parts, align, (left, right))) in cases.into_iter().enumerate() {
        let (_, layout) = spread_annotation(wide, parts, align, 1000.0);
        let key = (10..)
            .step_by(2)
            .zip(parts)
            .find(|(_, (_, boxed))| *boxed)
            .map_or(NodeKey(0), |(key, _)| NodeKey(key));
        assert_eq!(
            fragments_of(&layout, key),
            vec![(0, left, right)],
            "case {at}"
        );
    }
    let all = [("aa bb cc dd ee ff gg hh ii jj kk", true)];
    let (_, layout) = spread_annotation("AAAA BBBB CCCC DDDD", &all, around, 400.0);
    assert_eq!(
        fragments_of(&layout, NodeKey(10)),
        vec![(0, 0.0, 360.0), (1, 0.0, 360.0)]
    );
}

/// An annotation's text runs take in the room `ruby-align` spreads beside
/// their text, and their glyphs stay where they were.
///
/// `AAAAAAAA` in 40px Ahem under `aa bb cc` at 20px. Chrome's range over
/// the text has one client rect, from 0 to 320: its fragment takes in the
/// 20 before it, the 60 after each space and the 20 after it. The glyphs
/// stand at 20, 40, 80, 140, 160, 200, 260 and 280. The runs cut at the
/// spaces, so they reach from 0 to 140, from 140 to 260 and from 260 to
/// 320. Under `center` the one run reaches from 0 to 320, its glyphs from
/// 80 on.
#[test]
fn an_annotation_run_takes_in_the_room_beside_its_text() {
    let read = |align| {
        let (_, layout) = spread_annotation("AAAAAAAA", &[("aa bb cc", false)], align, 1000.0);
        let line = layout.line(0).expect("a line");
        let runs: Vec<_> = line
            .annotations()
            .flat_map(|note| note.runs().collect::<Vec<_>>())
            .map(|run| (run.inline().left, run.inline().right, run.advance()))
            .collect();
        let glyphs: Vec<_> = line
            .annotations()
            .flat_map(|note| note.runs().collect::<Vec<_>>())
            .flat_map(|run| run.glyphs().map(|glyph| glyph.x).collect::<Vec<_>>())
            .collect();
        (runs, glyphs)
    };
    let (runs, glyphs) = read(RubyAlign::SpaceAround);
    assert_eq!(
        runs,
        vec![
            (0.0, 140.0, 140.0),
            (140.0, 260.0, 120.0),
            (260.0, 320.0, 60.0)
        ]
    );
    assert_eq!(
        glyphs,
        vec![20.0, 40.0, 60.0, 140.0, 160.0, 180.0, 260.0, 280.0]
    );
    let (runs, glyphs) = read(RubyAlign::Center);
    assert_eq!(runs, vec![(0.0, 320.0, 320.0)]);
    assert_eq!(glyphs.first(), Some(&80.0));
}

/// Selection, carets and hit testing in a spread annotation take the room
/// beside its text as Chrome's text fragments do.
///
/// `AAAAAAAA` in 40px Ahem under `aa bb cc` at 20px. Chrome's client rects
/// of each character's range run 0–40, 40–60, 60–140, 140–160, 160–180,
/// 180–260, 260–280 and 280–320: the first character takes the room before
/// the text, each space the room after it, and the last the room after the
/// text. Its carets after each character stand at the ends of those, and a
/// hit at 12 finds offset 0, at 25 offset 1, at 305 and 315 offset 8. Under
/// `center` the first character runs 0–100 and the last 220–320, and a hit
/// at 40 finds offset 0, at 85 offset 1.
#[test]
fn selection_in_a_spread_annotation_takes_in_the_room_beside_its_text() {
    use crate::config::PastLines;
    use crate::selection::{Affinity, Position};
    let note = "aa bb cc";
    let check = |align, chars: [(f32, f32); 8], hits: &[(f32, usize)]| {
        let (_, layout) = spread_annotation("AAAAAAAA", &[(note, false)], align, 1000.0);
        let start = layout.text().find(note).expect("the annotation's text");
        let rects: Vec<_> = (0..note.len())
            .map(|at| {
                let rects: Vec<_> = layout
                    .selection_rects(start + at..start + at + 1)
                    .map(|rect| (rect.inline.left, rect.inline.right))
                    .collect();
                rects
            })
            .collect();
        let expected: Vec<_> = chars.iter().map(|&rect| vec![rect]).collect();
        assert_eq!(rects, expected, "{align:?}");
        for (at, &(_, right)) in chars.iter().enumerate() {
            let caret = layout
                .caret(Position::new(start + at + 1, Affinity::Upstream))
                .expect("a caret");
            assert_eq!(caret.inline.left, right, "{align:?} caret {}", at + 1);
        }
        for &(x, offset) in hits {
            let hit = layout.hit_test(x, 10.0, PastLines::Column);
            assert_eq!(
                hit.map(|hit| hit.offset - start),
                Some(offset),
                "{align:?} at {x}"
            );
        }
    };
    check(
        RubyAlign::SpaceAround,
        [
            (0.0, 40.0),
            (40.0, 60.0),
            (60.0, 140.0),
            (140.0, 160.0),
            (160.0, 180.0),
            (180.0, 260.0),
            (260.0, 280.0),
            (280.0, 320.0),
        ],
        &[(12.0, 0), (25.0, 1), (305.0, 8), (315.0, 8)],
    );
    check(
        RubyAlign::Center,
        [
            (0.0, 100.0),
            (100.0, 120.0),
            (120.0, 140.0),
            (140.0, 160.0),
            (160.0, 180.0),
            (180.0, 200.0),
            (200.0, 220.0),
            (220.0, 320.0),
        ],
        &[(40.0, 0), (85.0, 1)],
    );
}

/// Lays out a ruby of `parts` in 20px Ahem at `line-height: 1` under the
/// 20px annotation `note`, `width` wide, under `align`, and right to left
/// where `rtl`. Part `n` has key `10 + 2n`; a boxed part's box has that key
/// and its text the next, painted where it is `Some(true)`.
fn spread_base(
    parts: &[(&str, Option<bool>)],
    note: &str,
    align: RubyAlign,
    (width, rtl): (f32, bool),
) -> Layout {
    let mut cx = context();
    let mut layout = Layout::new();
    let root = tight_ahem(20.0);
    let mut ruby = root;
    ruby.ruby.align = align;
    let painted = ComputedStyle {
        paints: true,
        ..root
    };
    let block = ComputedBlockStyle {
        direction: if rtl {
            BaseDirection::Rtl
        } else {
            BaseDirection::Ltr
        },
        ..ComputedBlockStyle::new(&root)
    };
    build(&mut cx, &mut layout, &block, |b| {
        b.open_ruby(NodeKey(1), &ruby, None);
        for (key, (text, boxed)) in (10..).step_by(2).zip(parts) {
            match boxed {
                Some(paints) => {
                    b.open_box(NodeKey(key), if *paints { &painted } else { &root }, None);
                    b.text(NodeKey(key + 1), text);
                    b.close_box();
                }
                None => b.text(NodeKey(key), text),
            }
        }
        b.open_annotation(NodeKey(3), &root, None);
        b.text(NodeKey(4), note);
        b.close_annotation();
        b.close_ruby();
    });
    layout.break_lines(&mut cx, Area::new(width), &mut NoExclusions);
    layout
}

/// Returns each character's selection rect before the annotation, as
/// `(line, left, right)` from the area's left, to a hundredth.
fn character_rects(layout: &Layout, note: &str) -> Vec<(usize, f32, f32)> {
    let end = layout.text().find(note).unwrap_or_default();
    let round = |x: f32| (x * 100.0).round() / 100.0;
    (0..end)
        .flat_map(|at| layout.selection_rects(at..at + 1).collect::<Vec<_>>())
        .map(|rect| {
            let left = layout
                .line(rect.line)
                .map_or(0.0, |line| line.metrics().left);
            (
                rect.line,
                round(left + rect.inline.left),
                round(left + rect.inline.right),
            )
        })
        .collect()
}

/// Asserts that `found` are `expected`, each edge within 0.05 px.
fn assert_near(found: &[(usize, f32, f32)], expected: &[(usize, f32, f32)]) {
    assert_eq!(found.len(), expected.len(), "{found:?}");
    for (found, expected) in found.iter().zip(expected) {
        assert!(
            found.0 == expected.0
                && (found.1 - expected.1).abs() < 0.05
                && (found.2 - expected.2).abs() < 0.05,
            "{found:?} against {expected:?}"
        );
    }
}

/// A base `ruby-align` spreads under a wider annotation takes the room
/// into its text, as Chrome's text fragments take their expansion in.
///
/// `aa bb cc` in 20px Ahem under 16 `A`s, 320 wide. Under `space-around`,
/// Chrome's character rects run 0–46.66, 46.66–66.66, 66.66–140,
/// 140–160, 160–180, 180–253.33, 253.33–273.33 and 273.33–320: 26.67
/// before the text and after it, and 53.33 after each space. Its carets
/// stand at those edges, and hits at 12 and 25 find offsets 0 and 1. Under
/// `center` the first character runs 0–100 and the last 220–320; under
/// `start` the last runs 140–320. Right to left, in a block 400 wide, the
/// base stands at the right: its characters from 80–126.66 to 353.33–400.
#[test]
fn a_spread_base_takes_in_the_room_beside_its_text() {
    use crate::config::PastLines;
    use crate::selection::{Affinity, Position};
    let note = "AAAAAAAAAAAAAAAA";
    let text = [("aa bb cc", None)];
    let around = [
        (0, 0.0, 46.66),
        (0, 46.66, 66.66),
        (0, 66.66, 140.0),
        (0, 140.0, 160.0),
        (0, 160.0, 180.0),
        (0, 180.0, 253.33),
        (0, 253.33, 273.33),
        (0, 273.33, 320.0),
    ];
    let layout = spread_base(&text, note, RubyAlign::SpaceAround, (1000.0, false));
    assert_near(&character_rects(&layout, note), &around);
    let carets: Vec<_> = (1..=8)
        .filter_map(|at| layout.caret(Position::new(at, Affinity::Upstream)))
        .map(|caret| (caret.line, caret.inline.left, caret.inline.left))
        .collect();
    let edges: Vec<_> = around
        .iter()
        .map(|&(line, _, right)| (line, right, right))
        .collect();
    assert_near(&carets, &edges);
    for (x, offset) in [(12.0, 0), (25.0, 1), (315.0, 8)] {
        let hit = layout.hit_test(x, 30.0, PastLines::Column);
        assert_eq!(hit.map(|hit| hit.offset), Some(offset), "at {x}");
    }
    let layout = spread_base(&text, note, RubyAlign::Center, (1000.0, false));
    let rects = character_rects(&layout, note);
    assert_near(&rects[..1], &[(0, 0.0, 100.0)]);
    assert_near(&rects[7..], &[(0, 220.0, 320.0)]);
    let layout = spread_base(&text, note, RubyAlign::Start, (1000.0, false));
    let rects = character_rects(&layout, note);
    assert_near(&rects[..1], &[(0, 0.0, 20.0)]);
    assert_near(&rects[7..], &[(0, 140.0, 320.0)]);
    let layout = spread_base(&text, note, RubyAlign::SpaceAround, (400.0, true));
    let shifted: Vec<_> = around
        .iter()
        .map(|&(line, left, right)| (line, left + 80.0, right + 80.0))
        .collect();
    assert_near(&character_rects(&layout, note), &shifted);
}

/// The boxes in a spread base reach over the room its text took in, as
/// Chrome's client rects do.
///
/// `aa bb cc` under 16 `A`s, in 20px Ahem: a painted span around `aa`
/// runs from 0 to 66.66, an unstyled one around `bb cc` from 140 to 320,
/// and a painted one around all of it from 0 to 320.
#[test]
fn a_box_in_a_spread_base_takes_in_the_room_beside_its_text() {
    let note = "AAAAAAAAAAAAAAAA";
    let fragments = |layout: &Layout, key| {
        layout
            .box_fragments(NodeKey(key))
            .map(|found| (found.line(), found.inline().left, found.inline().right))
            .collect::<Vec<_>>()
    };
    let parts = [("aa", Some(true)), (" ", None), ("bb cc", Some(false))];
    let layout = spread_base(&parts, note, RubyAlign::SpaceAround, (1000.0, false));
    assert_near(&fragments(&layout, 10), &[(0, 0.0, 66.66)]);
    assert_near(&fragments(&layout, 14), &[(0, 140.0, 320.0)]);
    let all = [("aa bb cc", Some(true))];
    let layout = spread_base(&all, note, RubyAlign::SpaceAround, (1000.0, false));
    assert_near(&fragments(&layout, 10), &[(0, 0.0, 320.0)]);
}

/// A spread base split across lines takes the room on each line into the
/// text there.
///
/// `aa bb … ll` in 20px Ahem under an annotation of 16, 16 and 10 `A`s,
/// 400 wide, over three lines. Chrome's character rects on the first two
/// lines run 0–24, 24–44, 44–72 and so on to 296–320: 4 before the text and
/// after it, and 8 after each space. On the last they run 0–45, 45–65,
/// 65–135, 135–155 and 155–200.
#[test]
fn a_split_spread_base_takes_in_the_room_on_each_line() {
    let note = "AAAAAAAAAAAAAAAA AAAAAAAAAAAAAAAA AAAAAAAAAA";
    let text = [("aa bb cc dd ee ff gg hh ii jj kk ll", None)];
    let layout = spread_base(&text, note, RubyAlign::SpaceAround, (400.0, false));
    let full = |line| {
        let mut rects = Vec::new();
        let mut left = 0.0;
        for at in 0..14 {
            let width = if at == 0 || at == 13 {
                24.0
            } else if at % 3 == 2 {
                28.0
            } else {
                20.0
            };
            rects.push((line, left, left + width));
            left += width;
        }
        rects
    };
    let mut expected = full(0);
    expected.extend(full(1));
    expected.extend([
        (2, 0.0, 45.0),
        (2, 45.0, 65.0),
        (2, 65.0, 135.0),
        (2, 135.0, 155.0),
        (2, 155.0, 200.0),
    ]);
    assert_near(&character_rects(&layout, note), &expected);
}

/// Each text item `ruby-align` spread room into carries the spread bit,
/// and no other item does, on a base line and on its annotation line.
///
/// A glyph walk reads the bit to take the prefix sums, so it agrees with
/// the spread rows. `AAAAAAAA` in 40px Ahem under `aa bb cc` at 20px
/// spreads room into the annotation; `AA BB` under a wider annotation
/// spreads room into the base.
#[test]
fn the_spread_bit_marks_the_text_with_spread_room() {
    use crate::unit::InlineLayoutUnit;
    let cases: [(&str, &str); 2] = [
        ("AAAAAAAA", "aa bb cc"),
        ("AA BB", "aaaa bbbb cccc dddd eeee"),
    ];
    for (base, annotation) in cases {
        let (_, layout) =
            spread_annotation(base, &[(annotation, false)], RubyAlign::SpaceAround, 800.0);
        let fragments = layout.fragments();
        let heads = fragments.line_heads.as_slice();
        let items = fragments.items.as_slice();
        let mut spread = 0;
        for (index, head) in heads.iter().enumerate() {
            let end = heads.get(index + 1).map_or(items.len(), |next| next.get());
            let line = LineId::new(index);
            for item in items.get(head.get()..end).unwrap_or_default() {
                if item.kind() != FragmentItemKind::Text {
                    continue;
                }
                let (left, right) = fragments.spread_room(line, item);
                let has_room = left != InlineLayoutUnit::ZERO || right != InlineLayoutUnit::ZERO;
                assert_eq!(item.is_spread(), has_room, "{base}: {item:?}");
                spread += usize::from(has_room);
            }
        }
        assert!(spread > 0, "{base}: nothing spread");
    }
}
