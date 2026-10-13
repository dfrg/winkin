//! Generated text tests. They pin:
//! - a hyphen drawn in as many runs as it needs fonts;
//! - a hyphen and an ellipsis read and painted as generated runs.

use super::*;

/// A hyphen whose characters the primary font does not all map is drawn a
/// generated run a font, one after the other where the one before ends:
/// `hyphenate-character: "a→"` in Test Latin, whose
/// arrow is Test Arrows', an em wide there where `a` is half an em, is two
/// runs at 20 px after `XX`, the arrow's drawn with its own glyph and not
/// the primary's missing one, and the line pays both. In a style reading
/// right to left the hyphen is shaped that way, as Chrome shapes a hyphen
/// in its style's direction, so its arrow is drawn on the left of its `a`.
#[test]
fn a_hyphen_is_drawn_in_one_run_per_font() {
    use crate::Generated;
    let mut arrows = TestFont::new("Test Arrows", &[(0x2190, 0x2193)]);
    arrows.advances = vec![('\u{2192}', 1000)];
    let fallback = ahem_fallback().family("Test Arrows");
    let mut cx = Context::new(collection(&[latin(), arrows], fallback));
    let mut layout = Layout::new();
    let mut root = sized(&LATIN, 20.0);
    root.text.hyphenate_character = Some("a\u{2192}");
    for (direction, want) in [
        (Direction::Ltr, [along(20.0, 30.0), along(30.0, 50.0)]),
        (Direction::Rtl, [along(20.0, 40.0), along(40.0, 50.0)]),
    ] {
        let style = ComputedStyle {
            bidi: BidiGroup {
                direction,
                ..root.bidi
            },
            ..root
        };
        build(
            &mut cx,
            &mut layout,
            &ComputedBlockStyle::new(&style),
            |b| {
                b.text(NodeKey(1), "XX\u{AD}XXXXXX");
            },
        );
        layout.break_lines(&mut cx, Area::new(70.0), &mut NoExclusions);
        assert_eq!(layout.lines().len(), 2, "{direction:?}");
        let line = layout.line(0).expect("a line");
        assert!(line.is_hyphenated());
        let hyphen: Vec<_> = line
            .items()
            .filter_map(|item| match item {
                Item::Generated(run) => Some(run),
                _ => None,
            })
            .collect();
        assert!(
            hyphen
                .iter()
                .all(|run| run.generated() == Some(Generated::Hyphen))
        );
        let places: Vec<_> = hyphen.iter().map(|run| run.inline()).collect();
        assert_eq!(places, want, "{direction:?}");
        // Each draws its one glyph, none of them a missing one.
        for run in &hyphen {
            let glyphs: Vec<_> = run.glyphs().collect();
            assert_eq!(glyphs.len(), 1, "{direction:?}");
            assert!(glyphs.iter().all(|glyph| glyph.id != 0), "{direction:?}");
        }
    }
}

/// A hyphen and an ellipsis are generated runs: each
/// holds no content, names its generated text, and draws its glyphs from
/// where it stands; the hyphen, of the text it ends, is painted in the
/// hyphen's phase after the line's text, and the ellipsis, the block's,
/// with the text; the tail a cut hides is laid out, read only by
/// `all_items`, and not painted. A line clamped to one, broken at its soft
/// hyphen, has both.
#[test]
fn a_line_reads_and_paints_its_hyphen_and_its_ellipsis() {
    use crate::Generated;
    use crate::style::{LineClamp, TextOverflow, TextWrapMode};
    let mut cx = context();
    let mut layout = Layout::new();
    let root = sized(&AHEM_FAMILY, 20.0);
    let clamped = ComputedBlockStyle {
        line_clamp: LineClamp::Lines(1),
        ..ComputedBlockStyle::new(&root)
    };
    build(&mut cx, &mut layout, &clamped, |b| {
        b.text(NodeKey(1), "XX\u{AD}XXXXXXXXXXXXXX XX");
    });
    layout.break_lines(&mut cx, Area::new(200.0), &mut NoExclusions);
    assert_eq!(layout.lines().len(), 1);
    let line = layout.line(0).expect("a line");
    assert!(line.is_hyphenated() && line.has_ellipsis());
    // What each generated run is, whose, where, and how many glyphs.
    type Read = (Option<Generated>, NodeKey, InlineExtents, usize);
    let generated: Vec<Read> = line
        .items()
        .filter_map(|item| match item {
            Item::Generated(run) => Some(run),
            _ => None,
        })
        .map(|run| {
            let glyphs: Vec<_> = run.glyphs().collect();
            assert!(glyphs.iter().all(|glyph| glyph.x == run.inline().left));
            assert!(run.clusters().next().is_none() && run.text_range().is_empty());
            assert_eq!(run.font().map(|font| font.size), Some(20.0));
            (run.generated(), run.key(), run.inline(), glyphs.len())
        })
        .collect();
    assert_eq!(
        generated,
        [
            (Some(Generated::Hyphen), NodeKey(1), along(40.0, 60.0), 1),
            (Some(Generated::Ellipsis), NodeKey(0), along(60.0, 80.0), 1),
        ]
    );
    // Nothing the clamp keeps is hidden: the clamped line fits, and the
    // rest of the text is on no line.
    assert_eq!(line.items().count(), line.all_items().count());
    let painted: Vec<String> = line
        .paints(|_| Decorates::None)
        .map(|item| match item {
            Paint::Background(background) => {
                alloc::format!("background {:?}", ends(background.inline()))
            }
            Paint::Text(run) => alloc::format!("text at {}", run.inline().left),
            Paint::Generated(run) => {
                alloc::format!("{:?} at {}", run.generated(), run.inline().left)
            }
            _ => String::from("other"),
        })
        .collect();
    assert_eq!(
        painted,
        [
            "background (0.0, 40.0)",
            "text at 0",
            "Some(Hyphen) at 40",
            "Some(Ellipsis) at 60",
        ]
    );
    // An overflowing line cut for an ellipsis hides its tail: read by
    // `all_items`, flagged, and not painted.
    let mut nowrap = root;
    nowrap.text.wrap_mode = TextWrapMode::NoWrap;
    let ellipsized = ComputedBlockStyle {
        text_overflow: TextOverflow::Ellipsis,
        ..ComputedBlockStyle::new(&nowrap)
    };
    build(&mut cx, &mut layout, &ellipsized, |b| {
        b.text(NodeKey(1), "XXXXXX");
        b.text(NodeKey(2), "XXXXXX");
    });
    layout.break_lines(&mut cx, Area::new(100.0), &mut NoExclusions);
    let line = layout.line(0).expect("a line");
    let hidden: Vec<(NodeKey, InlineExtents)> = line
        .all_items()
        .filter_map(|item| match item {
            Item::Text(run) if run.is_hidden() => Some((run.key(), run.inline())),
            _ => None,
        })
        .collect();
    assert_eq!(
        hidden,
        [
            (NodeKey(1), along(80.0, 120.0)),
            (NodeKey(2), along(120.0, 240.0))
        ]
    );
    assert!(line.items().all(|item| match item {
        Item::Text(run) => !run.is_hidden(),
        _ => true,
    }));
    let painted = line
        .paints(|_| Decorates::None)
        .filter(|item| matches!(item, Paint::Text(_) | Paint::Generated(_)))
        .count();
    assert_eq!(painted, 2, "the four Xs kept and the ellipsis");
}
