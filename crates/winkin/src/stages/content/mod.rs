//! The content: what the builder makes of the caller's calls, which every
//! later stage reads.
//!
//! In: the builder's calls, with their styles, text and sizes. Out:
//! [`Content`]: the text as laid out, the nodes, the items in reading order,
//! the facts each style lowers into and the lists they name, the atomic
//! inlines, the floats and the absolutely positioned boxes, and flags for
//! what the content holds.
//! Start at: [`ContentWriter::new`], then the builder's calls in
//! `writer_calls`.
//!
//! - `writer`: writes the content, call by call, and finishes it.
//! - `collapse`: the white space collapsing state machine.
//! - `facts`: the facts each style lowers into.
//! - `lists`: the interned lists the facts name.
//! - `memo`: the text facts each style in a build lowered into.
//! - `transform`: `text-transform`, applied as text is written.
//! - `first_letter`: where `::first-letter` ends.
//! - `first_line`: the first paragraph as its first line draws it.
//! - `map`: the offset map back to the caller's text.
//!
//! The content keeps no style, and building reads no font and no width, so
//! it is a pure function of the caller's input. Only the writer writes it,
//! and the content is frozen once the build finishes. Clearing keeps every
//! allocation, so rebuilding similar content allocates nothing.

/// Maximum depth of separately modeled ruby containers. The writer
/// normalizes deeper containers to boxes, keeping later walks bounded.
pub(super) const MAX_RUBY_DEPTH: usize = 32;

mod collapse;
mod facts;
#[cfg(any(debug_assertions, test))]
mod facts_check;
mod first_letter;
mod first_line;
mod lists;
mod map;
mod memo;
#[cfg(test)]
mod tests;
mod transform;
mod writer;
mod writer_calls;
#[cfg(debug_assertions)]
mod writer_check;
mod writer_first_letter;
mod writer_lower;
mod writer_nodes;
mod writer_text;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;
use core::ops::Range;

use crate::build::{BoxSize, BuildReport, Clear, FloatSide, OriginalDisplay};
use crate::data::{
    HashIndex, Id, Keyed, Table, TextOffset, define_flags, define_id, find_sorted, heap_bytes,
};
use crate::style::{
    ComputedStyle, FirstLineVariant, InitialLetter, VerticalAlign, WhiteSpaceTrim, WritingMode,
};
use crate::unit::LayoutUnit;
use crate::work;
use map::{Map, MapUnitId, OffsetMap};

use collapse::Collapser;
#[cfg(test)]
pub(super) use facts::ShapingFacts;
pub(crate) use facts::{
    BlockFacts, BoxFacts, BoxFactsId, BoxFlags, Facts, FontRequest, FontRequestId, ShapingFactsId,
    TextFacts, TextFactsId, TextFlags, TextLineHeight, TextSetting,
};
#[cfg(any(debug_assertions, test))]
pub(crate) use facts_check::check;
use lists::Lists;
pub(crate) use lists::{
    FamilyListId, FamilyLists, FamilyName, HyphenStringId, HyphenStrings, LanguageId,
};
pub(crate) use map::MapSide;
pub(crate) use memo::FontKey;
use memo::{StyleKey, StyleMemo};
use transform::Transforms;
use writer::OpenId;
pub(crate) use writer::{ContentLimits, ContentScratch};

/// An application-defined node identifier.
///
/// Used to retrieve paint styles and associate results with source nodes.
/// Text uses the text-node key, separate from the enclosing element key.
/// The crate does not interpret the value.
///
/// Keys need not be unique. Reusing a key for a separate node gives queries
/// such as [`Layout::box_fragments`](crate::Layout::box_fragments) results
/// for all matching nodes. Use unique keys for individual-node queries.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct NodeKey(pub u64);

impl From<u64> for NodeKey {
    fn from(key: u64) -> Self {
        Self(key)
    }
}

define_id! {
    /// Names a node, in pre-order: the block is node 0.
    pub(crate) struct NodeId(u32);
}

impl NodeId {
    /// The block container's own node, which every build writes first.
    /// Its style holds the block's own inline properties.
    pub(crate) const BLOCK: Self = Self(0);
}

define_id! {
    /// Names an item, in reading order.
    pub(crate) struct ItemId(u32);
}

define_id! {
    /// Names an atomic inline, in reading order.
    pub(crate) struct AtomicId(u32);
}

define_id! {
    /// Names a float, in reading order.
    pub(crate) struct FloatId(u32);
}

define_id! {
    /// Names an absolutely positioned box's anchor, in reading order.
    pub(crate) struct AbsoluteId(u32);
}

define_id! {
    /// Names a forced break that clears floats, in reading order.
    pub(super) struct BreakClearanceId(u32);
}

/// What a node is.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub(crate) enum NodeKind {
    /// The block container itself, node 0, styled by the root style.
    Block,
    /// An inline box: `open` to `close`.
    Box,
    /// A text node: text under one key, from one `text` call or several in a
    /// row.
    Text,
    /// An atomic inline: an inline-block, a replaced element.
    Atomic,
    /// A float's anchor.
    Float,
    /// An absolutely positioned box's anchor.
    Absolute,
    /// A forced break: `<br>`.
    LineBreak,
    /// A ruby container.
    Ruby,
    /// A ruby annotation.
    Annotation,
    /// A generated `::first-letter` box around the first letter of the text
    /// that follows the request. The letter becomes the box's own text.
    FirstLetter,
}

impl NodeKind {
    /// Whether it is an inline box: one the caller opened, or a generated
    /// `::first-letter` box, which is one too.
    pub(crate) fn is_inline_box(self) -> bool {
        matches!(self, Self::Box | Self::FirstLetter)
    }

    /// Whether it is a container opened and closed around what it holds,
    /// with edges of its own there: an inline box, a ruby container, an
    /// annotation, or a `::first-letter` box.
    fn is_container(self) -> bool {
        matches!(
            self,
            Self::Box | Self::Ruby | Self::Annotation | Self::FirstLetter
        )
    }

    /// Whether its own `vertical-align` shifts it in its line: an inline
    /// box's or an atomic inline's does. The block's own value places the
    /// block in a line outside it, so nothing here reads it.
    fn reads_vertical_align(self) -> bool {
        matches!(self, Self::Box | Self::Atomic | Self::FirstLetter)
    }
}

/// What an item is.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
#[repr(u8)]
pub(crate) enum ItemKind {
    /// A piece of one node's text.
    Text,
    /// An inline box's start (or a `::first-letter` box's): empty, at a
    /// boundary.
    Open,
    /// An inline box's end: empty, at a boundary.
    Close,
    /// An atomic inline: one U+FFFC.
    Atomic,
    /// A float's anchor: empty, at a boundary.
    Float,
    /// An absolutely positioned box's anchor: empty, at a boundary.
    Absolute,
    /// A forced break: one `\n`.
    Break,
    /// A ruby container's start: empty.
    RubyOpen,
    /// A ruby container's end: empty.
    RubyClose,
    /// A ruby annotation's start: empty.
    AnnotationOpen,
    /// A ruby annotation's end: empty.
    AnnotationClose,
}

impl ItemKind {
    /// Whether the item holds text: every other kind sits at a boundary.
    pub(super) fn has_text(self) -> bool {
        matches!(self, Self::Text | Self::Atomic | Self::Break)
    }

    /// Whether it opens a container: an inline box, a ruby container or an
    /// annotation.
    ///
    /// An annotation's own marks are flagged [`ItemFlags::ANNOTATION`] with
    /// what it holds, so a reader that passes over annotations by the flag
    /// meets a box's and a ruby container's here, and never an
    /// annotation's.
    pub(super) fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::RubyOpen | Self::AnnotationOpen)
    }

    /// Whether it closes a container: an inline box, a ruby container or an
    /// annotation. As with [`is_open`](Self::is_open), an annotation's close
    /// is flagged with what it holds.
    pub(super) fn is_close(self) -> bool {
        matches!(self, Self::Close | Self::RubyClose | Self::AnnotationClose)
    }
}

define_flags! {
    /// What only the builder knows about an item and every later stage
    /// needs.
    ///
    /// Nothing else goes here. What a later stage learns about an item goes
    /// in that stage's output, indexed by [`ItemId`].
    pub(crate) struct ItemFlags(u8) {
        /// Inside a ruby annotation, its marks included: out of the main
        /// line's flow.
        pub(crate) const ANNOTATION = 1 << 0;
        /// Made by the builder, not written by the caller.
        ///
        /// This is the U+200B of a `<wbr>`, or of a wrap opportunity that
        /// collapsing keeps. Each is a text item of its own, in its text
        /// node, or between nodes in the box around it. Analysis gives its
        /// cluster a class of its own, which ends a shaping run; a U+200B
        /// the caller wrote does not.
        pub(super) const GENERATED = 1 << 1;
    }
}

/// One entry in reading order: 16 bytes.
///
/// Items tile the text. Those that hold text cover every byte once and in
/// order, and the rest sit at the boundaries between them. The writer checks
/// this in debug builds. It lets analysis walk items and text together.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Item {
    /// Where its text starts, or where it sits.
    pub(super) start: TextOffset,
    /// Where its text ends: its start, for an item that holds none.
    pub(super) end: TextOffset,
    /// The node it belongs to.
    pub(crate) node: NodeId,
    pub(crate) kind: ItemKind,
    /// What the builder knew about it.
    pub(crate) flags: ItemFlags,
}

/// A node: one for each thing the caller named, and for each generated
/// `::first-letter` box. 32 bytes.
///
/// The writer writes the row whole as it makes the node, except `end`,
/// which it writes when the node's items end.
///
/// Its facts come in both variants. The first line's are what its
/// `::first-line` style lowers into, after the builder pins what
/// `::first-line` may not change. Where the block has no `::first-line`,
/// they are its own. The four fact ids are 16 bits each; widening them
/// takes the node past 32 bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Node {
    /// The caller's key, which every view that names the node hands back.
    key: NodeKey,
    /// What its text is: a text node's and a break's are its container's.
    text: TextFactsId,
    first_line_text: TextFactsId,
    /// What it is as a box: [`BoxFactsId::INITIAL`] for a node that is no
    /// box.
    box_: BoxFactsId,
    first_line_box: BoxFactsId,
    /// Its first item, or where it would be for a node with none.
    first_item: ItemId,
    /// One past its last item, its close included: its items are
    /// `first_item..end`. Nodes are in pre-order and items in reading order,
    /// so `a` is an ancestor of `b` exactly when `a < b` and `b`'s
    /// `first_item` is before `a`'s `end`.
    end: ItemId,
    /// Its parent; the block is its own.
    parent: NodeId,
    kind: NodeKind,
}

/// The nodes, in pre-order, their item ranges nesting as the nodes do.
///
/// Each is looked up by its id, and a lookup never misses a node the
/// content names.
pub(crate) struct Nodes {
    nodes: Table<NodeId, Node>,
}

impl Nodes {
    const fn new() -> Self {
        Self {
            nodes: Table::new(),
        }
    }

    fn clear(&mut self) {
        self.nodes.clear();
    }

    /// How many nodes there are.
    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Node `node`, for the writer making it, or `None` past the last.
    fn get_mut(&mut self, node: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(node)
    }

    /// Returns what `node`'s text is in `variant` (see [`TextFacts`]).
    ///
    /// A text node's and a break's are its container's. Every node the
    /// content names has them. A lookup that misses is a bug, and answers
    /// with the block's.
    #[inline]
    pub(crate) fn text_facts(&self, node: NodeId, variant: FirstLineVariant) -> TextFactsId {
        match (self.nodes.get(node), variant) {
            (Some(row), FirstLineVariant::Standard) => row.text,
            (Some(row), FirstLineVariant::FirstLine) => row.first_line_text,
            (None, _) => {
                debug_assert!(false, "{node:?} is not a node");
                TextFactsId::new(0)
            }
        }
    }

    /// Returns what `node` is as a box in `variant` (see [`BoxFacts`]).
    /// A node that is no box, or one past the last, gets the initial row.
    #[inline]
    pub(crate) fn box_facts(&self, node: NodeId, variant: FirstLineVariant) -> BoxFactsId {
        match (self.nodes.get(node), variant) {
            (Some(row), FirstLineVariant::Standard) => row.box_,
            (Some(row), FirstLineVariant::FirstLine) => row.first_line_box,
            (None, _) => {
                debug_assert!(false, "{node:?} is not a node");
                BoxFactsId::INITIAL
            }
        }
    }

    /// What `node` is, or `None` past the last node.
    pub(crate) fn kind(&self, node: NodeId) -> Option<NodeKind> {
        self.nodes.get(node).map(|node| node.kind)
    }

    /// The caller's key for `node`, which every view that names a node
    /// hands back; the default key past the last node.
    pub(crate) fn key(&self, node: NodeId) -> NodeKey {
        self.nodes
            .get(node)
            .map(|node| node.key)
            .unwrap_or_default()
    }

    /// Returns `node`'s parent. The block is its own parent, and so is a
    /// node the lookup misses.
    pub(crate) fn parent(&self, node: NodeId) -> NodeId {
        self.nodes.get(node).map_or(node, |node| node.parent)
    }

    /// `node`'s items: `first_item..end`, its close included; none past the
    /// last node.
    pub(crate) fn items(&self, node: NodeId) -> Range<ItemId> {
        self.nodes
            .get(node)
            .map_or(ItemId::default()..ItemId::default(), |node| {
                node.first_item..node.end
            })
    }

    /// Whether `node` is an anonymous ruby container: one the builder made
    /// around an annotation written outside every ruby container.
    ///
    /// It takes its annotation's key, and the annotation is its next node,
    /// opening right after it, over an empty base.
    pub(crate) fn is_anonymous_ruby(&self, node: NodeId) -> bool {
        let annotation = NodeId::new(node.get() + 1);
        self.kind(node) == Some(NodeKind::Ruby)
            && self.kind(annotation) == Some(NodeKind::Annotation)
            && self.key(annotation) == self.key(node)
            && self.items(annotation).start.get() == self.items(node).start.get() + 1
    }

    /// Whether `ancestor` is `node` or one of its ancestors. It takes one
    /// comparison, since nodes are in pre-order and items in reading order.
    pub(crate) fn contains(&self, ancestor: NodeId, node: NodeId) -> bool {
        ancestor == node || (ancestor < node && self.items(node).start < self.items(ancestor).end)
    }
}

heap_bytes! {
    Nodes { nodes }
}

/// An atomic inline's size, found from its item.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct Atomic {
    /// Its item, which holds its U+FFFC.
    pub(super) item: ItemId,
    /// Its border box and baseline as the host laid it out: finite and not
    /// negative.
    pub(super) size: BoxSize,
    /// Its margin box along the line. This is its border box, truncated
    /// onto the grid as Chrome holds a box's lengths, plus its style's
    /// margins along the line on either side, on the grid.
    pub(super) margin_inline: LayoutUnit,
    /// Its style's margins across the line, `(over, under)`, on the grid,
    /// which its extent reaches past its border box.
    pub(super) margins_across: (LayoutUnit, LayoutUnit),
}

impl Atomic {
    /// Makes the atomic inline of `item`, `size` as the host laid it out,
    /// set in `style` in a block of `writing_mode`. The writer lowers its
    /// margin box here.
    fn new(
        item: ItemId,
        size: BoxSize,
        style: &ComputedStyle<'_>,
        writing_mode: WritingMode,
    ) -> Self {
        let margin = style.edges.margin;
        let (left, right) = margin.along_line_on_grid(writing_mode);
        Self {
            item,
            size,
            margin_inline: LayoutUnit::from_px_truncated(size.inline) + left + right,
            margins_across: margin.across_line_on_grid(writing_mode),
        }
    }
}

impl Keyed for Atomic {
    type Key = ItemId;

    /// Its item: the content's atomic inlines are in item order.
    #[inline]
    fn key(&self) -> ItemId {
        self.item
    }
}

/// A search of the atomic inlines for those with one key, round from a
/// cursor: from it to the last, then from the first up to it.
///
/// Where no two atomic inlines share a key, it ends at the first it finds.
/// A search started where the last ended then costs the distance between
/// the two, so keys in reading order cost one pass in total.
pub(crate) struct KeyedAtomics {
    key: NodeKey,
    /// Where the search started, and stops on coming round to it.
    start: AtomicId,
    /// The next atomic inline to look at.
    next: AtomicId,
    /// Whether the search has come round past the last.
    wrapped: bool,
    /// Whether no two atomic inlines share a key.
    unique: bool,
    /// The last atomic inline found.
    last: Option<AtomicId>,
}

impl KeyedAtomics {
    /// Returns the next atomic inline of `content` with the key, or `None`
    /// once the search has come round.
    pub(crate) fn next_match(&mut self, content: &Content) -> Option<AtomicId> {
        if self.unique && self.last.is_some() {
            return None;
        }
        let atomics = content.atomics();
        loop {
            if self.wrapped && self.next >= self.start {
                return None;
            }
            let Some(atomic) = atomics.get(self.next) else {
                if self.wrapped || self.start == AtomicId::new(0) {
                    return None;
                }
                self.wrapped = true;
                self.next = AtomicId::new(0);
                continue;
            };
            work::step();
            let id = self.next;
            self.next = AtomicId::new(id.get() + 1);
            if content.atomic_key(atomic) == self.key {
                self.last = Some(id);
                return Some(id);
            }
        }
    }

    /// Returns where the next search starts: after the last atomic inline
    /// found, or where this one started if it found none.
    pub(crate) fn cursor(&self) -> AtomicId {
        self.last
            .map_or(self.start, |last| AtomicId::new(last.get() + 1))
    }
}

/// A float's size and side, found from its anchor item.
#[derive(Copy, Clone, PartialEq, Debug)]
pub(crate) struct Float {
    /// Its anchor: an item with no text, where the float was written.
    pub(crate) item: ItemId,
    /// Its border box as the host laid it out: finite and not negative.
    size: BoxSize,
    /// The side it floats to.
    pub(crate) side: FloatSide,
    /// Its margin box, as `(along the line, across it)`, never negative.
    ///
    /// It is the border box as the host laid it out plus its style's
    /// margins on either side. Each is truncated onto layout's grid, as
    /// Chrome holds a box's lengths. The writer lowers it.
    ///
    /// This is the one size a float takes. The breaker asks the host to
    /// place it, and the intrinsic sizes count it. Negative margins that
    /// leave less than nothing take nothing, as Chrome clamps a float's
    /// contribution (`ClampNegativeToZero`).
    pub(crate) margin_box: (LayoutUnit, LayoutUnit),
}

impl Float {
    /// Makes the float anchored at `item`, `size` as the host laid it out, on
    /// `side`, set in `style` in a block of `writing_mode`.
    fn new(
        item: ItemId,
        size: BoxSize,
        side: FloatSide,
        style: &ComputedStyle<'_>,
        writing_mode: WritingMode,
    ) -> Self {
        let (left, right) = style.edges.margin.along_line_on_grid(writing_mode);
        let (over, under) = style.edges.margin.across_line_on_grid(writing_mode);
        let margin_box = |size: f32, start: LayoutUnit, end: LayoutUnit| {
            (LayoutUnit::from_px_truncated(size) + start + end).max(LayoutUnit::ZERO)
        };
        Self {
            item,
            size,
            side,
            margin_box: (
                margin_box(size.inline, left, right),
                margin_box(size.block, over, under),
            ),
        }
    }
}

/// An absolutely positioned box's anchor and its outer display, found from
/// its item.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Absolute {
    /// Its anchor: an item with no text, where the box was written.
    pub(crate) item: ItemId,
    /// Its outer display before positioning blockified it.
    pub(crate) display: OriginalDisplay,
}

impl Keyed for Absolute {
    type Key = ItemId;

    /// Its item: the content's anchors are in item order.
    #[inline]
    fn key(&self) -> ItemId {
        self.item
    }
}

/// A forced break that clears floats: its item, and the floats it clears.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct BreakClearance {
    /// The break's item, which holds its `\n`.
    pub(super) item: ItemId,
    pub(super) clear: Clear,
}

/// The first paragraph as its first line draws it, where some node's
/// `::first-line` transform differs from its own.
///
/// The first-line variant selects fonts for this text and shapes it. It is
/// empty unless the first line transforms some text its own way. The
/// `first_line` module builds it.
#[derive(Default)]
struct FirstLineSource {
    /// The text, from the content's start through its first paragraph.
    text: String,
    /// Where the content's offsets are in `text`, one entry a piece, sorted.
    map: Vec<MapEntry>,
}

heap_bytes! {
    FirstLineSource { text, map }
}

/// What the content keeps only where a build asks for it or has it.
///
/// - The offset map, where the build records one.
/// - The first paragraph as its first line draws it, where that line
///   transforms some text its own way.
/// - The atomic inlines, the floats and the absolutely positioned boxes.
///
/// It is boxed, and made the first time a build keeps any, so a plain
/// layout holds only the box's pointer. Once made, it is cleared with the
/// content and never dropped. A layout that gains and loses them keeps
/// their capacity.
#[derive(Default)]
struct ContentExtras {
    map: Map,
    first_line_source: FirstLineSource,
    /// The atomic inlines, in reading order.
    atomics: Table<AtomicId, Atomic>,
    /// The floats, in reading order.
    floats: Table<FloatId, Float>,
    /// The absolutely positioned boxes' anchors, in reading order.
    absolutes: Table<AbsoluteId, Absolute>,
    /// The forced breaks that clear floats, in reading order.
    clearances: Table<BreakClearanceId, BreakClearance>,
}

/// What content with no clearing break reads its clearances as: none.
static NO_CLEARANCES: Table<BreakClearanceId, BreakClearance> = Table::new();

/// What content with no atomic inline reads its atomic inlines as: none.
static NO_ATOMICS: Table<AtomicId, Atomic> = Table::new();

/// What content with no float reads its floats as: none.
static NO_FLOATS: Table<FloatId, Float> = Table::new();

/// What content with no absolutely positioned box reads its anchors as:
/// none.
static NO_ABSOLUTES: Table<AbsoluteId, Absolute> = Table::new();

impl ContentExtras {
    /// The offset map, where this build records it.
    fn recorded_map(&mut self) -> Option<&mut Map> {
        self.map.recording.then_some(&mut self.map)
    }
}

heap_bytes! {
    ContentExtras { map, first_line_source, atomics, floats, absolutes, clearances }
}

/// One entry of [`FirstLineSource`]'s map, for a piece of text. 12 bytes.
///
/// It records where the piece starts in the content's text and in the first
/// line's. It also records whether the two match byte for byte through it,
/// or map only as wholes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct MapEntry {
    content: TextOffset,
    first_line: TextOffset,
    aligned: bool,
}

/// Text as a variant reads it, found by content offsets.
///
/// It is the content's text, or the first line's where that line transforms
/// some of it its own way.
#[derive(Copy, Clone)]
pub(crate) struct VariantText<'a> {
    content: &'a str,
    first_line: Option<&'a FirstLineSource>,
}

impl<'a> From<&'a str> for VariantText<'a> {
    /// Reads `content`, the content's text, as its own variant. A reader
    /// that holds only the text asks it for a cluster's characters.
    fn from(content: &'a str) -> Self {
        Self {
            content,
            first_line: None,
        }
    }
}

impl<'a> VariantText<'a> {
    /// The whole text it reads, in its own offsets.
    pub(super) fn text(&self) -> &'a str {
        match self.first_line {
            Some(first_line) => &first_line.text,
            None => self.content,
        }
    }

    /// Where content offset `at` is in [`text`](Self::text).
    pub(super) fn offset(&self, at: TextOffset) -> TextOffset {
        match self.first_line {
            Some(first_line) => first_line.offset(at),
            None => at,
        }
    }

    /// The text of the content's `range`: nothing where it is not the
    /// content's.
    pub(super) fn slice(&self, range: Range<TextOffset>) -> &'a str {
        match self.first_line {
            Some(first_line) => first_line.slice(range),
            None => self
                .content
                .get(range.start.get()..range.end.get())
                .unwrap_or_default(),
        }
    }

    /// Returns a cursor reading this text in the text's order.
    pub(super) fn cursor(self) -> TextCursor<'a> {
        TextCursor {
            text: self,
            after: Cell::new(0),
        }
    }
}

/// A variant's text, read by a walk in the text's order.
///
/// It keeps its place in the first line's map. Offsets asked in order step
/// through the map and search nothing. One far before the last asked costs
/// one search. A clone keeps the place, and moves apart from it.
#[derive(Clone)]
pub(crate) struct TextCursor<'a> {
    text: VariantText<'a>,
    /// How many of the map's entries start at or before the offset last
    /// asked.
    after: Cell<usize>,
}

impl<'a> TextCursor<'a> {
    /// The whole text it reads, in its own offsets.
    pub(super) fn text(&self) -> &'a str {
        self.text.text()
    }

    /// Where content offset `at` is in [`text`](Self::text).
    pub(super) fn offset(&self, at: TextOffset) -> TextOffset {
        match self.text.first_line {
            Some(first_line) => first_line.offset_stepped(at, &self.after),
            None => at,
        }
    }

    /// Returns the one ASCII character the content's `range` holds.
    ///
    /// It answers only where the range holds one byte and the variant reads
    /// the content's own text. A walk over Latin text asks it to skip
    /// slicing and decoding.
    #[inline]
    pub(super) fn ascii(&self, range: Range<TextOffset>) -> Option<char> {
        let text = self.text;
        if text.first_line.is_some() || range.end.get() != range.start.get() + 1 {
            return None;
        }
        let byte = *text.content.as_bytes().get(range.start.get())?;
        byte.is_ascii().then_some(char::from(byte))
    }

    /// The text of the content's `range`: nothing where it is not the
    /// content's.
    pub(super) fn slice(&self, range: Range<TextOffset>) -> &'a str {
        match self.text.first_line {
            Some(_) => {
                let (start, end) = (self.offset(range.start), self.offset(range.end));
                self.text().get(start.get()..end.get()).unwrap_or_default()
            }
            None => self.text.slice(range),
        }
    }
}

define_flags! {
    /// What the content holds, as flags a later stage can decline work on
    /// at once.
    ///
    /// Each records what the content *contains*, never a conclusion. The
    /// writer sets it where it first sees the thing. Stages derive their
    /// own gates from these and from what they find in the text. The style
    /// flags are the one answer to whether any style sets something.
    pub(crate) struct ContentFlags(u32) {
        /// An inline box has margin, border or padding on some side, or is
        /// the block's initial letter. An initial letter's ink takes room at
        /// its edges however its own edges are set.
        pub(super) const BOXES_WITH_EDGES = 1 << 0;
        /// There is an atomic inline.
        pub(super) const ATOMICS = 1 << 1;
        /// There is a float.
        pub(super) const FLOATS = 1 << 2;
        /// There is a ruby container.
        pub(crate) const RUBY = 1 << 3;
        /// Some node's `::first-line` style differs from its own.
        pub(crate) const FIRST_LINE_RESTYLE = 1 << 4;
        /// The block's first line transforms some text otherwise than the
        /// text's own transform does. The content keeps the first paragraph
        /// as that line draws it.
        const TRANSFORM_SIDE_TEXT = 1 << 5;
        /// Some style sets emphasis marks.
        pub(crate) const EMPHASIS = 1 << 6;
        /// Some style sets an initial letter, so the block may have one.
        /// Only the box that may be the initial letter keeps the property.
        pub(super) const INITIAL_LETTER = 1 << 7;
        /// An inline box clones its decoration where it breaks.
        pub(super) const CLONE_BOXES = 1 << 8;
        /// Some style sets `line-padding`.
        pub(super) const LINE_PADDING = 1 << 9;
        /// Some style sets `hanging-punctuation`.
        pub(super) const HANGING_PUNCTUATION = 1 << 10;
        /// Some style sets nonzero letter or word spacing.
        pub(super) const NONZERO_SPACING = 1 << 11;
        /// An inline box or an atomic inline sets `vertical-align` other
        /// than `baseline`, so line heights are worked out box by box. The
        /// block's own value places the block in a line outside it, and
        /// does not count.
        pub(super) const VERTICAL_ALIGN = 1 << 12;
        /// Some style sets `dominant-baseline` other than `auto`. The
        /// measure stage reads it only where the config asks it to.
        pub(super) const DOMINANT_BASELINE = 1 << 13;
        /// A node below the block is an inline box. A line it is open
        /// across holds its strut; without one, no line looks for struts.
        pub(super) const INLINE_BOXES = 1 << 14;
        /// Some style sets `text-wrap-mode: nowrap`.
        pub(super) const NOWRAP = 1 << 15;
        /// Some style's `text-spacing-trim` trims the start of a line that
        /// wrapped.
        pub(super) const TRIMS_WRAPPED_START = 1 << 16;
        /// Some style's `text-spacing-trim` trims punctuation at all.
        pub(super) const TRIMS_PUNCTUATION = 1 << 17;
        /// Some style's `text-autospace` takes some seam.
        pub(super) const AUTOSPACE = 1 << 18;
        /// An inline box's style decorates its text (`decorates`). A culled
        /// box may then draw a decoration line, and a line's paint looks
        /// for one only where this is set.
        pub(crate) const DECORATED_BOXES = 1 << 19;
        /// Some style sets `font-variant-position`.
        ///
        /// A used font may synthesize it, drawing its glyphs raised or
        /// lowered off their run's baseline. Without it, in a line on the
        /// alphabetic baseline, a reader places every glyph on its run's
        /// baseline and looks for no font to place it by.
        pub(crate) const VARIANT_POSITION = 1 << 20;
        /// Some node's first-line text shapes otherwise than its own: its
        /// [`ShapingFactsId`] differs.
        pub(crate) const FIRST_LINE_RESHAPES = 1 << 21;
        /// Two atomic inlines have the same key.
        const SHARED_ATOMIC_KEYS = 1 << 22;
        /// Some style asks for whole-node mathematical italic mapping.
        pub(super) const MATH_AUTO = 1 << 23;
    }
}

/// The content stage's data.
pub(crate) struct Content {
    /// The text as laid out: white space collapsed, U+FFFC for each atomic
    /// inline, `\n` for each `<br>`, U+200B for each `<wbr>` and each break
    /// opportunity collapsing kept. At most
    /// [`TextOffset::MAX`] bytes.
    pub(crate) text: String,
    pub(crate) nodes: Nodes,
    /// The items, in reading order.
    pub(crate) items: Table<ItemId, Item>,
    /// The lists the facts name: font families, settings, languages and
    /// hyphen strings.
    pub(super) lists: Lists,
    /// The facts the writer lowers each style into.
    pub(crate) facts: Facts,
    /// The offset map, the first line's side text, the atomic inlines and
    /// the floats, where a build keeps any.
    extras: Option<Box<ContentExtras>>,
    /// The block's own properties, and its facts.
    pub(crate) block: BlockFacts,
    /// What the content holds.
    pub(crate) flags: ContentFlags,
}

impl Content {
    /// Empty content, allocating nothing.
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            nodes: Nodes::new(),
            items: Table::new(),
            lists: Lists::new(),
            facts: Facts::new(),
            extras: None,
            block: BlockFacts::INITIAL,
            flags: ContentFlags::NONE,
        }
    }

    /// Returns whether an annotation of the ruby container whose base goes
    /// on at item `from` opens before the container closes.
    ///
    /// Base text with no annotation after it is no ruby column. Chrome lays
    /// it out as text of the line (`LineBreaker::HandleRuby`: "No
    /// ruby-text"). The annotations of nested containers are not the
    /// container's, but those of a container written as a box are.
    pub(crate) fn annotation_follows(&self, from: ItemId) -> bool {
        let mut nested = 0u32;
        for item in self.items.as_slice().get(from.get()..).unwrap_or_default() {
            work::step();
            match item.kind {
                ItemKind::AnnotationOpen if nested == 0 => return true,
                ItemKind::RubyOpen => nested += 1,
                ItemKind::RubyClose if nested == 0 => return false,
                ItemKind::RubyClose => nested -= 1,
                _ => {}
            }
        }
        false
    }

    /// Empties everything for a new build, keeping every allocation. The
    /// build records the offset map where `map` asks.
    fn clear(&mut self, block: BlockFacts, map: bool) {
        self.text.clear();
        self.nodes.clear();
        self.items.clear();
        self.lists.clear();
        self.facts.clear();
        if map {
            self.extras.get_or_insert_with(Box::default);
        }
        if let Some(extras) = &mut self.extras {
            extras.map.clear(map);
            extras.first_line_source.clear();
            extras.atomics.clear();
            extras.floats.clear();
            extras.absolutes.clear();
            extras.clearances.clear();
        }
        self.block = block;
        self.flags = ContentFlags::NONE;
    }

    /// Records in the offset map with `record`, where this build records
    /// one, and returns what `record` returned.
    fn record<R>(&mut self, record: impl FnOnce(&mut Map) -> R) -> Option<R> {
        let map = self
            .extras
            .as_deref_mut()
            .and_then(ContentExtras::recorded_map)?;
        Some(record(map))
    }

    /// The first paragraph as its first line draws it, where the build
    /// keeps it.
    fn first_line_source(&self) -> Option<&FirstLineSource> {
        self.extras
            .as_deref()
            .map(|extras| &extras.first_line_source)
            .filter(|source| !source.is_empty())
    }

    /// The extras, for the writer keeping one of them: made the first time
    /// a build keeps any.
    fn extras_mut(&mut self) -> &mut ContentExtras {
        self.extras.get_or_insert_with(Box::default)
    }

    /// The first line's side text, for the writer keeping it: made the
    /// first time a build keeps one.
    fn first_line_source_mut(&mut self) -> &mut FirstLineSource {
        &mut self.extras_mut().first_line_source
    }

    /// The atomic inlines, in reading order: none where the build kept
    /// none.
    #[inline]
    pub(super) fn atomics(&self) -> &Table<AtomicId, Atomic> {
        self.extras
            .as_deref()
            .map_or(&NO_ATOMICS, |extras| &extras.atomics)
    }

    /// The floats, in reading order: none where the build kept none.
    #[inline]
    pub(crate) fn floats(&self) -> &Table<FloatId, Float> {
        self.extras
            .as_deref()
            .map_or(&NO_FLOATS, |extras| &extras.floats)
    }

    /// The absolutely positioned boxes' anchors, in reading order: none
    /// where the build wrote none.
    #[inline]
    pub(crate) fn absolutes(&self) -> &Table<AbsoluteId, Absolute> {
        self.extras
            .as_deref()
            .map_or(&NO_ABSOLUTES, |extras| &extras.absolutes)
    }

    /// The forced breaks that clear floats, in reading order: none where
    /// the build wrote none.
    #[inline]
    pub(super) fn clearances(&self) -> &Table<BreakClearanceId, BreakClearance> {
        self.extras
            .as_deref()
            .map_or(&NO_CLEARANCES, |extras| &extras.clearances)
    }

    /// The offset map, where the build recorded it. It maps each content
    /// offset to the text the caller gave each node, and back.
    pub(crate) fn offset_map(&self) -> Option<OffsetMap<'_>> {
        self.extras
            .as_deref()?
            .map
            .read(&self.nodes, self.text.len())
    }

    /// The node `item` belongs to. Past the last item it is the block,
    /// though the content's own lookups never go there.
    pub(crate) fn item_node(&self, item: ItemId) -> NodeId {
        self.items.get(item).map_or(NodeId::BLOCK, |item| item.node)
    }

    /// The absolutely positioned box `item` anchors, or `None` where it
    /// anchors none. The anchors are in item order, so this searches by
    /// halving.
    pub(crate) fn item_absolute(&self, item: ItemId) -> Option<&Absolute> {
        find_sorted(self.absolutes().as_slice(), item)
    }

    /// Whether the atomic inlines' block sizes and baselines can be set
    /// in place: the content has no ruby and no initial letter, whose
    /// measure reads an atomic inline's extent beyond the inline's own item.
    pub(crate) fn can_resize_atomics(&self) -> bool {
        !self.flags.contains(ContentFlags::RUBY)
            && !self.flags.contains(ContentFlags::INITIAL_LETTER)
    }

    /// Returns a search for the atomic inlines keyed `key`, round from
    /// `from`.
    pub(crate) fn keyed_atomics(&self, key: NodeKey, from: AtomicId) -> KeyedAtomics {
        let start = from.min(self.atomics().next_id());
        KeyedAtomics {
            key,
            start,
            next: start,
            wrapped: false,
            unique: !self.flags.contains(ContentFlags::SHARED_ATOMIC_KEYS),
            last: None,
        }
    }

    /// The key of the node `atomic` belongs to.
    fn atomic_key(&self, atomic: &Atomic) -> NodeKey {
        self.nodes.key(self.item_node(atomic.item))
    }

    /// The border box and baseline atomic inline `atomic` has, or `None`
    /// past the last.
    pub(crate) fn atomic_size(&self, atomic: AtomicId) -> Option<BoxSize> {
        self.atomics().get(atomic).map(|atomic| atomic.size)
    }

    /// Gives atomic inline `atomic` the block size and baseline of `size`,
    /// a sanitized size with the inline size it was built with. Returns
    /// whether its size changed.
    pub(crate) fn set_atomic_size(&mut self, atomic: AtomicId, size: BoxSize) -> bool {
        let Some(atomic) = self
            .extras
            .as_deref_mut()
            .and_then(|extras| extras.atomics.get_mut(atomic))
        else {
            return false;
        };
        if atomic.size == size {
            return false;
        }
        atomic.size = size;
        true
    }

    /// The atomic inline `item` places, or `None` where it places none.
    /// The atomic inlines are in item order, so this searches by halving.
    pub(crate) fn item_atomic(&self, item: ItemId) -> Option<&Atomic> {
        find_sorted(self.atomics().as_slice(), item)
    }

    /// Returns its text's bytes plus its items, a measure every stage's
    /// scratch grows with.
    pub(crate) fn size(&self) -> usize {
        self.text.len().saturating_add(self.items.len())
    }

    /// The text as `variant` reads it. For the first line, where it
    /// transforms some text its own way, this is the first paragraph as
    /// that line draws it; otherwise it is the content's text.
    pub(crate) fn text(&self, variant: FirstLineVariant) -> VariantText<'_> {
        VariantText {
            content: &self.text,
            first_line: match variant {
                FirstLineVariant::FirstLine => self.first_line_source(),
                FirstLineVariant::Standard => None,
            },
        }
    }

    /// Whether the block's first line sets some text otherwise than its
    /// other lines do.
    ///
    /// It does where a node's first-line text shapes otherwise than its own
    /// (its [`ShapingFactsId`] differs), or where the first line has a
    /// transform of its own. Where it does not, the first line's fonts and
    /// glyphs are the text's, and only its measurements differ.
    pub(super) fn first_line_reshapes(&self) -> bool {
        self.flags.contains(ContentFlags::FIRST_LINE_RESTYLE)
            && (self.flags.contains(ContentFlags::TRANSFORM_SIDE_TEXT)
                || self.flags.contains(ContentFlags::FIRST_LINE_RESHAPES))
    }
}

heap_bytes! {
    /// Its tables, and the extras' box and theirs where a build made it.
    Content { text, nodes, items, lists, facts, extras; block, flags }
}

// The writer and the records its files share: `writer` builds it, and
// `writer_calls`, `writer_nodes`, `writer_text`, `writer_lower`,
// `writer_first_letter` and `writer_check` each add to `ContentWriter`'s
// `impl`.

/// U+FFFC OBJECT REPLACEMENT CHARACTER, which stands for an atomic inline.
const OBJECT: &str = "\u{FFFC}";

/// What `initial-letter` comes to on a node as the writer lowers its style.
///
/// The block has at most one initial letter: an inline box at its start
/// (CSS Inline 3, section 7.3.1). Everywhere else the property's used value
/// is `normal`. So a style that sets it belongs to the initial letter, and
/// the font and measure stages size it by its facts alone.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum InitialLetterUse {
    /// The style sets none.
    Unset,
    /// The box may be the block's initial letter. The property is kept,
    /// and `vertical-align` is set to `baseline`, since it does not apply
    /// to an initial letter (CSS Inline 3, section 7.5.1).
    Kept,
    /// Its used value, `normal`.
    Normal,
}

impl InitialLetterUse {
    /// Returns `style` with `initial-letter` as used.
    fn apply<'a>(self, style: &ComputedStyle<'a>) -> ComputedStyle<'a> {
        let mut used = *style;
        match self {
            Self::Unset => {}
            Self::Kept => used.line.vertical_align = VerticalAlign::Baseline,
            Self::Normal => used.line.initial_letter = InitialLetter::NONE,
        }
        used
    }
}

/// The facts a node's style lowered into, in one variant (see `facts`).
/// The writer gives these to a node it makes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct NodeFacts {
    text: TextFactsId,
    box_: BoxFactsId,
}

/// What the writer made of a node's styles.
///
/// It holds the node's facts in both variants (the first line's where the
/// block has `::first-line`), and what the writer reads of its styles for
/// the text it holds.
#[derive(Copy, Clone, Debug)]
struct LoweredNode {
    own: NodeFacts,
    first_line: Option<NodeFacts>,
    /// Its first-line style differs from its own.
    restyles: bool,
    /// How the text written in it is transformed.
    transforms: Transforms,
}

/// Where the writer is in keeping the first line's text.
///
/// That text is the first paragraph as the block's first line draws it,
/// kept where a first-line transform differs from a node's own (see
/// `first_line`).
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Mirror {
    /// The block has `::first-line`, and no text in the first paragraph has
    /// had a first-line transform of its own yet: nothing is kept.
    Waiting,
    /// Every write to the first paragraph is made to the first line's text
    /// too.
    Writing,
    /// The first paragraph has ended, past which no first line reaches, or
    /// the block has no `::first-line`.
    Done,
}

/// Where the writer is with `::first-letter`.
///
/// Its armed styles wait here whole, a few hundred bytes. There is one
/// writer, held by its builder, and boxing them would allocate in every
/// build that asks for a first letter.
#[allow(clippy::large_enum_variant)]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum FirstLetter {
    /// None asked for yet, and the block holds nothing it would have to come
    /// before.
    Unarmed,
    /// Asked for. Its key and keyed styles wait for the first text that
    /// holds a letter, which splits the box off. The styles are lowered
    /// then, once its parent and whether it is the initial letter are
    /// known.
    Armed {
        key: NodeKey,
        style: StyleKey,
        first_line: Option<StyleKey>,
    },
    /// Its box holds the punctuation a text ended with, and is closed. The
    /// letter is looked for in what follows; where none comes, the box is
    /// made plain.
    Punctuation { node: NodeId },
    /// Found, found not to be, or no longer to be looked for: the block
    /// holds something a first letter would have had to come before.
    Done,
}

/// A container a caller opened and has not closed.
///
/// Each entry also says where calls that look past it go, so no call
/// searches the stack however deep the caller nests. It names the innermost
/// container at or below it that has a node, which owns what is written
/// inside. A ruby container or an annotation also names the next one out.
#[derive(Copy, Clone, Debug)]
struct Open {
    /// Its node, or `None` where it was dropped at a limit. A dropped one
    /// stays on the stack only so that it, not the box outside, takes its
    /// close.
    node: Option<NodeId>,
    kind: ContainerKind,
    /// Its opening item.
    item: ItemId,
    /// Its `white-space-trim`.
    trim: WhiteSpaceTrim,
    /// Its node, or, where it was dropped, the node of the container it is
    /// in. Outside every other container this is the block.
    container: NodeId,
    /// How the text written in it is transformed: as its styles say, or,
    /// where it was dropped, as its container's.
    transforms: Transforms,
    /// For a ruby container, an annotation or a ruby container opened
    /// inside another, where on the stack the next of those out is, if any.
    outer: Option<OpenId>,
    /// It is an anonymous ruby container around an annotation written
    /// outside every other, which closes with it.
    anonymous: bool,
}

/// What a container on the stack is.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum ContainerKind {
    /// An inline box.
    Box,
    /// A ruby container.
    Ruby,
    /// A ruby annotation.
    Annotation,
    /// A container normalized to a box inside an annotation or past the
    /// nesting limit. Its annotations become levels of the enclosing column.
    FlattenedRuby,
}

impl ContainerKind {
    /// The kind of node it is written as.
    fn node_kind(self) -> NodeKind {
        match self {
            Self::Box | Self::FlattenedRuby => NodeKind::Box,
            Self::Ruby => NodeKind::Ruby,
            Self::Annotation => NodeKind::Annotation,
        }
    }

    /// The kind of item that opens it.
    fn open_item_kind(self) -> ItemKind {
        match self {
            Self::Box | Self::FlattenedRuby => ItemKind::Open,
            Self::Ruby => ItemKind::RubyOpen,
            Self::Annotation => ItemKind::AnnotationOpen,
        }
    }

    /// The kind of item that closes it.
    fn close_item_kind(self) -> ItemKind {
        match self {
            Self::Box | Self::FlattenedRuby => ItemKind::Close,
            Self::Ruby => ItemKind::RubyClose,
            Self::Annotation => ItemKind::AnnotationClose,
        }
    }

    /// Whether it is linked into the chain of ruby containers and
    /// annotations the ruby calls follow.
    fn is_ruby(self) -> bool {
        matches!(self, Self::Ruby | Self::Annotation | Self::FlattenedRuby)
    }
}

/// Writes the content, and is the only thing that does.
pub(crate) struct ContentWriter<'a> {
    content: &'a mut Content,
    /// The containers open, innermost last. The block is not on it.
    stack: &'a mut Table<OpenId, Open>,
    /// Where capitalize joins the character before a text to it.
    words: &'a mut String,
    /// The text facts each style given this build was lowered into, where
    /// a build has made it.
    memo: &'a mut Option<Box<StyleMemo>>,
    /// The atomic inlines written so far by the hash of their keys, until
    /// two share one, where a build has made it.
    atomic_keys: &'a mut Option<Box<HashIndex>>,
    /// How the text written in the block, outside every container, is
    /// transformed.
    block_transforms: Transforms,
    collapser: Collapser,
    /// The last character of the text written before, or U+0020 where
    /// something else came between.
    ///
    /// Capitalize reads this one character of what came before, as Chrome
    /// reads a text's `PreviousCharacter`. A `<br>` leaves its `\n`, a
    /// `<wbr>` its U+200B, and an atomic inline or a float a space, as in
    /// Chrome.
    last_char: char,
    /// The text node `text` continues while its key is the same, until
    /// anything else is written.
    text_node: Option<NodeId>,
    /// One character held until another call continues its node or ends it.
    math_text: Option<(NodeKey, char)>,
    /// A source text node already known to contain more than one character.
    math_key: Option<NodeKey>,
    /// That node's item text is appended to, once it has one.
    open_item: Option<ItemId>,
    /// How far the writer has got in the text the caller gave the text
    /// node: every byte so far, kept or collapsed. The offset map counts in
    /// these bytes.
    source: u32,
    /// The offset map's unit for the white space run pending, which takes
    /// the space the run owes if it is written.
    run_unit: Option<MapUnitId>,
    /// Where the text node the `::first-letter` box cut its letter from
    /// goes on: its key, and the offset in its text after the letter. It is
    /// kept while the next node the writer makes may be that node's rest.
    continues: Option<(NodeKey, u32)>,
    /// How many annotations are open, so their items are flagged.
    annotations: usize,
    /// Where on the stack the innermost ruby container, annotation or
    /// nested ruby container is, each linking to the next one out.
    ruby: Option<OpenId>,
    /// How many normalized ruby containers, written as boxes, still owe
    /// the `end_ruby` that closes each. One that an annotation of its
    /// column has already closed still takes its `end_ruby`, which then
    /// closes nothing.
    flattened_rubies: usize,
    /// Real ruby containers open; bounded before writing a new one.
    ruby_depth: usize,
    /// Close items owed to the containers open, which every other item
    /// leaves room for.
    reserved: usize,
    /// Whether the block has `::first-line`, and every node a first-line
    /// style.
    first_line: bool,
    /// Whether the first line's text is being kept.
    mirror: Mirror,
    /// Where `::first-letter` is.
    first_letter: FirstLetter,
    /// Nothing but boxes' opening edges and float anchors has been written.
    /// A box opening now is a first child all the way up, where
    /// `initial-letter` may apply.
    at_start: bool,
    /// A box has taken `initial-letter`. The block has its initial letter,
    /// and no box after it or inside it is another.
    initial_letter: bool,
    /// Where the block's leading trim would cut: just after the last
    /// segment break. It is set while the block trims its kept white space
    /// (`white-space-trim: discard-inner`) and only white space has been
    /// written.
    lead_trim: Option<TextOffset>,
    /// Whether the block trims its kept white space at both ends. The
    /// trims cut at kept segment breaks, so the offset map starts a unit at
    /// each, and no unit runs across a cut.
    block_trims: bool,
    /// Whether the text reached its limit: nothing more is written.
    full: bool,
    limits: ContentLimits,
    report: BuildReport,
}
