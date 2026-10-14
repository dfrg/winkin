//! The Unicode bidirectional algorithm's classes, and its reordering of a
//! line (UAX #9, rule L2).
//!
//! [`BidiClass`] is each character's class from the tables. Analysis
//! resolves a paragraph's levels from these classes, and line layout puts a
//! line's pieces in visual order with [`reorder_bidi`].

// --------------------------------------------------------------------------
// Classes
// --------------------------------------------------------------------------

/// A Bidi_Class in ICU4C's `UCharDirection` numbering.
///
/// The core word stores the same numbers, so a lookup needs no conversion.
#[derive(Copy, Clone, Default, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub(crate) struct BidiClass(pub(crate) u8);

impl BidiClass {
    /// Left-to-right letter (L).
    pub(crate) const LEFT_TO_RIGHT: Self = Self(0);
    /// Right-to-left letter (R).
    pub(crate) const RIGHT_TO_LEFT: Self = Self(1);
    /// European number (EN).
    pub(crate) const EUROPEAN_NUMBER: Self = Self(2);
    /// European separator (ES).
    pub(crate) const EUROPEAN_SEPARATOR: Self = Self(3);
    /// European terminator (ET).
    pub(crate) const EUROPEAN_TERMINATOR: Self = Self(4);
    /// Arabic number (AN).
    pub(crate) const ARABIC_NUMBER: Self = Self(5);
    /// Common separator (CS).
    pub(crate) const COMMON_SEPARATOR: Self = Self(6);
    /// Paragraph separator (B).
    pub(crate) const PARAGRAPH_SEPARATOR: Self = Self(7);
    /// Segment separator (S).
    pub(crate) const SEGMENT_SEPARATOR: Self = Self(8);
    /// Whitespace (WS).
    pub(crate) const WHITE_SPACE: Self = Self(9);
    /// Other neutral (ON).
    pub(crate) const OTHER_NEUTRAL: Self = Self(10);
    /// Left-to-right embedding (LRE).
    pub(crate) const LEFT_TO_RIGHT_EMBEDDING: Self = Self(11);
    /// Left-to-right override (LRO).
    pub(crate) const LEFT_TO_RIGHT_OVERRIDE: Self = Self(12);
    /// Arabic letter (AL).
    pub(crate) const ARABIC_LETTER: Self = Self(13);
    /// Right-to-left embedding (RLE).
    pub(crate) const RIGHT_TO_LEFT_EMBEDDING: Self = Self(14);
    /// Right-to-left override (RLO).
    pub(crate) const RIGHT_TO_LEFT_OVERRIDE: Self = Self(15);
    /// Pop directional format (PDF).
    pub(crate) const POP_DIRECTIONAL_FORMAT: Self = Self(16);
    /// Nonspacing mark (NSM).
    pub(crate) const NONSPACING_MARK: Self = Self(17);
    /// Boundary neutral (BN).
    pub(crate) const BOUNDARY_NEUTRAL: Self = Self(18);
    /// First strong isolate (FSI).
    pub(crate) const FIRST_STRONG_ISOLATE: Self = Self(19);
    /// Left-to-right isolate (LRI).
    pub(crate) const LEFT_TO_RIGHT_ISOLATE: Self = Self(20);
    /// Right-to-left isolate (RLI).
    pub(crate) const RIGHT_TO_LEFT_ISOLATE: Self = Self(21);
    /// Pop directional isolate (PDI).
    pub(crate) const POP_DIRECTIONAL_ISOLATE: Self = Self(22);
}

/// Returns the class a cluster resolves as, from its first character's
/// class `first` and the other characters' classes `rest`.
///
/// This is `first`, except that white space or a separator followed by a
/// character X9 keeps, such as a space or plus sign with a combining mark,
/// is ON.
///
/// Per character, the mark takes the class before it (W1). A space and its
/// mark are one neutral, of which L1 resets only the space. A separator and
/// its mark are two separators, which W4 does not join numbers across and W6
/// makes neutral. ON resolves as the pair does in both cases and is never
/// reset, so the per-cluster answer matches the per-character one over both
/// conformance files (`stages/analysis/tests/bidi_conformance.rs`).
pub(crate) fn cluster_bidi_class(
    first: BidiClass,
    mut rest: impl Iterator<Item = BidiClass>,
) -> BidiClass {
    if !matches!(
        first,
        BidiClass::WHITE_SPACE | BidiClass::EUROPEAN_SEPARATOR | BidiClass::COMMON_SEPARATOR
    ) {
        return first;
    }
    if rest.any(|class| !class.is_removed_by_x9()) {
        BidiClass::OTHER_NEUTRAL
    } else {
        first
    }
}

impl BidiClass {
    /// Returns true if this class is removed by rule X9.
    pub(crate) const fn is_removed_by_x9(self) -> bool {
        self.mask() & REMOVED_BY_X9_MASK != 0
    }

    /// Returns the class as a single bit, for testing membership of a set of
    /// classes.
    ///
    /// Every defined class is under 31, so the shift saturates: any value
    /// from 31 up maps to the top bit, which no class has. A wrapping shift
    /// would alias 200 onto class 8.
    pub(crate) const fn mask(self) -> u32 {
        1u32 << if self.0 < 31 { self.0 as u32 } else { 31 }
    }
}

/// The classes rule X9 removes: the embeddings and overrides, PDF and BN.
pub(crate) const REMOVED_BY_X9_MASK: u32 =
    OVERRIDE_MASK | BidiClass::POP_DIRECTIONAL_FORMAT.mask() | BidiClass::BOUNDARY_NEUTRAL.mask();

/// The embeddings and overrides.
pub(crate) const OVERRIDE_MASK: u32 = BidiClass::RIGHT_TO_LEFT_EMBEDDING.mask()
    | BidiClass::LEFT_TO_RIGHT_EMBEDDING.mask()
    | BidiClass::RIGHT_TO_LEFT_OVERRIDE.mask()
    | BidiClass::LEFT_TO_RIGHT_OVERRIDE.mask();

/// A paired bracket, as rule N0 needs it.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub(crate) struct BidiBracket {
    /// The bracket's position in the paragraph's units.
    ///
    /// The class there must be [`BidiClass::OTHER_NEUTRAL`], as it is for
    /// every character with a Bidi_Paired_Bracket_Type.
    pub(crate) index: u32,
    /// The closing character of the pair, for either end.
    ///
    /// N0 matches openers to closers on this value alone.
    pub(crate) closing: char,
    /// Whether this is the opening bracket of the pair.
    pub(crate) is_open: bool,
}

// --------------------------------------------------------------------------
// Reordering a line: L2
// --------------------------------------------------------------------------

/// Computes a visual order from embedding levels by rule L2.
///
/// `order` receives a permutation of `0..order.len()`: entry `v` is the
/// logical index of the item at visual position `v`, left to right.
/// `levels(i)` returns the level of item `i`.
///
/// The closure lets this order anything with a level without building a
/// `&[u8]`. L2 walks the items once per level from the lowest odd level to
/// the highest, so `levels` should be cheap. Entries are `u32` to halve what
/// a kept order costs.
pub(crate) fn reorder_bidi<F>(order: &mut [u32], levels: F)
where
    F: Fn(usize) -> u8,
{
    let mut max_level = 0;
    let mut lowest_odd_level = 255;
    for (i, o) in order.iter_mut().enumerate() {
        *o = i as u32;
        let level = levels(i);
        if level > max_level {
            max_level = level;
        }
        if level & 1 != 0 && level < lowest_odd_level {
            lowest_odd_level = level;
        }
    }
    let len = order.len();
    for level in (lowest_odd_level..=max_level).rev() {
        let mut i = 0;
        while i < len {
            if levels(i) >= level {
                let mut end = i + 1;
                while end < len && levels(end) >= level {
                    end += 1;
                }
                let mut j = i;
                let mut k = end - 1;
                while j < k {
                    order.swap(j, k);
                    j += 1;
                    k -= 1;
                }
                i = end;
            }
            i += 1;
        }
    }
}
