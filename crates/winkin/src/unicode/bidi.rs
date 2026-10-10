//! A dependency-free implementation of the [Unicode bidirectional
//! algorithm](https://www.unicode.org/reports/tr9/).
//!
//! [`resolve_bidi`] takes a paragraph's units, each saying the [`BidiClass`] it
//! resolves as, and fills in the embedding level of each. [`reorder_bidi`] then
//! turns levels into a visual order.
//!
//! The file holds the classes and the [`Unit`] trait first. Resolving a
//! paragraph's levels (P2–P3, X1–X10, W1–W7, N0–N2, I1–I2) comes next;
//! analysis calls it once per paragraph. Reordering (L2) comes last; line
//! layout calls it once per line.
//!
//! The module is a leaf, as UAX #9 is. It imports neither a stage nor the
//! Unicode tables, which hand their classes over as its [`BidiClass`]. So
//! its tests, `unicode/tests/bidi_conformance.rs` and
//! `unicode/tests/bidi_allocations.rs`, call it with ICU's classes alone.
//!
//! The module is crate-private, so its example cannot be a doctest. It is
//! `bidi::tests::the_bidi_module_example`, which resolves "abc" followed
//! by two Hebrew letters.
//!
//! # What the caller supplies
//!
//! This module ships no Unicode tables. Classes come from the caller,
//! whether from ICU, another crate's tables, or its own.
//!
//! A unit is anything that says its class ([`Unit`]), and one level comes
//! back per unit. So the caller chooses bytes, characters or grapheme
//! clusters, and may keep what each unit stands for in the unit itself.
//! Clusters are usually the right unit in a layout engine and the cheapest,
//! since combining marks never reach the algorithm.
//!
//! This crate's analysis hands over one unit per cluster and one per bidi
//! control a style synthesizes, each saying which it is. The levels go back
//! to their clusters by what the units say, with no second array kept in
//! step. A cluster resolves as [`cluster_bidi_class`] says, which the conformance
//! tests hold to the per-character answer over both files. A bare
//! [`BidiClass`] is a unit too, which is how the conformance tests call it.
//!
//! # What is left to the caller
//!
//! Splitting text into paragraphs is rule P1; see [`resolve_bidi`].
//!
//! Rule L1's line-dependent half resets the trailing whitespace of each
//! line, so it needs to know where lines end, which only the caller does.
//! The conformance tests reset the characters the rule names, by their
//! classes. This crate's line layout does it as Chrome does instead: by the
//! white space left at the line's end once the breaker has dealt with it.

use alloc::vec::Vec;
use core::num::NonZeroU32;
use core::ops::Range;

use crate::data::sort_by_key;

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

/// One element of a paragraph as [`resolve_bidi`] reads it: whatever the caller
/// resolves, so long as it says its class.
///
/// The resolver reads the class and nothing else, through this one method,
/// so a caller can keep what each unit stands for in the unit itself.
pub(crate) trait Unit: Copy {
    /// The class the unit resolves as.
    fn class(self) -> BidiClass;
}

impl Unit for BidiClass {
    #[inline]
    fn class(self) -> BidiClass {
        self
    }
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
/// files (`unicode/tests/bidi_conformance.rs`).
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
    /// Creates a bidi class from its ICU numeric value.
    pub(super) const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Returns true if this class is removed by rule X9.
    pub(crate) const fn is_removed_by_x9(self) -> bool {
        self.mask() & REMOVED_BY_X9_MASK != 0
    }

    /// The class as a single bit, for testing membership of a set of classes.
    ///
    /// Saturating the shift keeps this total, and branchless, without letting
    /// an undefined value masquerade as a defined one: every class the
    /// algorithm defines is well under 31, so anything from 31 up lands on the
    /// top bit and stays outside [`VALID_CLASS_MASK`]. Wrapping the shift
    /// instead would alias 200 onto class 8.
    pub(crate) const fn mask(self) -> u32 {
        1u32 << if self.0 < 31 { self.0 as u32 } else { 31 }
    }
}

// --------------------------------------------------------------------------
// Sets of classes, and the small things that read them
// --------------------------------------------------------------------------

/// The classes the algorithm defines, which are numbered contiguously from
/// [`BidiClass::LEFT_TO_RIGHT`] to [`BidiClass::POP_DIRECTIONAL_ISOLATE`].
///
/// The numeric representation is a plain `u8`, so a data source that maps
/// its own enum incorrectly can produce a value that means nothing here, and
/// [`resolve_bidi`] refuses a paragraph holding one rather than resolving
/// nonsense.
pub(super) const VALID_CLASS_MASK: u32 = (1 << (BidiClass::POP_DIRECTIONAL_ISOLATE.0 + 1)) - 1;

const REMOVED_BY_X9_MASK: u32 =
    OVERRIDE_MASK | BidiClass::POP_DIRECTIONAL_FORMAT.mask() | BidiClass::BOUNDARY_NEUTRAL.mask();

const OVERRIDE_MASK: u32 = BidiClass::RIGHT_TO_LEFT_EMBEDDING.mask()
    | BidiClass::LEFT_TO_RIGHT_EMBEDDING.mask()
    | BidiClass::RIGHT_TO_LEFT_OVERRIDE.mask()
    | BidiClass::LEFT_TO_RIGHT_OVERRIDE.mask();

const ISOLATE_MASK: u32 = BidiClass::RIGHT_TO_LEFT_ISOLATE.mask()
    | BidiClass::LEFT_TO_RIGHT_ISOLATE.mask()
    | BidiClass::FIRST_STRONG_ISOLATE.mask();

const EXPLICIT_MASK: u32 = OVERRIDE_MASK | ISOLATE_MASK;

const RTL_MASK: u32 = BidiClass::RIGHT_TO_LEFT_EMBEDDING.mask()
    | BidiClass::RIGHT_TO_LEFT_OVERRIDE.mask()
    | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask();

/// The classes whose presence means bidi resolution can change something.
///
/// A paragraph containing none of them, with a left-to-right base, resolves to
/// all-zero levels, so it need not be resolved at all. Analysis gathers this
/// mask as it builds a paragraph's units, so it is visible here.
pub(crate) const BIDI_MASK: u32 = EXPLICIT_MASK
    | BidiClass::RIGHT_TO_LEFT.mask()
    | BidiClass::ARABIC_LETTER.mask()
    | BidiClass::ARABIC_NUMBER.mask();

// --------------------------------------------------------------------------
// The types resolution deals in
// --------------------------------------------------------------------------

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
    pub(super) closing: char,
    /// Whether this is the opening bracket of the pair.
    pub(super) is_open: bool,
}

/// Reusable working memory for [`resolve_bidi`].
///
/// Holds nothing meaningful between calls; keep one alive across paragraphs
/// so that its buffers are not reallocated each time.
#[derive(Clone, Default)]
pub(crate) struct BidiScratch {
    /// One entry per FSI in the paragraph, in logical order.
    fsi_entries: Vec<FsiEntry>,
    /// FSIs still waiting for a strong character, innermost last.
    pending_fsi: Vec<PendingFsi>,
    /// Isolate nesting depth reached so far by `scan_isolates`.
    isolate_depth: u32,
    /// Two buffers in one allocation. `types[..classes.len()]` is the
    /// paragraph-wide working copy that the X rules rewrite in place;
    /// everything past that is scratch holding the current isolating run
    /// sequence, compacted so that characters removed by X9 are absent.
    /// `indices` maps each of those scratch entries back to its paragraph
    /// position.
    types: Vec<BidiClass>,
    /// Bracket pairs of the isolating run sequence being resolved, in
    /// sequence positions. Rebuilt for each sequence and not read afterwards.
    bracket_pairs: Vec<(u32, u32)>,
    runs: Vec<Run>,
    /// Paragraph positions of the current isolating run sequence, parallel to
    /// the scratch region of `types`.
    ///
    /// Left empty while the sequence is one unbroken range, which is the
    /// common case; `seq_start` and `seq_len` then describe it on their own.
    /// It is filled in only once a gap appears. Read through
    /// [`SeqPositions`], never directly.
    indices: Vec<u32>,
    /// First paragraph position of the current isolating run sequence.
    seq_start: u32,
    /// Number of characters in the current isolating run sequence.
    seq_len: u32,
    /// Level of the paragraph's first strong character outside any isolate,
    /// which is the base level under rules P2 and P3 when the caller does not
    /// supply one.
    first_strong: Option<u8>,
}

impl BidiScratch {
    /// Creates an empty scratch buffer, allocating nothing.
    ///
    /// One of these is worth keeping alive across paragraphs. This crate's
    /// lives in the analysis scratch. Its own methods are the paragraph
    /// prepass, further down with the rest of the algorithm.
    pub(crate) const fn new() -> Self {
        Self {
            fsi_entries: Vec::new(),
            pending_fsi: Vec::new(),
            isolate_depth: 0,
            types: Vec::new(),
            bracket_pairs: Vec::new(),
            runs: Vec::new(),
            indices: Vec::new(),
            seq_start: 0,
            seq_len: 0,
            first_strong: None,
        }
    }
}

// --------------------------------------------------------------------------
// The entry point
// --------------------------------------------------------------------------

/// Resolves the embedding level of every unit in a paragraph.
///
/// `classes` holds the units in logical order, each saying its bidi class
/// ([`Unit`]), and `levels` receives one level per unit, so the two must be
/// the same length.
/// `brackets` lists the paragraph's paired brackets in ascending `index`
/// order; pass an empty slice to skip rule N0. `base_level` supplies the
/// paragraph level: `Some(0)` for LTR, `Some(1)` for RTL, or `None` to
/// derive it from the first strong character under rules P2 and P3. The
/// resolved base level is returned.
///
/// `scratch` is working memory; reuse one across paragraphs to avoid
/// reallocating.
///
/// # Paragraphs
///
/// Splitting text into paragraphs is rule P1 and is the caller's job: this
/// resolves `classes` as exactly one paragraph.
///
/// Passing several paragraphs in one call is allowed but is not the same as
/// resolving each separately: one base level is derived for the whole slice,
/// so a later paragraph does not take its direction from its own first strong
/// character. A paragraph separator within the slice still takes the base
/// level along with the whitespace before it (rule L1) and terminates an
/// embedding or override left open there (rule X8).
///
/// `levels` implements UAX #9 through rule L1's paragraph- and
/// segment-separator resets. The rest of L1, resetting the trailing whitespace
/// and isolate formatting characters of each *line*, is the caller's, since
/// only they know where lines end: it resets them to the returned base level
/// before calling [`reorder_bidi`].
///
/// # Refusals
///
/// Returns `None` if:
/// - `levels` and `classes` differ in length;
/// - the paragraph is longer than `u32::MAX` characters;
/// - a class is not one the algorithm defines;
/// - `brackets` is out of order or points outside the paragraph.
///
/// Each is a caller's mistake, which a caller that gets its slices right
/// never sees. They are refused rather than asserted, so no input makes this
/// function panic.
pub(crate) fn resolve_bidi<U: Unit>(
    scratch: &mut BidiScratch,
    classes: &[U],
    brackets: &[BidiBracket],
    base_level: Option<u8>,
    levels: &mut [u8],
) -> Option<u8> {
    if classes.len() != levels.len() || classes.len() > u32::MAX as usize {
        return None;
    }
    // N0 binary searches `brackets`, so an unsorted or out of range entry
    // would quietly pair the wrong brackets. Checking is a walk over a slice
    // that holds at most one entry per bracket character, against an algorithm
    // that walks every character several times.
    let mut previous = None;
    for bracket in brackets {
        // Kept as written; `is_none_or` would say the same.
        #[allow(clippy::unnecessary_map_or)]
        let ordered = previous.map_or(true, |previous| previous < bracket.index);
        if !ordered || bracket.index as usize >= classes.len() {
            return None;
        }
        previous = Some(bracket.index);
    }
    scratch.clear();

    // The union of the classes present decides which rules can be skipped, and
    // doubles as the validity check: an undefined class contributes a bit
    // outside VALID_CLASS_MASK and nothing else does, so the whole paragraph is
    // validated by the fold it needed anyway.
    let mut mask = 0u32;
    for unit in classes {
        mask |= unit.class().mask();
    }
    if mask & !VALID_CLASS_MASK != 0 {
        return None;
    }

    let requested = base_level.map(|level| level & 1);
    // A left-to-right paragraph containing nothing that can raise a level
    // above the base has the same answer for every character, so the whole
    // algorithm collapses to filling the array in. This is the common case for
    // plain English text, and it does not even need the P2/P3 scan: with no
    // R, AL, AN or explicit control present, the first strong character can
    // only be L.
    if requested.unwrap_or(0) == 0 && mask & BIDI_MASK == 0 {
        levels.fill(0);
        return Some(0);
    }

    scratch.scan_isolates(classes);
    let base_level = requested.unwrap_or_else(|| scratch.first_strong.unwrap_or(0));
    Resolver {
        scratch,
        classes,
        brackets,
        levels,
        base_level,
    }
    .run();
    Some(base_level)
}

// --------------------------------------------------------------------------
// The small things that read the sets of classes
// --------------------------------------------------------------------------

fn is_isolate_initiator(ty: BidiClass) -> bool {
    ty.mask() & ISOLATE_MASK != 0
}

/// BD13: links each run into its isolating run sequence.
///
/// A run ending in an isolate initiator continues into the run that starts
/// with the matching PDI, which is always at the same level, so this keeps one
/// stack per level of the runs awaiting their PDI. Indexing by level is what
/// makes an unmatched PDI harmless: it can only pop a run that is genuinely
/// waiting at its own level.
///
/// The stacks are intrusive. `heads[level]` is the top of that level's stack
/// and each run's `stack_prev` is the entry below it, so the whole structure
/// is one `u32` per run and no allocation. A run can only ever be waiting on
/// one PDI at one level, so it can only ever be on one stack, which is what
/// makes threading the links through the runs themselves safe.
///
/// The heads are one per level the explicit rules can give a run, 126 of
/// them, 504 bytes of stack.
fn link_sequences(runs: &mut [Run]) {
    let mut heads = [RunIndex::NONE; MAX_STACK + 1];
    for i in 0..runs.len() {
        let level = runs[i].level as usize;
        debug_assert!(level <= MAX_STACK, "run level {level} past the deepest");
        // Kept as written; a let chain would say the same.
        #[allow(clippy::collapsible_if)]
        if runs[i].starts_with_pdi {
            if let Some(open) = heads[level].get() {
                heads[level] = runs[open].stack_prev;
                runs[i].in_sequence = true;
                runs[open].next = RunIndex::new(i);
            }
        }
        if runs[i].ends_with_isolate {
            runs[i].stack_prev = heads[level];
            heads[level] = RunIndex::new(i);
        }
    }
}

/// Returns a default bidi type for a level.
fn class_from_level(level: u8) -> BidiClass {
    if level & 1 == 0 {
        BidiClass::LEFT_TO_RIGHT
    } else {
        BidiClass::RIGHT_TO_LEFT
    }
}

/// The direction rule N0 assigns to a class when scanning inside a bracket
/// pair, where European and Arabic numbers count as right-to-left.
///
/// Returns [`BidiClass::OTHER_NEUTRAL`] for classes that carry no direction.
#[inline]
fn n0_strong_dir(ty: BidiClass) -> BidiClass {
    const N0_RTL_MASK: u32 = BidiClass::EUROPEAN_NUMBER.mask()
        | BidiClass::ARABIC_NUMBER.mask()
        | BidiClass::ARABIC_LETTER.mask()
        | BidiClass::RIGHT_TO_LEFT.mask();
    let mask = ty.mask();
    if mask & BidiClass::LEFT_TO_RIGHT.mask() != 0 {
        BidiClass::LEFT_TO_RIGHT
    } else if mask & N0_RTL_MASK != 0 {
        BidiClass::RIGHT_TO_LEFT
    } else {
        BidiClass::OTHER_NEUTRAL
    }
}

fn find_limit_by_mask(types: &[BidiClass], offset: usize, mask: u32) -> usize {
    offset
        + types[offset..]
            .iter()
            .position(|&t| t.mask() & mask == 0)
            .unwrap_or(types.len() - offset)
}

// --------------------------------------------------------------------------
// The algorithm, in the order it runs
// --------------------------------------------------------------------------

/// The prepass, which runs over the classes before any level is assigned.
impl BidiScratch {
    /// Returns the bytes its working buffers have allocated, which the
    /// analysis scratch counts as its own.
    pub(crate) fn heap_bytes(&self) -> usize {
        fn bytes<T>(vec: &Vec<T>) -> usize {
            vec.capacity() * size_of::<T>()
        }
        bytes(&self.fsi_entries)
            + bytes(&self.pending_fsi)
            + bytes(&self.types)
            + bytes(&self.bracket_pairs)
            + bytes(&self.runs)
            + bytes(&self.indices)
    }

    fn clear(&mut self) {
        self.types.clear();
        self.bracket_pairs.clear();
        self.runs.clear();
        self.indices.clear();
        self.seq_start = 0;
        self.seq_len = 0;
        self.fsi_entries.clear();
        self.pending_fsi.clear();
        self.isolate_depth = 0;
        self.first_strong = None;
    }

    /// Resolves the direction of every FSI (rule P3) and finds the paragraph's
    /// first strong character (rule P2), in one forward pass.
    ///
    /// An FSI takes the direction of the first strong character inside it, and
    /// a strong character settles the innermost FSI still waiting only if it
    /// sits at exactly that FSI's depth, so that anything nested in a further
    /// isolate does not count. An FSI that never sees one keeps the level 0
    /// its entry starts with, which is the LTR default P3 calls for.
    fn scan_isolates<U: Unit>(&mut self, classes: &[U]) {
        const STRONG_START_MASK: u32 = BidiClass::LEFT_TO_RIGHT.mask()
            | BidiClass::RIGHT_TO_LEFT.mask()
            | BidiClass::ARABIC_LETTER.mask();
        for (index, class) in classes.iter().map(|unit| unit.class()).enumerate() {
            let mask = class.mask();
            if mask & ISOLATE_MASK != 0 {
                self.isolate_depth += 1;
                if class == BidiClass::FIRST_STRONG_ISOLATE {
                    let slot = self.fsi_entries.len() as u32;
                    self.fsi_entries.push(FsiEntry {
                        index: index as u32,
                        level: 0,
                    });
                    self.pending_fsi.push(PendingFsi {
                        slot,
                        open_depth: self.isolate_depth,
                    });
                }
            } else if class == BidiClass::POP_DIRECTIONAL_ISOLATE {
                if self.isolate_depth > 0 {
                    self.isolate_depth -= 1;
                }
                // Drop every FSI this PDI closed, so that a later strong
                // character cannot settle one of them.
                while let Some(pending) = self.pending_fsi.last() {
                    if pending.open_depth <= self.isolate_depth {
                        break;
                    }
                    self.pending_fsi.pop();
                }
            } else if mask & STRONG_START_MASK != 0 {
                let level = if class == BidiClass::LEFT_TO_RIGHT {
                    0
                } else {
                    1
                };
                if self.isolate_depth == 0 && self.first_strong.is_none() {
                    self.first_strong = Some(level);
                }
                // Kept as written; a let chain would say the same.
                #[allow(clippy::collapsible_if)]
                if self
                    .pending_fsi
                    .last()
                    .is_some_and(|pending| pending.open_depth == self.isolate_depth)
                {
                    if let Some(pending) = self.pending_fsi.pop() {
                        self.fsi_entries[pending.slot as usize].level = level;
                    }
                }
            }
        }
    }
}

/// Resolves one paragraph, bundling the buffers the rules work across.
struct Resolver<'a, U> {
    scratch: &'a mut BidiScratch,
    classes: &'a [U],
    brackets: &'a [BidiBracket],
    levels: &'a mut [u8],
    base_level: u8,
}

impl<U: Unit> Resolver<'_, U> {
    /// The class the caller gave the unit at paragraph position `i`: what
    /// the rules below call its original class, before any of them rewrote
    /// it in `types`.
    #[inline]
    fn class(&self, i: usize) -> BidiClass {
        self.classes[i].class()
    }

    fn run(&mut self) {
        let len = self.classes.len();
        self.scratch
            .types
            .extend(self.classes.iter().map(|unit| unit.class()));
        self.resolve_levels();
        self.resolve_runs();
        self.resolve_sequences(len);
        self.apply_separator_resets(len);
    }

    /// X1 through X8: walks the explicit embedding, override and isolate
    /// controls, assigning each character its embedding level and applying
    /// any override in force.
    fn resolve_levels(&mut self) {
        let base = self.base_level;
        let len = self.scratch.types.len();
        let mut stack = LevelStack::new();
        let mut overflow_isolates = 0;
        let mut overflow_embedding = 0;
        let mut valid_isolates = 0;
        let mut fsi_cursor = 0usize;
        stack.push(base, BidiClass::OTHER_NEUTRAL, false);

        for (i, (t, level)) in self
            .scratch
            .types
            .iter_mut()
            .zip(self.levels.iter_mut())
            .enumerate()
        {
            let tmask = t.mask();
            if tmask & EXPLICIT_MASK != 0 {
                let is_isolate = tmask & ISOLATE_MASK != 0;
                // X5c: an FSI acts as RLI or LRI according to the direction
                // `scan_isolates` already worked out for it.
                let is_rtl = if *t == BidiClass::FIRST_STRONG_ISOLATE && i + 1 < len {
                    let mut level = 0;
                    // Kept as written; a let chain would say the same.
                    #[allow(clippy::collapsible_if)]
                    if let Some(entry) = self.scratch.fsi_entries.get(fsi_cursor) {
                        if entry.index as usize == i {
                            level = entry.level;
                            fsi_cursor += 1;
                        }
                    }
                    level == 1
                } else {
                    tmask & RTL_MASK != 0
                };
                if is_isolate {
                    *level = stack.embedding_level();
                    let os = stack.override_status();
                    if os != BidiClass::OTHER_NEUTRAL {
                        *t = os;
                    }
                }
                let new_level = if is_rtl {
                    (stack.embedding_level() + 1) | 1
                } else {
                    (stack.embedding_level() + 2) & !1
                };
                if new_level <= MAX_STACK as u8 && overflow_isolates == 0 && overflow_embedding == 0
                {
                    if is_isolate {
                        valid_isolates += 1;
                    }
                    stack.push(
                        new_level,
                        if *t == BidiClass::LEFT_TO_RIGHT_OVERRIDE {
                            BidiClass::LEFT_TO_RIGHT
                        } else if *t == BidiClass::RIGHT_TO_LEFT_OVERRIDE {
                            BidiClass::RIGHT_TO_LEFT
                        } else {
                            BidiClass::OTHER_NEUTRAL
                        },
                        is_isolate,
                    );
                } else if is_isolate {
                    overflow_isolates += 1;
                } else if overflow_isolates == 0 {
                    overflow_embedding += 1;
                }
            } else if *t == BidiClass::POP_DIRECTIONAL_ISOLATE {
                if overflow_isolates > 0 {
                    overflow_isolates -= 1;
                } else if valid_isolates == 0 {
                    // empty
                } else {
                    overflow_embedding = 0;
                    while !stack.isolate_status() {
                        stack.pop();
                    }
                    stack.pop();
                    valid_isolates -= 1;
                }
                *level = stack.embedding_level();
                if stack.override_status() != BidiClass::OTHER_NEUTRAL {
                    *t = stack.override_status();
                }
            } else if *t == BidiClass::POP_DIRECTIONAL_FORMAT {
                *level = stack.embedding_level();
                if overflow_isolates > 0 {
                    // empty
                } else if overflow_embedding > 0 {
                    overflow_embedding -= 1;
                } else if !stack.isolate_status() && stack.depth >= 2 {
                    stack.pop();
                }
            } else if *t == BidiClass::PARAGRAPH_SEPARATOR {
                stack.depth = 1;
                stack.cur = stack.entries[0];
                overflow_isolates = 0;
                overflow_embedding = 0;
                valid_isolates = 0;
                *level = base;
            } else if *t != BidiClass::BOUNDARY_NEUTRAL {
                *level = stack.embedding_level();
                if stack.override_status() != BidiClass::OTHER_NEUTRAL {
                    *t = stack.override_status();
                }
            }
        }
    }

    /// BD7 and BD13: splits the paragraph into level runs, computes the sos
    /// and eos types for each, and links the runs into isolating run
    /// sequences.
    fn resolve_runs(&mut self) {
        let len = self.scratch.types.len();
        self.scratch.runs.clear();
        let start = self
            .scratch
            .types
            .iter()
            .position(|&t| !t.is_removed_by_x9())
            .unwrap_or(len);
        if start == len {
            return;
        }
        let mut cur_level = self.levels[start];
        let mut run_start = start;
        for (i, (_t, &level)) in self
            .scratch
            .types
            .iter()
            .zip(&*self.levels)
            .enumerate()
            .skip(start + 1)
            .filter(|(_, (t, _))| !t.is_removed_by_x9())
        {
            if level != cur_level {
                self.scratch.runs.push(Run::new(cur_level, run_start, i));
                run_start = i;
                cur_level = level;
            }
        }
        if run_start < len {
            self.scratch.runs.push(Run::new(cur_level, run_start, len));
        }
        for run in &mut self.scratch.runs {
            let mut start = run.start as usize;
            let mut end = run.end as usize;
            while start < end {
                if self.scratch.types[start].is_removed_by_x9() {
                    start += 1;
                } else {
                    break;
                }
            }
            while end > start {
                if self.scratch.types[end - 1].is_removed_by_x9() {
                    end -= 1;
                } else {
                    break;
                }
            }
            run.start = start as u32;
            run.end = end as u32;
            if start == end {
                continue;
            }
            if self.scratch.types[start] == BidiClass::POP_DIRECTIONAL_ISOLATE {
                run.starts_with_pdi = true;
            }
            let mut prev_level = self.base_level;
            for i in (0..start).rev() {
                if !self.scratch.types[i].is_removed_by_x9() {
                    prev_level = self.levels[i];
                    break;
                }
            }
            run.sos = class_from_level(prev_level.max(run.level));
            if is_isolate_initiator(self.classes[end - 1].class()) {
                run.ends_with_isolate = true;
                run.eos = class_from_level(self.base_level.max(run.level));
            } else {
                let mut next_level = self.base_level;
                for i in end..len {
                    if !self.scratch.types[i].is_removed_by_x9() {
                        next_level = self.levels[i];
                        break;
                    }
                }
                run.eos = class_from_level(next_level.max(run.level));
            }
        }
        link_sequences(&mut self.scratch.runs);
    }

    /// X10: builds each isolating run sequence and runs the W, N and I rules
    /// over it.
    ///
    /// `len` is the paragraph length, which is where the scratch region of
    /// `types` begins.
    fn resolve_sequences(&mut self, len: usize) {
        for i in 0..self.scratch.runs.len() {
            let run = &self.scratch.runs[i];
            // Runs that continue an earlier sequence are visited through that
            // sequence's chain rather than started again here.
            if run.in_sequence {
                continue;
            }
            self.scratch.types.truncate(len);
            self.scratch.indices.clear();
            let mut sequence_mask = 0u32;
            let mut seq_start = 0usize;
            let mut count = 0usize;
            let mut cur = i;
            let level = run.level;
            let sos = run.sos;
            let mut eos;
            // Walk the chain of runs making up this sequence, copying the
            // characters X9 does not remove into the scratch region.
            loop {
                let run = &self.scratch.runs[cur];
                for index in run.range() {
                    let ty = self.scratch.types[index];
                    if ty.is_removed_by_x9() {
                        continue;
                    }
                    self.scratch.types.push(ty);
                    sequence_mask |= ty.mask();
                    if count == 0 {
                        seq_start = index;
                    } else if self.scratch.indices.is_empty() {
                        // Still one unbroken range, so nothing to record
                        // unless this character breaks it.
                        if index != seq_start + count {
                            self.scratch
                                .indices
                                .extend((0..count).map(|k| (seq_start + k) as u32));
                            self.scratch.indices.push(index as u32);
                        }
                    } else {
                        self.scratch.indices.push(index as u32);
                    }
                    count += 1;
                }
                eos = run.eos;
                cur = match run.next.get() {
                    Some(next) => next,
                    None => break,
                };
            }
            self.scratch.seq_start = seq_start as u32;
            self.scratch.seq_len = count as u32;
            self.resolve_sequence(level, sos, eos, sequence_mask);
        }
    }

    #[allow(clippy::needless_range_loop)]
    /// Applies rules W1 through W7, N0 through N2, and I1/I2 to the
    /// isolating run sequence sitting in the scratch region of `types`.
    ///
    /// `sequence_mask` is the union of the class masks in the sequence, used
    /// to skip whole rule groups the sequence cannot be affected by.
    fn resolve_sequence(&mut self, level: u8, sos: BidiClass, eos: BidiClass, sequence_mask: u32) {
        // Built from the fields directly rather than through a method, so that
        // this borrows only `indices` and leaves `types` free to be borrowed
        // mutably below.
        let seq = if self.scratch.indices.is_empty() {
            SeqPositions::Contiguous {
                start: self.scratch.seq_start,
                len: self.scratch.seq_len,
            }
        } else {
            SeqPositions::Scattered(&self.scratch.indices)
        };
        let len = seq.len();
        if len == 0 {
            return;
        }
        const W1_MASK: u32 = BidiClass::LEFT_TO_RIGHT_ISOLATE.mask()
            | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask()
            | BidiClass::FIRST_STRONG_ISOLATE.mask()
            | BidiClass::POP_DIRECTIONAL_ISOLATE.mask();
        const W2_MASK: u32 = BidiClass::LEFT_TO_RIGHT.mask()
            | BidiClass::RIGHT_TO_LEFT.mask()
            | BidiClass::ARABIC_LETTER.mask();
        const W4_MASK: u32 =
            BidiClass::EUROPEAN_SEPARATOR.mask() | BidiClass::COMMON_SEPARATOR.mask();
        const W1_TO_W4_MASK: u32 = BidiClass::NONSPACING_MARK.mask()
            | W1_MASK
            | BidiClass::EUROPEAN_NUMBER.mask()
            | BidiClass::ARABIC_LETTER.mask()
            | W4_MASK;
        let mut prev = sos;
        let mut prev_strong = prev;
        let mut pending_separator = None::<(usize, BidiClass)>;
        let types = &mut self.scratch.types[self.classes.len()..];
        if sequence_mask & W1_TO_W4_MASK != 0 {
            for i in 0..types.len() {
                let mut t = types[i];
                let tmask = t.mask();
                if t == BidiClass::NONSPACING_MARK {
                    // W1
                    t = prev;
                    types[i] = prev;
                }
                if tmask & W1_MASK != 0 {
                    prev = BidiClass::OTHER_NEUTRAL;
                    pending_separator = None;
                    continue;
                }
                if t == BidiClass::EUROPEAN_NUMBER {
                    // W2
                    if prev_strong == BidiClass::ARABIC_LETTER {
                        t = BidiClass::ARABIC_NUMBER;
                        types[i] = t;
                        if let Some((sep_idx, BidiClass::ARABIC_NUMBER)) = pending_separator {
                            types[sep_idx] = BidiClass::ARABIC_NUMBER;
                        }
                    } else {
                        if let Some((sep_idx, BidiClass::EUROPEAN_NUMBER)) = pending_separator {
                            types[sep_idx] = BidiClass::EUROPEAN_NUMBER;
                        }
                    }
                    pending_separator = None;
                } else if t == BidiClass::ARABIC_NUMBER {
                    if let Some((sep_idx, BidiClass::ARABIC_NUMBER)) = pending_separator {
                        types[sep_idx] = BidiClass::ARABIC_NUMBER;
                    }
                    pending_separator = None;
                } else if tmask & W2_MASK != 0 {
                    prev_strong = t;
                    // W3
                    if t == BidiClass::ARABIC_LETTER {
                        t = BidiClass::RIGHT_TO_LEFT;
                        types[i] = t;
                    }
                    pending_separator = None;
                } else if tmask & W4_MASK != 0 {
                    // W4
                    if t == BidiClass::EUROPEAN_SEPARATOR {
                        if prev == BidiClass::EUROPEAN_NUMBER {
                            pending_separator = Some((i, BidiClass::EUROPEAN_NUMBER));
                        } else {
                            pending_separator = None;
                        }
                    } else {
                        // We must have a common separator here
                        if prev == BidiClass::EUROPEAN_NUMBER || prev == BidiClass::ARABIC_NUMBER {
                            pending_separator = Some((i, prev));
                        } else {
                            pending_separator = None;
                        }
                    }
                } else {
                    pending_separator = None;
                }
                prev = t;
            }
        }
        // W5
        if sequence_mask & BidiClass::EUROPEAN_TERMINATOR.mask() != 0 {
            let mut seen_en = false;
            let mut et_start = None::<usize>;
            for i in 0..types.len() {
                match types[i] {
                    BidiClass::EUROPEAN_NUMBER => {
                        if let Some(start_idx) = et_start {
                            types[start_idx..i].fill(BidiClass::EUROPEAN_NUMBER);
                            et_start = None;
                        }
                        seen_en = true;
                    }
                    BidiClass::EUROPEAN_TERMINATOR => {
                        if seen_en {
                            types[i] = BidiClass::EUROPEAN_NUMBER;
                        } else if et_start.is_none() {
                            et_start = Some(i);
                        }
                    }
                    _ => {
                        seen_en = false;
                        et_start = None;
                    }
                }
            }
        }
        // W6, W7
        const W6_MASK: u32 = BidiClass::EUROPEAN_SEPARATOR.mask()
            | BidiClass::EUROPEAN_TERMINATOR.mask()
            | BidiClass::COMMON_SEPARATOR.mask();
        if sequence_mask & (W6_MASK | BidiClass::EUROPEAN_NUMBER.mask()) != 0 {
            prev_strong = sos;
            for t in types.iter_mut() {
                if t.mask() & W6_MASK != 0 {
                    // W6
                    *t = BidiClass::OTHER_NEUTRAL;
                } else if *t == BidiClass::EUROPEAN_NUMBER {
                    // W7
                    if prev_strong == BidiClass::LEFT_TO_RIGHT {
                        *t = BidiClass::LEFT_TO_RIGHT;
                    }
                } else if *t == BidiClass::LEFT_TO_RIGHT || *t == BidiClass::RIGHT_TO_LEFT {
                    prev_strong = *t;
                }
            }
        }
        // N0
        let start = seq.pos(0);
        let end = seq.pos(len - 1);
        // Index of the first bracket at or after the sequence, or the length
        // when the sequence has none.
        let bracket_start = match self
            .brackets
            .binary_search_by(|bracket| bracket.index.cmp(&(start as u32)))
        {
            Ok(index) | Err(index) => index,
        };
        let has_sequence_brackets = sequence_mask & BidiClass::OTHER_NEUTRAL.mask() != 0
            && self
                .brackets
                .get(bracket_start)
                .is_some_and(|bracket| bracket.index as usize <= end);
        if has_sequence_brackets {
            self.scratch.bracket_pairs.clear();
            let mut bracket_stack = BracketStack::new();
            for &bracket in self.brackets.iter().skip(bracket_start) {
                let text_index = bracket.index as usize;
                if text_index > end {
                    break;
                }
                let Some(i) = seq.find(text_index) else {
                    continue;
                };
                if types[i] != BidiClass::OTHER_NEUTRAL {
                    continue;
                }
                if bracket.is_open {
                    if bracket_stack.depth == MAX_BRACKET_STACK {
                        break;
                    }
                    bracket_stack.push(i, bracket.closing);
                } else if let Some(open) = bracket_stack.find_and_pop(bracket.closing) {
                    self.scratch.bracket_pairs.push((open as u32, i as u32));
                }
            }
            if !self.scratch.bracket_pairs.is_empty() {
                let embed_dir = if level & 1 != 0 {
                    BidiClass::RIGHT_TO_LEFT
                } else {
                    BidiClass::LEFT_TO_RIGHT
                };
                let bracket_pairs = &mut self.scratch.bracket_pairs[..];
                sort_by_key(bracket_pairs, |pair| pair.0);
                let mut strong_scan = 0;
                let mut preceding_strong = BidiClass::OTHER_NEUTRAL;
                for &(open, close) in &*bracket_pairs {
                    // Back to usize once, so the rule below reads normally.
                    let (open, close) = (open as usize, close as usize);
                    let mut pair_dir = BidiClass::OTHER_NEUTRAL;
                    for i in open + 1..close {
                        let dir = n0_strong_dir(types[i]);
                        if dir == BidiClass::OTHER_NEUTRAL {
                            continue;
                        }
                        pair_dir = dir;
                        if dir == embed_dir {
                            break;
                        }
                    }
                    if pair_dir == BidiClass::OTHER_NEUTRAL {
                        continue;
                    }
                    if pair_dir != embed_dir {
                        while strong_scan < open {
                            let dir = n0_strong_dir(types[strong_scan]);
                            if dir != BidiClass::OTHER_NEUTRAL {
                                preceding_strong = dir;
                            }
                            strong_scan += 1;
                        }
                        pair_dir = if preceding_strong == BidiClass::OTHER_NEUTRAL {
                            sos
                        } else {
                            preceding_strong
                        };
                        if pair_dir == embed_dir || pair_dir == BidiClass::OTHER_NEUTRAL {
                            pair_dir = embed_dir;
                        }
                    }
                    types[open] = pair_dir;
                    types[close] = pair_dir;
                    // Characters that were originally NSM and immediately
                    // follow either bracket of the pair take the direction the
                    // bracket resolved to.
                    for i in open + 1..close {
                        if self.classes[seq.pos(i)].class() != BidiClass::NONSPACING_MARK {
                            break;
                        }
                        types[i] = pair_dir;
                    }
                    for i in close + 1..len {
                        if self.classes[seq.pos(i)].class() != BidiClass::NONSPACING_MARK {
                            break;
                        }
                        types[i] = pair_dir;
                    }
                }
            }
        }
        // N1, N2
        const N_MASK: u32 = BidiClass::PARAGRAPH_SEPARATOR.mask()
            | BidiClass::SEGMENT_SEPARATOR.mask()
            | BidiClass::WHITE_SPACE.mask()
            | BidiClass::OTHER_NEUTRAL.mask()
            | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask()
            | BidiClass::LEFT_TO_RIGHT_ISOLATE.mask()
            | BidiClass::FIRST_STRONG_ISOLATE.mask()
            | BidiClass::POP_DIRECTIONAL_ISOLATE.mask();
        if sequence_mask & (N_MASK | W6_MASK) != 0 {
            let mut i = 0;
            while i < len {
                let t = types[i];
                if t.mask() & N_MASK != 0 {
                    let offset = i;
                    let limit = find_limit_by_mask(types, offset, N_MASK);
                    let mut leading;
                    let mut trailing;
                    if offset == 0 {
                        leading = sos;
                    } else {
                        leading = types[offset - 1];
                        if leading == BidiClass::ARABIC_NUMBER
                            || leading == BidiClass::EUROPEAN_NUMBER
                        {
                            leading = BidiClass::RIGHT_TO_LEFT;
                        }
                    }
                    if limit == len {
                        trailing = eos;
                    } else {
                        trailing = types[limit];
                        if trailing == BidiClass::ARABIC_NUMBER
                            || trailing == BidiClass::EUROPEAN_NUMBER
                        {
                            trailing = BidiClass::RIGHT_TO_LEFT;
                        }
                    }
                    let resolved = if leading == trailing {
                        // N1
                        leading
                    } else {
                        // N2
                        if level & 1 != 0 {
                            BidiClass::RIGHT_TO_LEFT
                        } else {
                            BidiClass::LEFT_TO_RIGHT
                        }
                    };
                    types[offset..limit].fill(resolved);
                    i = limit - 1;
                }
                i += 1;
            }
        }
        // Implicit levels (I1, I2).
        //
        // When the sequence occupies a contiguous range we can write levels
        // through a slice, which keeps the loop free of the indirection
        // through `indices` and lets it autovectorize.
        if let SeqPositions::Contiguous { start, .. } = seq {
            let start = start as usize;
            let dst = &mut self.levels[start..start + len];
            if level & 1 == 0 {
                // I1: L -> level, R -> level + 1, anything else -> level + 2.
                for (dst, &t) in dst.iter_mut().zip(&types[..len]) {
                    *dst = if t == BidiClass::LEFT_TO_RIGHT {
                        level
                    } else if t == BidiClass::RIGHT_TO_LEFT {
                        level + 1
                    } else {
                        level + 2
                    };
                }
            } else {
                // I2: R -> level, anything else -> level + 1.
                for (dst, &t) in dst.iter_mut().zip(&types[..len]) {
                    *dst = if t == BidiClass::RIGHT_TO_LEFT {
                        level
                    } else {
                        level + 1
                    };
                }
            }
        } else if level & 1 == 0 {
            // I1 for scattered indices.
            for (i, &t) in types[..len].iter().enumerate() {
                let index = seq.pos(i);
                if t == BidiClass::RIGHT_TO_LEFT {
                    self.levels[index] = level + 1;
                } else if t != BidiClass::LEFT_TO_RIGHT {
                    self.levels[index] = level + 2;
                } else {
                    self.levels[index] = level;
                }
            }
        } else {
            // I2 for scattered indices.
            for (i, &t) in types[..len].iter().enumerate() {
                let index = seq.pos(i);
                if t != BidiClass::RIGHT_TO_LEFT {
                    self.levels[index] = level + 1;
                } else {
                    self.levels[index] = level;
                }
            }
        }
    }

    /// The part of L1 that does not depend on line breaks: segment and
    /// paragraph separators reset to the base level, as does any run of
    /// whitespace or isolate formatting characters before one.
    ///
    /// Characters removed by X9 never got a level of their own, so they
    /// inherit from the left to keep the output array free of holes.
    fn apply_separator_resets(&mut self, len: usize) {
        for i in 0..len {
            let ty = self.class(i);
            if ty == BidiClass::SEGMENT_SEPARATOR || ty == BidiClass::PARAGRAPH_SEPARATOR {
                self.levels[i] = self.base_level;
                for j in (0..i).rev() {
                    let ty = self.class(j);
                    if ty.is_removed_by_x9() {
                        continue;
                    } else if ty == BidiClass::WHITE_SPACE
                        || is_isolate_initiator(ty)
                        || ty == BidiClass::POP_DIRECTIONAL_ISOLATE
                    {
                        self.levels[j] = self.base_level;
                    } else {
                        break;
                    }
                }
            } else if ty.is_removed_by_x9() {
                if i == 0 {
                    self.levels[i] = self.base_level;
                } else {
                    self.levels[i] = self.levels[i - 1];
                }
            }
        }
    }
}

// --------------------------------------------------------------------------
// Supporting types
// --------------------------------------------------------------------------

/// The direction an FSI resolved to.
///
/// Rule P3 gives an FSI the direction of the first strong character inside it.
/// `BidiScratch::scan_isolates` settles that in the same forward pass that
/// finds the paragraph's own first strong character, so each FSI gets an entry
/// here when it is reached and the entry is filled in by the first strong
/// character that follows at the same isolate depth.
#[derive(Copy, Clone)]
struct FsiEntry {
    /// Index of the FSI character in the paragraph.
    index: u32,
    /// Level of the first strong character inside the isolate.
    ///
    /// P3 defaults an FSI with no strong character to LTR, which is level 0,
    /// so an entry that is never settled is already correct and needs no
    /// separate "unresolved" state.
    level: u8,
}

/// An FSI whose direction has not been settled yet.
#[derive(Copy, Clone)]
struct PendingFsi {
    /// Index into `BidiScratch::fsi_entries`.
    slot: u32,
    /// The isolate depth just inside this FSI. A strong character settles the
    /// FSI only when it appears at exactly this depth, so that characters
    /// nested inside a further isolate do not count.
    open_depth: u32,
}

const MAX_STACK: usize = 125;

#[derive(Copy, Clone)]
struct LevelStackEntry {
    embedding_level: u8,
    /// The direction characters are forced to by an enclosing LRO or RLO, or
    /// [`BidiClass::OTHER_NEUTRAL`] when no override is in force.
    override_status: BidiClass,
    isolate_status: bool,
}

struct LevelStack {
    entries: [LevelStackEntry; MAX_STACK + 1],
    cur: LevelStackEntry,
    depth: usize,
}

impl LevelStack {
    fn new() -> Self {
        Self {
            depth: 0,
            entries: [LevelStackEntry {
                embedding_level: 0,
                override_status: BidiClass::OTHER_NEUTRAL,
                isolate_status: false,
            }; MAX_STACK + 1],
            cur: LevelStackEntry {
                embedding_level: 0,
                override_status: BidiClass::OTHER_NEUTRAL,
                isolate_status: false,
            },
        }
    }

    fn push(&mut self, level: u8, override_status: BidiClass, isolate_status: bool) {
        let d = self.depth;
        let entry = LevelStackEntry {
            embedding_level: level,
            override_status,
            isolate_status,
        };
        self.entries[d] = entry;
        self.cur = entry;
        self.depth += 1;
    }

    fn pop(&mut self) {
        if self.depth > 1 {
            self.depth -= 1;
            self.cur = self.entries[self.depth - 1];
        }
    }

    fn embedding_level(&self) -> u8 {
        self.cur.embedding_level
    }

    fn override_status(&self) -> BidiClass {
        self.cur.override_status
    }

    fn isolate_status(&self) -> bool {
        self.cur.isolate_status
    }
}

/// An optional index into `BidiScratch::runs`.
///
/// Stored as index + 1 so that the `NonZeroU32` niche keeps this to four
/// bytes. `Option<u32>` has no niche and would take eight, and `Option<usize>`
/// takes sixteen.
#[derive(Copy, Clone, Default, PartialEq, Eq)]
struct RunIndex(Option<NonZeroU32>);

impl RunIndex {
    const NONE: Self = Self(None);

    #[inline]
    fn new(index: usize) -> Self {
        debug_assert!(index < u32::MAX as usize);
        Self(NonZeroU32::new(index as u32 + 1))
    }

    #[inline]
    fn get(self) -> Option<usize> {
        self.0.map(|index| index.get() as usize - 1)
    }
}

/// A level run, plus the links that join runs into isolating run sequences.
#[derive(Clone)]
struct Run {
    level: u8,
    /// Start and end of the run in paragraph positions, trimmed of the
    /// characters X9 removes.
    start: u32,
    end: u32,
    /// The types treated as preceding and following the sequence this run
    /// belongs to, per X10.
    sos: BidiClass,
    eos: BidiClass,
    /// Whether this run can be joined to a neighbor, and in which direction.
    ends_with_isolate: bool,
    starts_with_pdi: bool,
    /// Set once this run has been chained onto an earlier one, so that
    /// `resolve_sequences` does not also start a sequence at it.
    in_sequence: bool,
    /// The next run in this sequence, if any.
    next: RunIndex,
    /// Scratch link used while building sequences; see `resolve_runs`.
    stack_prev: RunIndex,
}

impl Run {
    fn new(level: u8, start: usize, end: usize) -> Self {
        Self {
            level,
            ends_with_isolate: false,
            starts_with_pdi: false,
            sos: BidiClass::OTHER_NEUTRAL,
            eos: BidiClass::OTHER_NEUTRAL,
            start: start as u32,
            end: end as u32,
            in_sequence: false,
            next: RunIndex::NONE,
            stack_prev: RunIndex::NONE,
        }
    }

    /// The run's paragraph positions.
    #[inline]
    fn range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// Maps positions within an isolating run sequence back to paragraph
/// positions.
///
/// The rules work on a compacted copy of the sequence, so every position they
/// hold is a sequence position and has to come back through here to touch
/// anything paragraph-wide.
#[derive(Copy, Clone)]
enum SeqPositions<'a> {
    /// The sequence covers one unbroken range of the paragraph, so each
    /// position is its offset from `start` and no array is needed. This is
    /// what ordinary text produces: a sequence is only broken up by runs
    /// joined across an isolate, or by characters X9 removes.
    Contiguous { start: u32, len: u32 },
    /// The sequence skips paragraph positions, so they are listed.
    Scattered(&'a [u32]),
}

impl<'a> SeqPositions<'a> {
    #[inline]
    fn len(self) -> usize {
        match self {
            Self::Contiguous { len, .. } => len as usize,
            Self::Scattered(positions) => positions.len(),
        }
    }

    /// Paragraph position of the `i`th character of the sequence.
    #[inline]
    fn pos(self, i: usize) -> usize {
        match self {
            Self::Contiguous { start, len } => {
                debug_assert!(i < len as usize);
                start as usize + i
            }
            Self::Scattered(positions) => positions[i] as usize,
        }
    }

    /// Position within the sequence of the character at paragraph position
    /// `pos`, if it is part of the sequence.
    ///
    /// Sequences are built in ascending paragraph order, so the scattered
    /// case can binary search.
    #[inline]
    fn find(self, pos: usize) -> Option<usize> {
        match self {
            Self::Contiguous { start, len } => {
                let i = pos.checked_sub(start as usize)?;
                (i < len as usize).then_some(i)
            }
            Self::Scattered(positions) => positions.binary_search(&(pos as u32)).ok(),
        }
    }
}

const MAX_BRACKET_STACK: usize = 63;

struct BracketStack {
    openers: [(u32, char); MAX_BRACKET_STACK],
    depth: usize,
}

impl BracketStack {
    fn new() -> Self {
        Self {
            openers: [(0u32, '\0'); MAX_BRACKET_STACK],
            depth: 0,
        }
    }

    fn push(&mut self, offset: usize, closer: char) {
        self.openers[self.depth] = (offset as u32, closer);
        self.depth += 1;
    }

    fn find_and_pop(&mut self, closer: char) -> Option<usize> {
        if self.depth == 0 {
            return None;
        }
        for i in (0..self.depth).rev() {
            let c = self.openers[i].1;
            if c == closer
                || (c == '\u{232A}' && closer == '\u{3009}')
                || (c == '\u{3009}' && closer == '\u{232A}')
            {
                self.depth = i;
                return Some(self.openers[i].0 as usize);
            }
        }
        None
    }
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

#[cfg(test)]
mod tests {
    /// The example from the module documentation.
    #[test]
    fn the_bidi_module_example() {
        use super::{BidiClass, BidiScratch, resolve_bidi};

        let mut scratch = BidiScratch::new();

        // "abc" followed by two Hebrew letters.
        let classes = [
            BidiClass::LEFT_TO_RIGHT,
            BidiClass::LEFT_TO_RIGHT,
            BidiClass::LEFT_TO_RIGHT,
            BidiClass::RIGHT_TO_LEFT,
            BidiClass::RIGHT_TO_LEFT,
        ];
        let mut levels = [0; 5];

        // `None` derives the paragraph direction from the first strong character.
        let base_level = resolve_bidi(&mut scratch, &classes, &[], None, &mut levels);

        assert_eq!(base_level, Some(0));
        assert_eq!(levels, [0, 0, 0, 1, 1]);
    }

    /// An isolate that ends a left-to-right paragraph after right-to-left
    /// text has its controls at the paragraph's level.
    ///
    /// `R LRI L PDI` resolves to 1, 0, 2, 0. The LRI and PDI are neutrals
    /// between R and the paragraph's end, so N2 sets them at the embedding
    /// level, as ICU resolves them for Blink.
    #[test]
    fn an_isolate_ending_a_paragraph_after_rtl_has_its_controls_at_the_paragraph_level() {
        use super::{BidiClass, BidiScratch, resolve_bidi};

        let mut scratch = BidiScratch::new();
        let classes = [
            BidiClass::RIGHT_TO_LEFT,
            BidiClass::LEFT_TO_RIGHT_ISOLATE,
            BidiClass::LEFT_TO_RIGHT,
            BidiClass::POP_DIRECTIONAL_ISOLATE,
        ];
        let mut levels = [0; 4];
        let base_level = resolve_bidi(&mut scratch, &classes, &[], Some(0), &mut levels);
        assert_eq!(base_level, Some(0));
        assert_eq!(levels, [1, 0, 2, 0]);
    }
}
