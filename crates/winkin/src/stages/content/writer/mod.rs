//! The content writer: every write to the content goes through
//! [`ContentWriter`].
//!
//! In: the builder's calls, one at a time, with their styles, text and
//! sizes. Out: a finished [`Content`] and a [`BuildReport`].
//! Start at: [`ContentWriter::new`], then the calls in `calls` and
//! the text ladder in `text`.
//!
//! - `calls`: the builder's calls but text, from `open` to `finish`.
//! - `first_letter`: the `::first-letter` box and where its letter ends.
//! - `text`: the text ladder: text and white space, collapsed,
//!   transformed and written.
//! - `nodes`: the container stack, nodes, items and the limit checks.
//! - `lower`: lowers each node's styles into facts.
//! - `check`: the debug build's checks of the content's invariants.
//!
//! This file holds the writer and the records its files share; each of
//! those files adds to `ContentWriter`'s `impl`.
//!
//! The writer keeps every invariant of the content: nodes in pre-order with
//! their item ranges, items tiling the text, the limits and the flags. It
//! asks the collapser about every character of white space.
//!
//! **The writer checks the limits.** The text holds at most
//! [`TextOffset::MAX`] bytes, and nodes and items at most what their ids
//! can name. What does not fit is dropped, whole characters and whole nodes
//! at a time, and counted in the report. The content stays valid for what
//! was kept. A caller cannot know whether its text crosses a limit once
//! collapsed, so crossing one is never fatal. An open box reserves the item
//! its close needs, so a box that opened always closes, and the items
//! always bracket.
//!
//! **The collapsed space is written where its run began.** That is before
//! any opaque items (box edges, floats, ruby marks, `<wbr>`) that arrived
//! while the run was pending. Those were written at the end of the text
//! when they arrived, so writing the space moves their offsets by one byte,
//! never their ids. Only zero width spaces can be written while a run is
//! pending, so this moves a few bytes and a few items at most.

mod calls;
#[cfg(debug_assertions)]
mod check;
mod first_letter;
mod lower;
mod nodes;
mod text;

use alloc::boxed::Box;
use alloc::string::String;

use super::collapse::Collapser;
use super::map::MapUnitId;
use super::memo::{StyleKey, StyleMemo};
use super::transform::Transforms;
use super::{
    BlockFacts, BoxFactsId, Content, ItemId, ItemKind, NodeId, NodeKey, NodeKind, TextFactsId,
    TextFitFacts,
};
use crate::build::{BuildOptions, BuildReport};
use crate::data::{HashIndex, Id, Table, TextOffset, define_id, heap_bytes};
use crate::style::{
    ComputedBlockStyle, ComputedStyle, InitialLetter, TextFitKind, VerticalAlign, WhiteSpaceTrim,
};

/// The most the content may hold: the ids' own limits and the text's,
/// unless a test asks for less.
#[derive(Copy, Clone, Debug)]
pub(crate) struct ContentLimits {
    /// Bytes of text.
    pub(super) text: usize,
    /// Nodes, the block included.
    pub(super) nodes: usize,
    /// Items.
    items: usize,
}

impl ContentLimits {
    /// What the ids and the text can hold.
    pub(crate) const MAX: Self = Self {
        text: TextOffset::MAX,
        nodes: NodeId::MAX,
        items: ItemId::MAX,
    };

    /// Returns these limits with at most `text` bytes of text. A test uses
    /// it to reach a limit with a few words.
    #[cfg(test)]
    pub(super) const fn with_text(self, text: usize) -> Self {
        Self { text, ..self }
    }

    /// Returns these limits with at most `nodes` nodes, the block included.
    #[cfg(test)]
    pub(super) const fn with_nodes(self, nodes: usize) -> Self {
        Self { nodes, ..self }
    }

    /// Returns these limits with at most `items` items.
    #[cfg(test)]
    pub(super) const fn with_items(self, items: usize) -> Self {
        Self { items, ..self }
    }
}

/// The content writer's working memory, which the layout keeps from one
/// build to the next so that a warm build allocates nothing.
///
/// It holds the open containers, and the text in which capitalize finds its
/// words where the text before carries a word on (see `transform`).
///
/// The context holds every other stage's scratch, but not this one. The
/// writer is not a pass: the builder feeds it one call at a time, and
/// building takes no context. So a host can build a nested layout while
/// this one's builder is open.
///
/// The writer's memo lives here too: the text facts each style given this
/// build lowered into (see `memo`). It is boxed, and made the first time a
/// build lowers a node's style past the block's, so a layout with no box
/// holds only its pointer. Once made, it is cleared with each build and
/// never dropped.
pub(crate) struct ContentScratch {
    open: Table<OpenId, Open>,
    words: String,
    memo: Option<Box<StyleMemo>>,
    /// The atomic inlines written so far by the hash of their keys, to find
    /// two that share one: made the first time a build writes one.
    atomic_keys: Option<Box<HashIndex>>,
}

impl ContentScratch {
    /// Nothing open, allocating nothing.
    pub(crate) const fn new() -> Self {
        Self {
            open: Table::new(),
            words: String::new(),
            memo: None,
            atomic_keys: None,
        }
    }
}

heap_bytes! {
    ContentScratch { open, words, memo, atomic_keys }
}

define_id! {
    /// Names a container on the writer's stack of open ones, outermost
    /// first. Each ruby container or annotation links to the next one out
    /// by it.
    struct OpenId(u32);
}

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

    /// Sets `initial-letter` as used in `style`, a keyed style.
    fn apply_key(self, style: &mut StyleKey) {
        match self {
            Self::Unset => {}
            Self::Kept => style.line.vertical_align = VerticalAlign::Baseline,
            Self::Normal => style.line.initial_letter = InitialLetter::NONE,
        }
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

impl FirstLetter {
    /// Returns whether the letter is armed with `math-auto` in its own style
    /// or its first-line style. The writer then measures text nodes for it.
    fn is_math_auto(&self) -> bool {
        match self {
            FirstLetter::Armed {
                style, first_line, ..
            } => {
                style.text.transform.is_math_auto()
                    || first_line.is_some_and(|s| s.text.transform.is_math_auto())
            }
            _ => false,
        }
    }
}

/// What the writer knows of the source text node `math-auto` measures.
///
/// `math-auto` maps a text node only where the node is one character. The
/// writer learns that only when the node ends, so a lone character waits.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum MathText {
    /// No text node is being measured.
    None,
    /// A text node of one character so far, held unwritten until the node
    /// continues or ends.
    Held(NodeKey, char),
    /// A text node known to be longer than one character: its text is
    /// written as it comes.
    Long(NodeKey),
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
    /// For a ruby container, the closing item of its last annotation so
    /// far, which the next annotation flags.
    annotation_close: Option<ItemId>,
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
///
/// Its state comes in groups, each kept mostly by one of the writer's
/// files: the text node being written (`text`), the ruby
/// containers open (`calls`), `::first-letter` and the initial
/// letter (`first_letter`), the block's trims and the first line's
/// text (`text`), and the room left (`nodes`).
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
    text_node: TextNode,
    ruby: RubyNesting,
    letters: Letters,
    trim: Trim,
    first_line: FirstLine,
    room: Room,
}

/// The text node the writer is writing, and what it knows of the text
/// written before.
struct TextNode {
    /// The text node `text` continues while its key is the same, until
    /// anything else is written.
    node: Option<NodeId>,
    /// That node's item text is appended to, once it has one.
    item: Option<ItemId>,
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
    /// The source text node `math-auto` measures, and its character while
    /// it may yet be the only one.
    math: MathText,
    /// The last character of the text written before, or U+0020 where
    /// something else came between.
    ///
    /// Capitalize reads this one character of what came before, as Chrome
    /// reads a text's `PreviousCharacter`. A `<br>` leaves its `\n`, a
    /// `<wbr>` its U+200B, and an atomic inline or a float a space, as in
    /// Chrome.
    last_char: char,
}

/// The ruby containers and annotations open.
struct RubyNesting {
    /// Where on the stack the innermost ruby container, annotation or
    /// nested ruby container is, each linking to the next one out.
    innermost: Option<OpenId>,
    /// How many annotations are open, so their items are flagged.
    annotations: usize,
    /// How many normalized ruby containers, written as boxes, still owe
    /// the `end_ruby` that closes each. One that an annotation of its
    /// column has already closed still takes its `end_ruby`, which then
    /// closes nothing.
    flattened: usize,
    /// Real ruby containers open; bounded before writing a new one.
    depth: usize,
}

/// Where the writer is with `::first-letter` and the initial letter.
struct Letters {
    /// Where `::first-letter` is.
    first_letter: FirstLetter,
    /// Nothing but boxes' opening edges and float anchors has been written.
    /// A box opening now is a first child all the way up, where
    /// `initial-letter` may apply.
    at_start: bool,
    /// A box has taken `initial-letter`. The block has its initial letter,
    /// and no box after it or inside it is another.
    initial_letter: bool,
}

/// The block's trims of its kept white space.
struct Trim {
    /// Where the block's leading trim would cut: just after the last
    /// segment break. It is set while the block trims its kept white space
    /// (`white-space-trim: discard-inner`) and only white space has been
    /// written.
    lead: Option<TextOffset>,
    /// Whether the block trims its kept white space at both ends. The
    /// trims cut at kept segment breaks, so the offset map starts a unit at
    /// each, and no unit runs across a cut.
    ends: bool,
}

/// The block's `::first-line`, and the first line's text the writer keeps.
struct FirstLine {
    /// Whether the block has `::first-line`, and every node a first-line
    /// style.
    styled: bool,
    /// Whether the first line's text is being kept.
    mirror: Mirror,
}

/// The room left in the content, and what did not fit.
struct Room {
    limits: ContentLimits,
    /// Close items owed to the containers open, which every other item
    /// leaves room for.
    reserved: usize,
    /// Whether the text reached its limit: nothing more is written.
    full: bool,
    report: BuildReport,
}

impl<'a> ContentWriter<'a> {
    /// Clears `content` and starts it with the block's node, keyed `key`.
    ///
    /// The node is set in `block`'s style and its `::first-line` style. The
    /// build records the offset map where `options` ask.
    pub(crate) fn new(
        content: &'a mut Content,
        scratch: &'a mut ContentScratch,
        key: NodeKey,
        block: &ComputedBlockStyle<'_>,
        options: BuildOptions,
        limits: ContentLimits,
    ) -> Self {
        let BuildOptions {
            map_source,
            percentage_basis,
        } = options;
        // A basis that is no length resolves percentages as zero does.
        let basis = if percentage_basis.is_finite() {
            percentage_basis
        } else {
            0.0
        };
        let (root, first_line) = (block.style, block.first_line);
        let facts = BlockFacts::new(block, root);
        #[cfg(any(debug_assertions, test))]
        super::facts_check::block_is(&facts, root);
        content.clear(facts, map_source, basis);
        if block.text_fit.kind != TextFitKind::None {
            content.extras_mut().text_fit = TextFitFacts::new(block.text_fit);
        }
        let ContentScratch {
            open: stack,
            words,
            memo,
            atomic_keys,
        } = scratch;
        stack.clear();
        if let Some(atomic_keys) = atomic_keys {
            atomic_keys.clear();
        }
        if let Some(memo) = memo {
            memo.clear();
        }
        let mut writer = Self {
            content,
            stack,
            words,
            memo,
            atomic_keys,
            block_transforms: Transforms::NONE,
            collapser: Collapser::new(),
            text_node: TextNode {
                node: None,
                item: None,
                source: 0,
                run_unit: None,
                continues: None,
                math: MathText::None,
                last_char: ' ',
            },
            ruby: RubyNesting {
                innermost: None,
                annotations: 0,
                flattened: 0,
                depth: 0,
            },
            letters: Letters {
                first_letter: FirstLetter::Unarmed,
                at_start: true,
                initial_letter: false,
            },
            trim: Trim {
                lead: None,
                ends: false,
            },
            first_line: FirstLine {
                styled: first_line.is_some(),
                mirror: if first_line.is_some() {
                    Mirror::Waiting
                } else {
                    Mirror::Done
                },
            },
            room: Room {
                limits: ContentLimits {
                    nodes: limits.nodes.min(NodeId::MAX),
                    items: limits.items.min(ItemId::MAX),
                    text: limits.text.min(TextOffset::MAX),
                },
                reserved: 0,
                full: false,
                report: BuildReport::default(),
            },
        };
        // The fact tables are empty, so the block's facts are their first.
        // An initial letter is an inline box, so the block is never one,
        // and its `initial-letter` is never read.
        let root = &InitialLetterUse::Normal.apply(root);
        let own = writer.key(root);
        let first_line =
            first_line.map(|first_line| writer.key(&root.pinned_first_line(first_line)));
        let lowered = writer.lower_both(&own, first_line.as_ref(), NodeKind::Block, None);
        writer.block_transforms = lowered.transforms;
        if writer.push_node(NodeKind::Block, lowered, key).is_none() {
            debug_assert!(false, "the block's node always fits");
        }
        if root.text.white_space_trim.discard_inner {
            writer.trim.lead = Some(TextOffset::new(0));
            writer.trim.ends = true;
        }
        writer
    }
}
