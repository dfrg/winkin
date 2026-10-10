//! Bidi levels for analysis: each paragraph's input, its resolution, and
//! where its clusters' levels change.
//!
//! The algorithm lives in `unicode::bidi`. This module builds its input and
//! records its answer, and the runs are split where the levels change.
//!
//! [`bidi_units`] builds one array of [`BidiUnit`]s per paragraph. Each unit
//! is a cluster or a control a style synthesized. The levels go back to the
//! clusters in order, skipping the controls, so no second array is kept in
//! step.
//!
//! - **A cluster** is one unit, of its first character's class. Two
//!   exceptions keep it to the per-character answer, which
//!   `unicode/tests/bidi_conformance.rs` checks. Whitespace or a separator carrying a mark
//!   resolves as the pair would ([`cluster_bidi_class`]). Upright text in a
//!   vertical line, and text `text-combine-upright: all` combines, is left to
//!   right with no brackets.
//! - **Controls come from styles**, never from the text, as Blink injects
//!   them (`InlineItemsBuilder::EnterInline`). The block's own override opens
//!   every paragraph, as Blink's `EnterBlock` pushes it. A ruby container
//!   adds its `unicode-bidi`'s controls, as a box does. A ruby annotation is
//!   a first-strong isolate, so it neither moves nor is moved by its base,
//!   and its own `unicode-bidi` adds controls inside it. A float is a
//!   neutral, as Blink resolves its U+FFFC. The block's initial letter is
//!   an isolate, so it resolves as the neutral U+FFFC Blink sets it as.
//! - **A paragraph re-opens** the controls of every box open across its
//!   start, as Blink does around a forced break (CSS Writing Modes 3, section
//!   2.4.3). [`track`] keeps the open boxes as the writer walks the items
//!   ([`BidiInput::walk`]), so a paragraph never walks up its ancestors.
//! - **A bidi paragraph may end inside one**, after U+001C, U+001D or
//!   U+001E, which are class B but break no line. What follows is resolved
//!   on its own ([`BidiInput::resolve_units`]), with the style controls
//!   re-opened (CSS Writing Modes 4, section 2.4.4).
//!
//! **Fast paths** skip the resolver. A paragraph with no right-to-left
//! character and no control, in a block that is not right to left, is not
//! walked: every cluster is at level 0. A paragraph whose only controls are
//! left-to-right and first-strong isolates gets its levels from them
//! ([`BidiInput::isolates_alone`]). A paragraph whose clusters all sit at
//! its own level records no changes.
//!
//! **Nothing here panics.** The resolver takes any nesting depth, keeping to
//! UAX #9's 125 levels, and any balance of controls. It refuses only input
//! this module cannot build, and a refused paragraph is set at its own level.

use alloc::vec::Vec;
use core::ops::Range;

use super::{Analysis, BidiLevel, ClusterId, Clusters, ItemClusters, ParagraphFlags};
use crate::data::{HeapBytes, Id, IdRange};
use crate::stages::content::{
    BoxFacts, Content, Item, ItemId, ItemKind, NodeId, NodeKind, TextFlags,
};
use crate::style::{Direction, FirstLineVariant, UnicodeBidi};
use crate::unicode::{
    BIDI_MASK, BidiBracket, BidiClass, BidiScratch, Unit, cluster_bidi_class, resolve_bidi,
};
use crate::{unicode, work};

/// One unit of a paragraph's bidi input: a class, and whether it is a
/// control a style synthesized rather than a cluster.
///
/// Clusters arrive in order, one unit each, so a cluster's level goes back
/// to it by counting the cluster units before it. A unit only needs to say
/// which of the two it is, so it takes two bytes. The class cannot say it,
/// since the text may hold bidi controls of its own, which are clusters.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct BidiUnit {
    class: BidiClass,
    /// The unit is a control a style synthesized, whose level goes nowhere;
    /// if not, it is the next cluster, whose level it has.
    control: bool,
}

impl Unit for BidiUnit {
    #[inline]
    fn class(self) -> BidiClass {
        self.class
    }
}

/// The level of the bidi controls a box's open or close item adds, where
/// they sit below the clusters on both sides.
///
/// Blink reorders a line with a level-only item for each control
/// (`LogicalLineBuilder::HandleItemResults`). A control at or above the
/// lower of its neighbours' levels leaves the clusters' visual order as it
/// is. So only these are kept, and line layout reorders with each of them.
/// An example is `R LRI L PDI` at level 0: the LRI at 0 keeps the R at 1
/// and the L at 2 from reversing together.
///
/// An absolutely positioned box's anchor is resolved as a U+FFFC, as
/// Chrome's `InlineNode::SegmentBidiRuns` resolves it. Its level is kept
/// whatever its neighbours' are, since it places the anchor on its line.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ControlLevel {
    /// The open or close item that adds the controls, or the anchor.
    pub(crate) item: ItemId,
    /// The lowest level among its controls that X9 keeps.
    pub(crate) level: BidiLevel,
}

/// The units that a box's open or close item adds as controls, or the one
/// unit of an anchor.
#[derive(Copy, Clone, Debug)]
struct ItemControls {
    item: ItemId,
    /// Its first unit in the paragraph's units.
    start: u32,
    /// The unit past its last.
    end: u32,
    /// Whether it is an absolutely positioned box's anchor, whose level is
    /// kept whatever its neighbours' are.
    anchor: bool,
}

/// The working memory for bidi resolution, kept in the analysis scratch.
///
/// It holds a paragraph's units, brackets and levels, the resolver's
/// scratch, the open boxes and where levels change. Each buffer is cleared,
/// never dropped, so a warm rebuild allocates nothing.
pub(super) struct BidiInput {
    units: Vec<BidiUnit>,
    brackets: Vec<BidiBracket>,
    /// One level per unit, the resolver's answer.
    levels: Vec<u8>,
    /// Where a cluster's level changes, in text order, with the new level.
    ///
    /// Only paragraphs with clusters off their own level record changes,
    /// starting at their first cluster. The runs are split at them as they
    /// are written out ([`scripts::emit`](super::scripts::emit)).
    pub(super) changes: Vec<(ClusterId, BidiLevel)>,
    /// The units of the controls each box's open or close item adds, in
    /// order, for the paragraph being resolved.
    item_controls: Vec<ItemControls>,
    /// The controls below the clusters on both sides, in item order
    /// ([`ControlLevel`]).
    pub(super) control_levels: Vec<ControlLevel>,
    /// The boxes and annotations that open controls and are open where the
    /// writer's walk is, outermost first ([`walk`](Self::walk)).
    open: Vec<NodeId>,
    /// The boxes and annotations that open controls and are open where a
    /// paragraph's own walk is, outermost first.
    ///
    /// It starts as a copy of `open` and follows the paragraph's items. A
    /// bidi paragraph ending inside re-opens these after it.
    inside: Vec<NodeId>,
    resolver: BidiScratch,
}

impl BidiInput {
    /// Returns empty buffers, allocating nothing.
    pub(super) fn new() -> Self {
        Self {
            units: Vec::new(),
            brackets: Vec::new(),
            levels: Vec::new(),
            changes: Vec::new(),
            item_controls: Vec::new(),
            control_levels: Vec::new(),
            open: Vec::new(),
            inside: Vec::new(),
            resolver: BidiScratch::new(),
        }
    }

    /// Forgets the last analysis's open boxes and level changes.
    pub(super) fn begin(&mut self) {
        self.open.clear();
        self.changes.clear();
        self.control_levels.clear();
    }

    /// Moves the writer's open boxes over `item`, returning whether one opened
    /// or closed.
    pub(super) fn walk(&mut self, content: &Content, item: &Item) -> bool {
        track(&mut self.open, content, item)
    }

    /// Starts a paragraph where the writer's walk is, returning whether any box
    /// whose controls may move a level is open across its start.
    ///
    /// The paragraph's bidi input re-opens those boxes' controls. The
    /// initial letter's isolate alone moves no level ([`track`]).
    pub(super) fn begin_paragraph(&mut self, content: &Content) -> bool {
        self.inside.clear();
        self.inside.extend_from_slice(&self.open);
        self.open.iter().any(|&open| moves_levels(content, open))
    }

    /// Adds the next cluster's unit, of `class`; returns its class's mask.
    fn cluster(&mut self, class: BidiClass) -> u32 {
        self.units.push(BidiUnit {
            class,
            control: false,
        });
        class.mask()
    }

    /// Adds a control a style synthesized, of `class`; returns its class's
    /// mask.
    fn control(&mut self, class: BidiClass) -> u32 {
        self.units.push(BidiUnit {
            class,
            control: true,
        });
        class.mask()
    }

    /// Resolves the units into `levels`, one per unit, and returns the
    /// paragraph's base level.
    ///
    /// `requested` is the base level the block asks for, or `None` under
    /// `auto`. Returns `None` if the resolver refuses the units, which are
    /// well formed.
    ///
    /// A cluster of class B ends a bidi paragraph (UAX #9 P1). The
    /// paragraph's own separator is its last unit. Any other is U+001C,
    /// U+001D or U+001E, which break no line. What follows one is resolved
    /// apart, with its own base level under `auto` and its own brackets.
    /// Nearly always there is none, and this is one call to the resolver.
    fn resolve_units(&mut self, requested: Option<u8>) -> Option<u8> {
        let Self {
            units,
            brackets,
            levels,
            resolver,
            ..
        } = self;
        levels.clear();
        levels.resize(units.len(), 0);
        let mut first = None;
        let mut from = 0;
        let mut bracket_from = 0;
        while from < units.len() {
            work::step();
            let rest = units.get(from..).unwrap_or_default();
            let to = rest
                .iter()
                .position(|unit| !unit.control && unit.class == BidiClass::PARAGRAPH_SEPARATOR)
                .map_or(units.len(), |at| from + at + 1);
            // Its brackets, counted from its start: they are in order, and
            // those before it were counted from the parts before.
            let (start, end) = (
                u32::try_from(from).unwrap_or(u32::MAX),
                u32::try_from(to).unwrap_or(u32::MAX),
            );
            let mut bracket_to = bracket_from;
            while let Some(bracket) = brackets
                .get_mut(bracket_to)
                .filter(|bracket| bracket.index < end)
            {
                work::step();
                bracket.index = bracket.index.saturating_sub(start);
                bracket_to += 1;
            }
            let base = resolve_bidi(
                resolver,
                units.get(from..to).unwrap_or_default(),
                brackets.get(bracket_from..bracket_to).unwrap_or_default(),
                requested,
                levels.get_mut(from..to).unwrap_or_default(),
            )?;
            first.get_or_insert(base);
            from = to;
            bracket_from = bracket_to;
        }
        Some(first.unwrap_or(requested.unwrap_or(0)))
    }

    /// Writes the resolver's levels into `levels` without the resolver.
    ///
    /// The units are at base level 0, hold nothing right to left, and have
    /// only left-to-right and first-strong isolates as controls. Then every
    /// first-strong isolate opens left to right and every level is even.
    /// Every class resolves to L at its own level: W7 makes each European
    /// number L, and N1 and N2 make every neutral between Ls L.
    ///
    /// So a unit is at the level its isolates give it (X5a to X6a):
    /// - Each isolate open around it adds two.
    /// - An isolate's initiator and its PDI sit at the level outside.
    /// - An isolate that would open past level 125 overflows, as X5a counts.
    ///
    /// Then the half of L1 that needs no line applies. A segment separator or
    /// the paragraph's own separator goes to level 0, with the white space
    /// and isolate controls before it. A boundary neutral, which X9 removed,
    /// takes the level of the unit before it, as the resolver hands it back.
    ///
    /// Returns the base level, 0. Returns `None` if a bidi paragraph ends
    /// before the last unit, after U+001C to U+001E.
    fn isolates_alone(&mut self) -> Option<u8> {
        let Self { units, levels, .. } = self;
        levels.clear();
        levels.resize(units.len(), 0);
        let last = units.len().saturating_sub(1);
        // The embedding level where the walk is, and how many isolates
        // opened past the deepest, which their PDIs close first.
        let mut level = 0_u8;
        let mut overflow = 0_u32;
        for at in 0..units.len() {
            work::step();
            let class = units
                .get(at)
                .map_or(BidiClass::OTHER_NEUTRAL, |unit| unit.class);
            let resolved = match class {
                BidiClass::LEFT_TO_RIGHT_ISOLATE | BidiClass::FIRST_STRONG_ISOLATE => {
                    let outside = level;
                    if overflow == 0 && level + 2 <= MAX_DEPTH {
                        level += 2;
                    } else {
                        overflow += 1;
                    }
                    outside
                }
                BidiClass::POP_DIRECTIONAL_ISOLATE => {
                    if overflow > 0 {
                        overflow -= 1;
                    } else {
                        level = level.saturating_sub(2);
                    }
                    level
                }
                BidiClass::BOUNDARY_NEUTRAL => at
                    .checked_sub(1)
                    .and_then(|before| levels.get(before))
                    .copied()
                    .unwrap_or(0),
                BidiClass::PARAGRAPH_SEPARATOR if at != last => return None,
                BidiClass::PARAGRAPH_SEPARATOR | BidiClass::SEGMENT_SEPARATOR => {
                    for before in (0..at).rev() {
                        work::step();
                        let Some(unit) = units.get(before) else {
                            break;
                        };
                        if unit.class.is_removed_by_x9() {
                            continue;
                        }
                        if !matches!(
                            unit.class,
                            BidiClass::WHITE_SPACE
                                | BidiClass::LEFT_TO_RIGHT_ISOLATE
                                | BidiClass::FIRST_STRONG_ISOLATE
                                | BidiClass::POP_DIRECTIONAL_ISOLATE
                        ) {
                            break;
                        }
                        if let Some(reset) = levels.get_mut(before) {
                            *reset = 0;
                        }
                    }
                    0
                }
                _ => level,
            };
            if let Some(slot) = levels.get_mut(at) {
                *slot = resolved;
            }
        }
        Some(0)
    }
}

impl BidiInput {
    /// Keeps the level of each box's controls that sit below the clusters
    /// on both sides, and of every anchor, in `control_levels`.
    ///
    /// One forward walk over the units: the controls met since the last
    /// cluster are settled at the next cluster. Controls with no cluster
    /// on one side sit at a line's edge or the paragraph's, where no
    /// level splits anything. An anchor is kept wherever it sits.
    fn keep_control_levels(&mut self) {
        let Self {
            units,
            levels,
            item_controls,
            control_levels,
            ..
        } = self;
        // The lowest level among `controls`' units that X9 keeps.
        let lowest = |controls: &ItemControls| {
            let from = controls.start as usize;
            let to = controls.end as usize;
            (units.get(from..to).unwrap_or_default().iter())
                .zip(levels.get(from..to).unwrap_or_default())
                .filter(|(unit, _)| !unit.class.is_removed_by_x9())
                .map(|(_, &level)| level)
                .min()
        };
        let mut rest = item_controls.as_slice();
        let mut left: Option<u8> = None;
        for (at, (unit, &right)) in units.iter().zip(levels.iter()).enumerate() {
            work::step();
            if unit.control {
                continue;
            }
            let at = u32::try_from(at).unwrap_or(u32::MAX);
            while let Some((controls, tail)) = rest.split_first()
                && controls.end <= at
            {
                rest = tail;
                let kept = match left {
                    _ if controls.anchor => true,
                    Some(left) => lowest(controls).is_some_and(|level| level < left.min(right)),
                    None => false,
                };
                if kept && let Some(level) = lowest(controls) {
                    control_levels.push(ControlLevel {
                        item: controls.item,
                        level: BidiLevel::new(level),
                    });
                }
            }
            left = Some(right);
        }
        // The anchors after the last cluster.
        for controls in rest.iter().filter(|controls| controls.anchor) {
            if let Some(level) = lowest(controls) {
                control_levels.push(ControlLevel {
                    item: controls.item,
                    level: BidiLevel::new(level),
                });
            }
        }
    }
}

/// The classes that rule out [`BidiInput::isolates_alone`].
///
/// They are the right-to-left classes, the embeddings and overrides, the
/// PDF that pops them, and the right-to-left isolate.
const NOT_ISOLATES_ALONE: u32 = BidiClass::RIGHT_TO_LEFT.mask()
    | BidiClass::ARABIC_LETTER.mask()
    | BidiClass::ARABIC_NUMBER.mask()
    | BidiClass::LEFT_TO_RIGHT_EMBEDDING.mask()
    | BidiClass::RIGHT_TO_LEFT_EMBEDDING.mask()
    | BidiClass::LEFT_TO_RIGHT_OVERRIDE.mask()
    | BidiClass::RIGHT_TO_LEFT_OVERRIDE.mask()
    | BidiClass::POP_DIRECTIONAL_FORMAT.mask()
    | BidiClass::RIGHT_TO_LEFT_ISOLATE.mask();

/// The deepest level UAX #9 opens (BD2's `max_depth`).
///
/// An isolate that would open past it overflows.
const MAX_DEPTH: u8 = 125;

impl HeapBytes for BidiInput {
    /// Counts its buffers and the resolver's.
    ///
    /// The resolver counts its own by a method, since it is compiled alone
    /// for its conformance tests and does not know this crate's trait.
    fn heap_bytes(&self) -> usize {
        let Self {
            units,
            brackets,
            levels,
            changes,
            item_controls,
            control_levels,
            open,
            inside,
            resolver,
        } = self;
        units.heap_bytes()
            + brackets.heap_bytes()
            + levels.heap_bytes()
            + changes.heap_bytes()
            + item_controls.heap_bytes()
            + control_levels.heap_bytes()
            + open.heap_bytes()
            + inside.heap_bytes()
            + resolver.heap_bytes()
    }
}

impl Default for BidiInput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl BidiInput {
    /// Sets a paragraph of one cluster per class in `classes`, so a test can
    /// compare both ways of finding its levels.
    pub(super) fn set_classes(&mut self, classes: &[BidiClass]) {
        self.units.clear();
        self.brackets.clear();
        for &class in classes {
            self.cluster(class);
        }
    }

    /// Returns the paragraph's levels from its isolates alone, if they
    /// answer.
    pub(super) fn levels_alone(&mut self) -> Option<Vec<u8>> {
        self.isolates_alone().map(|_| self.levels.clone())
    }

    /// Returns the paragraph's base level and levels from the resolver, at
    /// the base level `requested` asks.
    pub(super) fn levels_resolved(&mut self, requested: Option<u8>) -> Option<(u8, Vec<u8>)> {
        self.resolve_units(requested)
            .map(|base| (base, self.levels.clone()))
    }
}

/// Resolves the paragraph over `range` of the clusters and returns its base
/// level and flags.
///
/// - `first_item` holds the paragraph's first cluster.
/// - `requested` is the level the block's direction sets, or `None` under
///   `auto`.
/// - `may_differ` says whether the paragraph holds anything right to left
///   or a control, which can take a cluster off its level.
///
/// If some cluster's level differs from the paragraph's, this records the
/// changes in `input` and sets `MIXED_LEVELS`. It sets `RIGHT_TO_LEFT` if
/// the paragraph or any cluster reads right to left.
pub(super) fn resolve(
    content: &Content,
    analysis: &Analysis,
    input: &mut BidiInput,
    range: Range<ClusterId>,
    first_item: ItemId,
    requested: Option<BidiLevel>,
    may_differ: bool,
) -> (BidiLevel, ParagraphFlags) {
    // A paragraph at `base` whose clusters all are.
    let own = |base: u8| {
        let level = BidiLevel::new(base);
        let flags = if level.is_rtl() {
            ParagraphFlags::RIGHT_TO_LEFT
        } else {
            ParagraphFlags::NONE
        };
        (level, flags)
    };
    let requested = requested.map(BidiLevel::get);
    // Nothing to resolve: its own level, which with nothing right to left
    // in it is 0 under `auto` too.
    let base = requested.unwrap_or(0);
    if !(may_differ || requested == Some(1)) || range.is_empty() {
        return own(base);
    }
    let mask = bidi_units(
        content,
        &analysis.clusters,
        &analysis.item_clusters,
        range.clone(),
        first_item,
        input,
    );
    if requested != Some(1) && mask & BIDI_MASK == 0 {
        return own(0);
    }
    // Nothing right to left and no control but left-to-right isolates, as
    // ruby's annotations and `<bdi>` make them: the isolates alone give the
    // levels, as the resolver would, without its scratch.
    let alone = if requested != Some(1) && mask & NOT_ISOLATES_ALONE == 0 {
        input.isolates_alone()
    } else {
        None
    };
    let Some(base) = alone.or_else(|| input.resolve_units(requested)) else {
        debug_assert!(false, "the units are well formed");
        return own(base);
    };
    let BidiInput {
        units,
        levels: resolved,
        changes,
        ..
    } = input;
    let differs = units
        .iter()
        .zip(resolved.iter())
        .any(|(unit, &level)| !unit.control && level != base);
    let (level, mut flags) = own(base);
    if differs {
        flags.insert(ParagraphFlags::MIXED_LEVELS);
        // The clusters' units are in cluster order, one a cluster, so the
        // levels go back to them by order: a change at the first, and at
        // each whose level is not the one before's.
        let mut cluster = range.start;
        let mut current = None;
        for (unit, &level) in units.iter().zip(resolved.iter()) {
            if unit.control {
                continue;
            }
            let level = BidiLevel::new(level);
            if current != Some(level) {
                changes.push((cluster, level));
                current = Some(level);
                if level.is_rtl() {
                    flags.insert(ParagraphFlags::RIGHT_TO_LEFT);
                }
            }
            cluster = ClusterId::new(cluster.get() + 1);
        }
        debug_assert_eq!(cluster, range.end, "a unit a cluster, in order");
        if flags.contains(ParagraphFlags::RIGHT_TO_LEFT) {
            input.keep_control_levels();
        }
    }
    (level, flags)
}

/// Builds a paragraph's bidi input into `input` and returns the units'
/// class mask.
///
/// The input is one unit per cluster and per synthesized control, in
/// logical order, plus the paired brackets. The fast path reads the mask
/// built from these same units, so it never misses a synthesized control.
///
/// The walk runs from the item holding the first cluster to the last item
/// holding one. Boxes open across the start are re-opened from what the
/// writer found open. The items after the separator belong to the next
/// paragraph. The writer is still writing clusters, and those written so far
/// end at this paragraph's end.
fn bidi_units(
    content: &Content,
    clusters: &Clusters,
    item_clusters: &ItemClusters,
    range: Range<ClusterId>,
    first_item: ItemId,
    input: &mut BidiInput,
) -> u32 {
    input.units.clear();
    input.brackets.clear();
    input.item_controls.clear();
    let text = &content.text;
    let nodes = &content.nodes;
    let items = &content.items;
    let count = clusters.end_id();
    // The boxes open across the paragraph's start, outermost first, as the
    // writer found them (`BidiInput::begin_paragraph`). They are the ancestors
    // of the first cluster's node, and the node itself if it is a box: a
    // `<wbr>` the builder made between two nodes is a text item of the box
    // around it. Only boxes that open controls are kept, so the cost is
    // linear in the items and controls, however deep the boxes nest.
    let mut mask = reopen(input, content);

    // Its items and clusters.
    for id in (first_item..items.next_id()).ids() {
        work::step();
        let Some(item) = items.get(id) else {
            break;
        };
        let Range {
            start: first,
            end: next,
        } = item_clusters.range_to(id, count);
        if first >= range.end {
            break;
        }
        if first < next {
            // Text set upright, and text combined whole, which is set as one
            // upright character. A run of combined digits resolves at one
            // level, as digits anywhere do.
            let facts = nodes.text_facts(item.node, FirstLineVariant::Standard);
            let upright = content.facts.text(facts).has(TextFlags::UPRIGHT_BIDI);
            for cluster in (first.max(range.start)..next.min(range.end)).ids() {
                let unit = cluster_unit(input, text, clusters, cluster, upright);
                mask |= unit;
                // A bidi paragraph ends here, inside this one: the next
                // starts with the controls re-opened, as this one did.
                if unit == BidiClass::PARAGRAPH_SEPARATOR.mask()
                    && cluster.get() + 1 < range.end.get()
                {
                    mask |= reopen(input, content);
                }
            }
            continue;
        }
        let node = item.node;
        let start = input.units.len();
        match item.kind {
            kind if kind.is_open() => mask |= enter(input, content, node),
            kind if kind.is_close() => mask |= exit(input, content, node),
            ItemKind::Float | ItemKind::Absolute => {
                mask |= input.control(BidiClass::OTHER_NEUTRAL);
            }
            _ => {}
        }
        let anchor = item.kind == ItemKind::Absolute;
        if (anchor || matches!(item.kind, ItemKind::Open | ItemKind::Close))
            && start < input.units.len()
            && let (Ok(start), Ok(end)) = (u32::try_from(start), u32::try_from(input.units.len()))
        {
            input.item_controls.push(ItemControls {
                item: id,
                start,
                end,
                anchor,
            });
        }
        track(&mut input.inside, content, item);
    }
    mask
}

/// Moves `open` over `item`, returning whether a box, a ruby container or
/// an annotation opened or closed whose controls may move a level.
///
/// `open` lists the open containers that open controls, outermost first.
/// The writer tracks them over every item, and each paragraph over its own
/// items. Only what opens controls is kept, so a `::first-letter` box
/// ([`opens_controls`]) is never among them unless it is the initial
/// letter.
///
/// Controls that move no level by themselves ([`moves_levels`]) alone do
/// not have the paragraph resolved.
fn track(open: &mut Vec<NodeId>, content: &Content, item: &Item) -> bool {
    let node = item.node;
    let moves = moves_levels(content, node);
    match item.kind {
        kind if kind.is_open() && opens_controls(content, node) => {
            open.push(node);
            moves
        }
        kind if kind.is_close() && open.last() == Some(&node) => {
            open.pop();
            moves
        }
        _ => false,
    }
}

/// Adds the controls a bidi paragraph starts with and returns their class
/// mask.
///
/// They are the block's own override and the controls of the boxes open
/// at the start (`input.inside`), outermost first. This runs at a
/// paragraph's start and after U+001C to U+001E inside one, as CSS Writing
/// Modes 4, section 2.4.4 re-opens what a bidi paragraph boundary closed.
fn reopen(input: &mut BidiInput, content: &Content) -> u32 {
    let mut mask = 0;
    if let Some(direction) = content.block.overrides {
        mask |= input.control(override_class(direction));
    }
    for at in 0..input.inside.len() {
        work::step();
        let Some(&open) = input.inside.get(at) else {
            continue;
        };
        mask |= enter(input, content, open);
    }
    mask
}

/// Adds the unit of `cluster`, and its bracket if it is one, returning its
/// class mask.
fn cluster_unit(
    input: &mut BidiInput,
    text: &str,
    clusters: &Clusters,
    cluster: ClusterId,
    upright: bool,
) -> u32 {
    let range = clusters.range(cluster);
    let written = text
        .get(range.start.get()..range.end.get())
        .unwrap_or_default();
    let mut chars = written.chars();
    let Some(first) = chars.next() else {
        return input.cluster(BidiClass::OTHER_NEUTRAL);
    };
    if upright {
        return input.cluster(BidiClass::LEFT_TO_RIGHT);
    }
    let props = unicode::core_props(first);
    let class = cluster_bidi_class(
        props.bidi_class(),
        chars.map(|ch| unicode::core_props(ch).bidi_class()),
    );
    // A paired bracket, for rule N0: both ends name the pair's closing
    // character, canonically, which is what N0 matches them by.
    if let Some(bracket) = u32::try_from(input.units.len())
        .ok()
        .and_then(|index| props.bidi_bracket(index))
    {
        input.brackets.push(bracket);
    }
    input.cluster(class)
}

/// An override in `direction`: LRO or RLO.
fn override_class(direction: Direction) -> BidiClass {
    match direction {
        Direction::Ltr => BidiClass::LEFT_TO_RIGHT_OVERRIDE,
        Direction::Rtl => BidiClass::RIGHT_TO_LEFT_OVERRIDE,
    }
}

/// Whether `node` opens controls: the block's initial letter, an
/// annotation, or a box or ruby container whose `unicode-bidi` is not
/// `normal`. [`enter`] adds those controls.
///
/// Not another `::first-letter` box: Blink's cascade drops `unicode-bidi`
/// and `direction` for the pseudo and injects nothing for it, and the
/// content's writer gives its style `normal` and its parent's direction to
/// match.
fn opens_controls(content: &Content, node: NodeId) -> bool {
    if content.block.initial_letter == Some(node) {
        return true;
    }
    match content.nodes.kind(node) {
        Some(NodeKind::Annotation) => true,
        Some(NodeKind::Box | NodeKind::Ruby) => {
            box_facts(content, node).bidi != UnicodeBidi::Normal
        }
        _ => false,
    }
}

/// Whether the controls `node` opens may move a level by themselves: put a
/// cluster at another level than 0 in a paragraph with nothing right to
/// left, in a block that is not.
///
/// Left-to-right embeddings, overrides and isolates, and first-strong
/// isolates with nothing right to left to find, raise even levels over even
/// ones. Blink then finds the paragraph unidirectional and leaves every
/// item at level 0 (`InlineNode::SegmentBidiRuns`), and so does a paragraph
/// they alone are in. The initial letter's isolate is one of them.
/// Right-to-left controls may move a level, and an annotation's isolate
/// raises its text, which splits the runs at its edges.
fn moves_levels(content: &Content, node: NodeId) -> bool {
    if content.block.initial_letter == Some(node) {
        return false;
    }
    match content.nodes.kind(node) {
        Some(NodeKind::Annotation) => true,
        Some(NodeKind::Box | NodeKind::Ruby) => {
            let facts = box_facts(content, node);
            match facts.bidi {
                UnicodeBidi::Normal | UnicodeBidi::Plaintext => false,
                UnicodeBidi::Embed
                | UnicodeBidi::BidiOverride
                | UnicodeBidi::Isolate
                | UnicodeBidi::IsolateOverride => facts.direction() == Direction::Rtl,
            }
        }
        _ => false,
    }
}

/// `node`'s box facts, which say what controls it opens.
fn box_facts(content: &Content, node: NodeId) -> &BoxFacts {
    let id = content.nodes.box_facts(node, FirstLineVariant::Standard);
    content.facts.box_facts(id)
}

/// Adds the controls that open `node`, as Blink injects them, returning
/// their class mask.
///
/// A box or a ruby container adds those of its `unicode-bidi`. An
/// annotation is a first-strong isolate, inside which its own
/// `unicode-bidi` adds its controls, so `dir` on it sets its direction.
///
/// The block's initial letter is an isolate in its own direction. Blink
/// sets its box as one U+FFFC, a neutral, and lays its text out apart
/// (`AppendOpaque` of `kInitialLetterBox`). So the box stands at the line's
/// start in either direction.
fn enter(input: &mut BidiInput, content: &Content, node: NodeId) -> u32 {
    if content.block.initial_letter == Some(node) {
        let facts = box_facts(content, node);
        let isolate = match facts.direction() {
            Direction::Ltr => BidiClass::LEFT_TO_RIGHT_ISOLATE,
            Direction::Rtl => BidiClass::RIGHT_TO_LEFT_ISOLATE,
        };
        return input.control(isolate) | enter_container(input, facts);
    }
    match content.nodes.kind(node) {
        Some(NodeKind::Annotation) => {
            input.control(BidiClass::FIRST_STRONG_ISOLATE)
                | enter_container(input, box_facts(content, node))
        }
        Some(NodeKind::Box | NodeKind::Ruby) => enter_container(input, box_facts(content, node)),
        _ => 0,
    }
}

/// Adds the controls that open a container of `facts`'s `unicode-bidi`
/// and direction, returning their class mask.
fn enter_container(input: &mut BidiInput, facts: &BoxFacts) -> u32 {
    let direction = facts.direction();
    let pick = |ltr, rtl| {
        if direction == Direction::Rtl {
            rtl
        } else {
            ltr
        }
    };
    match facts.bidi {
        UnicodeBidi::Normal => 0,
        UnicodeBidi::Embed => input.control(pick(
            BidiClass::LEFT_TO_RIGHT_EMBEDDING,
            BidiClass::RIGHT_TO_LEFT_EMBEDDING,
        )),
        UnicodeBidi::BidiOverride => input.control(override_class(direction)),
        UnicodeBidi::Isolate => input.control(pick(
            BidiClass::LEFT_TO_RIGHT_ISOLATE,
            BidiClass::RIGHT_TO_LEFT_ISOLATE,
        )),
        UnicodeBidi::Plaintext => input.control(BidiClass::FIRST_STRONG_ISOLATE),
        // An isolate of the first strong direction outside, and an
        // override inside, as Blink injects it.
        UnicodeBidi::IsolateOverride => {
            input.control(BidiClass::FIRST_STRONG_ISOLATE)
                | input.control(override_class(direction))
        }
    }
}

/// Adds the controls that close `node`, matching [`enter`], returning their
/// class mask.
fn exit(input: &mut BidiInput, content: &Content, node: NodeId) -> u32 {
    if content.block.initial_letter == Some(node) {
        return exit_container(input, box_facts(content, node))
            | input.control(BidiClass::POP_DIRECTIONAL_ISOLATE);
    }
    match content.nodes.kind(node) {
        Some(NodeKind::Annotation) => {
            exit_container(input, box_facts(content, node))
                | input.control(BidiClass::POP_DIRECTIONAL_ISOLATE)
        }
        Some(NodeKind::Box | NodeKind::Ruby) => exit_container(input, box_facts(content, node)),
        _ => 0,
    }
}

/// Adds the controls that close a container of `facts`'s `unicode-bidi`,
/// matching [`enter_container`], returning their class mask.
fn exit_container(input: &mut BidiInput, facts: &BoxFacts) -> u32 {
    match facts.bidi {
        UnicodeBidi::Normal => 0,
        UnicodeBidi::Embed | UnicodeBidi::BidiOverride => {
            input.control(BidiClass::POP_DIRECTIONAL_FORMAT)
        }
        UnicodeBidi::Isolate | UnicodeBidi::Plaintext => {
            input.control(BidiClass::POP_DIRECTIONAL_ISOLATE)
        }
        UnicodeBidi::IsolateOverride => {
            input.control(BidiClass::POP_DIRECTIONAL_FORMAT)
                | input.control(BidiClass::POP_DIRECTIONAL_ISOLATE)
        }
    }
}
