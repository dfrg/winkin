//! Relayout tests. They pin:
//! - a relayout equal to a fresh layout;
//! - a layout breaking the same in a new context, the context being a cache.

use super::*;

/// A relayout at another width equals a fresh layout, record for record and
/// glyph for glyph, reshaped edges included.
///
/// A rebuild leaves no lines from the content before.
#[test]
fn a_relayout_equals_a_fresh_layout() {
    let mut fixture = fixture();
    let style = break_all(&sized(&LATIN, 20.0));
    let text = "office AVAVA flat fit ffi AVAIL waffle";
    let mut layout = Layout::new();
    fixture.text(&mut layout, &style, text);
    for width in [12.0, 26.0, 28.0, 57.5, 90.0, 400.0, 3.0, 26.0] {
        fixture.lay_out(&mut layout, width);
        let mut fresh = Layout::new();
        fixture.text(&mut fresh, &style, text);
        fixture.lay_out(&mut fresh, width);
        let (a, b) = (layout.line_records(), fresh.line_records());
        assert_eq!(a.lines.as_slice(), b.lines.as_slice(), "at {width}");
        assert_eq!(a.edges().shapes.as_slice(), b.edges().shapes.as_slice());
        assert_eq!(a.edges().words.as_slice(), b.edges().words.as_slice());
        assert_eq!(a.edges().glyphs.as_slice(), b.edges().glyphs.as_slice());
        assert_eq!(a.edges().advances.as_slice(), b.edges().advances.as_slice());
        assert_eq!(a.block, b.block);
    }
    fixture.text(&mut layout, &style, "x");
    assert_eq!(layout.lines().len(), 0, "a rebuild clears the lines");
}

// The context is a cache -----------------------------------------------------

/// Returns everything two breaks that should agree must agree in.
///
/// That is the line records, the reshaped edges, and every glyph as a
/// painter reads it: its line, id, place and advance.
#[derive(PartialEq, Debug)]
struct Broken {
    lines: Vec<LineRecord>,
    shapes: Vec<EdgeShape>,
    words: Vec<GlyphWord>,
    glyphs: Vec<SidecarGlyph>,
    advances: Vec<TextUnit>,
    untrimmed_end: LayoutUnit,
    drawn: Vec<DrawnGlyph>,
}

/// A glyph as a painter reads it.
#[derive(PartialEq, Debug)]
struct DrawnGlyph {
    /// The index of its line.
    line: usize,
    id: u32,
    x: f32,
    y: f32,
    advance: f32,
}

impl Broken {
    fn new(layout: &Layout) -> Self {
        let lines = layout.line_records();
        let mut drawn = Vec::new();
        for (at, line) in layout.lines().enumerate() {
            for item in line.items() {
                if let crate::Item::Text(run) = item {
                    drawn.extend(run.glyphs().map(|g| DrawnGlyph {
                        line: at,
                        id: g.id,
                        x: g.x,
                        y: g.y,
                        advance: g.advance,
                    }));
                }
            }
        }
        Self {
            lines: lines.lines.as_slice().to_vec(),
            shapes: lines.edges().shapes.as_slice().to_vec(),
            words: lines.edges().words.as_slice().to_vec(),
            glyphs: lines.edges().glyphs.as_slice().to_vec(),
            advances: lines.edges().advances.as_slice().to_vec(),
            untrimmed_end: untrimmed_end(&lines.block),
            drawn,
        }
    }
}

/// Shapes and breaks text in two other fonts, so a new context's tables
/// hold their data first.
///
/// A layout that named a font by its table place would then name one of
/// these.
fn other_fonts(fixture: &mut Fixture) {
    let arabic_style = break_all(&sized(&ARABIC, 20.0));
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&arabic_style)
    };
    let word: String = [BEH, TEH, SEEN, MEEM, BEH].iter().collect();
    let mut arabic = Layout::new();
    fixture.build(&mut arabic, &rtl, |b| {
        b.text(NodeKey(1), &word);
    });
    fixture.lay_out(&mut arabic, 25.0);
    let mut narrow = Layout::new();
    fixture.text(
        &mut narrow,
        &break_all(&sized(&NARROW, 20.0)),
        "office AVAVA",
    );
    fixture.lay_out(&mut narrow, 26.0);
}

/// The context is a cache: a layout names nothing of it, so it breaks the
/// same in any context.
///
/// A new context over the same fonts is filled with two other fonts' data
/// first, so its ids differ. It breaks an old layout:
/// - at the old width into the same lines and glyphs, its unsafe edges
///   reshaped from the layout's own record of its font;
/// - at new widths into what a fresh layout in a fresh context makes.
///
/// A context with no fonts at all breaks it the same too.
#[test]
fn a_layout_breaks_the_same_in_a_new_context() {
    let mut fixture = fixture();
    let style = break_all(&sized(&LATIN, 20.0));
    let text = "office AVAVA flat fit ffi AVAIL waffle";
    let mut layout = Layout::new();
    fixture.text(&mut layout, &style, text);
    let glyphs = glyph_words(&layout);
    fixture.lay_out(&mut layout, 12.0);
    let warm = Broken::new(&layout);
    assert!(!warm.shapes.is_empty(), "edges are reshaped at 12 px");

    fixture.cx = Context::new(fixture.cx.collection().clone());
    let shaping = fixture.cx.shaping();
    assert_eq!(shaping.plan_count(), 0);
    assert_eq!(fixture.cx.font_context().instances().len(), 0);
    other_fonts(&mut fixture);
    let others = fixture.cx.shaping().plan_count();
    fixture.lay_out(&mut layout, 12.0);
    assert_eq!(Broken::new(&layout), warm);
    // The misses are made again from the layout's own record of its font,
    // beside the other fonts'.
    assert!(fixture.cx.shaping().plan_count() > others);

    for width in [28.0, 40.0, 60.0, 90.0] {
        fixture.cx = Context::new(fixture.cx.collection().clone());
        other_fonts(&mut fixture);
        fixture.lay_out(&mut layout, width);
        let relaid = Broken::new(&layout);
        assert!(
            !relaid.shapes.is_empty(),
            "edges are reshaped at {width} px"
        );
        let mut fresh_fixture = self::fixture();
        let mut fresh = Layout::new();
        fresh_fixture.text(&mut fresh, &style, text);
        fresh_fixture.lay_out(&mut fresh, width);
        assert_eq!(relaid, Broken::new(&fresh), "at {width} px");
    }

    let mut bare = Context::new(Collection::new());
    layout.break_lines(&mut bare, Area::new(12.0), &mut NoExclusions);
    assert_eq!(Broken::new(&layout), warm, "in a context with no fonts");
    assert_eq!(glyph_words(&layout), glyphs);
}
