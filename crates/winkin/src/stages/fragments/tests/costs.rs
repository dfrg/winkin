//! Cost tests. They pin:
//! - timings of relayouts and reads over long articles, run in release;
//! - timings over deeply nested boxes;
//! - step counts in line with the items, however deep boxes nest.

use super::*;

/// Times a relayout of the ragged article and a read of every line's items and glyphs.
///
/// The relayout is breaking and line layout. The read should cost less than
/// the relayout. The article is ten words repeated 300 times, in Test Latin
/// at 16 px, at 900 px. Run in release:
///
/// ```text
/// cargo test --release -p winkin --lib -- --ignored --nocapture ragged_article
/// ```
#[test]
#[ignore = "a timing: run in release"]
fn the_ragged_article_relays_out_and_reads_back() {
    use std::time::Instant;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ".repeat(300);
    let style = sized(&LATIN, 16.0);
    fixture.block_text(&mut layout, &ComputedBlockStyle::new(&style), &words);
    let area = Area::new(900.0);
    layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
    let lines = layout.lines().len();
    let rounds = 2000;
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    // Breaking and line layout.
    let mut relayout = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        relayout.push(start.elapsed().as_secs_f64() * 1e6);
    }
    // Breaking alone, into a table of its own.
    let mut breaking = Vec::with_capacity(rounds);
    let mut lines_alone = Lines::new();
    for _ in 0..rounds {
        let (shaping, scratch) = fixture.cx.breaking();
        let input = crate::stages::lines::BreakInput {
            stages: layout.stages(),
            area,
            emphasis_room: EmphasisRoom::Shared,
            pretty: Pretty::Limited,
            ruby_break_within: RubyBreakWithin::BaseOpportunities,
        };
        lines_alone.clear();
        let start = Instant::now();
        lines::break_lines(
            &input,
            &mut ShapeSession::new(shaping, None),
            scratch,
            &mut NoExclusions,
            &mut lines_alone,
        );
        breaking.push(start.elapsed().as_secs_f64() * 1e6);
    }
    // Every line's items, and every glyph of every text item.
    let mut reading = Vec::with_capacity(rounds);
    let mut sum = 0u64;
    let mut glyph_count = 0usize;
    for _ in 0..rounds {
        let start = Instant::now();
        let input = layout.read_input();
        let fragments = layout.fragments();
        glyph_count = 0;
        for (id, line) in layout.line_records().lines.iter() {
            for item in fragments.line_items(id) {
                sum = sum.wrapping_add(item.inline.raw() as u64);
                if item.kind() == FragmentItemKind::Text {
                    for glyph in GlyphWalk::new(&input, id, line, item) {
                        sum = sum.wrapping_add(u64::from(glyph.id) ^ glyph.x.raw() as u64);
                        glyph_count += 1;
                    }
                }
            }
        }
        reading.push(start.elapsed().as_secs_f64() * 1e6);
    }
    // The part of the read that finds each line's items and starts each
    // run's glyphs.
    let mut starting = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        let input = layout.read_input();
        let fragments = layout.fragments();
        for (id, line) in layout.line_records().lines.iter() {
            for item in fragments.line_items(id) {
                let glyphs = GlyphWalk::new(&input, id, line, item);
                sum = sum.wrapping_add(glyphs.size_hint().0 as u64);
            }
        }
        starting.push(start.elapsed().as_secs_f64() * 1e6);
    }
    // The same read through the public API, as a painter makes it. It reads
    // every glyph's id, place in pixels, advance and cluster.
    let mut public = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        for line in layout.lines() {
            for item in line.items() {
                if let crate::Item::Text(run) = item {
                    for glyph in run.glyphs() {
                        let place = glyph.x.to_bits() ^ glyph.y.to_bits() ^ glyph.advance.to_bits();
                        sum = sum
                            .wrapping_add(u64::from(glyph.id ^ place) ^ glyph.text_offset as u64);
                    }
                }
            }
        }
        public.push(start.elapsed().as_secs_f64() * 1e6);
    }
    std::println!(
        "ragged article: {lines} lines, {glyph_count} glyphs, {} items; relayout F-H {:.1} µs, \
         F alone {:.1} µs, read {:.1} µs, of which starting each run {:.1} µs; read through \
         the public API {:.1} µs ({sum})",
        layout.fragments().items.len(),
        median(&mut relayout),
        median(&mut breaking),
        median(&mut reading),
        median(&mut starting),
        median(&mut public),
    );
}

/// Times a relayout and a public read of the ragged article dense with ruby and emphasis marks.
///
/// Every fourth word is a ruby base under a half-size reading of itself.
/// Every third word is marked. So the breaker makes room for annotations and
/// marks on every line, and line layout writes an annotation line per column.
/// The read covers every line's items, annotations, glyphs and marks. Run in
/// release:
///
/// ```text
/// cargo test --release -p winkin --lib -- --ignored --nocapture ruby_article
/// ```
#[test]
#[ignore = "a timing: run in release"]
fn the_ruby_article_relays_out_and_reads_back() {
    use std::time::Instant;
    let mut fixture = fixture();
    let style = sized(&LATIN, 16.0);
    let small = sized(&LATIN, 8.0);
    let mut marked = style;
    marked.text.emphasis.marks = true;
    let words = [
        "alpha ", "bravo ", "charlie ", "delta ", "echo ", "foxtrot ", "golf ", "hotel ", "india ",
        "juliet ",
    ];
    let mut layout = Layout::new();
    fixture.build(&mut layout, &ComputedBlockStyle::new(&style), |b| {
        for n in 0..3000u64 {
            let word = words[(n % 10) as usize];
            let key = NodeKey(4 * n);
            if n % 4 == 1 {
                b.open_ruby(key, &style, None);
                b.text(NodeKey(4 * n + 1), word.trim_end());
                b.open_annotation(NodeKey(4 * n + 2), &small, None);
                b.text(NodeKey(4 * n + 3), word.trim_end());
                b.close_annotation();
                b.close_ruby();
                b.text(NodeKey(4 * n + 3), " ");
            } else if n % 3 == 0 {
                b.open_box(key, &marked, None);
                b.text(NodeKey(4 * n + 1), word);
                b.close_box();
            } else {
                b.text(key, word);
            }
        }
    });
    let area = Area::new(900.0);
    layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
    let lines = layout.lines().len();
    let rounds = 2000;
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let mut relayout = Vec::with_capacity(rounds);
    for _ in 0..rounds {
        let start = Instant::now();
        layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        relayout.push(start.elapsed().as_secs_f64() * 1e6);
    }
    // Breaking alone, into a table of its own.
    let mut breaking = Vec::with_capacity(rounds);
    let mut lines_alone = Lines::new();
    for _ in 0..rounds {
        let (shaping, scratch) = fixture.cx.breaking();
        let input = crate::stages::lines::BreakInput {
            stages: layout.stages(),
            area,
            emphasis_room: EmphasisRoom::Shared,
            pretty: Pretty::Limited,
            ruby_break_within: RubyBreakWithin::BaseOpportunities,
        };
        lines_alone.clear();
        let start = Instant::now();
        lines::break_lines(
            &input,
            &mut ShapeSession::new(shaping, None),
            scratch,
            &mut NoExclusions,
            &mut lines_alone,
        );
        breaking.push(start.elapsed().as_secs_f64() * 1e6);
    }
    let mut reading = Vec::with_capacity(rounds);
    let mut sum = 0u64;
    let (mut glyphs, mut marks, mut annotations) = (0usize, 0usize, 0usize);
    for _ in 0..rounds {
        let start = Instant::now();
        (glyphs, marks, annotations) = (0, 0, 0);
        for line in layout.lines() {
            let mut read = |run: crate::TextRun<'_>| {
                for glyph in run.glyphs() {
                    sum = sum.wrapping_add(u64::from(glyph.id ^ glyph.x.to_bits()));
                    glyphs += 1;
                }
                for mark in run.emphasis_marks() {
                    sum = sum.wrapping_add(u64::from(mark.x.to_bits() ^ mark.baseline.to_bits()));
                    marks += 1;
                }
            };
            for item in line.items() {
                if let crate::Item::Text(run) = item {
                    read(run);
                }
            }
            for annotation in line.annotations() {
                annotations += 1;
                for run in annotation.runs() {
                    read(run);
                }
            }
        }
        reading.push(start.elapsed().as_secs_f64() * 1e6);
    }
    std::println!(
        "ruby article: {lines} lines, {annotations} annotations, {glyphs} glyphs, {marks} marks, \
         {} items; relayout F-H {:.1} µs, F alone {:.1} µs, read through the public API \
         {:.1} µs ({sum})",
        layout.fragments().items.len(),
        median(&mut relayout),
        median(&mut breaking),
        median(&mut reading),
    );
}

/// Times a relayout of the ragged article with shifted boxes and pictures, against the plain article.
///
/// Every seventh word is in a box of its own, taking each `vertical-align`
/// value in turn, `top` and `bottom` among them. Every thirteenth is a
/// picture, some aligned. So every line is measured box by box and settles
/// what waits in it. Line layout places each item on its box's baseline.
/// The plain article's lines are one flat union. Run in release:
///
/// ```text
/// cargo test --release -p winkin --lib -- --ignored --nocapture shifted_article
/// ```
#[test]
#[ignore = "a timing: run in release"]
fn the_shifted_article_relays_out() {
    use crate::style::VerticalAlign;
    use std::time::Instant;
    let mut fixture = fixture();
    let style = sized(&LATIN, 16.0);
    let aligns = [
        VerticalAlign::Super,
        VerticalAlign::Middle,
        VerticalAlign::Top,
        VerticalAlign::Sub,
        VerticalAlign::TextTop,
        VerticalAlign::Bottom,
        VerticalAlign::Px(2.0),
        VerticalAlign::TextBottom,
        VerticalAlign::Fraction(0.2),
    ];
    let words: Vec<&str> = "alpha bravo charlie delta echo foxtrot golf hotel india juliet "
        .repeat(300)
        .leak()
        .split(' ')
        .collect();
    let build = |fixture: &mut Fixture, layout: &mut Layout, shift: bool| {
        fixture.build(layout, &ComputedBlockStyle::new(&style), |b| {
            let mut key = 0;
            for (n, word) in words.iter().enumerate() {
                key += 1;
                if n % 7 == 3 {
                    let align = if shift {
                        aligns[(n / 7) % aligns.len()]
                    } else {
                        VerticalAlign::Baseline
                    };
                    let boxed = ComputedStyle {
                        line: LineGroup {
                            vertical_align: align,
                            ..style.line
                        },
                        font: FontGroup {
                            size: 12.0,
                            ..style.font
                        },
                        ..style
                    };
                    b.open_box(NodeKey(key), &boxed, None);
                    key += 1;
                    b.text(NodeKey(key), word);
                    b.close_box();
                    key += 1;
                    b.text(NodeKey(key), " ");
                } else if n % 13 == 5 {
                    let align = if shift {
                        aligns[(n / 13) % aligns.len()]
                    } else {
                        VerticalAlign::Baseline
                    };
                    let pictured = ComputedStyle {
                        line: LineGroup {
                            vertical_align: align,
                            ..style.line
                        },
                        ..style
                    };
                    let size = BoxSize {
                        inline: 18.0,
                        block: 10.0 + (n % 17) as f32,
                        baseline: None,
                    };
                    b.atomic(NodeKey(key), &pictured, None, size);
                    key += 1;
                    b.text(NodeKey(key), " ");
                } else {
                    b.text(NodeKey(key), word);
                    key += 1;
                    b.text(NodeKey(key), " ");
                }
            }
        });
    };
    let mut shifted = Layout::new();
    build(&mut fixture, &mut shifted, true);
    let mut unshifted = Layout::new();
    build(&mut fixture, &mut unshifted, false);
    let mut plain = Layout::new();
    fixture.block_text(
        &mut plain,
        &ComputedBlockStyle::new(&style),
        &words.join(" "),
    );
    let area = Area::new(900.0);
    let rounds = 2000;
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let mut time = |layout: &mut Layout| {
        layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        let mut samples = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = Instant::now();
            layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
            samples.push(start.elapsed().as_secs_f64() * 1e6);
        }
        median(&mut samples)
    };
    let (shifted_time, unshifted_time, plain_time) =
        (time(&mut shifted), time(&mut unshifted), time(&mut plain));
    std::println!(
        "shifted article: {} lines, {} items, {} shifts settled; relayout F-H {:.1} us; \
         the same boxes and pictures on the baseline: {} lines, {} items, {:.1} us; \
         the article as it is: {} lines, relayout F-H {:.1} us",
        shifted.lines().len(),
        shifted.fragments().items.len(),
        shifted.line_records().shifts.len(),
        shifted_time,
        unshifted.lines().len(),
        unshifted.fragments().items.len(),
        unshifted_time,
        plain.lines().len(),
        plain_time,
    );
}

/// Times a relayout and a glyph read of the ragged article, justified and letter-spaced, against it plain.
///
/// Justifying walks each justified line's clusters to count its
/// opportunities. A reader adds each justified cluster's share with a running
/// sum. Letter-spacing is in the prefix, so it costs a relayout nothing. Run
/// in release:
///
/// ```text
/// cargo test --release -p winkin --lib -- --ignored --nocapture justified_article
/// ```
#[test]
#[ignore = "a timing: run in release"]
fn the_justified_article_relays_out_and_reads_back() {
    use std::time::Instant;
    let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ".repeat(300);
    let area = Area::new(900.0);
    let rounds = 2000;
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let ragged = ComputedBlockStyle::default();
    let justified = aligned(TextAlign::Justify, TextAlignLast::Auto);
    let plain = sized(&LATIN, 16.0);
    let mut spaced = plain;
    spaced.text.letter_spacing = 0.5;
    for (name, block, style) in [
        ("ragged", ragged, plain),
        ("justified", justified, plain),
        ("letter-spaced", ragged, spaced),
    ] {
        let mut fixture = fixture();
        let mut layout = Layout::new();
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            &words,
        );
        layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        let mut relayout = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = Instant::now();
            layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
            relayout.push(start.elapsed().as_secs_f64() * 1e6);
        }
        let mut reading = Vec::with_capacity(rounds);
        let mut sum = 0u64;
        for _ in 0..rounds {
            let start = Instant::now();
            let input = layout.read_input();
            let fragments = layout.fragments();
            for (id, line) in layout.line_records().lines.iter() {
                for item in fragments.line_items(id) {
                    if item.kind() == FragmentItemKind::Text {
                        for glyph in GlyphWalk::new(&input, id, line, item) {
                            sum = sum.wrapping_add(u64::from(glyph.id) ^ glyph.x.raw() as u64);
                        }
                    }
                }
            }
            reading.push(start.elapsed().as_secs_f64() * 1e6);
        }
        std::println!(
            "{name} article: {} lines, {} justified; relayout F-H {:.1} µs, read {:.1} µs ({sum})",
            layout.lines().len(),
            layout.fragments().justified().len(),
            median(&mut relayout),
            median(&mut reading),
        );
    }
}

/// Times a rebuild and a relayout of a mixed-direction paragraph against the same text all left to right.
///
/// The text is the ragged article with an Arabic phrase in every sentence.
/// Every other phrase is in a padded right-to-left isolate, and every fifth
/// is an override. The block is left to right, at 900 px. The rebuild runs
/// every stage from the content up, so it prices resolving levels, splitting
/// runs and shaping both ways. The relayout prices reordering and the box
/// parts. Run in release:
///
/// ```text
/// cargo test --release -p winkin --lib -- --ignored --nocapture mixed_direction
/// ```
#[test]
#[ignore = "a timing: run in release"]
fn a_mixed_direction_paragraph_rebuilds_and_relays_out() {
    use crate::style::{Direction, UnicodeBidi};
    use std::time::Instant;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let families = [
        FontFamilyName::named("Test Latin"),
        FontFamilyName::named("Test Arabic"),
    ];
    let style = sized(&families, 16.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::<f32> {
                left: 2.0,
                right: 2.0,
                ..Sides::ZERO
            }
            .into(),
            ..EdgesGroup::INITIAL
        },
        ..style
    };
    let isolate = reading(&padded, Direction::Rtl, UnicodeBidi::Isolate);
    let overriding = reading(&style, Direction::Rtl, UnicodeBidi::BidiOverride);
    let arabic: String = [BEH, TEH, SEEN, ' ', MEEM, BEH, TEH, ' ', SEEN, MEEM]
        .iter()
        .collect();
    let latin = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ";
    let sentences = 300;
    let mixed = |b: &mut LayoutBuilder<'_>| {
        for n in 0..sentences {
            b.text(NodeKey(3 * n), latin);
            if n % 5 == 0 {
                b.open_box(NodeKey(3 * n + 1), &overriding, None);
            } else if n % 2 == 0 {
                b.open_box(NodeKey(3 * n + 1), &isolate, None);
            }
            b.text(NodeKey(3 * n + 2), &arabic);
            if n % 5 == 0 || n % 2 == 0 {
                b.close_box();
            }
            b.text(NodeKey(3 * n + 3), " ");
        }
    };
    let plain = |b: &mut LayoutBuilder<'_>| {
        for n in 0..sentences {
            b.text(NodeKey(3 * n), latin);
            b.text(NodeKey(3 * n + 2), "kilo lima mike nov");
            b.text(NodeKey(3 * n + 3), " ");
        }
    };
    let area = Area::new(900.0);
    let rounds = 300;
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let mut report = |name: &str, calls: &dyn Fn(&mut LayoutBuilder<'_>)| {
        fixture.build(&mut layout, &ComputedBlockStyle::new(&style), calls);
        layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        let mut rebuild = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = Instant::now();
            fixture.build(&mut layout, &ComputedBlockStyle::new(&style), calls);
            rebuild.push(start.elapsed().as_secs_f64() * 1e6);
        }
        layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        let mut relayout = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = Instant::now();
            layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
            relayout.push(start.elapsed().as_secs_f64() * 1e6);
        }
        let mixed = layout
            .analysis()
            .flags
            .contains(ParagraphFlags::MIXED_LEVELS);
        std::println!(
            "{name}: {} clusters, levels mixed {mixed}, {} runs, {} lines, {} items; rebuild \
             {:.1} µs, relayout F-H {:.1} µs",
            layout.analysis().clusters.len(),
            layout.analysis().runs.len(),
            layout.lines().len(),
            layout.fragments().items.len(),
            median(&mut rebuild),
            median(&mut relayout),
        );
    };
    report("left to right", &plain);
    report("mixed direction", &mixed);
}

/// Times a relayout in a fresh context over the same fonts against one in a warm context.
///
/// - The ragged and justified articles end lines at spaces and reshape nothing.
/// - An article of ligatures and kerned pairs, broken anywhere at 899 px, has
///   a third of its lines end or start inside one and reshape there.
///
/// The context is only a cache. A relayout in a fresh one rebuilds the font's
/// shaping plan from the layout's own font record, and grows the breaker's
/// and line layout's scratch. A relayout that reshapes nothing needs only
/// the scratch. Run in release:
///
/// ```text
/// cargo test --release -p winkin --lib -- --ignored --nocapture new_context
/// ```
#[test]
#[ignore = "a timing: run in release"]
fn a_new_context_relays_out_the_articles() {
    use std::time::Instant;
    let words = "alpha bravo charlie delta echo foxtrot golf hotel india juliet ".repeat(300);
    let joined = "office waffle AVAIL fifty flat affix fluffier ".repeat(300);
    let rounds = 500;
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    let plain = sized(&LATIN, 16.0);
    let mut anywhere = plain;
    anywhere.text.word_break = WordBreak::BreakAll;
    let justified = aligned(TextAlign::Justify, TextAlignLast::Auto);
    for (name, block, style, text, width) in [
        (
            "ragged",
            ComputedBlockStyle::default(),
            plain,
            &words,
            900.0,
        ),
        ("justified", justified, plain, &words, 900.0),
        (
            "broken anywhere",
            ComputedBlockStyle::default(),
            anywhere,
            &joined,
            899.0,
        ),
    ] {
        let area = Area::new(width);
        let mut fixture = fixture();
        let mut layout = Layout::new();
        fixture.block_text(
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            text,
        );
        layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
        let edges = layout.line_records().edges.shapes.len();
        let mut warm = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            let start = Instant::now();
            layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
            warm.push(start.elapsed().as_secs_f64() * 1e6);
        }
        let mut cold = Vec::with_capacity(rounds);
        for _ in 0..rounds {
            fixture.cx = Context::new(fixture.cx.collection().clone());
            let start = Instant::now();
            layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
            cold.push(start.elapsed().as_secs_f64() * 1e6);
        }
        std::println!(
            "{name} article: {} lines, {edges} edges reshaped; relayout F-H warm {:.1} µs, \
             in a new context {:.1} µs",
            layout.lines().len(),
            median(&mut warm),
            median(&mut cold),
        );
    }
}

// Deep nesting -------------------------------------------------------------

/// The ways the nesting benchmarks below set their boxes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Nesting {
    /// Every box kept for its padding, left to right.
    Kept,
    /// Every box culled: no edge, no paint, no shift.
    Culled,
    /// Every box kept and isolated, alternating direction.
    ///
    /// The levels climb past UAX #9's 125, and every line is reordered.
    Isolated,
    /// Every box kept, taking each `vertical-align` value in turn.
    ///
    /// Each line is measured box by box and settles what waits.
    Shifted,
    /// The outermost box an isolate and the rest culled, each word ending a paragraph.
    ///
    /// A forced break ends each paragraph. Every paragraph re-opens the
    /// isolate at its start, inside every box around it.
    Paragraphs,
}

/// Builds into `layout` boxes nested `depth` deep, set as `nesting` says, each holding a word.
///
/// Box `n` is keyed `2n + 1`. Its word, and the break after it where there
/// is one, are keyed `2n + 2`. The block is keyed 0.
fn nest(fixture: &mut Fixture, layout: &mut Layout, nesting: Nesting, depth: u64) {
    use crate::style::{Direction, UnicodeBidi, VerticalAlign};
    let root = sized(&LATIN, 16.0);
    let padded = ComputedStyle {
        edges: EdgesGroup {
            padding: Sides::from_px(1.0),
            ..EdgesGroup::INITIAL
        },
        ..root
    };
    let aligns = [
        VerticalAlign::Top,
        VerticalAlign::Middle,
        VerticalAlign::TextTop,
        VerticalAlign::Baseline,
        VerticalAlign::Bottom,
        VerticalAlign::Super,
        VerticalAlign::TextBottom,
        VerticalAlign::Px(2.0),
    ];
    let styles: Vec<ComputedStyle<'_>> = match nesting {
        Nesting::Kept => vec![padded],
        Nesting::Culled => vec![root],
        Nesting::Isolated => vec![
            reading(&padded, Direction::Rtl, UnicodeBidi::Isolate),
            reading(&padded, Direction::Ltr, UnicodeBidi::Isolate),
        ],
        Nesting::Shifted => aligns
            .iter()
            .map(|&align| vertically(&padded, align))
            .collect(),
        Nesting::Paragraphs => vec![reading(&root, Direction::Rtl, UnicodeBidi::Isolate), root],
    };
    fixture.build(layout, &ComputedBlockStyle::new(&root), |b| {
        for n in 0..depth {
            let style = match nesting {
                Nesting::Paragraphs => &styles[usize::from(n > 0)],
                _ => &styles[(n as usize) % styles.len()],
            };
            b.open_box(NodeKey(2 * n + 1), style, None);
            b.text(NodeKey(2 * n + 2), "ab ");
            if nesting == Nesting::Paragraphs {
                b.line_break(NodeKey(2 * n + 2));
            }
        }
        for _ in 0..depth {
            b.close_box();
        }
    });
}

/// Reads back a layout that [`nest`] built `depth` deep, as a host does.
///
/// It reads every line's items and glyphs, and every line's paint with the
/// block decorated. It queries the fragments of 64 evenly spread boxes, one
/// at a time. Returns a sum of what it read, so nothing goes unread.
fn read_back(layout: &Layout, depth: u64) -> u64 {
    let mut sum = 0u64;
    let decorates = |key: NodeKey| {
        if key == NodeKey(0) {
            Decorates::Both
        } else {
            Decorates::None
        }
    };
    for line in layout.lines() {
        for item in line.all_items() {
            if let crate::Item::Text(run) = item {
                for glyph in run.glyphs() {
                    sum = sum.wrapping_add(u64::from(glyph.id) ^ u64::from(glyph.x.to_bits()));
                }
            }
        }
        sum = sum.wrapping_add(line.paints(decorates).count() as u64);
    }
    for n in (0..64).map(|k| k * depth / 64) {
        for piece in layout.box_fragments(NodeKey(2 * n + 1)) {
            sum = sum.wrapping_add(u64::from(piece.inline().left.to_bits()));
        }
    }
    sum
}

/// Times every stage over boxes nested 500 to 4,000 deep, each way [`Nesting`] names.
///
/// [`nest`] builds the boxes, each holding a word. They lay out on one line
/// and broken at 300 px. The stages timed are building (through
/// measurement), breaking, line layout with the block, and [`read_back`].
///
/// The caller sets the nesting, so no stage may cost more than its input and
/// output. On one line, every stage grows with the depth. Broken, every line
/// carries the kept boxes open across it, as Chrome's does. So the items per
/// line grow with the depth, and the time per item stays level. Run in
/// release:
///
/// ```text
/// cargo test --release -p winkin --lib -- --ignored --nocapture deep_nesting
/// ```
#[test]
#[ignore = "a timing: run in release"]
fn a_deep_nesting_lays_out_in_time_with_its_depth() {
    use std::time::Instant;
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let rounds = 3;
    let median = |samples: &mut Vec<f64>| {
        samples.sort_by(f64::total_cmp);
        samples[samples.len() / 2]
    };
    for nesting in [
        Nesting::Kept,
        Nesting::Culled,
        Nesting::Isolated,
        Nesting::Shifted,
        Nesting::Paragraphs,
    ] {
        for width in [1e7, 300.0] {
            for depth in [500u64, 1000, 2000, 4000] {
                let area = Area::new(width);
                let (mut build, mut breaking, mut placing, mut reading) =
                    (Vec::new(), Vec::new(), Vec::new(), Vec::new());
                let mut sum = 0u64;
                let mut lines = Lines::new();
                let mut fragments = Fragments::new();
                let mut scratch = PlaceScratch::new();
                for _ in 0..rounds {
                    let start = Instant::now();
                    nest(&mut fixture, &mut layout, nesting, depth);
                    build.push(start.elapsed().as_secs_f64() * 1e3);
                    let (shaping, breaker) = fixture.cx.breaking();
                    let input = crate::stages::lines::BreakInput {
                        stages: layout.stages(),
                        area,
                        emphasis_room: EmphasisRoom::Shared,
                        pretty: Pretty::Limited,
                        ruby_break_within: RubyBreakWithin::BaseOpportunities,
                    };
                    lines.clear();
                    fragments.clear();
                    let start = Instant::now();
                    lines::break_lines(
                        &input,
                        &mut ShapeSession::new(shaping, None),
                        breaker,
                        &mut NoExclusions,
                        &mut lines,
                    );
                    breaking.push(start.elapsed().as_secs_f64() * 1e3);
                    let input = PlaceInput {
                        stages: layout.stages(),
                        lines: &lines,
                        area,
                        tab_justification: fixture.cx.config().tab_justification,
                        ellipsis_space: fixture.cx.config().ellipsis_space,
                        placements: fixture.cx.placing().0,
                    };
                    let start = Instant::now();
                    place_fragments(&input, &mut scratch, &mut fragments);
                    placing.push(start.elapsed().as_secs_f64() * 1e3);
                    layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
                    let start = Instant::now();
                    sum = sum.wrapping_add(read_back(&layout, depth));
                    reading.push(start.elapsed().as_secs_f64() * 1e3);
                }
                std::println!(
                    "{nesting:?} at {width} px, {depth} deep: {} lines, {} items; build {:.2} \
                     ms, break {:.2} ms, lay out {:.2} ms, read back {:.2} ms ({sum})",
                    layout.lines().len(),
                    layout.fragments().items.len(),
                    median(&mut build),
                    median(&mut breaking),
                    median(&mut placing),
                    median(&mut reading),
                );
            }
        }
    }
}

/// Returns the steps each stage takes over boxes [`nest`] builds, and the items the stages hold.
///
/// The boxes are nested `depth` deep in `nesting`, broken in an area `width`
/// wide. The stages are building (through measurement), breaking, line
/// layout with the block, and [`read_back`]. `crate::work` counts the steps,
/// only in debug builds. The item count adds the content's items, the
/// fragment items and the lines.
#[cfg(debug_assertions)]
fn steps(fixture: &mut Fixture, nesting: Nesting, depth: u64, width: f32) -> ([u64; 4], u64) {
    let mut layout = Layout::new();
    work::take();
    nest(fixture, &mut layout, nesting, depth);
    let build = work::take();
    let area = Area::new(width);
    let mut lines = Lines::new();
    let (shaping, breaker) = fixture.cx.breaking();
    let input = crate::stages::lines::BreakInput {
        stages: layout.stages(),
        area,
        emphasis_room: EmphasisRoom::Shared,
        pretty: Pretty::Limited,
        ruby_break_within: RubyBreakWithin::BaseOpportunities,
    };
    lines::break_lines(
        &input,
        &mut ShapeSession::new(shaping, None),
        breaker,
        &mut NoExclusions,
        &mut lines,
    );
    let breaking = work::take();
    let input = PlaceInput {
        stages: layout.stages(),
        lines: &lines,
        area,
        tab_justification: fixture.cx.config().tab_justification,
        ellipsis_space: fixture.cx.config().ellipsis_space,
        placements: fixture.cx.placing().0,
    };
    let mut fragments = Fragments::new();
    place_fragments(&input, &mut PlaceScratch::new(), &mut fragments);
    let placing = work::take();
    layout.break_lines(&mut fixture.cx, area, &mut NoExclusions);
    work::take();
    read_back(&layout, depth);
    let reading = work::take();
    let size = layout.content().items.len() + layout.fragments().items.len() + layout.lines().len();
    ([build, breaking, placing, reading], size as u64)
}

/// Every stage takes steps in proportion to its input and output, however deep boxes nest.
///
/// Boxes nest 300 and 1,200 deep, each way [`Nesting`] names, on one line
/// and broken into many. The steps per item of each stage stay level as the
/// depth grows fourfold. On one line, the items grow with the depth. Broken,
/// every line carries a fragment of each kept box open across it, as
/// Chrome's does. So the items grow with the depth's square, and the steps
/// may grow no faster.
///
/// The test counts steps (`crate::work`), not time. A walk over a line's
/// boxes once per box takes four times the steps per item at four times the
/// depth. It fails on a loaded machine as on an idle one, where a clock would
/// be flaky. The test covers every walk whose length the nesting sets.
/// `a_deep_nesting_lays_out_in_time_with_its_depth` times them. Steps are
/// counted only in debug builds, so release builds, whose timing tests the
/// counting would slow, have no such test.
#[cfg(debug_assertions)]
#[test]
fn nestings_take_steps_in_line_with_their_items() {
    let mut fixture = fixture();
    let stages = ["building", "breaking", "line layout", "reading back"];
    for nesting in [
        Nesting::Kept,
        Nesting::Culled,
        Nesting::Isolated,
        Nesting::Shifted,
        Nesting::Paragraphs,
    ] {
        for width in [1e7, 300.0] {
            let (shallow, shallow_items) = steps(&mut fixture, nesting, 300, width);
            let (deep, deep_items) = steps(&mut fixture, nesting, 1200, width);
            for (at, stage) in stages.iter().enumerate() {
                let each = |steps: [u64; 4], items: u64| steps[at] as f64 / items as f64;
                let (shallow, deep) = (each(shallow, shallow_items), each(deep, deep_items));
                assert!(
                    deep <= 1.5 * shallow + 0.1,
                    "{nesting:?} at {width} px, {stage}: {shallow:.2} steps an item 300 deep, \
                     {deep:.2} 1,200 deep"
                );
            }
        }
    }
}
