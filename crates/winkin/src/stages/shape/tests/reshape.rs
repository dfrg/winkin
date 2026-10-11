//! Reshaping tests: unsafe-to-break bits as harfrust sets them, a
//! sub-range shaped at a safe boundary equal to the slice of the whole,
//! what a reshaped piece reports, the context's caches, and the cost of
//! prose in the glyph table.

use super::*;
use crate::style::FirstLineVariant;
use crate::style::LengthPercentage;

// Unsafe bits --------------------------------------------------------------

/// Each cluster's unsafe bit is harfrust's flag for the glyph at its
/// start, shaped the same way directly: a ligature's inside and a kerned
/// pair's second are unsafe to break before, the rest safe. The first
/// cluster of a run keeps none.
#[test]
fn unsafe_bits_match_harfrusts_flags() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    let text = "An office AVAIL fit, Quiet R.";
    fixture.span(&mut layout, &families_style(&LATIN_HEBREW), text);
    let bytes = latin_hebrew().build();
    let font = harfrust::Font::new(bytes, 0).unwrap();
    let shaper = harfrust::ShaperFont::new(&font).with_scale(16 * 65_536);
    let mut output = harfrust::Buffer::new();
    output.set_direction(harfrust::Direction::LeftToRight);
    output.set_script(Some(Script::LATIN));
    output.set_cluster_level(harfrust::ClusterLevel::MonotoneCharacters);
    output.set_flags(harfrust::BufferFlags::BEGINNING_OF_TEXT);
    for (at, ch) in text.char_indices() {
        output.push(u32::from(ch), u32::try_from(at).unwrap());
    }
    harfrust::shape(&shaper, &mut output, harfrust::ShapeOptions::new()).unwrap();
    let shaped = layout.shaped().text(FirstLineVariant::Standard);
    let clusters = &layout.analysis().clusters;
    let mut unsafe_count = 0;
    for (cluster, word) in shaped.glyphs.iter().skip(1) {
        let start = u32::try_from(clusters.start(cluster).get()).unwrap();
        let glyph = output.glyph_infos().iter().find(|g| g.cluster == start);
        let to_break = glyph.is_none_or(|g| g.unsafe_to_break());
        assert_eq!(word.is_unsafe_to_break(), to_break, "{cluster:?}");
        unsafe_count += usize::from(to_break);
    }
    // Inside `ffi` twice, inside `fi`, and after the kerned `A`.
    assert!(unsafe_count >= 4, "{unsafe_count}");
    assert!(layout.shaped().flags.contains(ShapedFlags::HAS_UNSAFE));
}

/// A run split at a style boundary starts safe: the kern across the split is
/// gone, and so is its bit, which a single run keeps.
#[test]
fn a_run_starts_safe() {
    let mut fixture = fixture();
    let style = families_style(&LATIN_HEBREW);
    let spaced = ComputedStyle {
        text: TextGroup {
            letter_spacing: LengthPercentage::from_px(1.0),
            ..style.text
        },
        ..style
    };
    let mut layout = Layout::new();
    fixture.span(&mut layout, &style, "AV");
    assert!(
        layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .glyphs
            .word(ClusterId::new(1))
            .is_unsafe_to_break()
    );
    fixture.spans(&mut layout, &style, &[(&style, "A"), (&spaced, "V")]);
    assert_eq!(
        layout.shaped().text(FirstLineVariant::Standard).runs.len(),
        2
    );
    assert!(
        !layout
            .shaped()
            .text(FirstLineVariant::Standard)
            .glyphs
            .word(ClusterId::new(1))
            .is_unsafe_to_break()
    );
}

// The shaping function -----------------------------------------------------

/// The one shaping function, on a sub-range of a run, as the breaker will
/// call it: at every boundary the run's words say is safe, the two sides
/// shaped apart are exactly the slices of the whole, glyphs, offsets,
/// advances, continuations and unsafe-to-break bits alike; at a boundary
/// they say is unsafe, some side differs, and the word said so. Latin with
/// ligatures and kerns, and Arabic, left to right and right to left. That is
/// the promise the breaker's reshape windows stand on, which start and end
/// where breaking is safe.
///
/// Arabic set against its direction is left out: harfrust reverses its
/// graphemes and then joins the reversed text's first letter to the
/// paragraph's context before it, as HarfBuzz does, so no break in it
/// shapes as the whole. That is Arabic under a left-to-right override. In a
/// left-to-right paragraph it resolves to level 1 and is shaped right to
/// left, so it is in.
#[test]
fn a_sub_range_at_a_safe_boundary_is_the_slice_of_the_whole() {
    let mut fixture = fixture();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::default()
    };
    let arabic_text: String = [BEH, TEH, ' ', SEEN, MEEM, BEH, ' ', TEH, SEEN]
        .iter()
        .collect();
    let hebrew_text: String = [ALEF, LAMED, BET, ' ', GIMEL, ALEF, LAMED].iter().collect();
    let cases = [
        (
            ComputedBlockStyle::default(),
            families_style(&LATIN_HEBREW),
            String::from("office AVAIL fit Quiet R flat"),
        ),
        (
            rtl,
            families_style(&LATIN_HEBREW),
            String::from("office AVAIL fit"),
        ),
        (rtl, families_style(&LATIN_HEBREW), hebrew_text),
        (rtl, families_style(&ARABIC), arabic_text.clone()),
        (
            ComputedBlockStyle::default(),
            families_style(&ARABIC),
            arabic_text,
        ),
    ];
    let mut differed = 0;
    for (block, style, text) in cases {
        let mut layout = Layout::new();
        fixture.build_spans(
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            &[(&style, &text)],
        );
        let advances = fixture.advances(&layout);
        let shaped = layout.shaped().text(FirstLineVariant::Standard);
        assert_eq!(shaped.runs.len(), 1, "{text}");
        let whole: Vec<(GlyphWord, Vec<Drawn>)> = shaped
            .glyphs
            .iter()
            .map(|(c, &w)| (w, drawn(w, &shaped.glyphs.sidecar, advances[c.get()])))
            .collect();
        let count = whole.len();
        let run = ShapedRunId::new(0);
        for cut in 1..count {
            let cut_id = ClusterId::new(cut);
            let head = fixture.reshape(&layout, run, ClusterId::new(0)..cut_id);
            let tail = fixture.reshape(&layout, run, cut_id..ClusterId::new(count));
            let mut pieces: Vec<(GlyphWord, Vec<Drawn>)> = Vec::new();
            for piece in [&head, &tail] {
                assert_eq!(piece.words.len(), piece.advances.len());
                pieces.extend(
                    piece.words.iter().map(|(c, &w)| {
                        (w, drawn(w, &piece.sidecar, piece.advances[c.get()].raw()))
                    }),
                );
            }
            assert_eq!(pieces.len(), count);
            let safe = !whole[cut].0.is_unsafe_to_break();
            let glyphs_equal = pieces.iter().zip(&whole).all(|(p, w)| p.1 == w.1);
            if safe {
                assert!(
                    glyphs_equal,
                    "{text:?} cut at {cut}: {pieces:?} against {whole:?}"
                );
                for (at, (piece, whole)) in pieces.iter().zip(&whole).enumerate() {
                    if at == cut {
                        // Shaped as a start: it keeps no bits, and harfrust
                        // said it was safe to break there too.
                        assert_eq!(bits(piece.0), (false, false));
                        assert!(!tail.start);
                        continue;
                    }
                    let (piece, whole) = (bits(piece.0), bits(whole.0));
                    let at_cut = format!("{text:?} cut at {cut}, cluster {at}");
                    assert_eq!(piece, whole, "{at_cut}");
                }
            } else if !glyphs_equal {
                differed += 1;
            }
        }
    }
    assert!(
        differed > 0,
        "an unsafe boundary changed the shaping somewhere"
    );
}

/// A word's bits: unsafe to break, a continuation.
fn bits(word: GlyphWord) -> (bool, bool) {
    (word.is_unsafe_to_break(), word.is_continuation())
}

/// A reshaped range reports what harfrust said of breaking at its start,
/// on which the breaker widens a line end's window. One
/// that starts with a joining letter is safe to break there: harfrust says
/// only that it is unsafe to concatenate, which nothing asks it for, as
/// nothing in Chrome does. Latin with nothing to join or
/// kern there is safe too, and the word the range starts with keeps no
/// bits either way.
#[test]
fn the_start_of_a_reshaped_range_is_reported() {
    let mut fixture = fixture();
    let style = families_style(&ARABIC);
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::new(&style)
    };
    let text: String = [BEH, TEH, SEEN, ' ', MEEM, BEH].iter().collect();
    let mut layout = Layout::new();
    fixture.build_spans(&mut layout, &rtl, &[(&style, &text)]);
    let run = ShapedRunId::new(0);
    let inside = fixture.reshape(&layout, run, ClusterId::new(1)..ClusterId::new(6));
    assert!(!inside.start);
    assert_eq!(bits(inside.words[ClusterId::new(0)]), (false, false));
    fixture.span(&mut layout, &families_style(&LATIN_HEBREW), "ab cd");
    let latin = fixture.reshape(&layout, run, ClusterId::new(3)..ClusterId::new(5));
    assert!(!latin.start);
}

/// Plans are the context's: a rebuild, or another layout of the same
/// content, compiles none, and one font is read once however many layouts
/// shape in it.
#[test]
fn plans_and_fonts_are_kept_by_the_context() {
    let mut fixture = fixture();
    let style = families_style(&LATIN_HEBREW);
    let mut layout = Layout::new();
    fixture.span(&mut layout, &style, "office AVAIL");
    let cx = fixture.cx.shaping();
    let plans = cx.plan_count();
    assert!(plans > 0);
    fixture.span(&mut layout, &style, "office AVAIL");
    let mut other = Layout::new();
    fixture.span(&mut other, &style, "flat fit");
    let cx = fixture.cx.shaping();
    assert_eq!(cx.plan_count(), plans);
}

/// The context's caches are keyed by what a layout's used font holds, never
/// by an id of theirs. A new context over the same fonts,
/// filled with another font's data first, so that its tables name the
/// layout's font by another id, shapes a layout built in the old one, and a
/// range of it as the breaker reshapes it, exactly as the old did warm,
/// making what it needs again from the layout's own record of its font.
#[test]
fn a_new_context_shapes_the_same() {
    let mut fixture = fixture();
    let mut layout = Layout::new();
    fixture.span(&mut layout, &families_style(&LATIN_HEBREW), "office AVAIL");
    let advances = fixture.advances(&layout);
    let run = ShapedRunId::new(0);
    let piece = fixture.reshape(&layout, run, ClusterId::new(2)..ClusterId::new(9));

    fixture.cx = Context::new(fixture.cx.collection().clone());
    let cx = fixture.cx.shaping();
    assert_eq!(cx.plan_count(), 0);
    let word: String = [BEH, TEH, SEEN, MEEM].iter().collect();
    let mut other = Layout::new();
    fixture.span(&mut other, &families_style(&ARABIC), &word);
    let others = fixture.cx.shaping().plan_count();
    assert!(others > 0);

    assert_eq!(fixture.advances(&layout), advances);
    let again = fixture.reshape(&layout, run, ClusterId::new(2)..ClusterId::new(9));
    assert_eq!(again.words.as_slice(), piece.words.as_slice());
    assert_eq!(again.sidecar.as_slice(), piece.sidecar.as_slice());
    assert_eq!(again.advances, piece.advances);
    assert_eq!(again.start, piece.start);
    assert_eq!(fixture.cx.shaping().plan_count(), others + 1);
}

/// Prose costs the glyph table 4 bytes a cluster, whatever its script,
/// where its fonts draw each cluster as one glyph with no offset: Latin with
/// ligatures and kerns, joined Arabic, and Japanese falling back to a CJK
/// font. Only what a font draws as several glyphs, or moves, reaches the
/// sidecar.
#[test]
fn prose_costs_four_bytes_a_cluster() {
    let mut fixture = fixture();
    let rtl = ComputedBlockStyle {
        direction: BaseDirection::Rtl,
        ..ComputedBlockStyle::default()
    };
    let word: String = [BEH, TEH, SEEN, MEEM].iter().collect();
    let arabic_text = [word.as_str(); 6].join(" ");
    let samples = [
        (
            ComputedBlockStyle::default(),
            families_style(&LATIN_HEBREW),
            "An office AVAIL of fine flat fields.",
        ),
        (rtl, families_style(&ARABIC), arabic_text.as_str()),
        (
            ComputedBlockStyle::default(),
            families_style(&AHEM_FAMILY),
            "日本語の文章は、漢字と仮名で書かれる。",
        ),
    ];
    for (block, style, text) in samples {
        let mut layout = Layout::new();
        fixture.build_spans(
            &mut layout,
            &ComputedBlockStyle {
                style: &style,
                ..block
            },
            &[(&style, text)],
        );
        let shaped = layout.shaped().text(FirstLineVariant::Standard);
        assert!(shaped.glyphs.sidecar.is_empty(), "{text}");
        assert!(
            shaped
                .glyphs
                .iter()
                .all(
                    |(_, w)| w.glyphs(&shaped.glyphs.sidecar) != ClusterGlyphs::None
                        || w.is_continuation()
                ),
            "{text}"
        );
    }
}
