//! The offset map: where each piece of the content's text came from in each
//! node's input text.
//!
//! The writer records a unit for each run of a node's text written as
//! given, each white space run that collapsing changed, each character a
//! transform changed the length of, and each character the builder wrote.
//! Units are in content order and tile the content's text: a unit ends where
//! the next starts, which keeps a unit at 12 bytes. **Plain text is one unit
//! per text node**: text written as given, a length-preserving transform,
//! and a lone space that stays one space all join the unit before.
//!
//! A node's units are contiguous: they are the units written while it was
//! the last node made. The map keeps each node's first unit, and a binary
//! search over those finds a unit's node. A break opportunity written
//! between nodes (a `<wbr>` after a box closes) belongs to the last node
//! written; it holds no input text, so its owner changes no answer.
//!
//! **Offsets in a node's text** count every byte of the node's `text` calls
//! in order, collapsed or not. An atomic inline and a `<br>` span their
//! node's offsets 0 to 1. A `::first-letter` box's text is cut from a text
//! node: its units, and the rest of that text node after it, answer to that
//! text node's key and continue its offsets.
//!
//! The map is recorded only where [`BuildOptions::map_source`] asks.
//! Otherwise it stays empty, and positions are content offsets only.
//!
//! [`BuildOptions::map_source`]: crate::BuildOptions::map_source

use core::ops::Range;

use super::{NodeId, NodeKey, NodeKind, Nodes};
use crate::data::{Id, Table, TextOffset, define_id, heap_bytes, index_to_u32, u32_to_index};
use crate::work;

define_id! {
    /// The id of a unit of the offset map.
    pub(super) struct MapUnitId(u32);
}

/// How a unit of the offset map relates the content's text to its node's
/// text.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub(super) enum MapUnitKind {
    /// Written as given, or by a length-preserving transform: offsets map
    /// one to one.
    Identity,
    /// A white space run that became one space or none: an offset inside
    /// maps to the space's position.
    Collapsed,
    /// A character a transform changed the length of (`ß` uppercased to
    /// `SS`). It is one caret stop, and an offset inside maps to an end.
    Variable,
    /// Written by the builder.
    ///
    /// An atomic inline's U+FFFC and a `<br>`'s `\n` span their node's
    /// offsets 0 to 1. A break opportunity's U+200B spans no input text.
    Generated,
}

impl MapUnitKind {
    const fn bits(self) -> u32 {
        match self {
            Self::Identity => 0,
            Self::Collapsed => 1,
            Self::Variable => 2,
            Self::Generated => 3,
        }
    }

    const fn from_bits(bits: u32) -> Self {
        match bits & 3 {
            0 => Self::Identity,
            1 => Self::Collapsed,
            2 => Self::Variable,
            _ => Self::Generated,
        }
    }
}

/// One unit of the offset map, 12 bytes. It ends in the content's text
/// where the next unit starts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct MapUnit {
    /// Its start in the content's text, shifted left two, with its kind in
    /// the two low bits. The text holds under 2^30 bytes.
    start: u32,
    /// Its start in its node's text.
    source: u32,
    /// Its end in its node's text.
    source_end: u32,
}

impl MapUnit {
    fn new(kind: MapUnitKind, at: TextOffset, source: Range<u32>) -> Self {
        Self {
            start: at.to_u32() << 2 | kind.bits(),
            source: source.start,
            source_end: source.end.max(source.start),
        }
    }

    fn start(&self) -> TextOffset {
        TextOffset::from_u32(self.start >> 2)
    }

    fn kind(&self) -> MapUnitKind {
        MapUnitKind::from_bits(self.start)
    }

    fn set_start(&mut self, at: TextOffset) {
        self.start = at.to_u32() << 2 | (self.start & 3);
    }

    fn set_kind(&mut self, kind: MapUnitKind) {
        self.start = self.start & !3 | kind.bits();
    }

    /// Returns how many bytes of its node's text it spans.
    fn source_len(&self) -> u32 {
        self.source_end - self.source
    }
}

/// A point to truncate the offset map back to: its unit count, and its
/// last unit then, which a later unit may have joined.
#[derive(Copy, Clone, Debug)]
pub(super) struct MapMark {
    units: MapUnitId,
    last: Option<MapUnit>,
}

/// The offset map, filled only where the build asks for it.
#[derive(Default)]
pub(super) struct Map {
    /// Whether this build records the map. The writer records and readers
    /// read only where it does.
    pub(super) recording: bool,
    /// The units, in content order, tiling the content's text.
    units: Table<MapUnitId, MapUnit>,
    /// Each node's first unit; a node's units run to the next node's first.
    /// One per node while recording, none otherwise.
    first_unit: Table<NodeId, MapUnitId>,
    /// The key of the text node the `::first-letter` box took its text
    /// from. The box's units answer to it.
    letter_key: Option<NodeKey>,
}

impl Map {
    /// Creates an empty map that records nothing, allocating nothing.
    pub(super) const fn new() -> Self {
        Self {
            recording: false,
            units: Table::new(),
            first_unit: Table::new(),
            letter_key: None,
        }
    }

    /// Removes every unit, keeping the allocations, for a new build that
    /// records the map if `recording` is set.
    pub(super) fn clear(&mut self, recording: bool) {
        self.recording = recording;
        self.units.clear();
        self.first_unit.clear();
        self.letter_key = None;
    }

    // Recording, as the writer writes. ------------------------------------

    /// Starts a new node's units with the next unit.
    pub(super) fn open_node(&mut self) {
        if self.first_unit.push(self.units.next_id()).is_none() {
            debug_assert!(false, "a node's first unit fits where the node does");
        }
    }

    /// Records that the `::first-letter` box takes its text from the text
    /// node keyed `key`.
    pub(super) fn letter_from(&mut self, key: NodeKey) {
        self.letter_key = Some(key);
    }

    /// Returns the first unit of the node made last.
    fn node_start(&self) -> MapUnitId {
        self.first_unit
            .last()
            .copied()
            .unwrap_or(self.units.next_id())
    }

    /// Returns the last unit if it belongs to the node made last.
    fn last_node_unit(&mut self) -> Option<&mut MapUnit> {
        let start = self.node_start();
        let last = self.units.last_id()?;
        if last < start {
            return None;
        }
        self.units.get_mut(last)
    }

    fn push(&mut self, unit: MapUnit) -> Option<MapUnitId> {
        let id = self.units.push(unit);
        // Units are bounded by the text's bytes and the builder's calls,
        // which a `u32` counts well past what fits in memory.
        debug_assert!(id.is_some(), "the map's units fit");
        id
    }

    /// Returns a mark for [`truncate`](Self::truncate) to restore.
    pub(super) fn mark(&self) -> MapMark {
        MapMark {
            units: self.units.next_id(),
            last: self.units.last().copied(),
        }
    }

    /// Drops every unit recorded since `mark`, and undoes anything joined
    /// to the unit that was last then.
    pub(super) fn truncate(&mut self, mark: MapMark) {
        self.units.truncate(mark.units);
        if let Some(last) = mark.last
            && let Some(slot) = self.units.last_mut()
        {
            *slot = last;
        }
    }

    /// Returns the end of the last unit of the node made last, or 0 where
    /// it has none.
    pub(super) fn source_end(&self) -> u32 {
        let start = self.node_start();
        match self.units.last_id() {
            Some(last) if last >= start => self.units.get(last).map_or(0, |unit| unit.source_end),
            _ => 0,
        }
    }

    /// Records `len` bytes of the node's text from `source`, written as
    /// given at content offset `at`.
    ///
    /// If `merge` is set, they join the unit before where that unit is the
    /// node's, written as given, and ends at `source`.
    pub(super) fn identity(&mut self, source: u32, len: usize, at: TextOffset, merge: bool) {
        if len == 0 {
            return;
        }
        let end = source.saturating_add(index_to_u32(len));
        if merge
            && let Some(last) = self.last_node_unit()
            && last.kind() == MapUnitKind::Identity
            && last.source_end == source
        {
            last.source_end = end;
            return;
        }
        self.push(MapUnit::new(MapUnitKind::Identity, at, source..end));
    }

    /// Records `source`, a character or run of the node's text whose length
    /// a transform changed, written at `at`.
    pub(super) fn variable(&mut self, source: Range<u32>, at: TextOffset) {
        self.push(MapUnit::new(MapUnitKind::Variable, at, source));
    }

    /// Records text the builder wrote at `at`, spanning `source` of its
    /// node's text.
    ///
    /// An atomic inline or a `<br>` spans offsets 0 to 1. A break
    /// opportunity spans none.
    pub(super) fn generated(&mut self, source: Range<u32>, at: TextOffset) {
        self.push(MapUnit::new(MapUnitKind::Generated, at, source));
    }

    /// Records white space `source` of the node's text, collapsed away at
    /// `at`.
    ///
    /// It joins the unit before where that unit is the node's, collapsed to
    /// nothing at `at`, and ends at `source.start`.
    pub(super) fn collapsed(&mut self, source: Range<u32>, at: TextOffset) {
        if source.is_empty() {
            return;
        }
        if let Some(last) = self.last_node_unit()
            && last.kind() == MapUnitKind::Collapsed
            && last.start() == at
            && last.source_end == source.start
        {
            last.source_end = source.end;
            return;
        }
        self.push(MapUnit::new(MapUnitKind::Collapsed, at, source));
    }

    /// Records `source`, the start of a white space run that owes one space
    /// at `at` once something follows it.
    ///
    /// It gets its own unit, which the rest of the run in this node joins
    /// and which takes the space if it is written. Returns the unit's id
    /// for [`space_written`](Self::space_written).
    pub(super) fn run(&mut self, source: Range<u32>, at: TextOffset) -> Option<MapUnitId> {
        self.push(MapUnit::new(MapUnitKind::Collapsed, at, source))
    }

    /// Records that the space `run` owed is written where the run began.
    ///
    /// Every later unit shifts by one byte, as the items do. Following
    /// Blink's `AppendCollapseWhitespace`, the run's first character becomes
    /// the space, written as given, and joins the unit before where both are
    /// the node's. The rest of the run collapses after the space, so an
    /// input offset past the run's first character maps after the space.
    ///
    /// The later units are those written while the run was pending. Adding
    /// or removing the unit for the run's rest shifts their ids once more.
    pub(super) fn space_written(&mut self, run: MapUnitId) {
        let end = self.units.next_id();
        if let Some(after) = self.units.get_slice_mut(MapUnitId::new(run.get() + 1)..end) {
            for unit in after {
                work::step();
                unit.set_start(TextOffset::new(unit.start().get() + 1));
            }
        }
        let Some(&unit) = self.units.get(run) else {
            return;
        };
        // Every collapsible character is one byte.
        let space = unit.source.saturating_add(1).min(unit.source_end);
        let at = unit.start();
        let rest = MapUnit::new(
            MapUnitKind::Collapsed,
            TextOffset::new(at.get() + 1),
            space..unit.source_end,
        );
        let joined = match run.get().checked_sub(1).map(MapUnitId::new) {
            Some(before) if self.same_node(before, run) => match self.units.get_mut(before) {
                Some(previous)
                    if previous.kind() == MapUnitKind::Identity
                        && previous.source_end == unit.source =>
                {
                    previous.source_end = space;
                    true
                }
                _ => false,
            },
            _ => false,
        };
        let rest_left = rest.source_len() > 0;
        if joined {
            if rest_left {
                if let Some(slot) = self.units.get_mut(run) {
                    *slot = rest;
                }
            } else if let Some(tail) = self.units.as_mut_slice().get_mut(run.get()..) {
                // Drop the run's unit and shift later units, and the
                // nodes' first units, back one.
                tail.rotate_left(1);
                self.units.pop();
                self.renumber_after(run, false);
            }
            return;
        }
        if let Some(slot) = self.units.get_mut(run) {
            *slot = MapUnit::new(MapUnitKind::Identity, at, unit.source..space);
        }
        if rest_left && self.push(rest).is_some() {
            // Pushed last, then rotated to just after the space.
            if let Some(after) = self.units.as_mut_slice().get_mut(run.get() + 1..) {
                after.rotate_right(1);
            }
            self.renumber_after(run, true);
        }
    }

    /// Shifts the first units of the nodes made while `run` was pending:
    /// forward one if `on` (a unit inserted after `run`), else back one (a
    /// unit removed at `run`).
    fn renumber_after(&mut self, run: MapUnitId, on: bool) {
        for first in self.first_unit.as_mut_slice().iter_mut().rev() {
            work::step();
            if *first <= run {
                break;
            }
            *first = match on {
                true => MapUnitId::new(first.get() + 1),
                false => MapUnitId::new(first.get() - 1),
            };
        }
    }

    /// Whether units `a` and `b`, `a` before `b`, belong to one node: no
    /// node's first unit is after `a` and at or before `b`.
    fn same_node(&self, a: MapUnitId, b: MapUnitId) -> bool {
        let firsts = self.first_unit.as_slice();
        firsts.partition_point(|&first| first <= a) == firsts.partition_point(|&first| first <= b)
    }

    /// Records that the block's leading trim of kept white space removed
    /// the first `cut` bytes of the content's text.
    ///
    /// Units before the cut collapse to nothing at offset 0, and the rest
    /// shift back.
    pub(super) fn drain_front(&mut self, cut: TextOffset) {
        for unit in self.units.as_mut_slice() {
            work::step();
            let start = unit.start();
            if start < cut {
                // The writer starts a unit at every kept segment break
                // while it trims, so no unit crosses the cut.
                unit.set_kind(MapUnitKind::Collapsed);
                unit.set_start(TextOffset::new(0));
            } else {
                unit.set_start(TextOffset::new(start.get() - cut.get()));
            }
        }
    }

    /// Records that the block's trailing trim cut the content's text at
    /// `cut`. Units from there collapse to nothing at the cut.
    pub(super) fn cut_end(&mut self, cut: TextOffset) {
        let units = self.units.as_mut_slice();
        let kept = units.partition_point(|unit| unit.start() < cut);
        for unit in units.get_mut(kept..).unwrap_or_default() {
            work::step();
            unit.set_start(cut);
            unit.set_kind(MapUnitKind::Collapsed);
        }
        // The writer starts a unit at every kept segment break where the
        // block trims, so the unit before ends at the cut. An identity unit
        // that ran past it would span more than it holds, so it becomes
        // collapsed.
        if let Some(last) = kept.checked_sub(1).and_then(|at| units.get_mut(at))
            && last.kind() == MapUnitKind::Identity
            && u32_to_index(last.source_len()) != cut.get() - last.start().get()
        {
            last.set_kind(MapUnitKind::Collapsed);
        }
    }

    /// Checks the map's invariants in debug builds.
    ///
    /// - Units are in content order within `text_len`.
    /// - There is one first unit per node, in order.
    /// - Each node's units are contiguous in its text.
    /// - An identity unit has the same length in the content and its node.
    #[cfg(debug_assertions)]
    pub(super) fn debug_check(&self, text_len: usize, nodes: usize) {
        if !self.recording {
            debug_assert!(self.units.is_empty() && self.first_unit.is_empty());
            return;
        }
        debug_assert_eq!(self.first_unit.len(), nodes, "one first unit a node");
        let firsts = self.first_unit.as_slice();
        debug_assert!(
            firsts.windows(2).all(|pair| pair[0] <= pair[1]),
            "the nodes' units follow one another"
        );
        let units = self.units.as_slice();
        for (at, unit) in units.iter().enumerate() {
            let end = units
                .get(at + 1)
                .map_or(text_len, |next| next.start().get());
            debug_assert!(unit.start().get() <= end, "unit {at} runs backwards");
            debug_assert!(unit.source <= unit.source_end);
            if unit.kind() == MapUnitKind::Identity {
                debug_assert_eq!(
                    end - unit.start().get(),
                    u32_to_index(unit.source_len()),
                    "identity unit {at} is as long both ways"
                );
            }
        }
        debug_assert!(
            units
                .last()
                .is_none_or(|unit| unit.start().get() <= text_len),
            "the units stay within the text"
        );
        for (node, &first) in self.first_unit.iter() {
            let end = self
                .first_unit
                .get(NodeId::new(node.get() + 1))
                .copied()
                .unwrap_or(self.units.next_id());
            let own = self.units.slice(first..end);
            debug_assert!(
                own.windows(2)
                    .all(|pair| pair[0].source_end == pair[1].source),
                "{node:?}'s units run on in its text"
            );
        }
    }

    // Reading. --------------------------------------------------------------

    /// Returns a reader of the map against `nodes`, over a text of
    /// `text_len` bytes, or `None` where the build did not record it.
    pub(super) fn read<'a>(&'a self, nodes: &'a Nodes, text_len: usize) -> Option<OffsetMap<'a>> {
        self.recording.then_some(OffsetMap {
            map: self,
            nodes,
            end: TextOffset::new(text_len),
        })
    }
}

heap_bytes! {
    Map { units, first_unit; recording, letter_key }
}

/// A reader of the offset map, mapping content offsets to node text offsets
/// and back.
///
/// [`Content::offset_map`](super::Content::offset_map) returns one where
/// the build recorded the map.
#[derive(Copy, Clone)]
pub(crate) struct OffsetMap<'a> {
    map: &'a Map,
    nodes: &'a Nodes,
    /// The end of the content's text, which is the last unit's end.
    end: TextOffset,
}

/// Which position an offset maps to where several positions in one text
/// are one position in the other.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum MapSide {
    /// The first position: before what was collapsed or generated there.
    Before,
    /// The last position: after what was collapsed or generated there.
    After,
}

impl MapSide {
    /// Picks this side's position among the units meeting at a boundary.
    ///
    /// `meeting` holds, each where present: the unit ending there, the
    /// first and last units collapsed to nothing there, and the unit
    /// starting there. `start` and `end` give a unit's bounds in the target
    /// text.
    /// - `Before` takes the end of the unit ending, else the start of the
    ///   first collapsed, else the start of the unit starting.
    /// - `After` takes the start of the unit starting, else the end of the
    ///   last collapsed, else the end of the unit ending.
    fn pick<T>(
        self,
        meeting: [Option<MapUnitId>; 4],
        start: impl Fn(MapUnitId) -> Option<T>,
        end: impl Fn(MapUnitId) -> Option<T>,
    ) -> Option<T> {
        let [ending, empty_first, empty_last, starting] = meeting;
        match self {
            Self::Before => ending
                .and_then(&end)
                .or_else(|| empty_first.and_then(&start))
                .or_else(|| starting.and_then(&start)),
            Self::After => starting
                .and_then(&start)
                .or_else(|| empty_last.and_then(&end))
                .or_else(|| ending.and_then(&end)),
        }
    }
}

impl<'a> OffsetMap<'a> {
    fn unit(&self, unit: MapUnitId) -> Option<&'a MapUnit> {
        self.map.units.get(unit)
    }

    /// Returns the first unit starting at or after `at`, by binary search.
    fn first_from(&self, at: TextOffset) -> MapUnitId {
        work::seek();
        MapUnitId::new(
            self.map
                .units
                .as_slice()
                .partition_point(|unit| unit.start() < at),
        )
    }

    /// Returns `unit`'s end in the content's text: the next unit's start.
    fn unit_end(&self, unit: MapUnitId) -> TextOffset {
        self.unit(MapUnitId::new(unit.get() + 1))
            .map_or(self.end, MapUnit::start)
    }

    /// Returns the node `unit` belongs to, by binary search: the last node
    /// whose first unit is at or before it, or else the block.
    fn node(&self, unit: MapUnitId) -> NodeId {
        self.map
            .first_unit
            .last_where(|&first| first <= unit)
            .unwrap_or(NodeId::BLOCK)
    }

    /// Returns `node`'s units: from its first to the next node's first.
    fn node_units(&self, node: NodeId) -> Range<MapUnitId> {
        let firsts = &self.map.first_unit;
        let start = firsts
            .get(node)
            .copied()
            .unwrap_or(self.map.units.next_id());
        let end = firsts
            .get(NodeId::new(node.get() + 1))
            .copied()
            .unwrap_or(self.map.units.next_id());
        start..end.max(start)
    }

    /// Returns the key a node's units answer to: the node's own, except
    /// that the `::first-letter` box answers to the text node it was cut
    /// from.
    fn key(&self, node: NodeId) -> NodeKey {
        match (self.nodes.kind(node), self.map.letter_key) {
            (Some(NodeKind::FirstLetter), Some(key)) => key,
            _ => self.nodes.key(node),
        }
    }

    /// Maps content offset `at` to a node's input text: the key of the node
    /// whose text holds it, and the offset there.
    ///
    /// `side` chooses where collapsing or generated text makes several
    /// positions one. Returns `None` where the text is empty.
    pub(crate) fn source_position(&self, at: TextOffset, side: MapSide) -> Option<(NodeKey, u32)> {
        let first = self.first_from(at);
        let before = first.get().checked_sub(1).map(MapUnitId::new);
        // Strictly inside the unit before.
        if let Some(inside) = before
            && self.unit_end(inside) > at
        {
            let unit = self.unit(inside)?;
            let offset = match unit.kind() {
                MapUnitKind::Identity => unit.source + index_to_u32(at.get() - unit.start().get()),
                _ => match side {
                    MapSide::Before => unit.source,
                    MapSide::After => unit.source_end,
                },
            };
            return Some((self.key(self.node(inside)), offset));
        }
        // At a boundary: find the units collapsed to nothing here and the
        // one starting here.
        let (mut empty_first, mut empty_last, mut starting) = (None, None, None);
        let mut next = first;
        while let Some(unit) = self.unit(next) {
            work::step();
            if unit.start() != at {
                break;
            }
            if self.unit_end(next) != at {
                starting = Some(next);
                break;
            }
            empty_first.get_or_insert(next);
            empty_last = Some(next);
            next = MapUnitId::new(next.get() + 1);
        }
        let (unit, offset) = side.pick(
            [before, empty_first, empty_last, starting],
            |unit| self.unit(unit).map(|found| (unit, found.source)),
            |unit| self.unit(unit).map(|found| (unit, found.source_end)),
        )?;
        Some((self.key(self.node(unit)), offset))
    }

    /// Maps `offset` in the input text of the node keyed `key` to the
    /// content's text.
    ///
    /// `side` chooses where the offset collapsed or sits beside generated
    /// text. A key shared by several nodes answers for the first whose text
    /// holds the offset; an offset past all their text maps to the last
    /// one's end. Returns `None` where no node with the key has text.
    pub(crate) fn content_offset(
        &self,
        key: NodeKey,
        offset: u32,
        side: MapSide,
    ) -> Option<TextOffset> {
        let mut past = None;
        for node in self.map.first_unit.ids() {
            work::step();
            let own = self.node_units(node);
            if own.is_empty() || self.key(node) != key {
                continue;
            }
            if let Some(found) = self.node_text_offset(own.clone(), offset, side) {
                return Some(found);
            }
            past = Some(self.unit_end(MapUnitId::new(own.end.get() - 1)));
        }
        past
    }

    /// Maps `offset` in the node text that units `own` span, or returns
    /// `None` past their text.
    fn node_text_offset(
        &self,
        own: Range<MapUnitId>,
        offset: u32,
        side: MapSide,
    ) -> Option<TextOffset> {
        let slice = self.map.units.get_slice(own.clone())?;
        // Skip the units ending before the offset by binary search.
        work::seek();
        let skipped = slice.partition_point(|unit| unit.source_end < offset);
        let (mut ending, mut empty_first, mut empty_last, mut starting) = (None, None, None, None);
        let mut next = MapUnitId::new(own.start.get() + skipped);
        while next < own.end {
            work::step();
            let unit = self.unit(next)?;
            if unit.source > offset {
                break;
            }
            if unit.source < offset && offset < unit.source_end {
                let start = unit.start();
                let found = match unit.kind() {
                    MapUnitKind::Identity => {
                        TextOffset::new(start.get() + u32_to_index(offset - unit.source))
                    }
                    MapUnitKind::Collapsed => start,
                    MapUnitKind::Variable | MapUnitKind::Generated => match side {
                        MapSide::Before => start,
                        MapSide::After => self.unit_end(next),
                    },
                };
                return Some(found);
            }
            if unit.source == unit.source_end {
                empty_first.get_or_insert(next);
                empty_last = Some(next);
            } else if unit.source_end == offset {
                ending = Some(next);
            } else {
                starting = Some(next);
                break;
            }
            next = MapUnitId::new(next.get() + 1);
        }
        side.pick(
            [ending, empty_first, empty_last, starting],
            |unit| self.unit(unit).map(MapUnit::start),
            |unit| Some(self.unit_end(unit)),
        )
    }

    /// Returns the range of the length-changed character that content
    /// offset `at` is strictly inside, if any. Such a character is one
    /// caret stop.
    pub(crate) fn variable_around(&self, at: TextOffset) -> Option<Range<TextOffset>> {
        let inside = MapUnitId::new(self.first_from(at).get().checked_sub(1)?);
        let unit = self.unit(inside)?;
        let end = self.unit_end(inside);
        (unit.kind() == MapUnitKind::Variable && end > at).then(|| unit.start()..end)
    }
}

#[cfg(test)]
mod listing {
    use alloc::vec::Vec;
    use core::ops::Range;

    use super::{Map, MapUnitKind};

    /// A unit as the tests see it: its kind, its range in the content's
    /// text, and its range in its node's text.
    type Listed = (MapUnitKind, Range<usize>, Range<u32>);

    impl Map {
        /// Returns every unit, over a text of `text_len` bytes.
        pub(crate) fn listed(&self, text_len: usize) -> Vec<Listed> {
            let units = self.units.as_slice();
            units
                .iter()
                .enumerate()
                .map(|(at, unit)| {
                    let end = units
                        .get(at + 1)
                        .map_or(text_len, |next| next.start().get());
                    (
                        unit.kind(),
                        unit.start().get()..end,
                        unit.source..unit.source_end,
                    )
                })
                .collect()
        }
    }
}
