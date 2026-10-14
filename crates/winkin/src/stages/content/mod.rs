//! The content: the builder's output, which every later stage reads.
//!
//! In: the builder's calls, with their styles, text and sizes. Out:
//! [`Content`]: the laid-out text, the nodes, the items in reading order,
//! the facts each style lowers into, the atomic inlines, floats and
//! absolutely positioned boxes, and flags for what the content holds.
//! Start at: [`ContentWriter::new`], then the builder's calls in
//! `writer::calls`.
//!
//! - `writer`: writes the content call by call and finishes it.
//! - `collapse`: the white space collapsing state machine.
//! - `facts`: the facts each style lowers into.
//! - `facts_check`: the debug oracle for the facts.
//! - `lists`: the interned lists styles name.
//! - `memo`: the builder's memo of lowered styles.
//! - `transform`: `text-transform`, applied as text is written.
//! - `first_letter`: where `::first-letter` ends.
//! - `first_line`: the first paragraph as its first line draws it.
//! - `map`: the offset map back to each node's input text.
//! - `extras`: the boxed tables only some builds have.
//!
//! The content keeps no style and building reads no font, so the content
//! depends only on the builder's calls and options. Only the writer writes
//! it, and it is frozen once the build finishes. Clearing keeps every
//! allocation, so a warm rebuild allocates nothing.

/// Maximum depth of separately modeled ruby containers. The writer
/// turns deeper containers into boxes, which keeps later walks bounded.
pub(super) const MAX_RUBY_DEPTH: usize = 32;

mod collapse;
mod extras;
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

use alloc::boxed::Box;
use alloc::string::String;
use core::cell::Cell;
use core::ops::Range;

use crate::build::BoxSize;
use crate::data::{
    Id, Keyed, SortedTable, Staging, Table, TextOffset, define_flags, define_id, find_sorted,
    heap_bytes,
};
use crate::style::FirstLineVariant;

pub(crate) use extras::{Absolute, Atomic, BreakClearance, Float, KeyedAtomics, SizeChange};
use extras::{ContentExtras, NO_EXTRAS};
use facts::FactsStaging;
#[cfg(test)]
pub(super) use facts::ShapingFacts;
pub(crate) use facts::{
    BlockFacts, BoxFacts, BoxFactsId, BoxFlags, Facts, FixedSpacing, FontRequest, FontRequestId,
    ShapingFactsId, TextFacts, TextFactsId, TextFitFacts, TextFlags, TextLineHeight, TextSetting,
};
#[cfg(any(debug_assertions, test))]
pub(crate) use facts_check::check;
use first_line::FirstLineSource;
use lists::Languages;
pub(crate) use lists::{HyphenStringId, HyphenStrings, LanguageId, Lists};
pub(crate) use map::MapSide;
use map::{Map, OffsetMap};
pub(crate) use memo::FontKey;
pub(crate) use writer::{ContainerKind, ContentLimits, ContentScratch, ContentWriter};

/// An application-defined node identifier.
///
/// Keys look up paint styles and tie results back to source nodes. Text
/// carries its text node's key, separate from the enclosing element's key.
/// The crate does not interpret the value.
///
/// Keys need not be unique. A query such as
/// [`Layout::box_fragments`](crate::Layout::box_fragments) returns results
/// for every node with the given key. Use unique keys to query single nodes.
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
    /// The block container's node, which every build writes first. Its
    /// style holds the block's inline properties.
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
    /// A text node: text under one key, from one or more consecutive `text`
    /// calls.
    Text,
    /// An atomic inline, such as an inline-block or a replaced element.
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
    /// after the request. The letter becomes the box's text.
    FirstLetter,
}

impl NodeKind {
    /// Whether it is an inline box: one the builder opened, or a generated
    /// `::first-letter` box.
    pub(crate) fn is_inline_box(self) -> bool {
        matches!(self, Self::Box | Self::FirstLetter)
    }

    /// Whether it is a container with open and close items: an inline box,
    /// a ruby container, an annotation or a `::first-letter` box.
    fn is_container(self) -> bool {
        matches!(
            self,
            Self::Box | Self::Ruby | Self::Annotation | Self::FirstLetter
        )
    }

    /// Whether its `vertical-align` shifts it in its line: true for inline
    /// boxes and atomic inlines. The block's value places the block in an
    /// outer line, so nothing here reads it.
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
    /// Whether the item holds text. Every other kind sits at a boundary.
    pub(super) fn has_text(self) -> bool {
        matches!(self, Self::Text | Self::Atomic | Self::Break)
    }

    /// Whether it opens an inline box, a ruby container or an annotation.
    ///
    /// An annotation's open item carries [`ItemFlags::ANNOTATION`], so a
    /// reader that skips flagged items sees only box and ruby opens.
    pub(super) fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::RubyOpen | Self::AnnotationOpen)
    }

    /// Whether it closes an inline box, a ruby container or an annotation.
    /// An annotation's close item carries [`ItemFlags::ANNOTATION`].
    pub(super) fn is_close(self) -> bool {
        matches!(self, Self::Close | Self::RubyClose | Self::AnnotationClose)
    }
}

define_flags! {
    /// What only the builder knows about an item and later stages need.
    ///
    /// What a later stage learns about an item goes in that stage's output,
    /// indexed by [`ItemId`].
    pub(crate) struct ItemFlags(u8) {
        /// Inside a ruby annotation, its open and close items included: out
        /// of the main line's flow.
        pub(crate) const ANNOTATION = 1 << 0;
        /// A U+200B the builder made for a `<wbr>` or for a wrap opportunity
        /// that collapsing keeps.
        ///
        /// Each is a text item of its own. Analysis gives its cluster its
        /// own class, which ends a shaping run; a U+200B in the input text
        /// does not.
        pub(super) const GENERATED = 1 << 1;
        /// A ruby container's open item, or an annotation's close item, that
        /// an annotation of the same container follows before the container
        /// closes.
        ///
        /// Base text with no annotation after it is no ruby column: Chrome
        /// lays it out as line text (`LineBreaker::HandleRuby`: "No
        /// ruby-text"). Annotations of nested containers don't count, but
        /// those of a container written as a box do. The writer sets the
        /// flag as each annotation opens.
        pub(crate) const ANNOTATION_FOLLOWS = 1 << 2;
    }
}

/// One entry in reading order: 16 bytes.
///
/// Items tile the text: those that hold text cover every byte once and in
/// order, and the rest sit at the boundaries between them. The writer checks
/// this in debug builds. Analysis relies on it to walk items and text
/// together.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct Item {
    /// The start of its text, or its position if it holds none.
    pub(super) start: TextOffset,
    /// The end of its text; equal to `start` if it holds none.
    pub(super) end: TextOffset,
    pub(crate) node: NodeId,
    pub(crate) kind: ItemKind,
    pub(crate) flags: ItemFlags,
}

/// A node: one per thing the builder named, plus one per generated
/// `::first-letter` box. 32 bytes.
///
/// The writer writes the row whole as it makes the node, except `end`,
/// which it writes when the node's items end.
///
/// Facts come in both variants. The first-line facts are what the node's
/// `::first-line` style lowers into, after the builder pins the properties
/// `::first-line` may not change. Without `::first-line` they equal the
/// standard facts. The four fact ids are 16 bits each; widening them takes
/// the node past 32 bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct Node {
    key: NodeKey,
    /// Text facts; a text node and a break take their container's.
    text: TextFactsId,
    first_line_text: TextFactsId,
    /// Box facts; [`BoxFactsId::INITIAL`] for a node that is no box.
    box_: BoxFactsId,
    first_line_box: BoxFactsId,
    /// Its first item, or where that would be for a node with none.
    first_item: ItemId,
    /// One past its last item, its close included: its items are
    /// `first_item..end`. Since nodes are in pre-order and items in reading
    /// order, `a` is an ancestor of `b` exactly when `a < b` and `b`'s
    /// `first_item` is before `a`'s `end`.
    end: ItemId,
    /// Its parent; the block is its own parent.
    parent: NodeId,
    kind: NodeKind,
}

/// The nodes in pre-order, their item ranges nesting as the nodes do.
///
/// A lookup by a node id the content names never misses.
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

    pub(crate) fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Returns `node` for the writer to finish, or `None` past the last.
    fn get_mut(&mut self, node: NodeId) -> Option<&mut Node> {
        self.nodes.get_mut(node)
    }

    /// Returns `node`'s text facts in `variant` (see [`TextFacts`]).
    ///
    /// A text node and a break take their container's. A lookup that misses
    /// is a bug and returns the block's.
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

    /// Returns `node`'s box facts in `variant` (see [`BoxFacts`]). A node
    /// that is no box, or one past the last, gets the initial row.
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

    /// Returns `node`'s kind, or `None` past the last node.
    pub(crate) fn kind(&self, node: NodeId) -> Option<NodeKind> {
        self.nodes.get(node).map(|node| node.kind)
    }

    /// Returns `node`'s key, or the default key past the last node.
    pub(crate) fn key(&self, node: NodeId) -> NodeKey {
        self.nodes
            .get(node)
            .map(|node| node.key)
            .unwrap_or_default()
    }

    /// Returns `node`'s parent. The block, and a node past the last, is its
    /// own parent.
    pub(crate) fn parent(&self, node: NodeId) -> NodeId {
        self.nodes.get(node).map_or(node, |node| node.parent)
    }

    /// Returns `node`'s items, its close included; empty past the last node.
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
    /// Such a container has an empty base. Its annotation is the next node,
    /// opens right after it and shares its key.
    pub(crate) fn is_anonymous_ruby(&self, node: NodeId) -> bool {
        let annotation = NodeId::new(node.get() + 1);
        self.kind(node) == Some(NodeKind::Ruby)
            && self.kind(annotation) == Some(NodeKind::Annotation)
            && self.key(annotation) == self.key(node)
            && self.items(annotation).start.get() == self.items(node).start.get() + 1
    }

    /// Whether `ancestor` is `node` or one of its ancestors, in constant
    /// time (see `Node::end`).
    pub(crate) fn contains(&self, ancestor: NodeId, node: NodeId) -> bool {
        ancestor == node || (ancestor < node && self.items(node).start < self.items(ancestor).end)
    }
}

heap_bytes! {
    Nodes { nodes }
}

/// An inline box, annotation or ruby container with its key, which a box
/// query by key may return. 16 bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct KeyedBox {
    pub(crate) key: NodeKey,
    pub(crate) node: NodeId,
}

impl Keyed for KeyedBox {
    type Key = (NodeKey, NodeId);

    /// Returns the sort key: the node key, then the node.
    #[inline]
    fn key(&self) -> (NodeKey, NodeId) {
        (self.key, self.node)
    }
}

/// The text one variant reads, addressed by content offsets.
///
/// This is the content's text, or the first line's text where that line
/// transforms some text differently.
#[derive(Copy, Clone)]
pub(crate) struct VariantText<'a> {
    content: &'a str,
    first_line: Option<&'a FirstLineSource>,
}

impl<'a> From<&'a str> for VariantText<'a> {
    /// Reads `content`, the content's text, with no first-line text. A
    /// reader holding only the text uses it to get a cluster's characters.
    fn from(content: &'a str) -> Self {
        Self {
            content,
            first_line: None,
        }
    }
}

impl<'a> VariantText<'a> {
    /// Returns the whole text, in its own offsets.
    pub(super) fn text(&self) -> &'a str {
        match self.first_line {
            Some(first_line) => &first_line.text,
            None => self.content,
        }
    }

    /// Maps content offset `at` to an offset in [`text`](Self::text).
    pub(super) fn offset(&self, at: TextOffset) -> TextOffset {
        match self.first_line {
            Some(first_line) => first_line.offset(at),
            None => at,
        }
    }

    /// Returns the text of content range `range`, or `""` where the range
    /// does not slice the text.
    pub(super) fn slice(&self, range: Range<TextOffset>) -> &'a str {
        match self.first_line {
            Some(first_line) => first_line.slice(range),
            None => self
                .content
                .get(range.start.get()..range.end.get())
                .unwrap_or_default(),
        }
    }

    /// Returns a cursor for reading this text in order.
    pub(super) fn cursor(self) -> TextCursor<'a> {
        TextCursor {
            text: self,
            after: Cell::new(0),
        }
    }
}

/// A variant's text, read in order by a walk.
///
/// It keeps its place in the first line's map. Offsets asked in order step
/// through the map without searching; one far behind the last costs one
/// search. A clone starts at the same place and moves independently.
#[derive(Clone)]
pub(crate) struct TextCursor<'a> {
    text: VariantText<'a>,
    /// The number of map entries that start at or before the last offset
    /// asked.
    after: Cell<usize>,
}

impl<'a> TextCursor<'a> {
    /// Returns the whole text, in its own offsets.
    pub(super) fn text(&self) -> &'a str {
        self.text.text()
    }

    /// Maps content offset `at` to an offset in [`text`](Self::text).
    pub(super) fn offset(&self, at: TextOffset) -> TextOffset {
        match self.text.first_line {
            Some(first_line) => first_line.offset_stepped(at, &self.after),
            None => at,
        }
    }

    /// Returns the ASCII character in content range `range`, if the range
    /// is one ASCII byte and the variant reads the content's text.
    ///
    /// Walks over Latin text use it to skip slicing and decoding.
    #[inline]
    pub(super) fn ascii(&self, range: Range<TextOffset>) -> Option<char> {
        let text = self.text;
        if text.first_line.is_some() || range.end.get() != range.start.get() + 1 {
            return None;
        }
        let byte = *text.content.as_bytes().get(range.start.get())?;
        byte.is_ascii().then_some(char::from(byte))
    }

    /// Returns the text of content range `range`, or `""` where the range
    /// does not slice the text.
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
    /// What the content holds, so later stages can skip work early.
    ///
    /// Each flag records what the content contains, never a conclusion. The
    /// writer sets each where it first sees the thing. Stages derive their
    /// own gates from these and from the text. The style flags are the only
    /// record of whether any style sets a property.
    pub(crate) struct ContentFlags(u32) {
        /// An inline box has margin, border or padding on some side, or is
        /// the block's initial letter, whose ink takes room at its edges
        /// whatever its own edges are.
        pub(crate) const BOXES_WITH_EDGES = 1 << 0;
        /// There is an atomic inline.
        pub(crate) const ATOMICS = 1 << 1;
        /// There is a float.
        pub(super) const FLOATS = 1 << 2;
        /// There is a ruby container.
        pub(crate) const RUBY = 1 << 3;
        /// Some node's `::first-line` style differs from its own.
        pub(crate) const FIRST_LINE_RESTYLE = 1 << 4;
        /// The block's first line transforms some text differently from
        /// its own transform. The content keeps the first paragraph as that
        /// line draws it.
        const TRANSFORM_SIDE_TEXT = 1 << 5;
        /// Some style sets emphasis marks.
        pub(crate) const EMPHASIS = 1 << 6;
        /// Some style sets an initial letter, so the block may have one.
        /// Only the box that may be the initial letter keeps the property.
        pub(super) const INITIAL_LETTER = 1 << 7;
        /// An inline box sets `box-decoration-break: clone`.
        pub(super) const CLONE_BOXES = 1 << 8;
        /// Some style sets `line-padding`.
        pub(super) const LINE_PADDING = 1 << 9;
        /// Some style sets `hanging-punctuation`.
        pub(super) const HANGING_PUNCTUATION = 1 << 10;
        /// Some style sets nonzero letter or word spacing.
        pub(super) const NONZERO_SPACING = 1 << 11;
        /// An inline box or atomic inline sets `vertical-align` other than
        /// `baseline`, so line heights are computed box by box. The block's
        /// own value does not count.
        pub(super) const VERTICAL_ALIGN = 1 << 12;
        /// Some style sets `dominant-baseline` other than `auto`. Measuring
        /// reads it only where the config applies `dominant-baseline`.
        pub(super) const DOMINANT_BASELINE = 1 << 13;
        /// Some node is an inline box. A line holds the strut of each box
        /// open across it; without this flag, no line looks for struts.
        pub(super) const INLINE_BOXES = 1 << 14;
        /// Some style sets `text-wrap-mode: nowrap`.
        pub(super) const NOWRAP = 1 << 15;
        /// Some style's `text-spacing-trim` trims the start of a line that
        /// wrapped.
        pub(super) const TRIMS_WRAPPED_START = 1 << 16;
        /// Some style's `text-spacing-trim` trims any punctuation.
        pub(super) const TRIMS_PUNCTUATION = 1 << 17;
        /// Some style's `text-autospace` adds space at some boundary.
        pub(super) const AUTOSPACE = 1 << 18;
        /// An inline box's style decorates its text (`decorates`). Even a
        /// culled box may then draw a decoration line; line paint looks for
        /// one only where this is set.
        pub(crate) const DECORATED_BOXES = 1 << 19;
        /// Some style sets `font-variant-position`.
        ///
        /// A used font may synthesize it, raising or lowering glyphs off
        /// their run's baseline. Without it, on an alphabetic-baseline line,
        /// every glyph sits on its run's baseline and no font is consulted.
        pub(crate) const VARIANT_POSITION = 1 << 20;
        /// Some node's first-line text shapes differently from its own text:
        /// its [`ShapingFactsId`] differs.
        pub(crate) const FIRST_LINE_SHAPING = 1 << 21;
        /// Two atomic inlines have the same key.
        const SHARED_ATOMIC_KEYS = 1 << 22;
        /// Some style sets `text-transform: math-auto`.
        const MATH_AUTO = 1 << 23;
        /// Some style sets `text-justify` other than `auto`, so a justified
        /// line reads each cluster's value.
        pub(crate) const TEXT_JUSTIFY = 1 << 24;
        /// A margin or padding of a box, atomic inline or float has a
        /// percentage, resolved against the content's basis.
        pub(crate) const PERCENTAGES = 1 << 25;
    }
}

/// The content: what the builder writes and every later stage reads.
pub(crate) struct Content {
    /// The text as laid out: white space collapsed, U+FFFC for each atomic
    /// inline, `\n` for each `<br>`, U+200B for each `<wbr>` and each break
    /// opportunity collapsing keeps. At most [`TextOffset::MAX`] bytes.
    pub(crate) text: String,
    pub(crate) nodes: Nodes,
    /// The items, in reading order.
    pub(crate) items: Table<ItemId, Item>,
    /// The languages the facts and analysis's script runs name. Shaping
    /// reads them at break time.
    pub(crate) languages: Languages,
    pub(crate) facts: Facts,
    /// The tables only some builds have (see [`ContentExtras`]); `None`
    /// until a build keeps one.
    extras: Option<Box<ContentExtras>>,
    /// The block's properties and facts.
    pub(crate) block: BlockFacts,
    pub(crate) flags: ContentFlags,
    /// The width, in pixels, that percentage margins and padding resolve
    /// against.
    basis: f32,
}

impl Content {
    /// Returns the inline boxes, annotations and ruby containers with key
    /// `key`, in node order. Costs one binary search.
    pub(crate) fn key_boxes(&self, key: NodeKey) -> &[KeyedBox] {
        let nodes = NodeId::new(0)..=NodeId::new(NodeId::MAX);
        (self.extras().keyed_boxes).range((key, *nodes.start())..=(key, *nodes.end()))
    }

    /// Returns the keyed boxes for the writer to fill, making the extras on
    /// first use.
    fn keyed_boxes_mut(&mut self) -> &mut SortedTable<KeyedBox> {
        &mut self.extras_mut().keyed_boxes
    }

    /// Creates empty content, allocating nothing.
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            nodes: Nodes::new(),
            items: Table::new(),
            languages: Languages::new(),
            facts: Facts::new(),
            extras: None,
            block: BlockFacts::INITIAL,
            flags: ContentFlags::NONE,
            basis: 0.0,
        }
    }

    /// Empties everything for a new build, keeping every allocation. The
    /// build records the offset map if `map` is set, and resolves
    /// percentages against `basis` pixels.
    fn clear(&mut self, block: BlockFacts, map: bool, basis: f32) {
        self.text.clear();
        self.nodes.clear();
        self.items.clear();
        self.languages.clear();
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
            extras.keyed_boxes.clear();
            extras.text_fit = TextFitFacts::NONE;
        }
        self.block = block;
        self.flags = ContentFlags::NONE;
        self.basis = basis;
    }

    /// Calls `record` on the offset map if this build records one, and
    /// returns its result.
    fn record<R>(&mut self, record: impl FnOnce(&mut Map) -> R) -> Option<R> {
        let map = self
            .extras
            .as_deref_mut()
            .and_then(ContentExtras::recorded_map)?;
        Some(record(map))
    }

    /// Returns the first paragraph as its first line draws it, where the
    /// build keeps it.
    fn first_line_source(&self) -> Option<&FirstLineSource> {
        Some(&self.extras().first_line_source).filter(|source| !source.is_empty())
    }

    /// Returns the block's `text-fit`: `none` where the build kept no
    /// extras.
    pub(crate) fn text_fit(&self) -> TextFitFacts {
        self.extras().text_fit
    }

    /// Returns the extras, or empty tables where the build kept none.
    #[inline]
    fn extras(&self) -> &ContentExtras {
        self.extras.as_deref().unwrap_or(&NO_EXTRAS)
    }

    /// Returns the extras for the writer to fill, making them on first use.
    fn extras_mut(&mut self) -> &mut ContentExtras {
        self.extras.get_or_insert_with(Box::default)
    }

    /// Returns the first line's text for the writer to fill, making the
    /// extras on first use.
    fn first_line_source_mut(&mut self) -> &mut FirstLineSource {
        &mut self.extras_mut().first_line_source
    }

    /// Returns the atomic inlines in reading order.
    #[inline]
    pub(super) fn atomics(&self) -> &Table<AtomicId, Atomic> {
        &self.extras().atomics
    }

    /// Returns the floats in reading order.
    #[inline]
    pub(crate) fn floats(&self) -> &Table<FloatId, Float> {
        &self.extras().floats
    }

    /// Returns the absolutely positioned boxes' anchors in reading order.
    #[inline]
    pub(crate) fn absolutes(&self) -> &Table<AbsoluteId, Absolute> {
        &self.extras().absolutes
    }

    /// Returns the forced breaks that clear floats, in reading order.
    #[inline]
    pub(super) fn clearances(&self) -> &Table<BreakClearanceId, BreakClearance> {
        &self.extras().clearances
    }

    /// Returns the offset map, or `None` where the build recorded none. It
    /// maps content offsets to each node's input text and back.
    pub(crate) fn offset_map(&self) -> Option<OffsetMap<'_>> {
        self.extras().map.read(&self.nodes, self.text.len())
    }

    /// Returns the node `item` belongs to, or the block past the last item.
    pub(crate) fn item_node(&self, item: ItemId) -> NodeId {
        self.items.get(item).map_or(NodeId::BLOCK, |item| item.node)
    }

    /// Returns the absolutely positioned box `item` anchors, or `None`.
    /// Binary search, since the anchors are in item order.
    pub(crate) fn item_absolute(&self, item: ItemId) -> Option<&Absolute> {
        find_sorted(self.absolutes().as_slice(), item)
    }

    /// Whether the atomic inlines' block sizes and baselines can be set in
    /// place: true without ruby or an initial letter, whose measuring reads
    /// an atomic inline's extent beyond its own item.
    pub(crate) fn can_resize_atomics(&self) -> bool {
        !self.flags.contains(ContentFlags::RUBY)
            && !self.flags.contains(ContentFlags::INITIAL_LETTER)
    }

    /// Whether measuring again with new box geometry can change anything:
    /// true with an atomic inline, a float, or a percentage margin or
    /// padding.
    pub(crate) fn can_measure_again(&self) -> bool {
        self.flags.contains(ContentFlags::PERCENTAGES)
            || !self.atomics().is_empty()
            || !self.floats().is_empty()
    }

    /// Returns a search for the atomic inlines with key `key`, wrapping
    /// round from `from`.
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

    /// Returns the key of `atomic`'s node.
    fn atomic_key(&self, atomic: &Atomic) -> NodeKey {
        self.nodes.key(self.item_node(atomic.item))
    }

    /// Sets atomic inline `atomic`'s border box and baseline to `size`,
    /// already sanitized, and returns how its size changed.
    pub(crate) fn set_atomic_size(&mut self, atomic: AtomicId, size: BoxSize) -> SizeChange {
        let writing_mode = self.block.writing_mode;
        let basis = self.basis;
        let Some(atomic) = self
            .extras
            .as_deref_mut()
            .and_then(|extras| extras.atomics.get_mut(atomic))
        else {
            return SizeChange::None;
        };
        if atomic.size == size {
            return SizeChange::None;
        }
        let along = atomic.size.inline != size.inline;
        atomic.size = size;
        if along {
            atomic.resolve(writing_mode, basis);
            SizeChange::Along
        } else {
            SizeChange::Across
        }
    }

    /// Sets float `float`'s border box to `size`, already sanitized, and
    /// returns whether its size changed.
    pub(crate) fn set_float_size(&mut self, float: FloatId, size: BoxSize) -> bool {
        let writing_mode = self.block.writing_mode;
        let basis = self.basis;
        let Some(float) = self
            .extras
            .as_deref_mut()
            .and_then(|extras| extras.floats.get_mut(float))
        else {
            return false;
        };
        if float.size == size {
            return false;
        }
        float.size = size;
        float.resolve(writing_mode, basis);
        true
    }

    /// Returns the key of `float`'s node.
    pub(crate) fn float_key(&self, float: &Float) -> NodeKey {
        self.nodes.key(self.item_node(float.item))
    }

    /// Resolves percentage margins and padding against `basis` pixels, and
    /// returns whether any length changed.
    ///
    /// Content with no percentage keeps its lengths whatever the basis.
    pub(crate) fn set_basis(&mut self, basis: f32) -> bool {
        if self.basis.to_bits() == basis.to_bits() {
            return false;
        }
        self.basis = basis;
        if !self.flags.contains(ContentFlags::PERCENTAGES) {
            return false;
        }
        let writing_mode = self.block.writing_mode;
        self.facts.resolve_boxes(writing_mode, basis);
        if let Some(extras) = self.extras.as_deref_mut() {
            for atomic in extras.atomics.as_mut_slice() {
                atomic.resolve(writing_mode, basis);
            }
            for float in extras.floats.as_mut_slice() {
                float.resolve(writing_mode, basis);
            }
        }
        true
    }

    /// Returns the width, in pixels, that percentages resolve against.
    pub(crate) fn basis(&self) -> f32 {
        self.basis
    }

    /// Returns the atomic inline `item` places, or `None`. Binary search,
    /// since the atomic inlines are in item order.
    pub(crate) fn item_atomic(&self, item: ItemId) -> Option<&Atomic> {
        find_sorted(self.atomics().as_slice(), item)
    }

    /// Returns the text's length in bytes plus the item count: the size
    /// every stage's scratch grows with.
    pub(crate) fn size(&self) -> usize {
        self.text.len().saturating_add(self.items.len())
    }

    /// Returns the text `variant` reads: the first paragraph as the first
    /// line draws it, where that line transforms text differently, or else
    /// the content's text.
    pub(crate) fn text(&self, variant: FirstLineVariant) -> VariantText<'_> {
        VariantText {
            content: &self.text,
            first_line: match variant {
                FirstLineVariant::FirstLine => self.first_line_source(),
                FirstLineVariant::Standard => None,
            },
        }
    }

    /// Whether the block's first line shapes some text differently from the
    /// other lines.
    ///
    /// True where a node's first-line [`ShapingFactsId`] differs, or where
    /// the first line has its own transform. Otherwise the first line shares
    /// the text's fonts and glyphs, and only its measurements differ.
    pub(super) fn first_line_reshapes(&self) -> bool {
        self.flags.contains(ContentFlags::FIRST_LINE_RESTYLE)
            && (self.flags.contains(ContentFlags::TRANSFORM_SIDE_TEXT)
                || self.flags.contains(ContentFlags::FIRST_LINE_SHAPING))
    }
}

heap_bytes! {
    /// Its tables, plus the extras' box and tables where a build made them.
    Content { text, nodes, items, languages, facts, extras; block, flags, basis }
}

/// The rooms (see [`Staging`]) the builder writes a new layout's content
/// tables in, so the layout keeps each table at its length.
struct ContentStaging {
    text: Staging<String>,
    nodes: Staging<Table<NodeId, Node>>,
    items: Staging<Table<ItemId, Item>>,
    facts: FactsStaging,
}

impl ContentStaging {
    /// Creates empty rooms, allocating nothing.
    const fn new() -> Self {
        Self {
            text: Staging::new(String::new()),
            nodes: Staging::new(Table::new()),
            items: Staging::new(Table::new()),
            facts: FactsStaging::new(),
        }
    }

    /// Lends the rooms to the tables of `content` that have no capacity.
    fn lend(&mut self, content: &mut Content) {
        self.text.lend(&mut content.text);
        self.nodes.lend(&mut content.nodes.nodes);
        self.items.lend(&mut content.items);
        content.facts.lend(&mut self.facts);
    }

    /// Copies the tables of `content` to exact-length allocations where
    /// needed and takes the rooms back.
    fn settle(&mut self, content: &mut Content) {
        self.text.settle(&mut content.text);
        self.nodes.settle(&mut content.nodes.nodes);
        self.items.settle(&mut content.items);
        content.facts.settle(&mut self.facts);
    }
}

heap_bytes! {
    ContentStaging { text, nodes, items, facts }
}
