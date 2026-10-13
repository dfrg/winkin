//! The content: what the builder makes of the caller's calls, which every
//! later stage reads.
//!
//! In: the builder's calls, with their styles, text and sizes. Out:
//! [`Content`]: the text as laid out, the nodes, the items in reading order,
//! the facts each style lowers into and the lists they name, the atomic
//! inlines, the floats and the absolutely positioned boxes, and flags for
//! what the content holds.
//! Start at: [`ContentWriter::new`], then the builder's calls in
//! `writer::calls`.
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
//! - `extras`: the atomic inlines, floats, anchors and clearing breaks, and
//!   the box that holds them with the offset map and the first line's text.
//!
//! The content keeps no style, and building reads no font and no width, so
//! it is a pure function of the caller's input. Only the writer writes it,
//! and the content is frozen once the build finishes. Clearing keeps every
//! allocation, so rebuilding similar content allocates nothing.

/// Maximum depth of separately modeled ruby containers. The writer
/// normalizes deeper containers to boxes, keeping later walks bounded.
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
        /// A ruby container's opening item, or an annotation's closing item,
        /// after which an annotation of the same container opens before
        /// the container closes.
        ///
        /// Base text with no annotation after it is no ruby column. Chrome
        /// lays it out as text of the line (`LineBreaker::HandleRuby`: "No
        /// ruby-text"). The annotations of nested containers are not the
        /// container's, but those of a container written as a box are. The
        /// writer sets it as each annotation opens.
        pub(crate) const ANNOTATION_FOLLOWS = 1 << 2;
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

/// A node a box query by key may answer: an inline box, an annotation or a
/// ruby container, with its key. 16 bytes.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct KeyedBox {
    pub(crate) key: NodeKey,
    pub(crate) node: NodeId,
}

impl Keyed for KeyedBox {
    type Key = (NodeKey, NodeId);

    /// Its key and then its node, which the boxes are sorted by.
    #[inline]
    fn key(&self) -> (NodeKey, NodeId) {
        (self.key, self.node)
    }
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
        pub(crate) const BOXES_WITH_EDGES = 1 << 0;
        /// There is an atomic inline.
        pub(crate) const ATOMICS = 1 << 1;
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
        pub(crate) const FIRST_LINE_SHAPING = 1 << 21;
        /// Two atomic inlines have the same key.
        const SHARED_ATOMIC_KEYS = 1 << 22;
        /// Some style asks for whole-node mathematical italic mapping.
        const MATH_AUTO = 1 << 23;
        /// Some style sets `text-justify` other than `auto`. A justified
        /// line then reads each cluster's own value.
        pub(crate) const TEXT_JUSTIFY = 1 << 24;
        /// A box's, atomic inline's or float's margin or padding has a
        /// percentage, which the content resolves against its basis.
        pub(crate) const PERCENTAGES = 1 << 25;
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
    /// The languages the facts and the analysis's script runs name, which
    /// shaping reads at break time.
    pub(crate) languages: Languages,
    /// The facts the writer lowers each style into.
    pub(crate) facts: Facts,
    /// The offset map, the first line's side text, the atomic inlines and
    /// the floats, where a build keeps any.
    extras: Option<Box<ContentExtras>>,
    /// The block's own properties, and its facts.
    pub(crate) block: BlockFacts,
    /// What the content holds.
    pub(crate) flags: ContentFlags,
    /// The width, in pixels, that the boxes' percentage margins and padding
    /// are resolved against.
    basis: f32,
}

impl Content {
    /// Returns the inline boxes, annotations and ruby containers `key`
    /// names, in node order: one search.
    pub(crate) fn key_boxes(&self, key: NodeKey) -> &[KeyedBox] {
        let nodes = NodeId::new(0)..=NodeId::new(NodeId::MAX);
        (self.extras().keyed_boxes).range((key, *nodes.start())..=(key, *nodes.end()))
    }

    /// The inline boxes, annotations and ruby containers by key, for the
    /// writer pushing and sorting them: made the first time a build has one.
    fn keyed_boxes_mut(&mut self) -> &mut SortedTable<KeyedBox> {
        &mut self.extras_mut().keyed_boxes
    }

    /// Empty content, allocating nothing.
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
    /// build records the offset map where `map` asks, and resolves
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
        Some(&self.extras().first_line_source).filter(|source| !source.is_empty())
    }

    /// The block's `text-fit`: `none` where the build kept no extras.
    pub(crate) fn text_fit(&self) -> TextFitFacts {
        self.extras().text_fit
    }

    /// The extras: none of each where the build kept none.
    #[inline]
    fn extras(&self) -> &ContentExtras {
        self.extras.as_deref().unwrap_or(&NO_EXTRAS)
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
        &self.extras().atomics
    }

    /// The floats, in reading order: none where the build kept none.
    #[inline]
    pub(crate) fn floats(&self) -> &Table<FloatId, Float> {
        &self.extras().floats
    }

    /// The absolutely positioned boxes' anchors, in reading order: none
    /// where the build wrote none.
    #[inline]
    pub(crate) fn absolutes(&self) -> &Table<AbsoluteId, Absolute> {
        &self.extras().absolutes
    }

    /// The forced breaks that clear floats, in reading order: none where
    /// the build wrote none.
    #[inline]
    pub(super) fn clearances(&self) -> &Table<BreakClearanceId, BreakClearance> {
        &self.extras().clearances
    }

    /// The offset map, where the build recorded it. It maps each content
    /// offset to the text the caller gave each node, and back.
    pub(crate) fn offset_map(&self) -> Option<OffsetMap<'_>> {
        self.extras().map.read(&self.nodes, self.text.len())
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

    /// Gives atomic inline `atomic` the border box and baseline of `size`,
    /// a sanitized size, and returns how its size changed.
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

    /// Gives float `float` the border box of `size`, a sanitized size, and
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

    /// Returns the key of the node float `float` anchors.
    pub(crate) fn float_key(&self, float: &Float) -> NodeKey {
        self.nodes.key(self.item_node(float.item))
    }

    /// Resolves the boxes' percentage margins and padding against `basis`
    /// pixels, and returns whether any length changed.
    ///
    /// Content with no percentage keeps its lengths, whatever the basis.
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

    /// The width, in pixels, that the boxes' percentages are resolved
    /// against.
    pub(crate) fn basis(&self) -> f32 {
        self.basis
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
                || self.flags.contains(ContentFlags::FIRST_LINE_SHAPING))
    }
}

heap_bytes! {
    /// Its tables, and the extras' box and theirs where a build made it.
    Content { text, nodes, items, languages, facts, extras; block, flags, basis }
}

/// The rooms the builder writes a new layout's content tables in, so that
/// the layout keeps each at its length.
struct ContentStaging {
    text: Staging<String>,
    nodes: Staging<Table<NodeId, Node>>,
    items: Staging<Table<ItemId, Item>>,
    facts: FactsStaging,
}

impl ContentStaging {
    /// Empty rooms, allocating nothing.
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

    /// Leaves the tables of `content` at their lengths and takes the rooms
    /// back.
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
