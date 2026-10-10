//! Hit testing tests. They pin:
//! - a point hitting the nearer edge, a ligature divided evenly;
//! - points at a direction change, past the lines and in a box's padding;
//! - the text an ellipsis hides staying live;
//! - a point on an annotation hitting its text.

use super::*;

/// A point hits the nearer edge of the character under it, a tie going left.
///
/// Past a line's end it hits the end, upstream. Past its start it hits the
/// start.
#[test]
fn a_point_hits_the_nearer_edge() {
    let layout = laid("abcd efgh", 400.0);
    assert_eq!(hit(&layout, 30.0, 0), Position::from(1));
    assert_eq!(hit(&layout, 31.0, 0), Position::from(2));
    assert_eq!(hit(&layout, -50.0, 0), Position::from(0));
    assert_eq!(hit(&layout, 999.0, 0), Position::new(9, Affinity::Upstream));
    let layout = laid("abcd efgh", 100.0);
    assert_eq!(hit(&layout, 90.0, 0), Position::new(4, Affinity::Upstream));
    assert_eq!(hit(&layout, 5.0, 1), Position::from(5));
    // Every caret hits back to where it is drawn.
    for layout in [
        laid("abcd efgh", 400.0),
        laid("abc \u{5D0}\u{5D1}\u{5D2} def", 400.0),
        laid_with(&ComputedBlockStyle::new(&latin()), 400.0, |b| {
            b.text(NodeKey(1), "a fine fit")
        }),
        // Arabic, its lam and alef one glyph.
        laid("\u{628}\u{644}\u{627} \u{644}\u{627}\u{628} end", 400.0),
    ] {
        // Where the text has a ligature, some run draws fewer glyphs than
        // it has clusters.
        let ligated = layout.lines().flat_map(|line| line.items()).any(
            |item| matches!(item, Item::Text(run) if run.glyphs().count() < run.clusters().count()),
        );
        let text = layout.text();
        assert_eq!(
            ligated,
            text.contains("fi") || text.contains('\u{644}'),
            "{text:?}"
        );
        for at in 0..=layout.text().len() {
            if !layout.text().is_char_boundary(at) {
                continue;
            }
            let (line, x) = caret(&layout, at);
            let back = hit(&layout, x, line);
            assert_eq!(
                position_caret(&layout, back),
                (line, x),
                "{at} in {:?}",
                layout.text()
            );
        }
    }
}

/// Returns the Latin test font, whose `fi` is one glyph over two em-wide characters.
fn latin() -> ComputedStyle<'static> {
    const LATIN: [FontFamilyName<'static>; 1] =
        [FontFamilyName::Named(Cow::Borrowed("Test Latin"))];
    let mut style = ahem();
    style.font.families = &LATIN;
    style
}

/// A ligature's carets divide it evenly.
#[test]
fn a_ligature_is_divided_evenly() {
    let layout = laid_with(&ComputedBlockStyle::new(&latin()), 400.0, |b| {
        b.text(NodeKey(1), "fix")
    });
    let (_, fi_end) = caret(&layout, 2);
    let (_, middle) = caret(&layout, 1);
    assert!(fi_end > 0.0);
    assert_eq!(middle, fi_end / 2.0);
    assert_eq!(hit(&layout, middle - 1.0, 0), Position::from(1));
}

/// At the edge of text in the other direction, a point hits the position whose caret is drawn there.
///
/// Blink's `BidiAdjustment::AdjustForHitTest` finds it so.
#[test]
fn a_point_at_a_change_of_direction_hits_the_caret_drawn_there() {
    let layout = laid("abc \u{5D0}\u{5D1}\u{5D2} def", 400.0);
    assert_eq!(hit(&layout, 75.0, 0), Position::new(4, Affinity::Upstream));
    assert_eq!(hit(&layout, 85.0, 0), Position::new(4, Affinity::Upstream));
    assert_eq!(hit(&layout, 95.0, 0), Position::from(8));
    assert_eq!(hit(&layout, 125.0, 0), Position::from(6));
    assert_eq!(hit(&layout, 135.0, 0), Position::from(10));
}

/// Past the lines, Windows keeps the point's column, and macOS and Linux go to the line's end.
///
/// That is Blink's `ShouldMoveCaretToHorizontalBoundaryWhenPastTopOrBottom`.
/// A point in a line's box hits that line, and its bottom edge the next
/// line.
#[test]
fn a_point_past_the_lines_hits_the_nearest() {
    let layout = laid("abcd efgh ijkl", 100.0);
    assert_eq!(
        layout.hit_test(30.0, -10.0, PastLines::Column),
        Some(Position::from(1))
    );
    assert_eq!(
        layout.hit_test(30.0, -10.0, PastLines::LineEnds),
        Some(Position::from(0))
    );
    assert_eq!(
        layout.hit_test(30.0, 99.0, PastLines::Column),
        Some(Position::from(11))
    );
    assert_eq!(
        layout.hit_test(30.0, 99.0, PastLines::LineEnds),
        Some(Position::new(14, Affinity::Upstream))
    );
    assert_eq!(
        layout.hit_test(10.0, 19.9, PastLines::Column),
        Some(Position::from(0))
    );
    assert_eq!(
        layout.hit_test(10.0, 20.0, PastLines::Column),
        Some(Position::from(5))
    );
    let platform_hit = if cfg!(windows) {
        Some(Position::from(1))
    } else {
        Some(Position::from(0))
    };
    assert_eq!(
        layout.hit_test(30.0, -10.0, PastLines::platform()),
        platform_hit
    );
    // Nothing laid out, nothing hit.
    assert_eq!(Layout::new().hit_test(0.0, 0.0, PastLines::Column), None);
    assert_eq!(Layout::new().caret(Position::from(0)), None);
}

/// A point in a box's padding hits the text beside it, with the caret outside the padding, as in Chrome.
#[test]
fn a_point_in_a_boxs_padding_hits_the_text_beside_it() {
    let padded = styled(|style| {
        style.edges = EdgesGroup {
            padding: Sides::<f32> {
                left: 10.0,
                right: 10.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        }
    });
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.text(NodeKey(1), "a");
        b.open_box(NodeKey(2), &padded, None);
        b.text(NodeKey(3), "bc");
        b.close_box();
        b.text(NodeKey(4), "d");
    });
    let left = hit(&layout, 25.0, 0);
    assert_eq!(left.offset, 1);
    assert_eq!(position_caret(&layout, left), (0, 20.0));
    let right = hit(&layout, 75.0, 0);
    assert_eq!(right.offset, 3);
    assert_eq!(position_caret(&layout, right), (0, 70.0));
}

/// Text an ellipsis hides keeps its carets and hits, under and past the ellipsis, and is copied.
///
/// The ellipsis passes hits through and paints no selection.
#[test]
fn the_text_an_ellipsis_hides_is_live() {
    let nowrap = styled(|style| style.text.wrap_mode = TextWrapMode::NoWrap);
    let cut = ComputedBlockStyle {
        text_overflow: TextOverflow::Ellipsis,
        ..ComputedBlockStyle::new(&nowrap)
    };
    let layout = laid_with(&cut, 100.0, |b| b.text(NodeKey(1), "abcdefghij"));
    assert!(layout.line(0).unwrap().has_ellipsis());
    assert_eq!(hit(&layout, 85.0, 0), Position::from(4));
    assert_eq!(hit(&layout, 95.0, 0), Position::from(5));
    assert_eq!(caret(&layout, 7), (0, 140.0));
    assert_eq!(copied(&layout, CopyKind::Text), "abcdefghij");
    assert_eq!(
        rects(&layout, 0..10),
        [(0, (0.0, 80.0), SelectionRectKind::Text)]
    );
    // End goes to the end of the hidden text, as Chrome's does.
    let end = moved(
        &layout,
        Selection::from(Position::from(1)),
        MotionDirection::Forward.moving(Granularity::LineBoundary),
    );
    assert_eq!(end.focus().offset, 10);
}

/// A ruby annotation's text is hit where the point is on it.
#[test]
fn a_point_on_an_annotation_hits_its_text() {
    let layout = laid_with(&ComputedBlockStyle::new(&ahem()), 400.0, |b| {
        b.open_ruby(NodeKey(1), &ahem(), None);
        b.text(NodeKey(2), "ab");
        b.open_annotation(NodeKey(3), &styled(|style| style.font.size = 10.0), None);
        b.text(NodeKey(4), "xyz");
        b.close_ruby();
        b.text(NodeKey(5), "cd");
    });
    assert_eq!(layout.text(), "abxyzcd");
    let line = layout.line(0).unwrap();
    let annotation = line.annotations().next().unwrap();
    let run = annotation.runs().next().unwrap();
    // The run takes in the room `ruby-align` leaves beside the text; its
    // first glyph stands where the text starts.
    let left = run.glyphs().next().unwrap().x;
    let CrossExtents {
        over: top,
        under: bottom,
    } = run.block();
    let metrics = line.metrics();
    let on = layout
        .hit_test(
            metrics.left + left + 12.0,
            metrics.top + (top + bottom) / 2.0,
            PastLines::Column,
        )
        .unwrap();
    assert_eq!(on.offset, 3);
    let (_, x) = caret(&layout, 3);
    assert_eq!(x, left + 10.0);
}
