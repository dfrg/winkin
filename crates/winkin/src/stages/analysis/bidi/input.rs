//! A paragraph's bidi input: its units, brackets and splits, the boxes open
//! across it, and the walk that builds them.

use alloc::vec::Vec;
use core::ops::Range;

use super::ControlLevel;
use super::resolver::{BidiScratch, BidiUnit};
use crate::data::{HeapBytes, Id, IdRange};
use crate::stages::analysis::{BidiLevel, ClusterId, Clusters, ItemClusters};
use crate::stages::content::{
    BoxFacts, Content, Item, ItemId, ItemKind, NodeId, NodeKind, TextFlags,
};
use crate::style::{Direction, FirstLineVariant, UnicodeBidi};
use crate::unicode::{BidiBracket, BidiClass, cluster_bidi_class};
use crate::{unicode, work};

/// The control units a box's open or close item adds, or an anchor's one
/// unit.
#[derive(Copy, Clone, Debug)]
pub(super) struct ItemControls {
    pub(super) item: ItemId,
    /// The index of its first unit in the paragraph's units.
    pub(super) start: u32,
    /// The index past its last unit.
    pub(super) end: u32,
    /// Whether it is an absolutely positioned box's anchor, whose level is
    /// always kept.
    pub(super) anchor: bool,
}

/// The working memory for bidi resolution, kept in the analysis scratch.
///
/// Holds a paragraph's units, brackets and levels, the resolver's scratch,
/// the open boxes, and where levels change. Buffers are cleared, never
/// dropped, so a warm rebuild allocates nothing.
pub(crate) struct BidiInput {
    pub(super) units: Vec<BidiUnit>,
    pub(super) brackets: Vec<BidiBracket>,
    /// The unit index where each bidi paragraph after the first starts:
    /// after U+001C, U+001D or U+001E, which are class B but end no line.
    pub(super) splits: Vec<u32>,
    /// The resolved level of each unit.
    pub(super) levels: Vec<u8>,
    /// Each cluster where the level changes, with the new level, in text
    /// order.
    ///
    /// Only paragraphs with clusters off their own level record changes,
    /// starting at their first cluster.
    /// [`scripts::emit`](super::super::scripts::emit) splits the runs at
    /// them.
    pub(crate) changes: Vec<(ClusterId, BidiLevel)>,
    /// The control units of each box's open or close item, and each anchor's
    /// unit, in the paragraph being resolved, in order.
    pub(super) item_controls: Vec<ItemControls>,
    /// The levels of box controls below the clusters on both sides, and of
    /// every anchor, in item order ([`ControlLevel`]).
    pub(crate) control_levels: Vec<ControlLevel>,
    /// The boxes and annotations that open controls and are open at the
    /// writer's position, outermost first ([`walk`](Self::walk)).
    open: Vec<NodeId>,
    /// The boxes and annotations that open controls and are open at the
    /// paragraph walk's position, outermost first.
    ///
    /// Starts as a copy of `open` and follows the paragraph's items. A bidi
    /// paragraph ending inside re-opens these after it.
    inside: Vec<NodeId>,
    pub(super) resolver: BidiScratch,
}

impl BidiInput {
    /// Returns empty buffers, allocating nothing.
    pub(crate) fn new() -> Self {
        Self {
            units: Vec::new(),
            brackets: Vec::new(),
            splits: Vec::new(),
            levels: Vec::new(),
            changes: Vec::new(),
            item_controls: Vec::new(),
            control_levels: Vec::new(),
            open: Vec::new(),
            inside: Vec::new(),
            resolver: BidiScratch::new(),
        }
    }

    /// Forgets the last analysis's open boxes, level changes and control
    /// levels.
    pub(crate) fn begin(&mut self) {
        self.open.clear();
        self.changes.clear();
        self.control_levels.clear();
    }

    /// Moves the writer's open boxes over `item`, whose node has box facts
    /// `facts`, and returns whether a box whose controls may move a level
    /// opened or closed.
    pub(crate) fn walk(&mut self, content: &Content, item: &Item, facts: &BoxFacts) -> bool {
        track(&mut self.open, content, item, facts)
    }

    /// Starts a paragraph at the writer's position and returns whether a box
    /// whose controls may move a level is open across its start.
    ///
    /// The paragraph's bidi input re-opens those boxes' controls. The
    /// initial letter's isolate alone moves no level ([`controls`]).
    pub(crate) fn begin_paragraph(&mut self, content: &Content) -> bool {
        self.inside.clear();
        self.inside.extend_from_slice(&self.open);
        self.open
            .iter()
            .any(|&open| controls(content, open, box_facts(content, open)) == Some(true))
    }

    /// Adds the next cluster's unit, of `class`, and returns the class mask.
    fn cluster(&mut self, class: BidiClass) -> u32 {
        self.units.push(BidiUnit {
            class,
            control: false,
        });
        class.mask()
    }

    /// Adds a control a style synthesized, of `class`, and returns the class
    /// mask.
    fn control(&mut self, class: BidiClass) -> u32 {
        self.units.push(BidiUnit {
            class,
            control: true,
        });
        class.mask()
    }
}

impl HeapBytes for BidiInput {
    /// Counts its buffers and the resolver's.
    fn heap_bytes(&self) -> usize {
        let Self {
            units,
            brackets,
            splits,
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
            + splits.heap_bytes()
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
    /// Sets a paragraph of one cluster per class in `classes`, splitting a
    /// bidi paragraph after each B.
    pub(crate) fn set_classes(&mut self, classes: &[BidiClass]) {
        self.units.clear();
        self.brackets.clear();
        self.splits.clear();
        for &class in classes {
            self.cluster(class);
            if class == BidiClass::PARAGRAPH_SEPARATOR {
                self.splits.push(self.units.len() as u32);
            }
        }
        if self.splits.last() == Some(&(self.units.len() as u32)) {
            self.splits.pop();
        }
    }

    /// Returns the paragraph's base level and unit levels from the resolver,
    /// with `requested` as the requested base level.
    pub(crate) fn levels_resolved(&mut self, requested: Option<u8>) -> (u8, Vec<u8>) {
        let mask = self
            .units
            .iter()
            .fold(0, |mask, unit| mask | unit.class.mask());
        let (base, _) = self.resolve_units(requested, mask, ClusterId::new(0));
        self.changes.clear();
        (base.get(), self.levels.clone())
    }
}

/// Builds a paragraph's bidi input into `input` and returns the units'
/// class mask.
///
/// The input is one unit per cluster and per synthesized control, in
/// logical order, plus the paired brackets. The fast path reads this mask,
/// so it never misses a synthesized control.
///
/// The walk runs from `first_item` to the last item with a cluster in
/// `range`; items after the separator belong to the next paragraph. Boxes
/// open across the start are re-opened from the writer's open boxes.
/// `range` ends at the last cluster written so far.
pub(super) fn bidi_units(
    content: &Content,
    clusters: &Clusters,
    item_clusters: &ItemClusters,
    range: Range<ClusterId>,
    first_item: ItemId,
    input: &mut BidiInput,
) -> u32 {
    input.units.clear();
    input.brackets.clear();
    input.splits.clear();
    input.item_controls.clear();
    let text = &content.text;
    let nodes = &content.nodes;
    let items = &content.items;
    let count = clusters.end_id();
    // The boxes open across the start, outermost first, as the writer found
    // them (`BidiInput::begin_paragraph`). They are the first cluster's
    // node's ancestors, and the node itself if it is a box: a `<wbr>` the
    // builder made between two nodes is a text item of the box around it.
    // Only boxes that open controls are kept, so the cost is linear in the
    // items and controls however deep the boxes nest.
    let mut mask = reopen(input, content);
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
            // Upright text, and text combined whole into one upright
            // character, is L. Combined digits resolve at one level, as
            // digits anywhere do.
            let facts = nodes.text_facts(item.node, FirstLineVariant::Standard);
            let upright = content.facts.text(facts).has(TextFlags::UPRIGHT_BIDI);
            for cluster in (first.max(range.start)..next.min(range.end)).ids() {
                let unit = cluster_unit(input, text, clusters, cluster, upright);
                mask |= unit;
                // A bidi paragraph ends inside this one. The next starts
                // with the controls re-opened.
                if unit == BidiClass::PARAGRAPH_SEPARATOR.mask()
                    && cluster.get() + 1 < range.end.get()
                {
                    input.splits.push(input.units.len() as u32);
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
        track(&mut input.inside, content, item, box_facts(content, node));
    }
    mask
}

/// Moves `open` over `item` and returns whether a box, ruby container or
/// annotation whose controls may move a level opened or closed.
///
/// `open` lists the open containers that open controls, outermost first; a
/// `::first-letter` box is among them only as the initial letter
/// ([`controls`]). `facts` are the box facts of the item's node. The writer
/// tracks every item, and each paragraph its own items.
fn track(open: &mut Vec<NodeId>, content: &Content, item: &Item, facts: &BoxFacts) -> bool {
    let node = item.node;
    match item.kind {
        kind if kind.is_open() => {
            let Some(moves) = controls(content, node, facts) else {
                return false;
            };
            open.push(node);
            moves
        }
        kind if kind.is_close() && open.last() == Some(&node) => {
            open.pop();
            controls(content, node, facts).unwrap_or(false)
        }
        _ => false,
    }
}

/// Adds the controls a bidi paragraph starts with and returns their class
/// mask.
///
/// These are the block's override and the controls of the boxes open at the
/// start (`input.inside`), outermost first. Runs at a paragraph's start and
/// after U+001C to U+001E inside one, as CSS Writing Modes 4, section 2.4.4
/// re-opens what a bidi paragraph boundary closed.
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

/// Adds the unit of `cluster`, and its bracket if it is one, and returns
/// its class mask.
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
    // A paired bracket for rule N0. Both ends name the pair's canonical
    // closing character, which N0 matches them by. Every paired bracket in
    // BidiBrackets.txt is ON.
    if class == BidiClass::OTHER_NEUTRAL
        && let Some(closing) = props.closing_bracket()
    {
        input.brackets.push(BidiBracket {
            index: input.units.len() as u32,
            closing,
            is_open: props.opens_bracket(),
        });
    }
    input.cluster(class)
}

/// Returns the override class for `direction`: LRO or RLO.
fn override_class(direction: Direction) -> BidiClass {
    match direction {
        Direction::Ltr => BidiClass::LEFT_TO_RIGHT_OVERRIDE,
        Direction::Rtl => BidiClass::RIGHT_TO_LEFT_OVERRIDE,
    }
}

/// Returns whether `node`, of box facts `facts`, may move a level by its
/// controls alone, or `None` if it opens no controls.
///
/// The block's initial letter, an annotation, and a box or ruby container
/// whose `unicode-bidi` is not `normal` open controls, which [`enter`] adds.
/// Another `::first-letter` box does not: Blink's cascade drops
/// `unicode-bidi` and `direction` for the pseudo, and the content's writer
/// gives its style `normal` and its parent's direction to match.
///
/// Controls move a level when they put a cluster off level 0 in a
/// paragraph and block with nothing right to left. Only right-to-left
/// controls do. Left-to-right controls, and first-strong isolates with
/// nothing right to left to find, raise only even levels, so Blink finds
/// the paragraph unidirectional and leaves every item at level 0
/// (`InlineNode::SegmentBidiRuns`). An annotation's isolate and the initial
/// letter's are of this kind.
fn controls(content: &Content, node: NodeId, facts: &BoxFacts) -> Option<bool> {
    if content.block.initial_letter == Some(node) {
        return Some(false);
    }
    match content.nodes.kind(node) {
        Some(NodeKind::Box | NodeKind::Ruby) if facts.bidi == UnicodeBidi::Normal => None,
        Some(NodeKind::Annotation | NodeKind::Box | NodeKind::Ruby) => Some(match facts.bidi {
            UnicodeBidi::Normal | UnicodeBidi::Plaintext => false,
            UnicodeBidi::Embed
            | UnicodeBidi::BidiOverride
            | UnicodeBidi::Isolate
            | UnicodeBidi::IsolateOverride => facts.direction() == Direction::Rtl,
        }),
        _ => None,
    }
}

/// Returns `node`'s box facts, which say what controls it opens.
fn box_facts(content: &Content, node: NodeId) -> &BoxFacts {
    let id = content.nodes.box_facts(node, FirstLineVariant::Standard);
    content.facts.box_facts(id)
}

/// Adds the controls that open `node` and returns their class mask.
///
/// A box or ruby container adds its `unicode-bidi` controls. An annotation
/// is a first-strong isolate with its own `unicode-bidi` controls inside,
/// so `dir` on it sets its direction.
///
/// The block's initial letter is an isolate in its own direction. Blink
/// sets its box as one neutral U+FFFC and lays its text out apart
/// (`AppendOpaque` of `kInitialLetterBox`), so the box stands at the line's
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

/// Adds the controls that open a container with `facts`'s `unicode-bidi`
/// and direction, and returns their class mask.
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
        // A first-strong isolate outside and an override inside.
        UnicodeBidi::IsolateOverride => {
            input.control(BidiClass::FIRST_STRONG_ISOLATE)
                | input.control(override_class(direction))
        }
    }
}

/// Adds the controls that close `node`, matching [`enter`], and returns
/// their class mask.
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

/// Adds the controls that close a container with `facts`'s
/// `unicode-bidi`, matching [`enter_container`], and returns their class
/// mask.
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
