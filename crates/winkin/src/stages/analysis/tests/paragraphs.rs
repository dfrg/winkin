//! Paragraph tests. They pin:
//! - where paragraphs end, at separators and forced breaks;
//! - each paragraph's base level and flags.

use super::*;

/// A paragraph ends after LF, CRLF, VT, FF, NEL, U+2028 and U+2029.
///
/// It does not end after a lone CR, which is a space (CSS Text 3, section 4).
/// Nor does it end after U+001C to U+001E, which are bidi class B but break
/// no line. A CRLF counts as its LF. VT, FF and NEL, the control characters
/// among them, are drawn.
#[test]
fn paragraphs_end_at_separators() {
    use ClusterClass as C;
    let layout = pre("a\nb\u{2029}c\u{2028}d\re\r\nf\u{B}g\u{C}h\u{85}i\u{1C}j");
    assert_eq!(
        paragraphs(&layout),
        [
            "a\n",
            "b\u{2029}",
            "c\u{2028}",
            "d e\n",
            "f\u{B}",
            "g\u{C}",
            "h\u{85}",
            "i\u{1C}j"
        ]
    );
    let ends: Vec<ClusterClass> = analysis(&layout)
        .paragraphs
        .iter()
        .filter_map(|(id, _)| {
            let end = analysis(&layout)
                .paragraphs
                .clusters(id)
                .end
                .get()
                .checked_sub(1)?;
            analysis(&layout).clusters.attrs(ClusterId::new(end))
        })
        .map(ClusterAttrs::class)
        .collect();
    assert_eq!(
        ends,
        [
            C::Separator,
            C::Separator,
            C::Separator,
            C::Separator,
            C::DrawnSeparator,
            C::DrawnSeparator,
            C::DrawnSeparator,
            C::Text
        ]
    );
    // Whatever the white space: under `normal`, the spaces either side of
    // each go with it.
    let layout = plain("a \u{B} b \u{C} c \u{85} d \u{2028} e \u{2029} f");
    assert_eq!(
        paragraphs(&layout),
        ["a\u{B}", "b\u{C}", "c\u{85}", "d\u{2028}", "e\u{2029}", "f"]
    );
}

#[test]
fn a_break_ends_a_paragraph_and_a_final_one_leaves_an_empty_one() {
    let layout = build(|b| {
        b.text(key(1), "one");
        b.line_break(key(2));
        b.text(key(3), "two");
        b.line_break(key(4));
    });
    assert_eq!(paragraphs(&layout), ["one\n", "two\n", ""]);
    // A leading break is a paragraph of its own.
    let layout = build(|b| {
        b.line_break(key(1));
        b.text(key(2), "x");
    });
    assert_eq!(paragraphs(&layout), ["\n", "x"]);
}

#[test]
fn an_empty_text_is_one_empty_paragraph() {
    let layout = build(|_| {});
    assert_eq!(paragraphs(&layout), [""]);
    assert!(analysis(&layout).runs.is_empty());
    let layout = build(|b| {
        b.open_box(key(1), &ComputedStyle::initial(), None);
        b.close_box();
    });
    assert_eq!(paragraphs(&layout), [""]);
    assert_eq!(
        analysis(&layout).item_clusters.firsts.as_slice(),
        [ClusterId::new(0); 2]
    );
}

/// The base level is the block's direction, or where that is `auto`, each
/// paragraph's first strong character's outside isolates (P2, P3).
#[test]
fn a_paragraphs_level_is_its_direction() {
    let root = white_space(WhiteSpaceCollapse::Preserve);
    let block = |direction| ComputedBlockStyle {
        direction,
        ..ComputedBlockStyle::new(&root)
    };
    let text = "abc\n\u{5D0}\u{5D1}\n123 \u{5D0}\n\u{2067}\u{5D0}\u{2069} a\n1";
    let levels_in = |direction| levels(&build_with(&block(direction), |b| b.text(key(1), text)));
    assert_eq!(levels_in(BaseDirection::Ltr), [0; 5]);
    assert_eq!(levels_in(BaseDirection::Rtl), [1; 5]);
    assert_eq!(levels_in(BaseDirection::Auto), [0, 1, 1, 0, 0]);
}

#[test]
fn a_paragraphs_flags_say_what_it_holds() {
    use ParagraphFlags as F;
    let flags = |layout: &Layout| -> Vec<ParagraphFlags> {
        analysis(layout)
            .paragraphs
            .iter()
            .map(|(_, paragraph)| paragraph.flags)
            .collect()
    };
    let layout = pre(
        "plain\n\u{5D0}\n\u{627}\n\u{661}\na\tb\nsoft\u{AD}\n日本\n\u{E01}\u{E02}\n🙂\n\u{2067}x\u{2069}\n\u{1100}",
    );
    // Hebrew, Arabic, an Arabic digit and an isolate in a left-to-right
    // block have clusters at other levels than their paragraphs', and the
    // Hebrew and the Arabic read right to left; the digit and the isolate
    // are at 2.
    let mixed = F::MIXED_LEVELS;
    let rtl = mixed.union(F::RIGHT_TO_LEFT);
    assert_eq!(
        flags(&layout),
        [
            F::NONE,
            rtl,
            rtl,
            mixed,
            F::HAS_TABS.union(F::HAS_SHAPING_STOPS),
            F::HAS_SOFT_HYPHEN,
            F::HAS_EAST_ASIAN,
            F::NONE,
            F::NONE,
            mixed,
            F::HAS_EAST_ASIAN,
        ]
    );
    let mut union = F::NONE;
    for flag in flags(&layout) {
        union.insert(flag);
    }
    assert_eq!(analysis(&layout).flags, union);

    // An atomic inline, which raises nothing, and a right-to-left isolate,
    // whose controls reach every paragraph it spans, and not the one after
    // it: what opens the bidi gate a paragraph is resolved behind.
    let isolate = styled(|style| {
        style.bidi.unicode_bidi = UnicodeBidi::Isolate;
        style.bidi.direction = Direction::Rtl;
    });
    let layout = build(|b| {
        b.text(key(1), "a");
        b.atomic(key(2), &ComputedStyle::initial(), None, BoxSize::default());
        b.line_break(key(3));
        b.open_box(key(4), &isolate, None);
        b.text(key(5), "b");
        b.line_break(key(6));
        b.text(key(7), "c");
        b.line_break(key(8));
        b.close_box();
        b.text(key(9), "d");
    });
    assert_eq!(cluster_levels(&layout), [0, 0, 0, 2, 0, 2, 0, 0]);
    // Left-to-right controls with nothing right to left move no level:
    // Blink finds such a paragraph unidirectional and leaves it at 0.
    for unicode_bidi in [
        UnicodeBidi::Embed,
        UnicodeBidi::Isolate,
        UnicodeBidi::BidiOverride,
        UnicodeBidi::IsolateOverride,
        UnicodeBidi::Plaintext,
    ] {
        let ltr = styled(|style| style.bidi.unicode_bidi = unicode_bidi);
        let layout = build(|b| {
            b.text(key(1), "a ");
            b.open_box(key(2), &ltr, None);
            b.text(key(3), "b 1.");
            b.close_box();
            b.text(key(4), " c");
        });
        assert!(
            cluster_levels(&layout).iter().all(|&level| level == 0),
            "{unicode_bidi:?}"
        );
        assert!(
            !analysis(&layout).flags.contains(F::MIXED_LEVELS),
            "{unicode_bidi:?}"
        );
    }
    // Right-to-left text beside one still has the paragraph resolved, the
    // isolate's text two levels up.
    let ltr = styled(|style| style.bidi.unicode_bidi = UnicodeBidi::Isolate);
    let layout = build(|b| {
        b.text(key(1), "\u{5D0} ");
        b.open_box(key(2), &ltr, None);
        b.text(key(3), "b");
        b.close_box();
    });
    assert_eq!(cluster_levels(&layout), [1, 0, 2]);
    // The block's left-to-right override moves no level either.
    let overriding = styled(|style| style.bidi.unicode_bidi = UnicodeBidi::BidiOverride);
    let layout = build_with(&ComputedBlockStyle::new(&overriding), |b| {
        b.text(key(1), "a");
        b.line_break(key(2));
        b.text(key(3), "b");
    });
    assert_eq!(cluster_levels(&layout), [0, 0, 0]);
}
