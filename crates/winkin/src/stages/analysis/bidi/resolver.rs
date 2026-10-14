//! The Unicode bidirectional algorithm's resolution of a paragraph's levels
//! (UAX #9: P2–P3, X1–X10, W1–W7, N0–N2, I1–I2 and L1's separator half).
//!
//! [`resolve_bidi`] fills in a level for each unit [`input`](super::input)
//! builds. [`reset_separators`] then applies L1's separator half and hands
//! out each unit's final level in order. Line layout reorders (L2) with
//! `unicode::reorder_bidi` and applies the rest of L1, as Chrome resets by
//! the white space left at a line's end.
//!
//! The input is trusted: every class is one UAX #9 defines, the brackets are
//! in order and inside the paragraph, and `levels` has a slot per unit.
//! Bidi paragraphs (P1) are split before resolution. A B unit inside the
//! slice resets the embeddings (X8) but sets no new base level.

use alloc::vec::Vec;
use core::mem;
use core::num::NonZeroU32;
use core::ops::Range;

use crate::data::sort_by_key;
use crate::unicode::{BidiBracket, BidiClass, OVERRIDE_MASK, REMOVED_BY_X9_MASK};

/// One unit of a paragraph's bidi input: a class, and whether it is a
/// control a style synthesized rather than a cluster.
///
/// Clusters arrive in order, one unit each, so a cluster's level is found
/// by counting the cluster units before it. The class cannot tell the two
/// apart, since bidi controls in the text are clusters.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct BidiUnit {
    pub(crate) class: BidiClass,
    /// Whether the unit is a synthesized control, whose level is dropped,
    /// rather than the next cluster.
    pub(crate) control: bool,
}

const ISOLATE_MASK: u32 = BidiClass::RIGHT_TO_LEFT_ISOLATE.mask()
    | BidiClass::LEFT_TO_RIGHT_ISOLATE.mask()
    | BidiClass::FIRST_STRONG_ISOLATE.mask();

const EXPLICIT_MASK: u32 = OVERRIDE_MASK | ISOLATE_MASK;

const RTL_MASK: u32 = BidiClass::RIGHT_TO_LEFT_EMBEDDING.mask()
    | BidiClass::RIGHT_TO_LEFT_OVERRIDE.mask()
    | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask();

/// Reusable working memory for [`resolve_bidi`].
///
/// Holds nothing between calls. The analysis scratch keeps one across
/// paragraphs, so its buffers are not reallocated.
#[derive(Clone, Default)]
pub(crate) struct BidiScratch {
    /// One entry per FSI in the paragraph, in logical order.
    fsi_entries: Vec<FsiEntry>,
    /// FSIs still waiting for a strong character, innermost last.
    pending_fsi: Vec<PendingFsi>,
    /// The paragraph's classes after the X rules. Past them, a copy of the
    /// isolating run sequence being resolved, without what X9 removes, when
    /// the sequence is not one unbroken range.
    types: Vec<BidiClass>,
    /// The bracket pairs of the isolating run sequence being resolved, in
    /// sequence positions. Rebuilt for each sequence.
    bracket_pairs: Vec<(u32, u32)>,
    runs: Vec<Run>,
    /// The paragraph position of each entry of the copied sequence in
    /// `types`.
    indices: Vec<u32>,
}

impl BidiScratch {
    /// Creates an empty scratch, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            fsi_entries: Vec::new(),
            pending_fsi: Vec::new(),
            types: Vec::new(),
            bracket_pairs: Vec::new(),
            runs: Vec::new(),
            indices: Vec::new(),
        }
    }

    /// Returns the bytes its buffers have allocated.
    pub(super) fn heap_bytes(&self) -> usize {
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

    /// Resolves the direction of every FSI (P3) and returns the level of the
    /// first strong character outside any isolate (P2).
    ///
    /// One forward pass; without an FSI (`fsi` false) it stops at that
    /// first strong character.
    fn scan_isolates(&mut self, units: &[BidiUnit], fsi: bool) -> Option<u8> {
        const STRONG_START_MASK: u32 = BidiClass::LEFT_TO_RIGHT.mask()
            | BidiClass::RIGHT_TO_LEFT.mask()
            | BidiClass::ARABIC_LETTER.mask();
        self.fsi_entries.clear();
        self.pending_fsi.clear();
        let mut depth = 0u32;
        let mut first_strong = None;
        for (index, unit) in units.iter().enumerate() {
            let (class, mask) = (unit.class, unit.class.mask());
            if mask & ISOLATE_MASK != 0 {
                depth += 1;
                if class == BidiClass::FIRST_STRONG_ISOLATE {
                    let slot = self.fsi_entries.len() as u32;
                    self.fsi_entries.push(FsiEntry {
                        index: index as u32,
                        level: 0,
                    });
                    self.pending_fsi.push(PendingFsi {
                        slot,
                        open_depth: depth,
                    });
                }
            } else if class == BidiClass::POP_DIRECTIONAL_ISOLATE {
                depth = depth.saturating_sub(1);
                // Drops every FSI this PDI closed, so a later strong
                // character cannot settle it.
                while self
                    .pending_fsi
                    .last()
                    .is_some_and(|pending| pending.open_depth > depth)
                {
                    self.pending_fsi.pop();
                }
            } else if mask & STRONG_START_MASK != 0 {
                let level = u8::from(class != BidiClass::LEFT_TO_RIGHT);
                if depth == 0 && first_strong.is_none() {
                    first_strong = Some(level);
                    if !fsi {
                        break;
                    }
                }
                if self
                    .pending_fsi
                    .last()
                    .is_some_and(|pending| pending.open_depth == depth)
                    && let Some(pending) = self.pending_fsi.pop()
                    && let Some(entry) = self.fsi_entries.get_mut(pending.slot as usize)
                {
                    entry.level = level;
                }
            }
        }
        first_strong
    }
}

/// Resolves the embedding level of each unit of a paragraph into `levels`
/// and returns the base level.
///
/// `brackets` lists the paired brackets in ascending `index` order. `mask`
/// holds every class among the units, or more. `base_level` is `Some(0)`
/// for left to right, `Some(1)` for right to left, or `None` to take it
/// from the first strong character (P2, P3). The levels stop short of L1,
/// which [`reset_separators`] applies.
pub(crate) fn resolve_bidi(
    scratch: &mut BidiScratch,
    units: &[BidiUnit],
    brackets: &[BidiBracket],
    mask: u32,
    base_level: Option<u8>,
    levels: &mut [u8],
) -> u8 {
    debug_assert_eq!(units.len(), levels.len(), "a level a unit");
    let fsi = mask & BidiClass::FIRST_STRONG_ISOLATE.mask() != 0;
    let first_strong = if fsi || base_level.is_none() {
        scratch.scan_isolates(units, fsi)
    } else {
        None
    };
    let base_level = base_level.map_or(first_strong.unwrap_or(0), |level| level & 1);
    let mut resolver = Resolver {
        scratch,
        units,
        brackets,
        levels,
        base_level,
    };
    resolver.resolve_levels();
    resolver.link_sequences();
    resolver.resolve_sequences(mask);
    base_level
}

/// Applies the half of L1 that needs no line and hands each unit's final
/// level to `each`, in order.
///
/// A segment or paragraph separator, and the white space and isolate
/// controls before it, go to the base level. A unit X9 removed takes the
/// level before it. A run of white space and isolate controls is handed out
/// once the next unit shows whether a separator resets it.
pub(crate) fn reset_separators(
    units: &[BidiUnit],
    levels: &mut [u8],
    base: u8,
    mut each: impl FnMut(usize, u8),
) {
    const TRAILING: u32 =
        ISOLATE_MASK | BidiClass::POP_DIRECTIONAL_ISOLATE.mask() | BidiClass::WHITE_SPACE.mask();
    const SEPARATOR: u32 =
        BidiClass::SEGMENT_SEPARATOR.mask() | BidiClass::PARAGRAPH_SEPARATOR.mask();
    // The start of the open run a separator would reset.
    let mut pending: Option<usize> = None;
    let mut before = base;
    for at in 0..units.len().min(levels.len()) {
        let mask = units[at].class.mask();
        let level = &mut levels[at];
        if mask & REMOVED_BY_X9_MASK != 0 {
            *level = before;
            if pending.is_some() {
                continue;
            }
        } else if mask & TRAILING != 0 {
            pending.get_or_insert(at);
            before = *level;
            continue;
        } else if mask & SEPARATOR != 0 {
            *level = base;
        }
        before = *level;
        if let Some(from) = pending.take() {
            let resets = mask & SEPARATOR != 0;
            flush(units, levels, from..at, resets, base, &mut each);
        }
        each(at, before);
    }
    if let Some(from) = pending {
        flush(units, levels, from..units.len(), false, base, &mut each);
    }
}

/// Hands out the levels of `range`, first setting its white space and
/// isolate controls to `base` if a separator ends it (`resets`).
fn flush(
    units: &[BidiUnit],
    levels: &mut [u8],
    range: Range<usize>,
    resets: bool,
    base: u8,
    each: &mut impl FnMut(usize, u8),
) {
    let units = units.get(range.clone()).unwrap_or_default();
    let levels = levels.get_mut(range.clone()).unwrap_or_default();
    for ((at, unit), level) in range.zip(units).zip(levels) {
        if resets && unit.class.mask() & REMOVED_BY_X9_MASK == 0 {
            *level = base;
        }
        each(at, *level);
    }
}

fn is_isolate_initiator(ty: BidiClass) -> bool {
    ty.mask() & ISOLATE_MASK != 0
}

/// Returns L for an even level and R for an odd one.
fn class_from_level(level: u8) -> BidiClass {
    if level & 1 == 0 {
        BidiClass::LEFT_TO_RIGHT
    } else {
        BidiClass::RIGHT_TO_LEFT
    }
}

/// Returns the direction rule N0 gives `ty` inside a bracket pair, where
/// numbers count as R, or [`BidiClass::OTHER_NEUTRAL`] for a class with no
/// direction.
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

/// The buffers the rules share while resolving one paragraph.
struct Resolver<'a> {
    scratch: &'a mut BidiScratch,
    units: &'a [BidiUnit],
    brackets: &'a [BidiBracket],
    levels: &'a mut [u8],
    base_level: u8,
}

impl Resolver<'_> {
    /// Applies X1 through X9 and BD7: assigns each unit its embedding level,
    /// applies any override in force, and splits the units X9 keeps into
    /// level runs.
    fn resolve_levels(&mut self) {
        let base = self.base_level;
        let len = self.units.len();
        let mut stack = LevelStack::new();
        let mut overflow_isolates = 0;
        let mut overflow_embedding = 0;
        let mut valid_isolates = 0;
        let mut fsi_cursor = 0usize;
        stack.push(base, BidiClass::OTHER_NEUTRAL, false);
        let scratch = &mut *self.scratch;
        scratch.types.clear();
        scratch.runs.clear();
        // The run being gathered.
        let mut open: Option<Run> = None;

        for (i, (unit, level)) in self.units.iter().zip(self.levels.iter_mut()).enumerate() {
            let mut t = unit.class;
            let tmask = t.mask();
            if tmask & EXPLICIT_MASK != 0 {
                let is_isolate = tmask & ISOLATE_MASK != 0;
                // X5c: an FSI acts as RLI or LRI by the direction
                // `scan_isolates` found for it.
                let is_rtl = if t == BidiClass::FIRST_STRONG_ISOLATE && i + 1 < len {
                    let mut fsi_level = 0;
                    if let Some(entry) = scratch.fsi_entries.get(fsi_cursor)
                        && entry.index as usize == i
                    {
                        fsi_level = entry.level;
                        fsi_cursor += 1;
                    }
                    fsi_level == 1
                } else {
                    tmask & RTL_MASK != 0
                };
                if is_isolate {
                    *level = stack.embedding_level();
                    let os = stack.override_status();
                    if os != BidiClass::OTHER_NEUTRAL {
                        t = os;
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
                        if t == BidiClass::LEFT_TO_RIGHT_OVERRIDE {
                            BidiClass::LEFT_TO_RIGHT
                        } else if t == BidiClass::RIGHT_TO_LEFT_OVERRIDE {
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
            } else if t == BidiClass::POP_DIRECTIONAL_ISOLATE {
                if overflow_isolates > 0 {
                    overflow_isolates -= 1;
                } else if valid_isolates > 0 {
                    overflow_embedding = 0;
                    while !stack.isolate_status() {
                        stack.pop();
                    }
                    stack.pop();
                    valid_isolates -= 1;
                }
                *level = stack.embedding_level();
                if stack.override_status() != BidiClass::OTHER_NEUTRAL {
                    t = stack.override_status();
                }
            } else if t == BidiClass::POP_DIRECTIONAL_FORMAT {
                *level = stack.embedding_level();
                if overflow_isolates == 0 {
                    if overflow_embedding > 0 {
                        overflow_embedding -= 1;
                    } else if !stack.isolate_status() && stack.depth >= 2 {
                        stack.pop();
                    }
                }
            } else if t == BidiClass::PARAGRAPH_SEPARATOR {
                stack.depth = 1;
                stack.cur = stack.entries[0];
                overflow_isolates = 0;
                overflow_embedding = 0;
                valid_isolates = 0;
                *level = base;
            } else if t != BidiClass::BOUNDARY_NEUTRAL {
                *level = stack.embedding_level();
                if stack.override_status() != BidiClass::OTHER_NEUTRAL {
                    t = stack.override_status();
                }
            }
            scratch.types.push(t);
            if t.is_removed_by_x9() {
                continue;
            }
            // BD7: a run ends where the level of the units X9 keeps changes.
            if let Some(run) = open.as_mut().filter(|run| run.level == *level) {
                run.end = i as u32 + 1;
                continue;
            }
            let mut run = Run::new(*level, i, t);
            let before = open.map_or(base, |run| run.level);
            run.sos = class_from_level(before.max(run.level));
            scratch.runs.extend(open.replace(run));
        }
        scratch.runs.extend(open);
    }

    /// Applies X10 and BD13: sets each run's eos and links the runs into
    /// isolating run sequences.
    ///
    /// A run ending in an isolate initiator continues into the run starting
    /// with the matching PDI, at the same level. Runs awaiting a PDI sit on
    /// one stack per level, threaded through `stack_prev`, since a run waits
    /// on at most one PDI.
    fn link_sequences(&mut self) {
        let base = self.base_level;
        let runs = &mut self.scratch.runs;
        let mut heads = [RunIndex::NONE; MAX_STACK + 1];
        for i in 0..runs.len() {
            let level = runs[i].level;
            let next = runs.get(i + 1).map_or(base, |run| run.level);
            let run = &mut runs[i];
            if is_isolate_initiator(self.units[run.end as usize - 1].class) {
                run.ends_with_isolate = true;
                run.eos = class_from_level(base.max(level));
            } else {
                run.eos = class_from_level(next.max(level));
            }
            let level = level as usize;
            debug_assert!(level <= MAX_STACK, "run level {level} past the deepest");
            if runs[i].starts_with_pdi
                && let Some(open) = heads[level].get()
            {
                heads[level] = runs[open].stack_prev;
                runs[i].in_sequence = true;
                runs[open].next = RunIndex::new(i);
            }
            if runs[i].ends_with_isolate {
                runs[i].stack_prev = heads[level];
                heads[level] = RunIndex::new(i);
            }
        }
    }

    /// Applies X10: resolves each isolating run sequence with the W, N and I
    /// rules.
    ///
    /// A sequence of one run, in a paragraph X9 removes nothing from, is
    /// resolved in place in `types`. Any other is copied past the paragraph
    /// without what X9 removes. `mask` holds every class in the paragraph and
    /// stands in for a single-run sequence's classes.
    fn resolve_sequences(&mut self, mask: u32) {
        let len = self.units.len();
        for i in 0..self.scratch.runs.len() {
            let run = &self.scratch.runs[i];
            // A run continuing an earlier sequence is resolved with it.
            if run.in_sequence {
                continue;
            }
            let (level, sos) = (run.level, run.sos);
            if run.next.get().is_none() && mask & REMOVED_BY_X9_MASK == 0 {
                let (start, end) = (run.start as usize, run.end as usize);
                let seq = SeqPositions::Contiguous {
                    start: run.start,
                    len: run.end - run.start,
                };
                self.resolve_sequence(start..end, seq, level, sos, run.eos, mask);
                continue;
            }
            self.scratch.types.truncate(len);
            self.scratch.indices.clear();
            let mut sequence_mask = 0u32;
            let mut cur = i;
            let eos = loop {
                let run = &self.scratch.runs[cur];
                for index in run.range() {
                    let ty = self.scratch.types[index];
                    if ty.is_removed_by_x9() {
                        continue;
                    }
                    self.scratch.types.push(ty);
                    sequence_mask |= ty.mask();
                    self.scratch.indices.push(index as u32);
                }
                match run.next.get() {
                    Some(next) => cur = next,
                    None => break run.eos,
                }
            };
            let indices = mem::take(&mut self.scratch.indices);
            let copied = len..self.scratch.types.len();
            let seq = SeqPositions::Scattered(&indices);
            self.resolve_sequence(copied, seq, level, sos, eos, sequence_mask);
            self.scratch.indices = indices;
        }
    }

    #[allow(clippy::needless_range_loop)]
    /// Applies rules W1 through W7, N0 through N2, and I1/I2 to the
    /// isolating run sequence in `types[range]`, whose units sit at `seq`.
    ///
    /// `sequence_mask` holds the sequence's classes, or more; rule groups
    /// none of them reach are skipped.
    fn resolve_sequence(
        &mut self,
        range: Range<usize>,
        seq: SeqPositions<'_>,
        level: u8,
        sos: BidiClass,
        eos: BidiClass,
        sequence_mask: u32,
    ) {
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
        let types = &mut self.scratch.types[range];
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
                    } else if let Some((sep_idx, BidiClass::EUROPEAN_NUMBER)) = pending_separator {
                        types[sep_idx] = BidiClass::EUROPEAN_NUMBER;
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
                    // W4: a European separator joins European numbers, a
                    // common separator numbers of either kind.
                    let joins = prev == BidiClass::EUROPEAN_NUMBER
                        || (t == BidiClass::COMMON_SEPARATOR && prev == BidiClass::ARABIC_NUMBER);
                    pending_separator = joins.then_some((i, prev));
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
        // The index of the first bracket at or after the sequence's start.
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
                let embed_dir = class_from_level(level);
                let bracket_pairs = &mut self.scratch.bracket_pairs[..];
                sort_by_key(bracket_pairs, |pair| pair.0);
                let mut strong_scan = 0;
                let mut preceding_strong = BidiClass::OTHER_NEUTRAL;
                for &(open, close) in &*bracket_pairs {
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
                    // Units originally NSM right after either bracket take the
                    // pair's direction.
                    for after in [open + 1..close, close + 1..len] {
                        for i in after {
                            if self.units[seq.pos(i)].class != BidiClass::NONSPACING_MARK {
                                break;
                            }
                            types[i] = pair_dir;
                        }
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
        // A number counts as right to left beside a neutral.
        let side = |t: BidiClass| {
            if t == BidiClass::ARABIC_NUMBER || t == BidiClass::EUROPEAN_NUMBER {
                BidiClass::RIGHT_TO_LEFT
            } else {
                t
            }
        };
        if sequence_mask & (N_MASK | W6_MASK) != 0 {
            let mut i = 0;
            while i < len {
                if types[i].mask() & N_MASK != 0 {
                    let limit = find_limit_by_mask(types, i, N_MASK);
                    let leading = if i == 0 { sos } else { side(types[i - 1]) };
                    let trailing = if limit == len {
                        eos
                    } else {
                        side(types[limit])
                    };
                    // N1, or N2 where the sides differ.
                    let resolved = if leading == trailing {
                        leading
                    } else {
                        class_from_level(level)
                    };
                    types[i..limit].fill(resolved);
                    i = limit;
                } else {
                    i += 1;
                }
            }
        }
        // I1: L at the level, R one above and numbers two above, at an even
        // level. I2: R at the level and anything else one above, at an odd
        // one.
        let implicit = |t: BidiClass| {
            if level & 1 == 0 {
                if t == BidiClass::LEFT_TO_RIGHT {
                    level
                } else if t == BidiClass::RIGHT_TO_LEFT {
                    level + 1
                } else {
                    level + 2
                }
            } else if t == BidiClass::RIGHT_TO_LEFT {
                level
            } else {
                level + 1
            }
        };
        match seq {
            SeqPositions::Contiguous { start, .. } => {
                let start = start as usize;
                for (dst, &t) in self.levels[start..start + len].iter_mut().zip(&*types) {
                    *dst = implicit(t);
                }
            }
            SeqPositions::Scattered(positions) => {
                for (&index, &t) in positions.iter().zip(&*types) {
                    self.levels[index as usize] = implicit(t);
                }
            }
        }
    }
}

/// The direction an FSI resolves to (P3).
///
/// `BidiScratch::scan_isolates` adds an entry when it reaches the FSI and
/// fills it at the first strong character after it at the same isolate
/// depth.
#[derive(Copy, Clone)]
struct FsiEntry {
    /// The FSI's unit index in the paragraph.
    index: u32,
    /// The level of the first strong character inside the isolate.
    ///
    /// P3 defaults an FSI with no strong character to level 0, so an
    /// unsettled entry is already correct.
    level: u8,
}

/// An FSI whose direction is not settled yet.
#[derive(Copy, Clone)]
struct PendingFsi {
    /// The FSI's index in `BidiScratch::fsi_entries`.
    slot: u32,
    /// The isolate depth just inside this FSI. Only a strong character at
    /// exactly this depth settles it, so nested isolates do not count.
    open_depth: u32,
}

/// The deepest level UAX #9 opens (BD2's `max_depth`).
const MAX_STACK: usize = 125;

#[derive(Copy, Clone)]
struct LevelStackEntry {
    embedding_level: u8,
    /// The class an enclosing LRO or RLO forces, or
    /// [`BidiClass::OTHER_NEUTRAL`] with no override in force.
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
        let entry = LevelStackEntry {
            embedding_level: 0,
            override_status: BidiClass::OTHER_NEUTRAL,
            isolate_status: false,
        };
        Self {
            depth: 0,
            entries: [entry; MAX_STACK + 1],
            cur: entry,
        }
    }

    fn push(&mut self, level: u8, override_status: BidiClass, isolate_status: bool) {
        let entry = LevelStackEntry {
            embedding_level: level,
            override_status,
            isolate_status,
        };
        self.entries[self.depth] = entry;
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
/// Stored as index + 1, so the `NonZeroU32` niche keeps it to four bytes.
#[derive(Copy, Clone, Default, PartialEq, Eq)]
struct RunIndex(Option<NonZeroU32>);

impl RunIndex {
    const NONE: Self = Self(None);

    #[inline]
    fn new(index: usize) -> Self {
        Self(NonZeroU32::new(index as u32 + 1))
    }

    #[inline]
    fn get(self) -> Option<usize> {
        self.0.map(|index| index.get() as usize - 1)
    }
}

/// A level run, plus the links that join runs into isolating run sequences.
#[derive(Copy, Clone)]
struct Run {
    level: u8,
    /// The run's paragraph positions: its first unit X9 keeps, and one past
    /// its last.
    start: u32,
    end: u32,
    /// The classes taken as before and after the run's sequence (X10).
    sos: BidiClass,
    eos: BidiClass,
    /// Whether the run ends with an isolate initiator, which links it to the
    /// run starting with the matching PDI.
    ends_with_isolate: bool,
    /// Whether the run starts with a PDI.
    starts_with_pdi: bool,
    /// Whether the run continues an earlier run's sequence, so
    /// `resolve_sequences` starts no sequence at it.
    in_sequence: bool,
    /// The next run in the sequence, if any.
    next: RunIndex,
    /// The run below this one on its level's stack in `link_sequences`.
    stack_prev: RunIndex,
}

impl Run {
    /// Creates a run at `level` starting at unit `start`, of class `first`.
    fn new(level: u8, start: usize, first: BidiClass) -> Self {
        Self {
            level,
            ends_with_isolate: false,
            starts_with_pdi: first == BidiClass::POP_DIRECTIONAL_ISOLATE,
            sos: BidiClass::OTHER_NEUTRAL,
            eos: BidiClass::OTHER_NEUTRAL,
            start: start as u32,
            end: start as u32 + 1,
            in_sequence: false,
            next: RunIndex::NONE,
            stack_prev: RunIndex::NONE,
        }
    }

    /// Returns the run's paragraph positions.
    #[inline]
    fn range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// Maps positions within an isolating run sequence back to paragraph
/// positions.
#[derive(Copy, Clone)]
enum SeqPositions<'a> {
    /// One unbroken range of the paragraph, each position an offset from
    /// `start`.
    Contiguous { start: u32, len: u32 },
    /// The listed paragraph positions, where the sequence skips some.
    Scattered(&'a [u32]),
}

impl SeqPositions<'_> {
    #[inline]
    fn len(self) -> usize {
        match self {
            Self::Contiguous { len, .. } => len as usize,
            Self::Scattered(positions) => positions.len(),
        }
    }

    /// Returns the paragraph position of the sequence's `i`th unit.
    #[inline]
    fn pos(self, i: usize) -> usize {
        match self {
            Self::Contiguous { start, .. } => start as usize + i,
            Self::Scattered(positions) => positions[i] as usize,
        }
    }

    /// Returns the sequence position of the unit at paragraph position
    /// `pos`, or `None` if the unit is not in the sequence.
    ///
    /// Positions ascend, so the scattered case binary searches.
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
