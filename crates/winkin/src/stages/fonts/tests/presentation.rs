//! Emoji presentation tests: color or text fonts by variation selector,
//! then by `font-variant-emoji`.

use super::*;

/// A text-presentation pictograph takes the first font that has it, color
/// or not; a variation selector 16 or `font-variant-emoji: emoji` asks for a
/// color font; an emoji takes one where the text font has none; and a
/// variation selector 15 asks for a text font, falling back to the color
/// one where there is no other.
#[test]
fn emoji_presentation_chooses_between_color_and_text() {
    let mut fixture = fixture_with(&[symbols(), emoji()]);
    let mut layout = Layout::new();
    let listed = [
        FontFamilyName::named("Test Symbols"),
        FontFamilyName::named("Test Emoji"),
    ];
    let style = families_style(&listed);
    fixture.span(
        &mut layout,
        &style,
        "\u{2665} \u{2665}\u{FE0F} \u{1F600} \u{1F600}\u{FE0E}",
    );
    assert_eq!(
        fixture.families(&layout),
        [
            "Test Symbols",
            "Test Symbols",
            "Test Emoji",
            "Test Symbols",
            "Test Emoji",
            "Test Symbols",
            "Test Emoji",
        ]
    );

    let mut forced = style;
    forced.font.variant_emoji = FontVariantEmoji::Emoji;
    fixture.span(&mut layout, &forced, "\u{2665}\u{2665}\u{FE0E}");
    assert_eq!(fixture.families(&layout), ["Test Emoji", "Test Symbols"]);
}

/// Each cluster's presentation, from its class and its selectors, then
/// `font-variant-emoji`, which forces a form on an emoji character (Emoji=Yes)
/// as Chrome's glyph callback does, and only prefers one for a pictograph
/// that is no emoji, as Chrome's fallback priority does. A keycap base is asked, as
/// Chrome asks it (e9, e11).
#[test]
fn presentation_follows_selectors_then_the_property() {
    use ClusterClass::{Emoji, Symbol, Text};
    use FontVariantEmoji::{Emoji as AsEmoji, Normal, Text as AsText, Unicode};
    use fontwich::Presentation as P;
    let cases = [
        (Symbol, "\u{2665}", Normal, P::Text),
        (Emoji, "\u{2665}\u{FE0F}", Normal, P::EmojiOnly),
        (Emoji, "1\u{FE0F}\u{20E3}", Normal, P::EmojiOnly),
        (Emoji, "\u{1F600}", Normal, P::Emoji),
        (Symbol, "\u{1F600}\u{FE0E}", Normal, P::TextOnly),
        (Symbol, "\u{2665}", AsEmoji, P::EmojiOnly),
        (Emoji, "\u{1F600}", AsText, P::TextOnly),
        (Symbol, "\u{2665}\u{FE0E}", AsEmoji, P::TextOnly),
        (Text, "a", AsEmoji, P::Text),
        // `unicode` forces the form Emoji_Presentation says, as Chrome's
        // `kUseUnicodeDefaultPresentation` does, and CSS: 😀 is emoji and ♥
        // text, a font of the other colour refused.
        (Emoji, "\u{1F600}", Unicode, P::EmojiOnly),
        (Symbol, "\u{2665}", Unicode, P::TextOnly),
        // The keycap bases: `#` with VS16 asks for emoji, `*` with VS15 for
        // text, a digit takes the property, and alone is text.
        (Text, "#\u{FE0F}", Normal, P::EmojiOnly),
        (Text, "*\u{FE0E}", AsEmoji, P::TextOnly),
        (Text, "0", AsEmoji, P::EmojiOnly),
        (Text, "7", AsText, P::TextOnly),
        (Text, "5", Normal, P::Text),
        // A keycap asks for emoji under `normal`, and the property and VS15
        // override it, as Chrome 155 draws WPT's `font-variant-emoji-005`.
        (Text, "1\u{20E3}", Normal, P::EmojiOnly),
        (Text, "1\u{20E3}", AsText, P::TextOnly),
        (Text, "2\u{20E3}", Unicode, P::TextOnly),
        (Text, "3\u{20E3}", AsEmoji, P::EmojiOnly),
        (Text, "5\u{FE0E}\u{20E3}", AsEmoji, P::TextOnly),
        // ★ is a pictograph and no emoji: the property prefers a form and
        // forces none, and `unicode` leaves it as `normal` does.
        (Symbol, "\u{2605}", AsEmoji, P::Emoji),
        (Symbol, "\u{2605}", AsText, P::Text),
        (Symbol, "\u{2605}", Unicode, P::Text),
    ];
    for (class, text, variant, expected) in cases {
        assert_eq!(
            coverage::presentation(class, text, variant),
            expected,
            "{text:?}"
        );
    }
}

/// Every emoji character (Emoji=Yes) starts a cluster presentation is asked
/// of: an emoji or a pictograph's, which the analysis classes by
/// Emoji_Presentation and Extended_Pictographic, or a keycap base's, which
/// it classes as text and presentation asks after; so asking of those three
/// misses none Chrome's glyph callback asks of.
#[test]
fn every_emoji_character_is_asked_its_presentation() {
    let mut keycaps = 0;
    for ch in (0..=0x10FFFF).filter_map(char::from_u32) {
        let props = unicode::core_props(ch);
        if !props.is_emoji() {
            continue;
        }
        let pictograph = props.is_emoji_presentation() || props.is_extended_pictographic();
        assert!(
            pictograph || coverage::is_keycap_base(ch),
            "U+{:04X}",
            u32::from(ch)
        );
        keycaps += usize::from(!pictograph);
    }
    assert_eq!(keycaps, 12, "#, * and the ten digits");
}
