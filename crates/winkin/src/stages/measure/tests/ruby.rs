//! Ruby measurement tests. They pin:
//! - a column's overhang over the text beside it, as in Chrome;
//! - split columns' min-content and max-content;
//! - nested columns keeping their own bases and levels;
//! - an empty inner annotation's width at its boundary.

use super::*;
use crate::style::FirstLineVariant;

/// Builds `before`, a ruby of `base` under `annotation` with
/// `ruby-overhang: overhang`, and `after`, in 16 px Ahem, and returns the
/// column.
fn ruby_column(
    fixture: &mut Fixture,
    layout: &mut Layout,
    (before, base, annotation, after): (&str, &str, &str, &str),
    overhang: RubyOverhang,
) -> RubyColumn {
    let ahem = families_style(&AHEM_FAMILY);
    let mut ruby = ahem;
    ruby.ruby.overhang = overhang;
    fixture.build(layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), before);
        b.open_ruby(NodeKey(2), &ruby, None);
        b.text(NodeKey(3), base);
        b.open_annotation(NodeKey(4), &ahem, None);
        b.text(NodeKey(5), annotation);
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(6), after);
    });
    layout
        .measured()
        .text(FirstLineVariant::Standard)
        .ruby_columns()
        .get(RubyColumnId::new(0))
        .expect("a column")
        .clone()
}

/// A column is as wide as its wider part, the base centred. Under `auto` it
/// overhangs each side by the lesser of the base's inset and half the
/// annotation's font size, as Chrome (WPT `ruby-overhang-none`): `X` under
/// `XXXX` between two `X`s is 64 wide, overhangs 8 each side, and the next
/// `X` starts at 64. `none`, and the JLREQ rule over non-kana, overhang
/// nothing: the next `X` starts at 80.
#[test]
fn a_column_reaches_over_the_text_beside_it_as_chrome_does() {
    use crate::style::RubyOverhang;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let parts = ("X", "X", "XXXX", "X");
    let column = ruby_column(&mut fixture, &mut layout, parts, RubyOverhang::Auto);
    assert_eq!(column.width, LayoutUnit::from_px(64.0));
    assert_eq!(column.room_before, LayoutUnit::from_px(24.0));
    assert_eq!(
        column.overhang,
        (LayoutUnit::from_px(8.0), LayoutUnit::from_px(8.0))
    );
    assert_eq!(
        positions(&layout),
        [0.0, 16.0, 32.0, 32.0, 32.0, 32.0, 64.0, 80.0]
    );
    let column = ruby_column(&mut fixture, &mut layout, parts, RubyOverhang::None);
    assert_eq!(column.overhang, (LayoutUnit::ZERO, LayoutUnit::ZERO));
    assert_eq!(positions(&layout)[6], 80.0);
    let config = Config {
        ruby_overhang: RubyOverhangRule::KanaOnly,
        ..*fixture.cx.config()
    };
    fixture.cx.set_config(config);
    let column = ruby_column(&mut fixture, &mut layout, parts, RubyOverhang::Auto);
    assert_eq!(column.overhang, (LayoutUnit::ZERO, LayoutUnit::ZERO));
    // Over kana it overhangs one annotation em, within half the room.
    let column = ruby_column(
        &mut fixture,
        &mut layout,
        ("\u{3042}", "X", "XXXX", "\u{3044}"),
        RubyOverhang::Auto,
    );
    assert_eq!(
        column.overhang,
        (LayoutUnit::from_px(16.0), LayoutUnit::from_px(16.0))
    );
}

/// Chrome 153 halves the annotation's font size in whole pixels
/// (`FontSize() / 2`): a 34 px `X` under six 17 px `X`s overhangs 8, not
/// 8.5, and under eight 13 px `X`s 6, not 6.5; the next `X` starts at 120
/// and 126.
#[test]
fn a_column_reaches_half_its_annotation_size_in_whole_pixels() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let base = sized(&AHEM_FAMILY, 34.0);
    for (size, reading, reach, after) in
        [(17.0, "XXXXXX", 8.0, 120.0), (13.0, "XXXXXXXX", 6.0, 126.0)]
    {
        let small = sized(&AHEM_FAMILY, size);
        fixture.build(&mut layout, &ComputedBlockStyle::new(&base), |b| {
            b.text(NodeKey(1), "X");
            b.open_ruby(NodeKey(2), &base, None);
            b.text(NodeKey(3), "X");
            b.open_annotation(NodeKey(4), &small, None);
            b.text(NodeKey(5), reading);
            b.close_annotation();
            b.close_ruby();
            b.text(NodeKey(6), "X");
        });
        let columns = layout
            .measured()
            .text(FirstLineVariant::Standard)
            .ruby_columns();
        let column = columns.get(RubyColumnId::new(0)).expect("a column");
        let reach = LayoutUnit::from_px(reach);
        assert_eq!(column.overhang, (reach, reach), "{size}px");
        let positions = positions(&layout);
        assert_eq!(positions.get(positions.len() - 2), Some(&after), "{size}px");
    }
}

/// A column never overhangs another column, even in its own container, as
/// in Chrome (`CommitPendingEndOverhang`): in
/// `X<ruby>X<rt>XXXX</rt>X<rt>X</rt></ruby>X` in 16 px Ahem, the first
/// column overhangs only the `X` before it, by 8.
#[test]
fn a_column_reaches_over_no_other_column() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let ahem = families_style(&AHEM_FAMILY);
    fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
        b.text(NodeKey(1), "X");
        b.open_ruby(NodeKey(2), &ahem, None);
        b.text(NodeKey(3), "X");
        b.open_annotation(NodeKey(4), &ahem, None);
        b.text(NodeKey(5), "XXXX");
        b.close_annotation();
        b.text(NodeKey(6), "X");
        b.open_annotation(NodeKey(7), &ahem, None);
        b.text(NodeKey(8), "X");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(9), "X");
    });
    let columns = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .ruby_columns();
    let overhangs: Vec<_> = [0, 1]
        .map(|n| {
            columns
                .get(RubyColumnId::new(n))
                .map(|column| column.overhang)
        })
        .into_iter()
        .collect();
    let eight = LayoutUnit::from_px(8.0);
    assert_eq!(
        overhangs,
        [
            Some((eight, LayoutUnit::ZERO)),
            Some((LayoutUnit::ZERO, LayoutUnit::ZERO))
        ]
    );
}

/// Under `ruby-overhang: spaces` a column overhangs only adjacent spaces,
/// within its inset (Blink's `GetOverhang`, `CommitPendingEndOverhang`): a
/// space each side of `X` under `XXXXXX` gives 16 each; letters give none.
#[test]
fn under_spaces_a_column_reaches_into_blank_alone() {
    use crate::style::RubyOverhang;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let column = ruby_column(
        &mut fixture,
        &mut layout,
        ("A ", "X", "XXXXXX", " B"),
        RubyOverhang::Spaces,
    );
    assert_eq!(column.width, LayoutUnit::from_px(96.0));
    assert_eq!(
        column.overhang,
        (LayoutUnit::from_px(16.0), LayoutUnit::from_px(16.0))
    );
    let column = ruby_column(
        &mut fixture,
        &mut layout,
        ("A", "X", "XXXXXX", "B"),
        RubyOverhang::Spaces,
    );
    assert_eq!(column.overhang, (LayoutUnit::ZERO, LayoutUnit::ZERO));
}

/// Under `ruby-overhang: spaces` a column overhangs a quarter em of an
/// adjacent middle dot whatever its glyph width, as Chrome's
/// `MaybeHanKerningMiddle` reads only the character (WPT
/// `ruby-overhang-spaces-019`): half-em `・` each side gives 4 at 16 px.
#[test]
fn under_spaces_a_column_reaches_over_a_narrow_middle_dot() {
    use crate::style::RubyOverhang;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let column = ruby_column(
        &mut fixture,
        &mut layout,
        ("A\u{30FB}", "X", "XXXXXX", "\u{30FB}B"),
        RubyOverhang::Spaces,
    );
    assert_eq!(
        column.overhang,
        (LayoutUnit::from_px(4.0), LayoutUnit::from_px(4.0))
    );
}

/// Under `ruby-overhang: spaces` two columns never share a space, as
/// Chrome's `GetOverhang` subtracts the earlier column's end overhang: with
/// one preserved space between two columns, the first takes it and the
/// second starts at 80; with two, the first takes both.
#[test]
fn under_spaces_two_columns_share_no_blank() {
    use crate::style::RubyOverhang;
    use crate::tests::tabbed;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let block = tabbed(10.0, 8.0);
    let mut ruby = sized(&AHEM_FAMILY, 40.0);
    ruby.ruby.overhang = RubyOverhang::Spaces;
    let reading = sized(&AHEM_FAMILY, 20.0);
    for (between, first_end) in [(" ", 10.0), ("  ", 20.0)] {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&block), |b| {
            for (n, key) in [(0, 1), (1, 5)] {
                if n == 1 {
                    b.text(NodeKey(4), between);
                }
                b.open_ruby(NodeKey(key), &ruby, None);
                b.text(NodeKey(key + 1), "X");
                b.open_annotation(NodeKey(key + 2), &reading, None);
                b.text(NodeKey(key + 3), "XXXX");
                b.close_annotation();
                b.close_ruby();
            }
        });
        let columns = layout
            .measured()
            .text(FirstLineVariant::Standard)
            .ruby_columns();
        let overhang = |n: usize| columns.get(RubyColumnId::new(n)).map(|c| c.overhang);
        let px = LayoutUnit::from_px;
        assert_eq!(overhang(0), Some((px(0.0), px(first_end))), "{between:?}");
        assert_eq!(overhang(1), Some((px(0.0), px(0.0))), "{between:?}");
        let positions = positions(&layout);
        assert_eq!(positions.last(), Some(&160.0), "{between:?}");
    }
}

/// A breakable base lowers min-content but keeps the whole column's
/// max-content, without cloned edges or hyphen costs on the outer line.
#[test]
fn inner_ruby_stops_only_change_split_column_min_content() {
    let mut fixture = fixture();
    let ahem = families_style(&AHEM_FAMILY);
    let mut expected: Option<(LayoutUnit, LayoutUnit, Vec<_>)> = None;
    for wrap in [false, true] {
        let mut style = edged(&ahem, [0.0, 0.0, 3.0], [0.0, 0.0, 5.0]);
        style.edges.decoration_break = BoxDecorationBreak::Clone;
        style.text.wrap_mode = if wrap {
            TextWrapMode::Wrap
        } else {
            TextWrapMode::NoWrap
        };
        style.text.word_break = WordBreak::BreakAll;
        style.text.overflow_wrap = OverflowWrap::Anywhere;
        let mut layout = Layout::new();
        fixture.build(&mut layout, &ComputedBlockStyle::new(&ahem), |b| {
            b.open_ruby(NodeKey(1), &ahem, None);
            b.open_box(NodeKey(2), &style, None);
            b.text(NodeKey(3), "AB C\u{AD}DE FG");
            b.close_box();
            b.open_annotation(NodeKey(4), &style, None);
            b.text(NodeKey(5), "abcdef ghijkl");
            b.close_annotation();
            b.close_ruby();
        });
        let measured = layout.measured();
        let actual = (
            measured.intrinsic.min,
            measured.intrinsic.max,
            measured
                .text(FirstLineVariant::Standard)
                .edge_costs()
                .iter()
                .cloned()
                .collect::<Vec<_>>(),
        );
        if let Some(expected) = &expected {
            assert_eq!(actual.1, expected.1);
            assert_eq!(actual.2, expected.2);
            assert!(actual.0 < expected.0);
        } else {
            expected = Some(actual);
        }
        if wrap {
            assert!(
                layout
                    .analysis()
                    .clusters
                    .first_opportunity(ClusterId::new(0)..layout.analysis().clusters.end_id())
                    .is_some()
            );
        }
    }
}

/// Inner columns take their full width in the outer base, keep their own
/// annotation levels, and never overhang siblings.
#[test]
fn nested_columns_keep_their_own_bases_and_levels() {
    let mut fixture = fixture();
    let base = families_style(&AHEM_FAMILY);
    let small = sized(&AHEM_FAMILY, 8.0);
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&base), |b| {
        b.open_ruby(NodeKey(1), &base, None);
        b.text(NodeKey(2), "C");
        b.open_ruby(NodeKey(3), &base, None);
        b.text(NodeKey(4), "A");
        b.open_annotation(NodeKey(5), &small, None);
        b.text(NodeKey(6), "abcd");
        b.close_annotation();
        b.text(NodeKey(7), "B");
        b.open_annotation(NodeKey(8), &small, None);
        b.text(NodeKey(9), "xyz");
        b.close_annotation();
        b.close_ruby();
        b.text(NodeKey(10), "D");
        b.open_annotation(NodeKey(11), &small, None);
        b.text(NodeKey(12), "reading");
        b.close_annotation();
        b.close_ruby();
    });
    let measured = layout.measured();
    let rubies = measured.text(FirstLineVariant::Standard).ruby_columns();
    let columns = rubies.iter().map(|(_, column)| column).collect::<Vec<_>>();
    assert_eq!(columns.len(), 3);
    assert_eq!(columns[0].parent, None);
    assert_eq!(columns[0].base_width, lu(88.0));
    assert_eq!(columns[0].width, lu(88.0));
    assert_eq!(columns[1].parent, Some(RubyColumnId::new(0)));
    assert_eq!(columns[2].parent, Some(RubyColumnId::new(0)));
    assert_eq!(columns[1].width, lu(32.0));
    assert_eq!(columns[2].width, lu(24.0));
    assert_eq!(rubies.levels(columns[0])[0].depth, 1);
    assert_eq!(rubies.levels(columns[1])[0].depth, 0);
    assert_eq!(rubies.levels(columns[2])[0].depth, 0);
    assert_eq!(
        rubies.base_containing(ClusterId::new(1)),
        Some(RubyColumnId::new(1))
    );
    assert_eq!(rubies.base_containing(ClusterId::new(2)), None);
    assert_eq!(
        rubies.base_containing(ClusterId::new(10)),
        Some(RubyColumnId::new(0))
    );
    assert_eq!(
        rubies.outermost_annotation(ClusterId::new(1), RubySide::Over),
        Some(columns[0].levels.start)
    );
    assert_eq!(
        rubies.outermost_annotation(ClusterId::new(2), RubySide::Over),
        None
    );
    // A `ColumnWalk` agrees with random-access reads in either direction.
    let mut walk = super::ColumnWalk::from_column(rubies, RubyColumnId::new(0), ClusterId::new(0));
    let end = layout.analysis().clusters.end_id().get();
    for at in (0..end).chain((0..end).rev()).chain([1, 10, 2, 0]) {
        let cluster = ClusterId::new(at);
        assert_eq!(walk.base(cluster), rubies.base_containing(cluster));
        for side in [RubySide::Over, RubySide::Under] {
            assert_eq!(
                walk.outermost(cluster, side),
                rubies.outermost_annotation(cluster, side)
            );
        }
    }
    let mut scope = super::ruby_columns::ColumnScope::new(rubies, ClusterId::new(0));
    for at in (0..end).chain((0..end).rev()).chain([1, 10, 2, 0]) {
        let cluster = ClusterId::new(at);
        for parent in [
            None,
            Some(RubyColumnId::new(0)),
            Some(RubyColumnId::new(1)),
            Some(RubyColumnId::new(2)),
        ] {
            assert_eq!(
                scope
                    .scope_column(cluster, parent)
                    .map(|column| column.open),
                walk.scope_column(cluster, parent).map(|column| column.open),
                "{at}, {parent:?}"
            );
        }
    }
    for parent in [None, Some(RubyColumnId::new(0)), Some(RubyColumnId::new(1))] {
        for at in (0..end).chain((0..end).rev()) {
            let cluster = ClusterId::new(at);
            assert_eq!(
                scope
                    .scope_column(cluster, parent)
                    .map(|column| column.open),
                walk.scope_column(cluster, parent).map(|column| column.open),
                "{at}, {parent:?}"
            );
        }
    }
    assert_eq!(measured.intrinsic.min, lu(88.0));
    assert_eq!(measured.intrinsic.max, lu(88.0));
}

#[test]
fn an_empty_inner_annotation_pays_its_width_at_its_boundary() {
    let mut fixture = fixture();
    let base = families_style(&AHEM_FAMILY);
    let annotation = edged(&base, [0.0, 0.0, 3.0], [0.0, 0.0, 5.0]);
    for before in [false, true] {
        let mut layout = Layout::new();
        fixture.build(&mut layout, &ComputedBlockStyle::new(&base), |b| {
            b.open_ruby(NodeKey(1), &base, None);
            if before {
                b.text(NodeKey(4), "A");
            }
            b.open_ruby(NodeKey(2), &base, None);
            b.open_annotation(NodeKey(3), &annotation, None);
            b.close_annotation();
            b.close_ruby();
            if !before {
                b.text(NodeKey(4), "A");
            }
            b.open_annotation(NodeKey(5), &base, None);
            b.close_annotation();
            b.close_ruby();
        });
        assert_eq!(layout.measured().intrinsic.min, lu(24.0));
        assert_eq!(layout.measured().intrinsic.max, lu(24.0));
        let stages = layout.stages().variant(FirstLineVariant::Standard);
        assert_eq!(
            stages.cluster_advance(ClusterId::new(0)).to_layout(),
            lu(16.0)
        );
    }
}
