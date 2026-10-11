//! The Unicode bidirectional algorithm's classes, and its reordering of a
//! line (UAX #9, rule L2).
//!
//! [`BidiClass`] is the class the tables give each character, in ICU4C's
//! numbering. Analysis resolves a paragraph's levels from these classes
//! (`stages::analysis`), and line layout puts a line's pieces in visual
//! order with [`reorder_bidi`].

// --------------------------------------------------------------------------
// The types a caller deals in
// --------------------------------------------------------------------------

/// Bidirectional class value using ICU's numeric `UCharDirection` representation.
///
/// The numeric values used here match ICU4C's bidi class constants so custom
/// Unicode engines can pass through ICU-derived data without remapping.
/// See <https://unicode-org.github.io/icu-docs/apidoc/dev/icu4c/ubidi_8h.html>
/// and the `UCharDirection` enum definition for the source values.
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

/// Returns the class a cluster resolves as, given its first character's
/// class `first` and the others' classes `rest`.
///
/// This is `first`, except that whitespace or a separator carrying anything
/// X9 does not remove is an other neutral. An example is a space or a plus
/// sign with a combining mark.
///
/// Per character, the mark takes the type before it (W1). A space and its
/// mark are then one neutral, of which rule L1 resets only the space,
/// stopping at the mark. A separator and its mark are two separators, which
/// W4 does not join two numbers across and W6 makes neutral. ON resolves as
/// the pair does both ways and is never reset. So the per-cluster answer
/// matches the per-character one over every string of both conformance
/// files (`stages/analysis/tests/bidi_conformance.rs`).
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

    /// The class as a single bit, for testing membership of a set of classes.
    ///
    /// Saturating the shift keeps this total, and branchless, without letting
    /// an undefined value masquerade as a defined one: every class the
    /// algorithm defines is well under 31, so anything from 31 up lands on the
    /// top bit, which no class has. Wrapping the shift instead would alias
    /// 200 onto class 8.
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
    /// Position of the bracket in the paragraph.
    ///
    /// The class at this position must be [`BidiClass::OTHER_NEUTRAL`], which
    /// is what every character with a Bidi_Paired_Bracket_Type carries.
    pub(crate) index: u32,
    /// The closing character of the pair.
    ///
    /// For an opening bracket this is the character it pairs with; for a
    /// closing bracket it is the character itself. N0 matches openers to
    /// closers on this value alone, so both ends of a pair carry the same one.
    pub(crate) closing: char,
    /// Whether this is the opening bracket of the pair.
    pub(crate) is_open: bool,
}

// --------------------------------------------------------------------------
// Reordering a line: L2
// --------------------------------------------------------------------------

// Line layout calls `reorder_bidi` once per line, for its pieces and for its
// annotations' pieces, after breaking has said where lines end.

/// Computes a visual ordering from embedding levels, applying rule L2.
///
/// `order` receives a permutation of `0..order.len()`: reading it left to
/// right gives the visual order of the items, each entry being the logical
/// position of the item that goes there. Its length decides how many items
/// there are, and `levels(i)` supplies the level of item `i`.
///
/// The level is taken through a closure rather than a slice so that this can
/// order anything carrying a level, such as a run, a grapheme cluster or a
/// character, without laying it out as a `&[u8]` first. L2 walks the items
/// once per level between the lowest odd level and the highest, so `levels`
/// is called many times and should be a cheap lookup.
///
/// The entries are `u32`. Nothing reordered here comes in counts that will not
/// hold, and a caller keeping the answer keeps half as much of it.
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
