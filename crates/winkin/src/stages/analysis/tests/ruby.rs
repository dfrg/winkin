//! Ruby break tests. They pin:
//! - no line break inside a ruby column, and the break after one as Chrome's
//!   `CanBreakAfterRubyColumn` decides it;
//! - the opportunities inside a column that a split may use, each part in its
//!   own style;
//! - a nested base breaking between its child columns.

use super::*;
use crate::data::IdRange;
use crate::stages::measure::RubyColumnId;
use crate::style::FirstLineVariant;
use core::cell::Cell;

/// No line breaks inside a ruby column, its base or its annotations.
///
/// The base's last character and the text after the column decide a break
/// after it, as Chrome's `CanBreakAfterRubyColumn` decides it:
/// - after `cd ef` under `gh ij` and before ` kl`, none, since a line does
///   not break before a space;
/// - after the space, before `kl`, one;
/// - between two columns of ideographs, and after the last before `x`, one.
#[test]
fn no_line_breaks_inside_a_ruby_column() {
    let plain = ComputedStyle::initial();
    let layout = build(|b| {
        b.text(key(1), "ab ");
        b.open_ruby(key(2), &plain, None);
        b.text(key(3), "cd ef");
        b.open_annotation(key(4), &plain, None);
        b.text(key(5), "gh ij");
        b.close_annotation();
        b.close_ruby();
        b.text(key(6), " kl");
        b.open_ruby(key(7), &plain, None);
        b.text(key(8), "漢");
        b.open_annotation(key(9), &plain, None);
        b.text(key(10), "かん");
        b.close_annotation();
        b.text(key(11), "字");
        b.open_annotation(key(12), &plain, None);
        b.text(key(13), "じ");
        b.close_annotation();
        b.close_ruby();
        b.text(key(14), "x");
    });
    assert_eq!(layout.content().text, "ab cd efgh ij kl漢かん字じx");
    let analysis = layout.analysis();
    let clusters = &analysis.clusters;
    let rubies = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .ruby_columns();
    let near = Cell::new(RubyColumnId::new(0));
    let outer = clusters
        .ids()
        .filter_map(|id| {
            let after = ClusterId::new(id.get() + 1);
            (clusters.attrs(id).unwrap().has(ClusterAttrs::BREAK_AFTER)
                && rubies.interior(after, &near).is_none())
            .then(|| clusters.end(id).unwrap().end().get())
        })
        .collect::<Vec<_>>();
    assert_eq!(outer, [3, 14, 16, 25, 31]);
}

/// Every ruby part keeps the same internal soft and emergency decisions as
/// ordinary text under its own style. Its last boundary, and boundaries
/// between annotation levels, offer no internal opportunity.
#[test]
fn ruby_inner_opportunities_follow_each_parts_style() {
    let plain = ComputedStyle::initial();
    for style in [
        plain,
        word_break(WordBreak::BreakAll),
        word_break(WordBreak::KeepAll),
        nowrap(),
        styled(|s| s.text.overflow_wrap = OverflowWrap::BreakWord),
        styled(|s| s.text.overflow_wrap = OverflowWrap::Anywhere),
    ] {
        for parts in [
            ["alpha bravo漢字", "one two漢字", "red blue漢字"],
            ["abc", "-123", "-456"],
            ["ภาษาไทยภาษา", "ไทยภาษาไทย", "ภาษาไทยภาษา"],
        ] {
            let layout = build_with(&ComputedBlockStyle::new(&nowrap()), |b| {
                b.open_ruby(key(1), &style, None);
                b.text(key(2), parts[0]);
                b.open_annotation(key(3), &style, None);
                b.text(key(4), parts[1]);
                b.close_annotation();
                b.open_annotation(key(5), &style, None);
                b.text(key(6), parts[2]);
                b.close_annotation();
                b.close_ruby();
            });
            let clusters = &layout.analysis().clusters;
            let mut first = 0;
            for text in parts {
                let ordinary = root_text(&style, text);
                let reference = &ordinary.analysis().clusters;
                for id in reference.ids() {
                    let here = ClusterId::new(first + id.get());
                    let attrs = reference.attrs(id).unwrap();
                    let actual = clusters.attrs(here).unwrap();
                    let soft = actual.has(ClusterAttrs::BREAK_AFTER);
                    let emergency = actual.has(ClusterAttrs::EMERGENCY_AFTER);
                    assert_eq!(
                        soft,
                        attrs.has(ClusterAttrs::BREAK_AFTER),
                        "{text:?} {id:?}"
                    );
                    assert_eq!(
                        emergency,
                        attrs.has(ClusterAttrs::EMERGENCY_AFTER),
                        "{text:?} {id:?}"
                    );
                }
                first += reference.len();
            }
            assert_eq!(first, clusters.len());
        }
    }
}

/// Box and item boundaries do not end a part. A divided grapheme and a
/// combined unit still offer no inner break, while a soft hyphen does.
#[test]
fn ruby_inner_opportunities_respect_graphemes_units_and_hyphens() {
    let style = styled(|s| {
        s.text.word_break = WordBreak::BreakAll;
        s.text.overflow_wrap = OverflowWrap::Anywhere;
    });
    let layout = build(|b| {
        b.open_ruby(key(1), &style, None);
        b.text(key(2), "a");
        b.open_box(key(3), &style, None);
        b.text(key(4), "\u{301}b\u{AD}c");
        b.close_box();
        b.open_annotation(key(5), &style, None);
        b.text(key(6), "xy");
        b.close_annotation();
        b.close_ruby();
    });
    let clusters = &layout.analysis().clusters;
    assert!(
        !clusters
            .attrs(ClusterId::new(0))
            .unwrap()
            .has(ClusterAttrs::BREAK_AFTER)
    );
    assert!(
        !clusters
            .attrs(ClusterId::new(0))
            .unwrap()
            .has(ClusterAttrs::EMERGENCY_AFTER)
    );
    let soft = clusters
        .ids()
        .find(|&id| clusters.attrs(id).unwrap().class() == ClusterClass::SoftHyphen)
        .unwrap();
    assert!(clusters.attrs(soft).unwrap().has(ClusterAttrs::BREAK_AFTER));

    let combined = styled(|s| {
        s.text.word_break = WordBreak::BreakAll;
        s.text.overflow_wrap = OverflowWrap::Anywhere;
        s.orientation.text_combine_upright = TextCombineUpright::All;
    });
    let plain = ComputedStyle::initial();
    let root = ComputedBlockStyle {
        writing_mode: WritingMode::VerticalRl,
        ..ComputedBlockStyle::new(&plain)
    };
    let layout = build_with(&root, |b| {
        b.open_ruby(key(1), &combined, None);
        b.text(key(2), "1234");
        b.open_annotation(key(3), &combined, None);
        b.text(key(4), "5678");
        b.close_annotation();
        b.close_ruby();
    });
    assert!(breaks(&layout).is_empty());
}

/// Style seams, generated opportunities and atomics use the same decisions
/// inside a base as in an ordinary line.
#[test]
fn ruby_inner_opportunities_cross_style_seams_and_inline_objects() {
    let plain = ComputedStyle::initial();
    let all = word_break(WordBreak::BreakAll);
    let keep = word_break(WordBreak::KeepAll);
    let part = |b: &mut LayoutBuilder<'_>| {
        b.text(key(2), "東京");
        b.open_box(key(3), &all, None);
        b.text(key(4), "abc");
        b.close_box();
        b.open_box(key(5), &keep, None);
        b.text(key(6), "大阪");
        b.close_box();
        b.text(key(7), "東京");
        b.break_opportunity();
        b.atomic(key(8), &plain, None, BoxSize::default());
        b.text(key(9), "字");
    };
    let reference = build(part);
    let layout = build(|b| {
        b.open_ruby(key(1), &plain, None);
        part(b);
        b.close_ruby();
    });
    let expected = &reference.analysis().clusters;
    let clusters = &layout.analysis().clusters;
    assert_eq!(clusters.len(), expected.len());
    for id in expected.ids() {
        let attrs = expected.attrs(id).unwrap();
        assert_eq!(
            clusters.attrs(id).unwrap().has(ClusterAttrs::BREAK_AFTER),
            attrs.has(ClusterAttrs::BREAK_AFTER)
        );
        assert_eq!(
            clusters
                .attrs(id)
                .unwrap()
                .has(ClusterAttrs::EMERGENCY_AFTER),
            attrs.has(ClusterAttrs::EMERGENCY_AFTER)
        );
    }
}

/// Whole-column queries over shared opportunities match an independent
/// linear oracle for every clipped range in either direction.
#[test]
fn shared_ruby_opportunities_can_preserve_whole_column_queries() {
    let style = styled(|s| {
        s.text.word_break = WordBreak::BreakAll;
        s.text.overflow_wrap = OverflowWrap::Anywhere;
    });
    let layout = build_with(&ComputedBlockStyle::new(&style), |b| {
        b.text(key(1), "before ");
        b.open_ruby(key(2), &style, None);
        b.text(key(3), "ABCDEFGHIJK");
        b.open_annotation(key(4), &style, None);
        b.text(key(5), "abcdefghijkl");
        b.close_annotation();
        b.open_annotation(key(6), &style, None);
        b.text(key(7), "second level");
        b.close_annotation();
        b.text(key(8), "XYZ");
        b.open_annotation(key(9), &style, None);
        b.text(key(10), "xyz");
        b.close_annotation();
        b.close_ruby();
        b.text(key(11), " after ");
        b.open_ruby(key(12), &style, None);
        b.open_annotation(key(13), &style, None);
        b.text(key(14), "empty base");
        b.close_annotation();
        b.close_ruby();
        b.open_ruby(key(15), &style, None);
        b.text(key(16), "empty annotation");
        b.open_annotation(key(17), &style, None);
        b.close_annotation();
        b.close_ruby();
        b.text(key(18), " finish");
    });
    let analysis = layout.analysis();
    let clusters = &analysis.clusters;
    assert!(clusters.len() > 64, "exercise bitmap word boundaries");
    let rubies = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .ruby_columns();
    let columns = (rubies.line_columns(RubyColumnId::new(0), ClusterId::new(0)..clusters.end_id()))
        .ids()
        .map(|id| {
            let column = rubies.get(id).unwrap();
            column.base.start..analysis.item_clusters.start(column.close)
        })
        .collect::<Vec<_>>();
    assert_eq!(columns.len(), 4);
    // One column walk serves every query, wherever the last left it.
    let near = Cell::new(RubyColumnId::new(0));
    for from in 0..=clusters.len() {
        for to in from..=clusters.len() {
            let range = ClusterId::new(from)..ClusterId::new(to);
            for back in [false, true] {
                for (emergency, bit) in [
                    (false, ClusterAttrs::BREAK_AFTER),
                    (true, ClusterAttrs::EMERGENCY_AFTER),
                ] {
                    let actual =
                        rubies.opportunity(clusters, range.clone(), None, (back, emergency), &near);
                    // Independent linear oracle over every boundary, including
                    // empty parts and clipped ranges on either side of a column.
                    let eligible = |id: &ClusterId| {
                        let boundary = ClusterId::new(id.get() + 1);
                        clusters.attrs(*id).unwrap().has(bit)
                            && !columns
                                .iter()
                                .any(|column| column.start < boundary && boundary < column.end)
                    };
                    let expected = if back {
                        range.clone().ids().rev().find(eligible)
                    } else {
                        range.clone().ids().find(eligible)
                    }
                    .map(|id| ClusterId::new(id.get() + 1));
                    assert_eq!(
                        actual, expected,
                        "{from}..{to} back={back} emergency={emergency}"
                    );
                }
            }
        }
    }
}

/// Rebuilding clears and reuses the shared opportunity index.
#[test]
fn ruby_inner_opportunities_clear_on_rebuild() {
    let style = word_break(WordBreak::BreakAll);
    let mut layout = Layout::new();
    let mut cx = no_fonts();
    for text in ["abcdef", "ab", "abcdef"] {
        let mut b = layout.builder(
            &mut cx,
            key(0),
            &ComputedBlockStyle::new(&style),
            BuildOptions::default(),
        );
        b.open_ruby(key(1), &style, None);
        b.text(key(2), text);
        b.close_ruby();
        b.finish();
        check(&layout);
        let clusters = &layout.analysis().clusters;
        assert_eq!(
            clusters.last_opportunity(ClusterId::new(0)..clusters.end_id()),
            Some(ClusterId::new(text.len() - 1))
        );
    }
    let mut b = layout.builder(
        &mut cx,
        key(0),
        &ComputedBlockStyle::new(&style),
        BuildOptions::default(),
    );
    b.text(key(1), "abcdef");
    b.finish();
    check(&layout);
    assert_eq!(breaks(&layout), [1, 2, 3, 4, 5]);
}

/// One shared index serves both the outer line and a nested base: the base
/// can break between child columns, and their internal stops stay hidden.
#[test]
fn nested_base_queries_use_child_closing_opportunities() {
    let plain = styled(|style| style.text.overflow_wrap = OverflowWrap::Anywhere);
    let layout = build(|b| {
        b.open_ruby(key(1), &plain, None);
        b.text(key(2), "甲");
        b.open_ruby(key(3), &plain, None);
        b.text(key(4), "漢字");
        b.open_annotation(key(5), &plain, None);
        b.text(key(6), "かんじ");
        b.close_annotation();
        b.text(key(7), "語");
        b.open_annotation(key(8), &plain, None);
        b.text(key(9), "ご");
        b.close_annotation();
        b.close_ruby();
        b.text(key(10), "乙");
        b.open_annotation(key(11), &plain, None);
        b.text(key(12), "outer");
        b.close_annotation();
        b.close_ruby();
    });
    let clusters = &layout.analysis().clusters;
    let rubies = layout
        .measured()
        .text(FirstLineVariant::Standard)
        .ruby_columns();
    let root = RubyColumnId::new(0);
    let base = rubies.get(root).unwrap().base.clone();
    let mut found = Vec::new();
    let mut start = base.start;
    while let Some(end) = rubies.opportunity(
        clusters,
        start..base.end,
        Some(root),
        (false, false),
        &Cell::new(root),
    ) {
        found.push(end.get());
        start = end;
    }
    assert_eq!(found, [1, 6, 8]);
    assert_eq!(
        rubies.opportunity(
            clusters,
            base.clone(),
            None,
            (false, false),
            &Cell::new(root)
        ),
        None
    );
    assert_eq!(
        rubies.opportunity(clusters, base, Some(root), (true, false), &Cell::new(root)),
        Some(ClusterId::new(8))
    );
    let children = [RubyColumnId::new(1), RubyColumnId::new(2)].map(|id| {
        let column = rubies.get(id).unwrap();
        column.base.start..layout.analysis().item_clusters.start(column.close)
    });
    for from in 0..=rubies.get(root).unwrap().base.end.get() {
        for to in from..=rubies.get(root).unwrap().base.end.get() {
            let range = ClusterId::new(from)..ClusterId::new(to);
            for back in [false, true] {
                for (emergency, bit) in [
                    (false, ClusterAttrs::BREAK_AFTER),
                    (true, ClusterAttrs::EMERGENCY_AFTER),
                ] {
                    let eligible = |id: &ClusterId| {
                        let end = ClusterId::new(id.get() + 1);
                        clusters.attrs(*id).unwrap().has(bit)
                            && !children
                                .iter()
                                .any(|child| child.start < end && end < child.end)
                    };
                    let expected = if back {
                        range.clone().ids().rev().find(eligible)
                    } else {
                        range.clone().ids().find(eligible)
                    }
                    .map(|id| ClusterId::new(id.get() + 1));
                    assert_eq!(
                        rubies.opportunity(
                            clusters,
                            range.clone(),
                            Some(root),
                            (back, emergency),
                            &Cell::new(root)
                        ),
                        expected
                    );
                }
            }
        }
    }
}
