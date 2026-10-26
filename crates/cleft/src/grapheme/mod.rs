//! Extended grapheme clusters (UAX #29), found by a forward state machine.
//!
//! [`GraphemeState`] reads one character's [`GraphemeProps`] at a time and
//! says whether a cluster starts there. [`Graphemes`] runs it over a stream
//! of `(offset, char)`. The rules are those of the Unicode version
//! [`UNICODE_VERSION`](crate::UNICODE_VERSION) names, GB1 to GB999.
//!
//! Every rule that reads more than the two characters beside a boundary
//! (GB9c, GB11, GB12 and GB13) reads a regular context, which a flag of the
//! state carries. A boundary is a safe place to start: the state there is the
//! same however the text before it reads.

use crate::GraphemeClusterBreak;

#[cfg(test)]
mod tests;

// Grapheme_Cluster_Break values, as `GraphemeProps` holds them.
const OTHER: u8 = GraphemeClusterBreak::Other as u8;
const CONTROL: u8 = GraphemeClusterBreak::Control as u8;
const CR: u8 = GraphemeClusterBreak::Cr as u8;
const LF: u8 = GraphemeClusterBreak::Lf as u8;
const EXTEND: u8 = GraphemeClusterBreak::Extend as u8;
const ZWJ: u8 = GraphemeClusterBreak::Zwj as u8;
const L: u8 = GraphemeClusterBreak::L as u8;
const V: u8 = GraphemeClusterBreak::V as u8;
const T: u8 = GraphemeClusterBreak::T as u8;
const LV: u8 = GraphemeClusterBreak::Lv as u8;
const LVT: u8 = GraphemeClusterBreak::Lvt as u8;
const SPACING_MARK: u8 = GraphemeClusterBreak::SpacingMark as u8;
const PREPEND: u8 = GraphemeClusterBreak::Prepend as u8;
const REGIONAL_INDICATOR: u8 = GraphemeClusterBreak::RegionalIndicator as u8;

/// The Grapheme_Cluster_Break bits of a `GraphemeProps`.
const CLASS_MASK: u8 = 0xF;
/// The Indic_Conjunct_Break bits of a `GraphemeProps`, ICU4C's numbering.
const CONJUNCT_MASK: u8 = 0x30;
const CONJUNCT_CONSONANT: u8 = 0x10;
const CONJUNCT_EXTEND: u8 = 0x20;
const CONJUNCT_LINKER: u8 = 0x30;
/// The Extended_Pictographic bit of a `GraphemeProps`.
const PICTOGRAPHIC: u8 = 0x40;

/// The properties UAX #29 reads of a character, packed in a byte.
///
/// They are Grapheme_Cluster_Break, Indic_Conjunct_Break and
/// Extended_Pictographic. [`grapheme_props`] returns them for a character,
/// and [`RareProps::grapheme_props`](crate::RareProps::grapheme_props) takes
/// them from its rare properties.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct GraphemeProps(u8);

impl GraphemeProps {
    /// Creates the properties from their bits: Grapheme_Cluster_Break in bits
    /// 0 to 3, Indic_Conjunct_Break in bits 4 and 5, Extended_Pictographic in
    /// bit 6.
    #[inline]
    pub(crate) const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the Grapheme_Cluster_Break value.
    #[inline]
    const fn class(self) -> u8 {
        self.0 & CLASS_MASK
    }

    /// Returns the state flags that join a character of these properties to
    /// what precedes it, if one is set.
    #[inline]
    const fn joining_flags(self) -> u8 {
        let mut flags = 0;
        if self.0 & CONJUNCT_MASK == CONJUNCT_CONSONANT {
            flags |= AFTER_LINKER;
        }
        if self.0 & PICTOGRAPHIC != 0 {
            flags |= AFTER_PICTOGRAPHIC_ZWJ;
        }
        if self.class() == REGIONAL_INDICATOR {
            flags |= ODD_REGIONAL_INDICATORS;
        }
        flags
    }
}

/// Returns the properties UAX #29 reads of `ch`.
///
/// Latin-1 reads a small table; anything else reads the rare properties.
#[inline]
pub fn grapheme_props(ch: char) -> GraphemeProps {
    match LATIN_1.get(ch as usize) {
        Some(&props) => GraphemeProps(props),
        None => crate::rare_props(ch).grapheme_props(),
    }
}

/// The `GraphemeProps` of U+0000 to U+00FF.
///
/// Every Latin-1 character is a control, CR, LF or Other, and only U+00A9
/// and U+00AE are Extended_Pictographic. The sweep test checks the table
/// against the rare properties.
static LATIN_1: [u8; 256] = {
    let mut table = [OTHER; 256];
    let mut ch = 0;
    while ch < 256 {
        table[ch] = match ch {
            0x0D => CR,
            0x0A => LF,
            0x00..=0x1F | 0x7F..=0x9F | 0xAD => CONTROL,
            0xA9 | 0xAE => OTHER | PICTOGRAPHIC,
            _ => OTHER,
        };
        ch += 1;
    }
    table
};

/// For each left Grapheme_Cluster_Break, the right ones it never breaks
/// before, as a bit each: the pairs GB3 and GB6 to GB9b join.
///
/// Every other pair breaks unless a context rule joins it (GB9c, GB11, GB12,
/// GB13). No context flag is set after a control, CR or LF, so GB4 and GB5
/// need no entry of their own.
static JOINS: [u16; 16] = {
    let mut table = [0; 16];
    let mut left = 0;
    while left < 16 {
        let mut right = 0;
        while right < 16 {
            if joins(left as u8, right as u8) {
                table[left] |= 1 << right;
            }
            right += 1;
        }
        left += 1;
    }
    table
};

/// Returns whether UAX #29 joins a pair of Grapheme_Cluster_Break values
/// whatever their context.
const fn joins(left: u8, right: u8) -> bool {
    match (left, right) {
        // GB3.
        (CR, LF) => true,
        // GB4, GB5.
        (CONTROL | CR | LF, _) | (_, CONTROL | CR | LF) => false,
        // GB6, GB7, GB8.
        (L, L | V | LV | LVT) | (LV | V, V | T) | (LVT | T, T) => true,
        // GB9, GB9a, GB9b.
        (_, EXTEND | ZWJ | SPACING_MARK) | (PREPEND, _) => true,
        _ => false,
    }
}

// The state's flags, each the context one rule reads before a boundary.

/// After a regional indicator that ends an odd run of them (GB12, GB13).
const ODD_REGIONAL_INDICATORS: u8 = 1;
/// After an Extended_Pictographic character and any Extend after it (GB11).
const AFTER_PICTOGRAPHIC: u8 = 2;
/// After `AFTER_PICTOGRAPHIC`'s context and a ZWJ (GB11).
const AFTER_PICTOGRAPHIC_ZWJ: u8 = 4;
/// After a conjunct linker and any InCB Extend after it (GB9c).
const AFTER_LINKER: u8 = 8;
/// Under Unicode 17.0's GB9c, after a consonant and any InCB Extend or
/// Linker after it.
#[cfg(test)]
const AFTER_CONSONANT: u8 = 16;
/// The state follows Unicode 17.0's GB9c, which needs a consonant before
/// the linker.
#[cfg(test)]
const UNICODE_17_CONJUNCTS: u8 = 32;

/// The state of a forward scan for grapheme cluster boundaries.
///
/// A scan starts with [`new`](Self::new) at the start of a text or at a
/// known boundary, and passes each character to
/// [`advance`](Self::advance), which says whether a cluster starts there.
/// The state is two bytes and `Copy`, so a scan can be saved and resumed.
///
/// # Examples
///
/// ```
/// use cleft::{GraphemeState, grapheme_props};
///
/// let mut state = GraphemeState::new();
/// let starts: Vec<bool> = "e\u{301}\r\nx"
///     .chars()
///     .map(|ch| state.advance(grapheme_props(ch)))
///     .collect();
/// assert_eq!(starts, [true, false, true, false, true]);
/// ```
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct GraphemeState {
    /// The previous character's Grapheme_Cluster_Break, or Control at the
    /// start, which breaks before anything (GB1).
    left: u8,
    flags: u8,
}

impl GraphemeState {
    /// Creates the state at the start of a text, or at a boundary.
    #[inline]
    pub const fn new() -> Self {
        Self {
            left: CONTROL,
            flags: 0,
        }
    }

    /// Creates the state at the start of a text under Unicode 17.0's GB9c.
    #[cfg(test)]
    pub(crate) const fn with_unicode_17_conjuncts() -> Self {
        Self {
            left: CONTROL,
            flags: UNICODE_17_CONJUNCTS,
        }
    }

    /// Moves past a character with properties `next`, and returns whether a
    /// grapheme cluster starts at it.
    ///
    /// The first character after [`new`](Self::new) always starts one.
    #[inline]
    pub fn advance(&mut self, next: GraphemeProps) -> bool {
        let class = next.class();
        let joined = JOINS[usize::from(self.left & CLASS_MASK)] & (1u16 << class) != 0;
        let boundary = !joined && (self.flags == 0 || self.flags & next.joining_flags() == 0);
        self.flags = self.next_flags(next);
        self.left = class;
        boundary
    }

    /// Returns the flags after a character with properties `next`.
    #[inline]
    const fn next_flags(&self, next: GraphemeProps) -> u8 {
        let old = self.flags;
        let class = next.class();
        let mut flags = 0;
        if next.0 & PICTOGRAPHIC != 0 {
            flags |= AFTER_PICTOGRAPHIC;
        }
        if old & AFTER_PICTOGRAPHIC != 0 {
            if class == EXTEND {
                flags |= AFTER_PICTOGRAPHIC;
            } else if class == ZWJ {
                flags |= AFTER_PICTOGRAPHIC_ZWJ;
            }
        }
        if class == REGIONAL_INDICATOR && old & ODD_REGIONAL_INDICATORS == 0 {
            flags |= ODD_REGIONAL_INDICATORS;
        }
        match next.0 & CONJUNCT_MASK {
            CONJUNCT_LINKER => flags |= AFTER_LINKER,
            CONJUNCT_EXTEND => flags |= old & AFTER_LINKER,
            _ => {}
        }
        #[cfg(test)]
        if old & UNICODE_17_CONJUNCTS != 0 {
            flags = (flags & !AFTER_LINKER) | unicode_17_conjuncts(old, next);
        }
        flags
    }
}

impl Default for GraphemeState {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

/// Returns the GB9c flags Unicode 17.0's rule keeps after `next`:
/// `Consonant [Extend Linker]* Linker [Extend Linker]* × Consonant`.
#[cfg(test)]
const fn unicode_17_conjuncts(old: u8, next: GraphemeProps) -> u8 {
    let after_consonant = old & AFTER_CONSONANT != 0;
    let mut flags = UNICODE_17_CONJUNCTS;
    match next.0 & CONJUNCT_MASK {
        CONJUNCT_CONSONANT => flags |= AFTER_CONSONANT,
        CONJUNCT_LINKER if after_consonant => flags |= AFTER_CONSONANT | AFTER_LINKER,
        CONJUNCT_EXTEND if after_consonant => flags |= AFTER_CONSONANT | (old & AFTER_LINKER),
        _ => {}
    }
    flags
}

/// The offsets where the extended grapheme clusters of a stream of
/// characters start.
///
/// The stream is any iterator of `(offset, char)`, such as
/// [`Utf8Scalars`](crate::Utf8Scalars) or
/// [`Utf16Scalars`](crate::Utf16Scalars). Its first character starts a
/// cluster, so a stream that starts at a known boundary finds the same
/// boundaries after it as one from the start of the text. The end of the
/// text is not yielded.
///
/// Offsets must increase. At the first one that does not, the iterator
/// ends, as if the text ended there.
///
/// # Examples
///
/// ```
/// use cleft::{Graphemes, Utf8Scalars};
///
/// let text = "a\u{308}🇫🇷!";
/// let starts: Vec<usize> = Graphemes::new(Utf8Scalars::new(text)).collect();
/// assert_eq!(starts, [0, 3, 11]);
///
/// // Whether a cluster starts at byte 3, scanning from the known
/// // boundary at 0.
/// let at = 3;
/// let mut from_known = Graphemes::new(Utf8Scalars::with_start(text, 0));
/// assert!(from_known.find(|&start| start >= at) == Some(at));
/// ```
#[derive(Clone, Debug)]
pub struct Graphemes<I> {
    scalars: I,
    state: GraphemeState,
    /// The least offset the next character may have, or `None` once the
    /// stream has ended.
    least: Option<usize>,
}

impl<I> Graphemes<I> {
    /// Creates an iterator over the cluster starts of `scalars`.
    #[inline]
    pub const fn new(scalars: I) -> Self {
        Self {
            scalars,
            state: GraphemeState::new(),
            least: Some(0),
        }
    }
}

impl<I: Iterator<Item = (usize, char)>> Iterator for Graphemes<I> {
    type Item = usize;

    #[inline]
    fn next(&mut self) -> Option<usize> {
        let mut least = self.least?;
        for (offset, ch) in self.scalars.by_ref() {
            if offset < least {
                break;
            }
            least = offset.saturating_add(1);
            if self.state.advance(grapheme_props(ch)) {
                self.least = Some(least);
                return Some(offset);
            }
        }
        self.least = None;
        None
    }
}

impl<I: Iterator<Item = (usize, char)>> core::iter::FusedIterator for Graphemes<I> {}
