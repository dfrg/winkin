//! The bidi resolver allocates nothing once warm: paragraphs that reach
//! every rule group the resolver can skip, resolved and reordered through a
//! scratch that has grown.

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use icu_properties::props::BidiClass as IcuBidiClass;
use icu_properties::props::{BidiMirroringGlyph, BidiPairedBracketType, EnumeratedProperty};

use super::bidi_conformance::resolve_paragraph;
use crate::stages::analysis::bidi::{BidiScratch, BidiUnit};
use crate::tests::allocator::count_allocations;
use crate::unicode::{BidiBracket, BidiClass, reorder_bidi};

/// A paragraph as the resolver takes it, with buffers for its answers.
struct Paragraph {
    units: Vec<BidiUnit>,
    brackets: Vec<BidiBracket>,
    levels: Vec<u8>,
    order: Vec<u32>,
}

impl Paragraph {
    // ICU4C's numbering is the resolver's; see `bidi_conformance.rs`.
    #[allow(deprecated)]
    fn new(text: &str) -> Self {
        let units: Vec<_> = text
            .chars()
            .map(|ch| BidiUnit {
                class: BidiClass(IcuBidiClass::for_char(ch).to_icu4c_value()),
                control: false,
            })
            .collect();
        let brackets = text
            .chars()
            .enumerate()
            .filter_map(|(i, ch)| {
                let bracket = BidiMirroringGlyph::for_char(ch);
                let (closing, is_open) = match bracket.paired_bracket_type {
                    BidiPairedBracketType::Open => (bracket.mirroring_glyph?, true),
                    BidiPairedBracketType::Close => (ch, false),
                    _ => return None,
                };
                Some(BidiBracket {
                    index: i as u32,
                    closing,
                    is_open,
                })
            })
            .collect();
        let len = units.len();
        Self {
            units,
            brackets,
            levels: vec![0; len],
            order: vec![0; len],
        }
    }

    /// Resolves and reorders the paragraph under every base direction.
    fn resolve(&mut self, scratch: &mut BidiScratch) {
        for base_level in [None, Some(0), Some(1)] {
            resolve_paragraph(
                scratch,
                &self.units,
                &self.brackets,
                base_level,
                &mut self.levels,
            );
            let levels = &self.levels;
            reorder_bidi(&mut self.order, |i| levels[i]);
        }
    }
}

/// Paragraphs that between them take every path through the resolver.
fn paragraphs() -> Vec<Paragraph> {
    const LRE: char = '\u{202A}';
    const RLE: char = '\u{202B}';
    const PDF: char = '\u{202C}';
    const RLO: char = '\u{202E}';
    const LRI: char = '\u{2066}';
    const RLI: char = '\u{2067}';
    const FSI: char = '\u{2068}';
    const PDI: char = '\u{2069}';
    const ZWJ: char = '\u{200D}';

    // Embeddings nested past the 125 levels the algorithm allows, so the level
    // stack fills, overflows and unwinds, and runs reach the deepest bucket.
    let deep: String = [
        "a".to_owned(),
        RLE.to_string().repeat(70),
        LRE.to_string().repeat(70),
        "b \u{5D0} 1".to_owned(),
        PDF.to_string().repeat(140),
        " c".to_owned(),
    ]
    .concat();
    // Isolates nested past it as well, closed out of order.
    let isolates: String = [
        RLI.to_string().repeat(130),
        "\u{5D0}x".to_owned(),
        PDI.to_string().repeat(131),
    ]
    .concat();

    [
        // Nothing to resolve: the fast path.
        "Plain left-to-right text, with punctuation (and brackets).".to_owned(),
        // Hebrew with Latin, numbers, separators, terminators and brackets.
        "\u{5E9}\u{5DC}\u{5D5}\u{5DD} (abc) 12.5% $3,000 [\u{5D0}] {x}".to_owned(),
        // Arabic letters and digits, where W2 turns European numbers Arabic.
        "\u{627}\u{644}\u{639}\u{631}\u{628}\u{64A}\u{629} 123 \u{661}\u{662}\u{663}, 4-5 (\u{628})"
            .to_owned(),
        // Nonspacing marks after brackets, which N0 carries along.
        "\u{5D0} (a)\u{300}\u{301} [\u{5D1}]\u{300} b".to_owned(),
        // Isolates splitting a sequence, so its positions are scattered, and
        // X9's removed characters leaving gaps.
        format!("a \u{5D0}{LRI}b{ZWJ}c{PDI}\u{5D1}{ZWJ} {RLI}1{PDI} d{FSI}\u{5D2}{PDI}"),
        // An override, and an FSI that finds no strong character.
        format!("{RLO}abc 12{PDF} {FSI}123{PDI} \u{5D0}"),
        // Segment and paragraph separators, with trailing whitespace.
        "a\t\u{5D0} b \u{2029}\u{5D1} c  \t 1 ".to_owned(),
        deep,
        isolates,
    ]
    .iter()
    .map(|text| Paragraph::new(text))
    .collect()
}

/// Each paragraph, once its own first call has grown a fresh scratch, and
/// then all of them through one scratch, as a caller holding one would.
#[test]
fn bidi_resolving_allocates_nothing_warm() {
    let mut paragraphs = paragraphs();

    let mut cold = 0;
    for (i, paragraph) in paragraphs.iter_mut().enumerate() {
        let mut scratch = BidiScratch::new();
        cold += count_allocations(|| paragraph.resolve(&mut scratch));
        let warm = count_allocations(|| paragraph.resolve(&mut scratch));
        assert_eq!(warm, 0, "paragraph {i} allocated with a warm scratch");
    }
    assert!(cold > 0, "a cold scratch grows, so the count moves");

    let mut scratch = BidiScratch::new();
    for paragraph in &mut paragraphs {
        paragraph.resolve(&mut scratch);
    }
    let warm = count_allocations(|| {
        for paragraph in &mut paragraphs {
            paragraph.resolve(&mut scratch);
        }
    });
    assert_eq!(warm, 0, "one scratch serves every paragraph once grown");
}
